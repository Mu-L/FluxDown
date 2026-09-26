//! 单一 agent 会话实体：所有快照 / 事件的唯一入口，向任意数量窗口的任意能力视图广播。
//!
//! 视图通过 [attach] 订阅：从持续折叠的同源快照秒开首帧，后续沿单会话
//! 的严格事件游标更新；订阅随视图销毁自动解除。
//!
//! 连接态对视图有宽限：启动时首个快照、断线后的重连快照只要在 [`OFFLINE_NOTICE_GRACE`]
//! 内到达，视图就不会进入「正在连接」只读态（期间发出的命令由客户端排队，重连后送达）。
//! 超过宽限仍未连上才广播 [`SessionSignal::Stale`]。

use std::sync::Arc;
use std::time::Duration;

use fluxdown_protocol::{
    AgentEvent, AgentSnapshot, DaemonEvent, EventFrame, RpcErrorData, ServiceEvent, Snapshot,
    SnapshotBody, accepted_runtime_status, apply_agent_event,
};
use gpui::{App, Context, Entity, EventEmitter};

use crate::agent_client::{AgentClient, AgentClientEvent};

/// 断线（含启动时尚未连上）多久仍未恢复才告知视图。本机回环重连与热启动首个快照都远低于
/// 此值；冷启动（需拉起 agent 与 daemon）超过它时才显示连接态。
const OFFLINE_NOTICE_GRACE: Duration = Duration::from_millis(800);

/// 会话向订阅者广播的信号。
pub enum SessionSignal {
    /// 连接 / 重连后的全量快照。
    Snapshot(Arc<Snapshot>),
    /// 单个事件帧。
    Event(Arc<EventFrame>),
    /// 连接断开超过宽限仍未恢复，视图进入只读的「正在连接」态。
    Stale,
    /// 不可恢复错误（协议不兼容 / 未授权）。
    Fatal(RpcErrorData),
    /// agent 已完全退出（`system.shutdown`）：界面应随之退出。
    ServiceStopped,
}

/// agent 会话状态：最近一次全量快照与连接健康度。
pub struct AgentSession {
    _client: Arc<AgentClient>,
    latest: Option<Arc<Snapshot>>,
    /// 连接当前不可用（事件帧不再并入快照）。
    stale: bool,
    /// 已向视图广播离线（宽限已过或不可恢复）。
    offline_notified: bool,
    /// 宽限计时器代际：新快照到达即作废在途计时。
    offline_generation: u64,
}

impl EventEmitter<SessionSignal> for AgentSession {}

impl AgentSession {
    /// 创建会话并开始启动宽限计时：首个快照在宽限内到达则视图全程不见连接态。
    pub fn new(client: Arc<AgentClient>, cx: &mut Context<Self>) -> Self {
        let mut session = Self {
            _client: client,
            latest: None,
            stale: true,
            offline_notified: false,
            offline_generation: 0,
        };
        session.schedule_offline_notice(cx);
        session
    }

    /// 最近一次同源完整投影（已折叠有序事件）。
    #[must_use]
    pub fn latest(&self) -> Option<&Arc<Snapshot>> {
        self.latest.as_ref()
    }

    /// 最近快照的 agent 主体。
    #[must_use]
    pub fn agent_snapshot(&self) -> Option<&AgentSnapshot> {
        self.latest.as_deref().and_then(agent_body)
    }

