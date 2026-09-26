//! 独立下载进度 / 完成窗口的开关决策（纯状态机，不碰窗口与会话）。
//!
//! 只为**用户在场时亲手开始**的任务开窗：新建下载表单提交的单个任务、单任务「继续」、
//! 「重新下载」。宿主在命令成功后以任务 ID 调用 [`ProgressWindowTracker::arm`]；队列调度、
//! 启动自动恢复、RSS、静默捕获、批量操作都不经过 `arm`，因此不开窗。
//!
//! 窗口生命周期：任务进入活跃态开进度窗口；完成时已开的窗口切换为完成视图（按设置 / 本窗口
//! 覆盖值决定是否保留），用户中途关掉进度窗口的任务完成时仍弹一次不抢焦点的完成窗口。

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

/// 设备本地偏好：用户开始下载时弹出进度窗口（缺省开启）。
pub const PROGRESS_WINDOW_PREF: &str = "desktop.progress_window";
/// 设备本地偏好：用户开始的下载完成时弹出完成窗口（缺省开启）。
pub const COMPLETION_WINDOW_PREF: &str = "desktop.completion_window";

/// daemon 任务状态码（`TaskDto::status`）。
const STATUS_DOWNLOADING: i32 = 1;
const STATUS_PAUSED: i32 = 2;
const STATUS_COMPLETED: i32 = 3;
const STATUS_ERROR: i32 = 4;
const STATUS_PREPARING: i32 = 5;

/// 两个窗口开关的当前值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgressWindowPrefs {
    pub progress: bool,
    pub completion: bool,
}

impl Default for ProgressWindowPrefs {
    fn default() -> Self {
        Self {
            progress: true,
            completion: true,
        }
    }
}

impl ProgressWindowPrefs {
    /// 从 agent 偏好读取；缺省或类型不符按开启处理。
    #[must_use]
    pub fn from_preferences(values: &BTreeMap<String, Value>) -> Self {
        let flag = |key: &str| values.get(key).and_then(Value::as_bool).unwrap_or(true);
        Self {
            progress: flag(PROGRESS_WINDOW_PREF),
            completion: flag(COMPLETION_WINDOW_PREF),
        }
    }
}

/// [`ProgressWindowTracker::observe`] 要求宿主执行的窗口操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressWindowEffect {
    /// 打开进度窗口并激活（用户刚发起的动作）。
    OpenProgress,
    /// 打开完成窗口，不抢焦点（后台完成）。
    OpenCompletion,
    /// 关闭该任务已打开的窗口。
    Close,
}

#[derive(Debug, Default)]
pub struct ProgressWindowTracker {
    /// 用户刚开始、尚未观察到活跃态的任务。
    armed: HashSet<String>,
    /// 用户开始且尚未完成的任务：完成时据此弹完成窗口。
    user_started: HashSet<String>,
    /// 单个窗口的「完成后显示完成窗口」覆盖值。
    completion_override: HashMap<String, bool>,
}

impl ProgressWindowTracker {
    /// 登记一次用户在场的开始动作。宿主随后应以任务当前状态调用一次 [`Self::observe`]，
    /// 覆盖命令响应晚于状态事件到达的情况。
    pub fn arm(&mut self, task_id: &str) {
        self.armed.insert(task_id.to_owned());
    }

    /// 该任务的开始意图是否仍在等待活跃态（宿主据此在无窗口时推迟界面退出）。
    #[must_use]
    pub fn is_armed(&self, task_id: &str) -> bool {
        self.armed.contains(task_id)
    }

    /// 该任务完成时是否显示完成视图（窗口覆盖值优先于全局设置）。
    #[must_use]
    pub fn show_completion(&self, task_id: &str, prefs: ProgressWindowPrefs) -> bool {
        self.completion_override
            .get(task_id)
            .copied()
            .unwrap_or(prefs.completion)
    }

    pub fn set_completion_override(&mut self, task_id: &str, value: bool) {
        self.completion_override.insert(task_id.to_owned(), value);
    }

    /// 任务被删除：丢弃全部相关状态（窗口由视图自行关闭）。
    pub fn forget(&mut self, task_id: &str) {
        self.armed.remove(task_id);
        self.user_started.remove(task_id);
        self.completion_override.remove(task_id);
    }

