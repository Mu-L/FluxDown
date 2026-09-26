//! 下载完成通知与活动下载期间的系统保持唤醒。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fluxdown_protocol::{AgentSnapshot, SnapshotBody};
use tokio::sync::broadcast;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::event_hub::AgentEventHub;
use crate::notification::{Notifier, completion_text, english_text};

/// 完成通知防抖：最后一次完成后静默这么久才合并发一条（同 Flutter `NotificationService`）。
const NOTIFY_DEBOUNCE: Duration = Duration::from_millis(800);
/// 持续密集完成时的最长合并等待。
const NOTIFY_MAX_WAIT: Duration = Duration::from_secs(3);

pub struct BackgroundEffects {
    events: AgentEventHub,
    notifier: Arc<Notifier>,
    #[cfg(feature = "desktop")]
    translator: Option<fluxdown_ui_i18n::Translator>,
}

/// 防抖中尚未发出的完成通知。
#[derive(Default)]
struct PendingCompletions {
    file_names: Vec<String>,
    /// 最近一批完成时的界面语言偏好（`general.locale`；`None` = 跟随系统）。
    locale: Option<String>,
    started_at: Option<Instant>,
    flush_at: Option<Instant>,
}

impl BackgroundEffects {
    /// `data_dir`：agent 数据目录（通知图标落盘位置）。
    #[must_use]
    pub fn new(events: AgentEventHub, data_dir: PathBuf) -> Self {
        Self {
            events,
            notifier: Arc::new(Notifier::new(data_dir)),
            #[cfg(feature = "desktop")]
            translator: match fluxdown_ui_i18n::I18nCatalog::load_embedded() {
                Ok(catalog) => {
                    Some(Arc::new(catalog).translator(&fluxdown_ui_i18n::system_locale()))
                }
                Err(error) => {
                    tracing::warn!(error = %error, "notification translations unavailable");
                    None
                }
            },
        }
    }

    pub async fn run(mut self, cancel: CancellationToken) {
        let (mut receiver, snapshot) = self.events.subscribe_and_snapshot();
        let mut awake = None;
        let mut pending = PendingCompletions::default();
        let mut statuses = {
            let initial = agent_snapshot(snapshot);
            reconcile_awake(should_keep_awake(&initial), &mut awake).await;
            initial
                .daemon
                .tasks
                .iter()
                .map(|task| (task.task_id.clone(), task.status))
                .collect::<HashMap<_, _>>()
        };
        loop {
            let flush_at = pending.flush_at;
            let flush = async move {
                match flush_at {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                _ = cancel.cancelled() => return,
                () = flush => self.flush(&mut pending),
                event = receiver.recv() => {
                    match event {
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                            // 每个进度帧都会走到这里：只在锁内提取所需字段，不克隆整份快照。
                            let (completed, should_hold, locale) = self.events.inspect(|snapshot| {
                                let completed = take_new_completions(snapshot, &mut statuses);
                                let locale = if completed.is_empty() {
                                    None
                                } else {
                                    locale_preference(snapshot)
                                };
                                (completed, should_keep_awake(snapshot), locale)
                            });
                            if !completed.is_empty() {
                                pending.push(completed, locale, Instant::now());
                            }
                            reconcile_awake(should_hold, &mut awake).await;
                        }
                        Err(broadcast::error::RecvError::Closed) => return,
                    }
                }
            }
        }
    }

    /// 把防抖中的完成合并成一条系统通知发出（发送阻塞，放进 blocking 线程）。
    fn flush(&mut self, pending: &mut PendingCompletions) {
        let batch = std::mem::take(pending);
        let Some((title, body)) = self.completion_notice(&batch.file_names, batch.locale) else {
            return;
        };
        let notifier = Arc::clone(&self.notifier);
        tokio::task::spawn_blocking(move || notifier.show(&title, &body));
    }

    #[cfg(feature = "desktop")]
    fn completion_notice(
        &mut self,
        file_names: &[String],
        locale: Option<String>,
    ) -> Option<(String, String)> {
        let Some(translator) = self.translator.as_mut() else {
            return completion_text(file_names, english_text);
        };
        translator.set_locale(&locale.unwrap_or_else(fluxdown_ui_i18n::system_locale));
        let translator = &*translator;
        completion_text(file_names, |key, count| match count {
            Some(count) => translator.text_with(key, &[("count", &count.to_string())]),
            None => translator.text(key).to_owned(),
        })
    }

    /// headless 构建不带文案目录：固定英文。
    #[cfg(not(feature = "desktop"))]
    #[allow(clippy::unused_self)]
    fn completion_notice(
        &mut self,
        file_names: &[String],
        _locale: Option<String>,
    ) -> Option<(String, String)> {
        completion_text(file_names, english_text)
    }
}

