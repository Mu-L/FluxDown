//! composition root：一个 agent 会话、一个窗口注册表、全局菜单与动作；各窗口按需装配。

use std::{
    borrow::Cow,
    collections::BTreeMap,
    env,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use fluxdown_protocol::{
    AgentEvent, DaemonEvent, DaemonRuntimeStatsDto, ServiceEvent, ShellStatusDto,
};
use fluxdown_ui_downloads::DownloadView;
use fluxdown_ui_i18n::{I18nCatalog, I18nError, Translator, system_locale};
use fluxdown_ui_settings::{SettingsStore, component_locale};
use fluxdown_ui_shell::{RouteId, ShellView};
use gpui::{App, AppContext as _, Entity, Global, WeakEntity};
use gpui_component::menu::AppMenuBar;
use tokio::sync::mpsc;

use crate::agent_client::{AgentClient, AgentClientConfig, AgentClientError};
use crate::assets::DesktopAssets;
use crate::instance_ipc::{self, ActivateMessage, ActivationRequest, Endpoint};
use crate::launch::{self, LaunchOptions};
use crate::service_bootstrap::ServiceBootstrap;
use crate::session::{AgentSession, SessionSignal, attach};
use crate::settings_port::AgentSettingsPort;
use crate::windows::WindowRegistry;

const MI_SANS_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/MiSans-Regular.ttf");
const MI_SANS_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/MiSans-Medium.ttf");
const MI_SANS_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/MiSans-Semibold.ttf");

/// 事件泵单次批量上限。
const EVENT_BATCH: usize = 256;
/// 次实例等待刚启动主实例的 IPC 端点就绪、或等待旧主实例释放锁的最长时间。
const ACTIVATION_RETRY_TIMEOUT: Duration = Duration::from_secs(5);
const ACTIVATION_RETRY_INTERVAL: Duration = Duration::from_millis(50);

/// 桌面入口完成后的进程语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunOutcome {
    Completed,
    NoPrimary,
}