    /// 任务状态更新。`window_open` = 该任务当前是否有进度 / 完成窗口。
    pub fn observe(
        &mut self,
        task_id: &str,
        status: i32,
        window_open: bool,
        prefs: ProgressWindowPrefs,
    ) -> Option<ProgressWindowEffect> {
        match status {
            STATUS_DOWNLOADING | STATUS_PREPARING => {
                if !self.armed.remove(task_id) {
                    return None;
                }
                self.user_started.insert(task_id.to_owned());
                (prefs.progress && !window_open).then_some(ProgressWindowEffect::OpenProgress)
            }
            STATUS_COMPLETED => {
                let armed = self.armed.remove(task_id);
                let user_started = self.user_started.remove(task_id) || armed;
                let show = self.show_completion(task_id, prefs);
                if window_open {
                    (!show).then_some(ProgressWindowEffect::Close)
                } else {
                    (user_started && show).then_some(ProgressWindowEffect::OpenCompletion)
                }
            }
            STATUS_PAUSED | STATUS_ERROR => {
                self.armed.remove(task_id);
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: ProgressWindowPrefs = ProgressWindowPrefs {
        progress: true,
        completion: true,
    };

    #[test]
    fn unarmed_tasks_never_open_windows() {
        let mut tracker = ProgressWindowTracker::default();
        assert_eq!(tracker.observe("t", 1, false, ON), None);
        assert_eq!(tracker.observe("t", 3, false, ON), None);
    }

    #[test]
    fn armed_task_opens_once_when_it_becomes_active() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm("t");
        assert_eq!(tracker.observe("t", 0, false, ON), None, "排队中不开窗");
        assert!(tracker.is_armed("t"), "排队期间意图保留，宿主据此保持界面");
        assert_eq!(
            tracker.observe("t", 5, false, ON),
            Some(ProgressWindowEffect::OpenProgress)
        );
        assert!(!tracker.is_armed("t"));
        assert_eq!(tracker.observe("t", 1, true, ON), None);
    }

    #[test]
    fn open_window_turns_into_completion_view_or_closes() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm("a");
        tracker.observe("a", 1, false, ON);
        assert_eq!(
            tracker.observe("a", 3, true, ON),
            None,
            "保留窗口切完成视图"
        );

        tracker.arm("b");
        tracker.observe("b", 1, false, ON);
        tracker.set_completion_override("b", false);
        assert_eq!(
            tracker.observe("b", 3, true, ON),
            Some(ProgressWindowEffect::Close)
        );
    }

    #[test]
    fn closed_progress_window_still_gets_one_completion_window() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm("t");
        tracker.observe("t", 1, false, ON);
        assert_eq!(
            tracker.observe("t", 3, false, ON),
            Some(ProgressWindowEffect::OpenCompletion)
        );
        assert_eq!(tracker.observe("t", 3, false, ON), None, "只弹一次");
    }

    #[test]
    fn prefs_gate_each_window_independently() {
        let progress_off = ProgressWindowPrefs {
            progress: false,
            completion: true,
        };
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm("t");
        assert_eq!(tracker.observe("t", 1, false, progress_off), None);
        assert_eq!(
            tracker.observe("t", 3, false, progress_off),
            Some(ProgressWindowEffect::OpenCompletion)
        );

        let completion_off = ProgressWindowPrefs {
            progress: true,
            completion: false,
        };
        tracker.arm("u");
        tracker.observe("u", 1, false, completion_off);
        assert_eq!(tracker.observe("u", 3, false, completion_off), None);
    }

    #[test]
    fn instant_completion_counts_as_user_started() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm("t");
        assert_eq!(
            tracker.observe("t", 3, false, ON),
            Some(ProgressWindowEffect::OpenCompletion)
        );
    }

    #[test]
    fn failure_before_start_drops_the_intent() {
        let mut tracker = ProgressWindowTracker::default();
        tracker.arm("t");
        tracker.observe("t", 4, false, ON);
        assert_eq!(tracker.observe("t", 1, false, ON), None);
    }

    #[test]
    fn missing_or_invalid_prefs_default_to_enabled() {
        let mut values = BTreeMap::new();
        values.insert(PROGRESS_WINDOW_PREF.to_owned(), Value::Bool(false));
        values.insert(COMPLETION_WINDOW_PREF.to_owned(), Value::String("x".into()));
        let prefs = ProgressWindowPrefs::from_preferences(&values);
        assert!(!prefs.progress);
        assert!(prefs.completion);
    }
}