impl PendingCompletions {
    /// 并入一批完成并重排发送时刻：静默 [`NOTIFY_DEBOUNCE`] 后发，但自本批首个完成起
    /// 最多等 [`NOTIFY_MAX_WAIT`]。
    fn push(&mut self, file_names: Vec<String>, locale: Option<String>, now: Instant) {
        self.file_names.extend(file_names);
        self.locale = locale;
        let started_at = *self.started_at.get_or_insert(now);
        self.flush_at = Some((now + NOTIFY_DEBOUNCE).min(started_at + NOTIFY_MAX_WAIT));
    }
}

/// 界面语言偏好（与托盘同源：`general.locale`，`system` / 缺省 = 跟随系统）。
fn locale_preference(snapshot: &AgentSnapshot) -> Option<String> {
    snapshot
        .preferences
        .values
        .get("general.locale")
        .and_then(serde_json::Value::as_str)
        .filter(|locale| *locale != "system")
        .map(str::to_owned)
}

fn agent_snapshot(snapshot: fluxdown_protocol::Snapshot) -> AgentSnapshot {
    match snapshot.body {
        SnapshotBody::Agent(snapshot) => *snapshot,
        SnapshotBody::Daemon(_) => AgentSnapshot::default(),
    }
}

/// 用最新任务状态原地刷新 `statuses`，返回本次新转为完成（status 3）且需要
/// 通知的文件名。首次出现的任务只登记不通知；已删除的任务从表中剔除。
fn take_new_completions(
    snapshot: &AgentSnapshot,
    statuses: &mut HashMap<String, i32>,
) -> Vec<String> {
    let enabled = preference_bool(
        snapshot,
        NOTIFY_ON_COMPLETE_PREF,
        NOTIFY_ON_COMPLETE_DEFAULT,
    );
    let tasks = &snapshot.daemon.tasks;
    let mut completed = Vec::new();
    for task in tasks {
        match statuses.get_mut(&task.task_id) {
            Some(previous) => {
                if enabled && task.status == 3 && *previous != 3 {
                    completed.push(task.file_name.clone());
                }
                *previous = task.status;
            }
            None => {
                statuses.insert(task.task_id.clone(), task.status);
            }
        }
    }
    // 上面的循环保证 statuses ⊇ 当前任务；只有发生删除时长度才会更大。
    if statuses.len() != tasks.len() {
        statuses.retain(|task_id, _| tasks.iter().any(|task| task.task_id == *task_id));
    }
    completed
}

fn should_keep_awake(snapshot: &AgentSnapshot) -> bool {
    preference_bool(snapshot, KEEP_AWAKE_PREF, KEEP_AWAKE_DEFAULT)
        && snapshot
            .daemon
            .tasks
            .iter()
            .any(|task| matches!(task.status, 1 | 5))
}

async fn reconcile_awake(should_hold: bool, awake: &mut Option<keepawake::KeepAwake>) {
    if should_hold && awake.is_none() {
        match tokio::task::spawn_blocking(|| {
            keepawake::Builder::default()
                .idle(true)
                .sleep(true)
                .reason("FluxDown active download")
                .app_name("FluxDown")
                .app_reverse_domain("dev.zerx.fluxdown")
                .create()
        })
        .await
        {
            Ok(Ok(guard)) => *awake = Some(guard),
            Ok(Err(error)) => tracing::warn!(error = %error, "could not inhibit sleep"),
            Err(error) => tracing::warn!(error = %error, "keep-awake worker failed"),
        }
    } else if !should_hold {
        *awake = None;
    }
}

/// 下载完成通知开关；未写过偏好时默认开启——与设置页开关
/// （`crates/settings/src/sections/notify.rs`）及 Flutter `SettingsProvider` 默认值一致，
/// 否则开关显示为开、实际却不通知。
const NOTIFY_ON_COMPLETE_PREF: &str = "download.notify_on_complete";
const NOTIFY_ON_COMPLETE_DEFAULT: bool = true;
/// 下载期间保持唤醒；默认关闭（与设置页开关一致）。
const KEEP_AWAKE_PREF: &str = "download.keep_awake";
const KEEP_AWAKE_DEFAULT: bool = false;

