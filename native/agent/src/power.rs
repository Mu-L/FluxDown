//! 完成后自动关机：需要活跃任务才能 arm，任务清空后倒计时，倒计时中若又出现活跃任务
//! 则取消倒计时回到 armed（不完全解除）；到点执行平台关机命令。
//!
//! 状态机归 agent：关闭全部 UI（托盘驻留）后仍需按时关机。UI 只经 `agent.power.*`
//! 发请求、经 `AgentEvent::PowerChanged` 渲染。

use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use fluxdown_protocol::{AgentEvent, PowerStatusDto};
use tokio_util::sync::CancellationToken;

use crate::event_hub::AgentEventHub;

/// 状态刷新 / 到点检查间隔。
const TICK: Duration = Duration::from_secs(1);

pub struct PowerService {
    state: Mutex<ShutdownState>,
    events: AgentEventHub,
}

impl PowerService {
    #[must_use]
    pub fn new(events: AgentEventHub) -> Self {
        Self {
            state: Mutex::new(ShutdownState::new()),
            events,
        }
    }

    /// 全部任务完成后延迟 `delay` 关机；没有活跃任务时拒绝并返回 `false`。
    pub fn arm(&self, delay: Duration) -> bool {
        let active = self
            .events
            .inspect(|snapshot| snapshot.daemon.runtime_stats.active_tasks);
        let accepted = self.lock().arm(delay, active);
        self.publish();
        accepted
    }

    /// 取消已排定的关机（无论是等待任务完成还是正在倒计时）。
    pub fn disarm(&self) {
        self.lock().disarm();
        self.publish();
    }

    pub async fn run(self: Arc<Self>, cancel: CancellationToken) {
        let mut interval = tokio::time::interval(TICK);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = interval.tick() => {}
            }
            let (active, pending) = self.events.inspect(|snapshot| {
                let stats = &snapshot.daemon.runtime_stats;
                (stats.active_tasks, stats.pending_tasks)
            });
            let fired = self.lock().advance(active, pending, Instant::now());
            self.publish();
            if fired {
                tokio::task::spawn_blocking(execute_shutdown);
            }
        }
    }

    fn publish(&self) {
        let status = self.lock().status(Instant::now());
        if self.events.inspect(|snapshot| snapshot.power) != status {
            self.events.publish(AgentEvent::PowerChanged(status));
        }
    }

    fn lock(&self) -> MutexGuard<'_, ShutdownState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// 完成后关机的纯状态机：`Instant` 由调用方传入，便于单测构造固定时钟。
struct ShutdownState {
    armed_delay: Option<Duration>,
    countdown_until: Option<Instant>,
}

impl ShutdownState {
    fn new() -> Self {
        Self {
            armed_delay: None,
            countdown_until: None,
        }
    }

    /// `active_tasks > 0` 才接受；返回是否被接受。
    fn arm(&mut self, delay: Duration, active_tasks: u32) -> bool {
        if active_tasks == 0 {
            return false;
        }
        self.armed_delay = Some(delay);
        self.countdown_until = None;
        true
    }

    fn disarm(&mut self) {
        self.armed_delay = None;
        self.countdown_until = None;
    }

    /// 按当前任务计数与时钟推进一次；返回 `true` 表示本次到点触发关机（armed 状态已复位）。
    fn advance(&mut self, active_tasks: u32, pending_tasks: u32, now: Instant) -> bool {
        let Some(delay) = self.armed_delay else {
            return false;
        };
        if active_tasks == 0 && pending_tasks == 0 {
            if self.countdown_until.is_none() {
                self.countdown_until = Some(now + delay);
            }
        } else {
            // 倒计时中又出现活跃 / 等待任务：取消倒计时，回到 armed（不完全解除）。
            self.countdown_until = None;
        }
        let Some(until) = self.countdown_until else {
            return false;
        };
        if now < until {
            return false;
        }
        self.armed_delay = None;
        self.countdown_until = None;
        true
    }

    fn status(&self, now: Instant) -> PowerStatusDto {
        PowerStatusDto {
            armed_delay_secs: self.armed_delay.map(|delay| delay.as_secs()),
            countdown_remaining_secs: self
                .countdown_until
                .map(|until| until.saturating_duration_since(now).as_secs()),
        }
    }
}

/// 平台关机命令；`FLUXDOWN_SHUTDOWN_DRY_RUN` 设置时只记日志，便于 QA 验证不真的关机。
fn execute_shutdown() {
    if std::env::var_os("FLUXDOWN_SHUTDOWN_DRY_RUN").is_some() {
        tracing::warn!("shutdown dry-run: would power off now");
        return;
    }
    tracing::info!("all downloads finished: powering off");
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = Command::new("shutdown")
            .args(["/s", "/f", "/t", "0"])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("osascript")
            .args(["-e", "tell app \"System Events\" to shut down"])
            .status();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let systemd_ok = Command::new("systemctl")
            .arg("poweroff")
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !systemd_ok {
            let _ = Command::new("loginctl").arg("poweroff").status();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_requires_active_tasks() {
        let mut state = ShutdownState::new();
        assert!(!state.arm(Duration::from_secs(60), 0));
        assert_eq!(state.status(Instant::now()).armed_delay_secs, None);
    }

    #[test]
    fn countdown_starts_only_after_all_tasks_finish() {
        let mut state = ShutdownState::new();
        assert!(state.arm(Duration::from_secs(60), 1));
        let now = Instant::now();
        assert!(!state.advance(1, 0, now));
        assert_eq!(state.status(now).countdown_remaining_secs, None);
        assert!(!state.advance(0, 1, now));
        assert_eq!(state.status(now).countdown_remaining_secs, None);
        assert!(!state.advance(0, 0, now));
        assert_eq!(state.status(now).countdown_remaining_secs, Some(60));
    }

    #[test]
    fn new_active_task_cancels_countdown_but_keeps_armed() {
        let mut state = ShutdownState::new();
        state.arm(Duration::from_secs(60), 1);
        let now = Instant::now();
        state.advance(0, 0, now);
        assert!(!state.advance(1, 0, now));
        let status = state.status(now);
        assert_eq!(status.countdown_remaining_secs, None);
        assert_eq!(status.armed_delay_secs, Some(60));
    }

    #[test]
    fn countdown_fires_once_when_delay_elapses() {
        let mut state = ShutdownState::new();
        state.arm(Duration::from_secs(5), 1);
        let now = Instant::now();
        state.advance(0, 0, now);
        assert!(!state.advance(0, 0, now + Duration::from_secs(4)));
        assert!(state.advance(0, 0, now + Duration::from_secs(5)));
        assert!(!state.advance(0, 0, now + Duration::from_secs(6)));
        assert_eq!(
            state.status(now + Duration::from_secs(6)),
            PowerStatusDto::default()
        );
    }

    #[test]
    fn zero_delay_fires_on_first_idle_tick() {
        let mut state = ShutdownState::new();
        state.arm(Duration::ZERO, 1);
        assert!(state.advance(0, 0, Instant::now()));
    }

    #[test]
    fn disarm_clears_countdown() {
        let mut state = ShutdownState::new();
        state.arm(Duration::from_secs(60), 1);
        let now = Instant::now();
        state.advance(0, 0, now);
        state.disarm();
        assert_eq!(state.status(now), PowerStatusDto::default());
        assert!(!state.advance(0, 0, now + Duration::from_secs(120)));
    }
}