    /// 已可呈现界面：拿到快照，或已确认离线（宽限已过 / 不可恢复），不必再等。
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.latest.is_some() || self.offline_notified
    }

    /// 把一批客户端事件逐个转为 [`SessionSignal`] 广播。
    pub fn ingest(&mut self, events: Vec<AgentClientEvent>, cx: &mut Context<Self>) {
        for event in events {
            match event {
                AgentClientEvent::Snapshot(snapshot) => {
                    let snapshot = Arc::new(*snapshot);
                    self.latest = Some(Arc::clone(&snapshot));
                    self.stale = false;
                    self.offline_notified = false;
                    self.offline_generation = self.offline_generation.wrapping_add(1);
                    cx.emit(SessionSignal::Snapshot(snapshot));
                }
                AgentClientEvent::Event(mut frame) => {
                    match self
                        .latest
                        .as_mut()
                        .map(|snapshot| apply_frame(Arc::make_mut(snapshot), &mut frame))
                    {
                        Some(Ok(true)) if !self.stale => {
                            cx.emit(SessionSignal::Event(Arc::new(*frame)));
                        }
                        Some(Ok(_)) => {}
                        _ => self.go_stale(cx),
                    }
                }
                AgentClientEvent::Stale => self.go_stale(cx),
                AgentClientEvent::Fatal(error) => {
                    self.notify_offline_now();
                    cx.emit(SessionSignal::Fatal(error));
                }
                AgentClientEvent::ServiceStopped => {
                    self.notify_offline_now();
                    cx.emit(SessionSignal::ServiceStopped);
                }
            }
        }
        cx.notify();
    }

    /// 连接中断：立即停止并帧，但只在宽限内仍未恢复时才告知视图。
    fn go_stale(&mut self, cx: &mut Context<Self>) {
        if self.stale {
            return;
        }
        self.stale = true;
        self.schedule_offline_notice(cx);
    }

    fn schedule_offline_notice(&mut self, cx: &mut Context<Self>) {
        self.offline_generation = self.offline_generation.wrapping_add(1);
        let generation = self.offline_generation;
        cx.spawn(async move |session, cx| {
            cx.background_executor().timer(OFFLINE_NOTICE_GRACE).await;
            let _ = session.update(cx, |session, cx| {
                if session.stale
                    && !session.offline_notified
                    && session.offline_generation == generation
                {
                    session.notify_offline_now();
                    cx.emit(SessionSignal::Stale);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 视图即将进入离线态：快照中的运行态作废（晚打开的视图也不显示过期速度）。
    fn notify_offline_now(&mut self) {
        self.stale = true;
        self.offline_notified = true;
        self.offline_generation = self.offline_generation.wrapping_add(1);
        if let Some(snapshot) = self.latest.as_mut()
            && let SnapshotBody::Agent(body) = &mut Arc::make_mut(snapshot).body
        {
            body.daemon_connected = false;
            body.daemon.task_runtime.clear();
        }
    }
}

/// 快照 / 事件消费者：每个能力视图实现一次，由 [`attach`] 驱动。
pub trait SessionConsumer: Sized + 'static {
    fn replace_snapshot(&mut self, snapshot: &AgentSnapshot, cx: &mut Context<Self>);
    fn apply_event(&mut self, event: &ServiceEvent, cx: &mut Context<Self>);
    fn mark_stale(&mut self, cx: &mut Context<Self>);
}

/// 快照主体（agent 角色）。
#[must_use]
pub fn agent_body(snapshot: &Snapshot) -> Option<&AgentSnapshot> {
    match &snapshot.body {
        SnapshotBody::Agent(body) => Some(body),
        SnapshotBody::Daemon(_) => None,
    }
}

/// 仅订阅唯一 AgentSession；同线程同步首帧，没有额外窗口连接或异步快照回放。
pub fn attach<V: SessionConsumer>(session: &Entity<AgentSession>, view: &Entity<V>, cx: &mut App) {
    view.update(cx, |_, cx| {
        cx.subscribe(session, move |view, _, signal, cx| match signal {
            SessionSignal::Event(frame) => view.apply_event(&frame.event, cx),
            SessionSignal::Snapshot(snapshot) => {
                if let Some(body) = agent_body(snapshot) {
                    view.replace_snapshot(body, cx);
                }
            }
            SessionSignal::Stale | SessionSignal::Fatal(_) | SessionSignal::ServiceStopped => {
                view.mark_stale(cx);
            }
        })
        .detach();
    });
    let (latest, offline) = {
        let session = session.read(cx);
        (session.latest.clone(), session.offline_notified)
    };
    if let Some(body) = latest.as_deref().and_then(agent_body) {
        view.update(cx, |view, cx| view.replace_snapshot(body, cx));
    }
    if offline {
        view.update(cx, |view, cx| view.mark_stale(cx));
    }
}

/// 严格连续地把帧并入同一 epoch 的快照；旧帧不可倒灌，缺口不可静默跳过。
fn apply_frame(snapshot: &mut Snapshot, frame: &mut EventFrame) -> Result<bool, ()> {
    if snapshot.epoch != frame.epoch {
        return Err(());
    }
    if frame.sequence <= snapshot.sequence {
        return Ok(false);
    }
    if frame.sequence != snapshot.sequence.saturating_add(1) {
        return Err(());
    }
    let (SnapshotBody::Agent(body), ServiceEvent::Agent(event)) =
        (&mut snapshot.body, &frame.event)
    else {
        return Err(());
    };
    let deliver = match event {
        AgentEvent::Daemon(DaemonEvent::TaskRuntimeChanged(runtime)) => {
            accepted_runtime_status(&body.daemon, runtime).is_some()
        }
        _ => true,
    };
    apply_agent_event(body, event);
    if deliver
        && let ServiceEvent::Agent(AgentEvent::Daemon(DaemonEvent::TaskRuntimeChanged(runtime))) =
            &mut frame.event
        && let Some(projected) = body.daemon.task_runtime.get(&runtime.task_id)
    {
        runtime.clone_from(projected);
    }
    snapshot.sequence = frame.sequence;
    Ok(deliver)
}

macro_rules! forward_consumer {
    ($($view:ty),* $(,)?) => {
        $(
            impl SessionConsumer for $view {
                fn replace_snapshot(&mut self, snapshot: &AgentSnapshot, cx: &mut Context<Self>) {
                    <$view>::replace_snapshot(self, snapshot, cx);
                }
                fn apply_event(&mut self, event: &ServiceEvent, cx: &mut Context<Self>) {
                    <$view>::apply_event(self, event, cx);
                }
                fn mark_stale(&mut self, cx: &mut Context<Self>) {
                    <$view>::mark_stale(self, cx);
                }
            }
        )*
    };
}

forward_consumer!(
    fluxdown_ui_downloads::DownloadView,
    fluxdown_ui_downloads::QueueManagerView,
    fluxdown_ui_downloads::TaskDetailView,
    fluxdown_ui_downloads::GroupDetailView,
    fluxdown_ui_settings::SettingsStore,
    fluxdown_ui_account::AccountView,
    fluxdown_ui_rss::RssView,
    fluxdown_ui_extensions::ExtensionsView,
);

#[cfg(test)]
mod tests {
    use fluxdown_protocol::{
        AgentEvent, DaemonEvent, EventFrame, ServiceEvent, Snapshot, SnapshotBody, TaskRuntimeDto,
    };

    use super::apply_frame;

    fn runtime_frame(epoch: &str, sequence: u64, active: u32) -> EventFrame {
        EventFrame {
            epoch: epoch.to_owned(),
            sequence,
            event: ServiceEvent::Agent(AgentEvent::Daemon(DaemonEvent::TaskRuntimeChanged(
                TaskRuntimeDto {
                    task_id: "task".into(),
                    sample_sequence: sequence,
                    active_transfers: Some(active),
                    ..Default::default()
                },
            ))),
        }
    }

    fn source_snapshot(epoch: &str, sequence: u64) -> Snapshot {
        let task = serde_json::from_value(serde_json::json!({
            "taskId":"task", "url":"https://example.com/x", "fileName":"x", "saveDir":"/tmp",
            "status":1, "downloadedBytes":0, "totalBytes":100, "errorMessage":"",
            "createdAt":"1", "proxyUrl":"", "queueId":"main", "checksum":""
        }))
        .expect("task fixture");
        let mut agent = fluxdown_protocol::AgentSnapshot::default();
        agent.daemon.tasks.push(task);
        Snapshot {
            epoch: epoch.into(),
            sequence,
            body: SnapshotBody::Agent(Box::new(agent)),
        }
    }

    #[test]
    fn late_view_sees_live_runtime_and_old_or_gapped_frames_never_rewind_it() {
        let mut snapshot = source_snapshot("a", 5);
        assert_eq!(
            apply_frame(&mut snapshot, &mut runtime_frame("a", 6, 3)),
            Ok(true)
        );
        assert_eq!(
            apply_frame(&mut snapshot, &mut runtime_frame("a", 6, 0)),
            Ok(false)
        );
        assert_eq!(
            apply_frame(&mut snapshot, &mut runtime_frame("b", 7, 0)),
            Err(())
        );
        assert_eq!(
            apply_frame(&mut snapshot, &mut runtime_frame("a", 8, 0)),
            Err(())
        );
        let SnapshotBody::Agent(ref body) = snapshot.body else {
            panic!("agent snapshot")
        };
        assert_eq!(body.daemon.task_runtime["task"].active_transfers, Some(3));
        assert_eq!(snapshot.sequence, 6);
        let mut older = runtime_frame("a", 7, 9);
        if let ServiceEvent::Agent(AgentEvent::Daemon(DaemonEvent::TaskRuntimeChanged(runtime))) =
            &mut older.event
        {
            runtime.sample_sequence = 5;
        }
        assert_eq!(apply_frame(&mut snapshot, &mut older), Ok(false));
        assert_eq!(snapshot.sequence, 7);
        let SnapshotBody::Agent(ref body) = snapshot.body else {
            panic!("agent snapshot")
        };
        assert_eq!(body.daemon.task_runtime["task"].active_transfers, Some(3));
        let mut complete = body.daemon.tasks[0].clone();
        complete.status = 3;
        let mut terminal = EventFrame {
            epoch: "a".into(),
            sequence: 8,
            event: ServiceEvent::Agent(AgentEvent::Daemon(DaemonEvent::TaskChanged(complete))),
        };
        assert_eq!(apply_frame(&mut snapshot, &mut terminal), Ok(true));
        let mut late = runtime_frame("a", 9, 4);
        assert_eq!(apply_frame(&mut snapshot, &mut late), Ok(true));
        let ServiceEvent::Agent(AgentEvent::Daemon(DaemonEvent::TaskRuntimeChanged(runtime))) =
            late.event
        else {
            panic!("runtime event")
        };
        assert_eq!(runtime.active_transfers, Some(0));
        // 重连快照作为新 epoch 的唯一权威基线。
        let mut recovered = source_snapshot("b", 20);
        assert_eq!(
            apply_frame(&mut recovered, &mut runtime_frame("b", 21, 4)),
            Ok(true)
        );
        assert_eq!(
            apply_frame(&mut recovered, &mut runtime_frame("a", 7, 0)),
            Err(())
        );
        let SnapshotBody::Agent(body) = recovered.body else {
            panic!("agent snapshot")
        };
        assert_eq!(body.daemon.task_runtime["task"].active_transfers, Some(4));
    }
}