fn preference_bool(snapshot: &AgentSnapshot, key: &str, default: bool) -> bool {
    snapshot
        .preferences
        .values
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use fluxdown_protocol::{AgentPreferencesDto, AgentSnapshot, TaskDto};
    use serde_json::json;

    use super::{
        KEEP_AWAKE_DEFAULT, KEEP_AWAKE_PREF, NOTIFY_DEBOUNCE, NOTIFY_MAX_WAIT,
        NOTIFY_ON_COMPLETE_DEFAULT, NOTIFY_ON_COMPLETE_PREF, PendingCompletions, locale_preference,
        preference_bool, take_new_completions,
    };

    #[test]
    fn completion_batches_debounce_but_never_wait_past_the_cap() {
        let start = tokio::time::Instant::now();
        let mut pending = PendingCompletions::default();
        pending.push(vec!["a.bin".to_owned()], None, start);
        assert_eq!(pending.flush_at, Some(start + NOTIFY_DEBOUNCE));

        // 连续完成不断顺延，但封顶于首个完成后 NOTIFY_MAX_WAIT。
        let late = start + NOTIFY_MAX_WAIT - Duration::from_millis(100);
        pending.push(vec!["b.bin".to_owned()], Some("zh".to_owned()), late);
        assert_eq!(pending.flush_at, Some(start + NOTIFY_MAX_WAIT));
        assert_eq!(pending.file_names, ["a.bin", "b.bin"]);
        assert_eq!(pending.locale.as_deref(), Some("zh"));
    }

    #[test]
    fn locale_preference_treats_system_as_follow_os() {
        let mut snapshot = AgentSnapshot::default();
        assert_eq!(locale_preference(&snapshot), None);
        snapshot
            .preferences
            .values
            .insert("general.locale".to_owned(), json!("system"));
        assert_eq!(locale_preference(&snapshot), None);
        snapshot
            .preferences
            .values
            .insert("general.locale".to_owned(), json!("zh"));
        assert_eq!(locale_preference(&snapshot).as_deref(), Some("zh"));
    }

    fn task(task_id: &str, status: i32) -> Result<TaskDto, serde_json::Error> {
        serde_json::from_value(json!({
            "taskId": task_id,
            "url": "https://example.com/file",
            "fileName": format!("{task_id}.bin"),
            "saveDir": "/tmp",
            "status": status,
            "downloadedBytes": 0,
            "totalBytes": 100,
            "errorMessage": "",
            "createdAt": "1",
            "proxyUrl": "",
            "queueId": "main",
            "checksum": ""
        }))
    }

    #[test]
    fn completions_fire_once_per_transition_and_forget_deleted_tasks()
    -> Result<(), serde_json::Error> {
        let mut snapshot = AgentSnapshot::default();
        snapshot
            .preferences
            .values
            .insert("download.notify_on_complete".to_owned(), json!(true));
        let mut statuses = HashMap::new();

        // 首次出现的任务（含已完成的）只登记，不通知。
        snapshot.daemon.tasks = vec![task("a", 1)?, task("b", 3)?];
        assert!(take_new_completions(&snapshot, &mut statuses).is_empty());

        // a 从下载中转为完成：恰好通知一次，重复帧不再通知。
        snapshot.daemon.tasks = vec![task("a", 3)?, task("b", 3)?];
        assert_eq!(take_new_completions(&snapshot, &mut statuses), ["a.bin"]);
        assert!(take_new_completions(&snapshot, &mut statuses).is_empty());

        // b 被删除后以同 id 重新出现并直接是完成态：视为新任务，不通知。
        snapshot.daemon.tasks = vec![task("a", 3)?];
        assert!(take_new_completions(&snapshot, &mut statuses).is_empty());
        assert!(!statuses.contains_key("b"));
        snapshot.daemon.tasks = vec![task("a", 3)?, task("b", 3)?];
        assert!(take_new_completions(&snapshot, &mut statuses).is_empty());

        // 关闭通知开关后，跃迁仍被记录但不产生通知。
        snapshot
            .preferences
            .values
            .insert("download.notify_on_complete".to_owned(), json!(false));
        snapshot.daemon.tasks = vec![task("a", 1)?, task("b", 3)?];
        assert!(take_new_completions(&snapshot, &mut statuses).is_empty());
        snapshot.daemon.tasks = vec![task("a", 3)?, task("b", 3)?];
        assert!(take_new_completions(&snapshot, &mut statuses).is_empty());
        assert_eq!(statuses.get("a"), Some(&3));
        Ok(())
    }

    #[test]
    fn unset_preferences_fall_back_to_settings_page_defaults() {
        let mut snapshot = AgentSnapshot {
            preferences: AgentPreferencesDto::default(),
            ..AgentSnapshot::default()
        };
        assert!(notifies_on_complete(&snapshot));
        assert!(!should_keep_awake_pref(&snapshot));
        snapshot
            .preferences
            .values
            .insert("download.notify_on_complete".to_owned(), json!(false));
        snapshot
            .preferences
            .values
            .insert("download.keep_awake".to_owned(), json!(true));
        assert!(!notifies_on_complete(&snapshot));
        assert!(should_keep_awake_pref(&snapshot));
    }

    fn notifies_on_complete(snapshot: &AgentSnapshot) -> bool {
        preference_bool(
            snapshot,
            NOTIFY_ON_COMPLETE_PREF,
            NOTIFY_ON_COMPLETE_DEFAULT,
        )
    }

    fn should_keep_awake_pref(snapshot: &AgentSnapshot) -> bool {
        preference_bool(snapshot, KEEP_AWAKE_PREF, KEEP_AWAKE_DEFAULT)
    }
}