/// 任何协调故障都必须阻止第二个 UI 启动。
#[derive(Debug, thiserror::Error)]
pub(crate) enum AppError {
    #[error(transparent)]
    I18n(#[from] I18nError),
    #[error("FluxDown desktop instance lock unavailable")]
    InstanceLock(#[source] std::io::Error),
    #[error("FluxDown desktop activation unavailable")]
    Activation(#[source] instance_ipc::SendError),
    #[error("FluxDown desktop activation listener unavailable")]
    ActivationListener(#[source] std::io::Error),
    #[error("FluxDown agent client could not start")]
    AgentClient(#[source] AgentClientError),
}

enum LaunchDisposition {
    Primary(launch::InstanceLock),
    Activated,
    NoPrimary,
}

/// 跨窗口共享的应用状态（composition root 独有）。
pub(crate) struct Desktop {
    pub translator: Entity<Translator>,
    pub session: Entity<AgentSession>,
    pub client: Arc<AgentClient>,
    pub settings_store: Entity<SettingsStore>,
    pub menu_bar: Entity<AppMenuBar>,
    /// 主窗口内的下载页（主窗口关闭后失效）。
    pub main_downloads: Option<WeakEntity<DownloadView>>,
    pub main_shell: Option<WeakEntity<ShellView>>,
    /// 最新偏好（快照 + `PreferencesChanged` 折叠）。
    pub preferences: BTreeMap<String, serde_json::Value>,
    /// 最新运行时统计（关窗 / 退出提示用）。
    pub runtime_stats: DaemonRuntimeStatsDto,
    /// agent 托盘可用性与驻留策略：决定关闭主窗口是只退出界面还是完全退出。
    pub shell: ShellStatusDto,
    /// 已进入退出流程：窗口关闭回调不再重复触发退出。
    pub quitting: bool,
}

impl Global for Desktop {}

impl Desktop {
    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn global_mut(cx: &mut App) -> &mut Self {
        cx.global_mut::<Self>()
    }

    pub fn active_task_count(cx: &App) -> u32 {
        Self::global(cx).runtime_stats.active_tasks
    }

    pub fn pref(cx: &App, key: &str) -> Option<serde_json::Value> {
        Self::global(cx).preferences.get(key).cloned()
    }
}

pub(crate) fn run() -> Result<RunOutcome, AppError> {
    let launch = LaunchOptions::from_args(env::args().skip(1));
    let token_path = agent_token_path();
    let instance_dir = launch::instance_dir(&token_path);
    let endpoint = Endpoint::for_instance_dir(&instance_dir);
    let message = ActivateMessage {
        urls: launch.urls.clone(),
        files: launch.torrent_files.clone(),
        activate: launch.activate_existing || !launch.capture_only,
    };
    let _instance_lock =
        match acquire_or_activate(&instance_dir, &endpoint, &message, launch.activate_existing)? {
            LaunchDisposition::Primary(lock) => lock,
            LaunchDisposition::Activated => return Ok(RunOutcome::Completed),
            LaunchDisposition::NoPrimary => return Ok(RunOutcome::NoPrimary),
        };
    let (activate_tx, mut activate_rx) = mpsc::channel::<ActivationRequest>(16);
    // Unix sockets can bind before Tokio starts, so parallel starters are queued immediately.
    #[cfg(unix)]
    let listener =
        instance_ipc::Listener::bind(endpoint.clone()).map_err(AppError::ActivationListener)?;
    let agent_config = AgentClientConfig {
        rpc_url: env::var("FLUXDOWN_AGENT_URL")
            .unwrap_or_else(|_| "ws://127.0.0.1:17800/rpc".to_owned()),
        bearer_path: token_path,
    };

    let catalog = Arc::new(I18nCatalog::load_embedded()?);
    let translator = catalog.translator(&system_locale());
    let locale = component_locale(translator.locale()).to_owned();

    let bootstrap = Arc::new(ServiceBootstrap::new());
    let (agent_client, mut agent_events) =
        AgentClient::start(agent_config, bootstrap).map_err(AppError::AgentClient)?;
    #[cfg(unix)]
    agent_client.spawn_background(listener.listen(activate_tx));
    #[cfg(windows)]
    start_windows_listener(&agent_client, endpoint, activate_tx)
        .map_err(AppError::ActivationListener)?;
    let launch_submissions = submit_captures_detached(
        &agent_client,
        launch.urls.clone(),
        launch.torrent_files.clone(),
    );
    let open_urls_client = agent_client.clone();

    let application = gpui_platform::application().with_assets(DesktopAssets);
    application.on_open_urls(move |urls| {
        let files = urls
            .iter()
            .filter_map(|url| launch::torrent_path(url))
            .collect();
        let urls = urls
            .into_iter()
            .filter(|url| fluxdown_protocol::capture_link::is_capture_url(url))
            .collect();
        drop(submit_captures_detached(&open_urls_client, urls, files));
    });
    application.on_reopen(|cx| {
        if cx.has_global::<Desktop>() {
            crate::windows::main::reveal(cx);
        }
    });
    application.run(move |cx| {
        if let Err(error) = cx.text_system().add_fonts(vec![
            Cow::Borrowed(MI_SANS_REGULAR),
            Cow::Borrowed(MI_SANS_MEDIUM),
            Cow::Borrowed(MI_SANS_SEMIBOLD),
        ]) {
            eprintln!("failed to load FluxDown UI fonts: {error:#}");
            return;
        }

        gpui_component::init(cx);
        crate::app_icon::install();
        fluxdown_ui_theme::init(cx);
        gpui_component::set_locale(&locale);
        let translator = cx.new(|_| translator);
        let session = cx.new(|cx| AgentSession::new(agent_client.clone(), cx));
        WindowRegistry::init(cx, agent_client.clone());

        // 设置存储跨窗口存活：窗口关闭后防抖中的写回仍完成，快照/事件持续进入。
        let settings_store =
            cx.new(|_| SettingsStore::new(Arc::new(AgentSettingsPort::new(agent_client.clone()))));
        attach(&session, &settings_store, cx);
        let quit_store = settings_store.clone();
        cx.on_app_quit(move |cx| {
            let calls = quit_store.update(cx, |store, _| store.drain_pending_calls());
            let shutdown = crate::lifecycle::shutdown_request_on_app_quit(cx);
            async move {
                for call in calls {
                    let _ = call.await;
                }
                if let Some(shutdown) = shutdown {
                    let _ = shutdown.await;
                }
            }
        })
        .detach();

        crate::menus::bind_keys(cx);
        let menu_bar = crate::menus::install(cx, &translator);
        crate::menus::install_global_actions(cx);
        cx.set_global(Desktop {
            translator: translator.clone(),
            session: session.clone(),
            client: agent_client.clone(),
            settings_store,
            menu_bar,
            main_downloads: None,
            main_shell: None,
            preferences: BTreeMap::new(),
            runtime_stats: DaemonRuntimeStatsDto::default(),
            shell: ShellStatusDto::default(),
            quitting: false,
        });

        // 会话 → 偏好 / 运行时统计折叠进 Desktop；外观与语言随偏好变化。
        cx.subscribe(&session, |_, signal, cx| match signal {
            SessionSignal::Snapshot(snapshot) => {
                if let Some(body) = crate::session::agent_body(snapshot) {
                    let values = body.preferences.values.clone();
                    let stats = body.daemon.runtime_stats.clone();
                    let shell = body.shell.clone();
                    let desktop = Desktop::global_mut(cx);
                    desktop.preferences = values;
                    desktop.runtime_stats = stats;
                    desktop.shell = shell;
                    apply_preferences(cx);
                }
            }
            SessionSignal::Event(frame) => match &frame.event {
                ServiceEvent::Agent(AgentEvent::PreferencesChanged(prefs)) => {
                    Desktop::global_mut(cx).preferences = prefs.values.clone();
                    apply_preferences(cx);
                }
                ServiceEvent::Agent(AgentEvent::ShellChanged(shell)) => {
                    Desktop::global_mut(cx).shell = shell.clone();
                }
                ServiceEvent::Agent(AgentEvent::Daemon(DaemonEvent::RuntimeStatsChanged(
                    stats,
                )))
                | ServiceEvent::Daemon(DaemonEvent::RuntimeStatsChanged(stats)) => {
                    Desktop::global_mut(cx).runtime_stats = stats.clone();
                }
                ServiceEvent::Agent(AgentEvent::DaemonSnapshotReplaced(snapshot))
                | ServiceEvent::Agent(AgentEvent::Daemon(DaemonEvent::SnapshotReplaced(
                    snapshot,
                ))) => {
                    Desktop::global_mut(cx).runtime_stats = snapshot.runtime_stats.clone();
                }
                _ => {}
            },
            SessionSignal::Fatal(error) => {
                eprintln!("fatal FluxDown agent error: {:?}", error.code);
            }
            SessionSignal::ServiceStopped => crate::lifecycle::service_stopped(cx),
            SessionSignal::Stale => {}
        })
        .detach();

        // 事件泵：按帧批处理，一次 `update` 广播一批。
        let pump_session = session.clone();
        cx.spawn(async move |cx| {
            loop {
                let Some(first) = agent_events.recv().await else {
                    break;
                };
                let mut batch = vec![first];
                while batch.len() < EVENT_BATCH {
                    match agent_events.try_recv() {
                        Ok(event) => batch.push(event),
                        Err(_) => break,
                    }
                }
                cx.update(|cx| {
                    pump_session.update(cx, |session, cx| session.ingest(batch, cx));
                });
            }
        })
        .detach();

        // 单实例激活通道：链接交给 agent，激活时重建 / 恢复 / 聚焦主窗口。
        let activate_client = agent_client.clone();
        cx.spawn(async move |cx| {
            while let Some(request) = activate_rx.recv().await {
                let (message, acknowledgement) = request.into_parts();
                drop(submit_captures_detached(
                    &activate_client,
                    message.urls,
                    message.files,
                ));
                if message.activate {
                    cx.update(crate::windows::main::reveal);
                }
                let _ = acknowledgement.send(());
            }
        })
        .detach();

        // 常驻能力归 agent（托盘 / 剪贴板监听 / 完成后关机执行）；这里只装界面投影。
        crate::power::install(cx);
        // 引擎选择请求窗口 / 外部捕获确认（并入新建下载窗口）：跟随会话事件独立开关，
        // 不依赖主窗口存在。
        crate::windows::selection::install(cx);
        crate::windows::new_download::install_captures(cx);

        if launch.capture_only {
            // 由 agent 为待确认交互拉起：不开主窗口；确认窗口随快照 / 事件打开，全部关闭后由
            // 窗口注册表退出。启动链接提交完成后若首个快照里已无待确认项，直接退出。
            quit_when_nothing_to_confirm(launch_submissions, cx);
            return;
        }
        if launch.minimized {
            // 开机自启（agent 判定需要界面时才拉起）：会话就绪后开最小化主窗口。
            after_session_settled(cx, open_main_minimized);
            return;
        }
        // 连接在 GPUI 初始化前已开始：热启动时首个快照几乎与事件循环同时到达，等它到了再开窗，
        // 首帧就是完整数据、主题与语言，不闪「正在连接」；冷启动（需拉起后台）最多等连接宽限。
        after_session_settled(cx, crate::windows::main::reveal);
    });

    Ok(RunOutcome::Completed)
}
fn acquire_or_activate(
    instance_dir: &Path,
    endpoint: &Endpoint,
    message: &ActivateMessage,
    activate_existing: bool,
) -> Result<LaunchDisposition, AppError> {
    let deadline = Instant::now() + ACTIVATION_RETRY_TIMEOUT;
    loop {
        match launch::InstanceLock::try_acquire(instance_dir).map_err(AppError::InstanceLock)? {
            Some(_lock) if activate_existing => return Ok(LaunchDisposition::NoPrimary),
            Some(lock) => return Ok(LaunchDisposition::Primary(lock)),
            None => match instance_ipc::send_to_primary(endpoint, message) {
                Ok(()) => return Ok(LaunchDisposition::Activated),
                Err(error) if error.is_retryable() && Instant::now() < deadline => {
                    std::thread::sleep(ACTIVATION_RETRY_INTERVAL);
                }
                Err(error) => return Err(AppError::Activation(error)),
            },
        }
    }
}

#[cfg(windows)]
fn start_windows_listener(
    client: &Arc<AgentClient>,
    endpoint: Endpoint,
    tx: mpsc::Sender<ActivationRequest>,
) -> Result<(), std::io::Error> {
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    client.spawn_background(async move {
        match instance_ipc::Listener::bind(endpoint) {
            Ok(listener) => {
                let _ = ready_tx.send(Ok(()));
                listener.listen(tx).await;
            }
            Err(error) => {
                let _ = ready_tx.send(Err(error));
            }
        }
    });
    ready_rx
        .recv_timeout(ACTIVATION_RETRY_TIMEOUT)
        .map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("activation listener did not start: {error}"),
            )
        })?
}
fn open_main_minimized(cx: &mut App) {
    if let Some(handle) = crate::windows::main::open(cx) {
        let _ = handle.update(cx, |_, window, _| window.minimize_window());
    }
}

/// 会话已就绪（拿到快照或已确认离线）立即执行，否则等到就绪后执行一次。
fn after_session_settled(cx: &mut App, run: fn(&mut App)) {
    let session = Desktop::global(cx).session.clone();
    if session.read(cx).is_settled() {
        run(cx);
        return;
    }
    let subscription = std::rc::Rc::new(std::cell::RefCell::new(None));
    let holder = std::rc::Rc::clone(&subscription);
    *subscription.borrow_mut() = Some(cx.subscribe(&session, move |_, signal, cx| {
        if matches!(
            signal,
            SessionSignal::Snapshot(_) | SessionSignal::Stale | SessionSignal::Fatal(_)
        ) && holder.borrow_mut().take().is_some()
        {
            run(cx);
        }
    }));
}

/// `--capture`：启动链接提交完、首个快照也已派发确认窗口后仍没有任何窗口（请求已被别处处理
/// 或已按默认值超时）时退出，避免留下无窗口、无托盘的界面进程。
fn quit_when_nothing_to_confirm(submissions: tokio::sync::oneshot::Receiver<()>, cx: &mut App) {
    cx.spawn(async move |cx| {
        let _ = submissions.await;
        cx.update(|cx| {
            after_first_snapshot(cx, |cx| {
                cx.defer(|cx| {
                    if WindowRegistry::open_count(cx) == 0 {
                        crate::lifecycle::quit_ui(cx);
                    }
                });
            });
        });
    })
    .detach();
}

/// 已有快照立即执行，否则等首个快照到达后执行一次。
fn after_first_snapshot(cx: &mut App, run: fn(&mut App)) {
    let session = Desktop::global(cx).session.clone();
    if session.read(cx).latest().is_some() {
        run(cx);
        return;
    }
    let subscription = std::rc::Rc::new(std::cell::RefCell::new(None));
    let holder = std::rc::Rc::clone(&subscription);
    *subscription.borrow_mut() = Some(cx.subscribe(&session, move |_, signal, cx| {
        if matches!(signal, SessionSignal::Snapshot(_)) {
            run(cx);
            holder.borrow_mut().take();
        }
    }));
}

/// 偏好快照 → 全局外观与语言。每次快照/偏好事件都幂等应用。
fn apply_preferences(cx: &mut App) {
    let values = Desktop::global(cx).preferences.clone();
    let translator = Desktop::global(cx).translator.clone();
    fluxdown_ui_theme::apply_appearance_preferences(&values, cx);
    apply_activity_bar_preferences(&values, cx);
    if let Some(locale) = values
        .get("general.locale")
        .and_then(serde_json::Value::as_str)
    {
        let target = if locale == "system" {
            system_locale()
        } else {
            locale.to_owned()
        };
        translator.update(cx, |translator, cx| {
            if translator.set_locale(&target) {
                gpui_component::set_locale(component_locale(translator.locale()));
                cx.notify();
            }
        });
    }
}

/// 偏好快照/事件 → 活动栏可选项可见性（RSS 路由、主题切换动作）。
fn apply_activity_bar_preferences(values: &BTreeMap<String, serde_json::Value>, cx: &mut App) {
    let Some(shell) = Desktop::global(cx).main_shell.clone() else {
        return;
    };
    let show_activity_rss = values
        .get("ui.show_activity_rss")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let show_activity_theme = values
        .get("ui.show_activity_theme")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    let _ = shell.update(cx, |shell, cx| {
        shell.set_route_visible(RouteId::new("rss"), show_activity_rss, cx);
        shell.set_action_visible("activity-theme", show_activity_theme, cx);
    });
}

/// 外部链接 / `.torrent` 文件 → agent 捕获入口的 RPC 列表。
fn capture_calls(
    client: &Arc<AgentClient>,
    urls: Vec<String>,
    files: Vec<std::path::PathBuf>,
) -> Vec<(String, crate::agent_client::AgentFuture<serde_json::Value>)> {
    let mut calls = Vec::with_capacity(urls.len() + files.len());
    for url in urls {
        let url = fluxdown_protocol::capture_link::normalize_capture_url(&url);
        let future = client.call::<serde_json::Value, serde_json::Value>(
            fluxdown_protocol::method::AGENT_CAPTURE_SUBMIT,
            Some(serde_json::json!({ "request": { "url": url }, "silent": true })),
        );
        calls.push((url, future));
    }
    for file in files {
        let path = file.display().to_string();
        let future = client.call::<serde_json::Value, serde_json::Value>(
            fluxdown_protocol::method::AGENT_CAPTURE_SUBMIT_TORRENT_FILE,
            Some(serde_json::json!({ "path": path, "silent": true })),
        );
        calls.push((path, future));
    }
    calls
}

/// 主实例：不阻塞 UI 线程，在后台把链接交给 agent；返回的接收端在全部提交结束后完成。
pub(crate) fn submit_captures_detached(
    client: &Arc<AgentClient>,
    urls: Vec<String>,
    files: Vec<std::path::PathBuf>,
) -> tokio::sync::oneshot::Receiver<()> {
    let (done, finished) = tokio::sync::oneshot::channel();
    if urls.is_empty() && files.is_empty() {
        let _ = done.send(());
        return finished;
    }
    let calls = capture_calls(client, urls, files);
    client.spawn_background(async move {
        for (source, future) in calls {
            if let Err(error) = future.await {
                eprintln!("failed to submit {source}: {:?}", error.code);
            }
        }
        let _ = done.send(());
    });
    finished
}

fn agent_token_path() -> std::path::PathBuf {
    if let Some(path) = env::var_os("FLUXDOWN_AGENT_TOKEN_FILE") {
        return path.into();
    }
    if let Some(path) = env::var_os("FLUXDOWN_AGENT_DATA_DIR") {
        return std::path::PathBuf::from(path).join("agent.token");
    }
    directories::ProjectDirs::from("dev", "zerx", "FluxDown")
        .map(|project| project.data_dir().join("agent").join("agent.token"))
        .unwrap_or_else(|| std::path::PathBuf::from("agent.token"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("fluxdown-app-{label}-{}", std::process::id()))
    }

    #[test]
    fn activate_existing_never_claims_a_free_lock() {
        let dir = test_dir("activate-only");
        let endpoint = Endpoint::for_instance_dir(&dir);
        let outcome = acquire_or_activate(&dir, &endpoint, &ActivateMessage::default(), true)
            .expect("coordinate launch");
        assert!(matches!(outcome, LaunchDisposition::NoPrimary));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retries_and_claims_lock_after_previous_primary_exits() {
        let dir = test_dir("takeover");
        let endpoint = Endpoint::for_instance_dir(&dir);
        let held = launch::InstanceLock::try_acquire(&dir)
            .expect("acquire initial lock")
            .expect("initial primary");
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(25));
            drop(held);
        });
        let outcome = acquire_or_activate(&dir, &endpoint, &ActivateMessage::default(), false)
            .expect("take over after release");
        releaser.join().expect("release thread");
        match outcome {
            LaunchDisposition::Primary(lock) => drop(lock),
            LaunchDisposition::Activated | LaunchDisposition::NoPrimary => {
                panic!("expected primary takeover")
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
