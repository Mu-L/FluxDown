//! 可重放事件帧与全量快照契约。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::agent::{
    AgentPreferencesDto, AgentSessionDto, CloudDevice, GatewayStatusDto, PendingCaptureDto,
    PowerStatusDto, RemoteTaskDto, ShellStatusDto, SyncStatusDto,
};
use crate::daemon::{
    ComponentStatusDto, DaemonConfigSnapshot, DaemonRuntimeStatsDto, GroupDto, LinkDeviceInfo,
    PluginDto, QueueDto, QueuePositionDto, RssSourceDto, SelectionRequestDto, TaskDto,
    WebhookDeliveryDto, WsServerMsg,
};

/// daemon 的完整物化投影。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct DaemonSnapshot {
    #[serde(default)]
    pub task_runtime: BTreeMap<String, crate::TaskRuntimeDto>,
    pub tasks: Vec<TaskDto>,
    pub queues: Vec<QueueDto>,
    pub queue_positions: Vec<QueuePositionDto>,
    pub groups: Vec<GroupDto>,
    pub config: DaemonConfigSnapshot,
    pub rss_sources: Vec<RssSourceDto>,
    pub rss_item_revisions: BTreeMap<String, u64>,
    pub plugins: Vec<PluginDto>,
    pub components: Vec<ComponentStatusDto>,
    pub webhook_deliveries: Vec<WebhookDeliveryDto>,
    pub priority: Vec<String>,
    pub runtime_stats: DaemonRuntimeStatsDto,
    pub pending_selections: Vec<SelectionRequestDto>,
}

/// agent 的完整物化投影。下载事实只来自嵌套 daemon 快照。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct AgentSnapshot {
    pub daemon: DaemonSnapshot,
    #[serde(default)]
    pub daemon_connected: bool,
    pub session: Option<AgentSessionDto>,
    pub sync: SyncStatusDto,
    pub preferences: AgentPreferencesDto,
    pub gateway: GatewayStatusDto,
    pub cloud_devices: Vec<CloudDevice>,
    pub linked_devices: Vec<LinkDeviceInfo>,
    pub remote_tasks: Vec<RemoteTaskDto>,
    pub pending_captures: Vec<PendingCaptureDto>,
    #[serde(default)]
    pub shell: ShellStatusDto,
    #[serde(default)]
    pub power: PowerStatusDto,
}

/// `system.snapshot` 的服务角色对应主体。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "role", content = "snapshot", rename_all = "camelCase")]
pub enum SnapshotBody {
    Daemon(Box<DaemonSnapshot>),
    Agent(Box<AgentSnapshot>),
}

/// 带原子事件位置的全量快照。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub epoch: String,
    pub sequence: u64,
    pub body: SnapshotBody,
}

/// daemon 状态变化事件。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DaemonEvent {
    TaskRuntimeChanged(crate::TaskRuntimeDto),
    TaskActivityAdded(crate::TaskActivityDto),
    SnapshotReplaced(DaemonSnapshot),
    Engine(WsServerMsg),
    TaskChanged(TaskDto),
    TaskDeleted {
        task_id: String,
    },
    QueuesChanged(Vec<QueueDto>),
    GroupsChanged(Vec<GroupDto>),
    ConfigChanged(DaemonConfigSnapshot),
    RssChanged {
        source_id: String,
        item_revision: u64,
    },
    PluginsChanged(Vec<PluginDto>),
    ComponentsChanged(Vec<ComponentStatusDto>),
    WebhooksChanged(Vec<WebhookDeliveryDto>),
    RuntimeStatsChanged(DaemonRuntimeStatsDto),
    SelectionPending(SelectionRequestDto),
    SelectionResolved {
        request_id: String,
    },
}

/// agent 自有或转发的状态变化事件。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AgentEvent {
    Daemon(DaemonEvent),
    DaemonSnapshotReplaced(DaemonSnapshot),
    DaemonConnectionChanged(bool),
    SessionChanged(Box<Option<AgentSessionDto>>),
    SyncChanged(SyncStatusDto),
    PreferencesChanged(AgentPreferencesDto),
    GatewayChanged(GatewayStatusDto),
    CloudDevicesChanged(Vec<CloudDevice>),
    LinkedDevicesChanged(Vec<LinkDeviceInfo>),
    RemoteTasksChanged(Vec<RemoteTaskDto>),
    PendingCapturesChanged(Vec<PendingCaptureDto>),
    ShellChanged(ShellStatusDto),
    PowerChanged(PowerStatusDto),
}

/// `service.event` notification 的事件主体。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "service", content = "event", rename_all = "camelCase")]
pub enum ServiceEvent {
    Daemon(DaemonEvent),
    Agent(AgentEvent),
}

/// 单个严格递增的服务事件帧。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct EventFrame {
    pub epoch: String,
    pub sequence: u64,
    pub event: ServiceEvent,
}

/// 将同源 agent 增量应用于完整 DTO 投影；daemon 与桌面会话共享此规则。
pub fn apply_agent_event(snapshot: &mut AgentSnapshot, event: &AgentEvent) {
    match event {
        AgentEvent::Daemon(event) => apply_daemon_event(&mut snapshot.daemon, event),
        AgentEvent::DaemonSnapshotReplaced(daemon) => snapshot.daemon.clone_from(daemon),
        AgentEvent::DaemonConnectionChanged(connected) => {
            snapshot.daemon_connected = *connected;
            if !*connected {
                snapshot.daemon.task_runtime.clear();
            }
        }
        AgentEvent::SessionChanged(session) => snapshot.session.clone_from(session.as_ref()),
        AgentEvent::SyncChanged(sync) => snapshot.sync.clone_from(sync),
        AgentEvent::PreferencesChanged(preferences) => snapshot.preferences.clone_from(preferences),
        AgentEvent::GatewayChanged(gateway) => snapshot.gateway.clone_from(gateway),
        AgentEvent::CloudDevicesChanged(devices) => snapshot.cloud_devices.clone_from(devices),
        AgentEvent::LinkedDevicesChanged(devices) => snapshot.linked_devices.clone_from(devices),
        AgentEvent::RemoteTasksChanged(tasks) => snapshot.remote_tasks.clone_from(tasks),
        AgentEvent::PendingCapturesChanged(captures) => {
            snapshot.pending_captures.clone_from(captures)
        }
        AgentEvent::ShellChanged(shell) => snapshot.shell.clone_from(shell),
        AgentEvent::PowerChanged(power) => snapshot.power = *power,
    }
}

/// 若任务存在且采样没有落后于已投影的源端序号，返回任务状态。
#[must_use]
pub fn accepted_runtime_status(
    snapshot: &DaemonSnapshot,
    runtime: &crate::TaskRuntimeDto,
) -> Option<i32> {
    if snapshot
        .task_runtime
        .get(&runtime.task_id)
        .is_some_and(|previous| {
            previous.sample_sequence != 0 && runtime.sample_sequence <= previous.sample_sequence
        })
    {
        return None;
    }
    snapshot
        .tasks
        .iter()
        .find(|task| task.task_id == runtime.task_id)
        .map(|task| task.status)
}

pub fn apply_daemon_event(snapshot: &mut DaemonSnapshot, event: &DaemonEvent) {
    match event {
        DaemonEvent::TaskRuntimeChanged(runtime) => {
            let Some(status) = accepted_runtime_status(snapshot, runtime) else {
                return;
            };
            let inactive = !matches!(status, 1 | 5);
            let mut runtime = runtime.clone();
            if runtime.segments.is_empty()
                && let Some(previous) = snapshot.task_runtime.get(&runtime.task_id)
            {
                runtime.segments.clone_from(&previous.segments);
            }
            if inactive {
                runtime.active_transfers = Some(0);
                for segment in &mut runtime.segments {
                    segment.active = Some(false);
                }
            }
            snapshot
                .task_runtime
                .insert(runtime.task_id.clone(), runtime);
        }
        DaemonEvent::TaskActivityAdded(_) => {}
        DaemonEvent::SnapshotReplaced(replacement) => snapshot.clone_from(replacement),
        DaemonEvent::Engine(message) => apply_engine_message(snapshot, message),
        DaemonEvent::TaskChanged(task) => {
            if let Some(existing) = snapshot
                .tasks
                .iter_mut()
                .find(|item| item.task_id == task.task_id)
            {
                existing.clone_from(task);
            } else {
                snapshot.tasks.push(task.clone());
            }
            if !matches!(task.status, 1 | 5) {
                clear_active_runtime(snapshot, &task.task_id);
            }
        }
        DaemonEvent::TaskDeleted { task_id } => {
            snapshot.tasks.retain(|task| task.task_id != *task_id);
            snapshot.task_runtime.remove(task_id);
        }
        DaemonEvent::QueuesChanged(queues) => snapshot.queues.clone_from(queues),
        DaemonEvent::GroupsChanged(groups) => snapshot.groups.clone_from(groups),
        DaemonEvent::ConfigChanged(config) => snapshot.config.clone_from(config),
        DaemonEvent::RssChanged {
            source_id,
            item_revision,
        } => {
            snapshot
                .rss_item_revisions
                .insert(source_id.clone(), *item_revision);
        }
        DaemonEvent::PluginsChanged(plugins) => snapshot.plugins.clone_from(plugins),
        DaemonEvent::ComponentsChanged(components) => snapshot.components.clone_from(components),
        DaemonEvent::WebhooksChanged(deliveries) => {
            snapshot.webhook_deliveries.clone_from(deliveries)
        }
        DaemonEvent::RuntimeStatsChanged(stats) => snapshot.runtime_stats.clone_from(stats),
        DaemonEvent::SelectionPending(request) => {
            snapshot
                .pending_selections
                .retain(|item| item.request_id != request.request_id);
            snapshot.pending_selections.push(request.clone());
        }
        DaemonEvent::SelectionResolved { request_id } => {
            snapshot
                .pending_selections
                .retain(|item| item.request_id != *request_id);
        }
    }
}

fn apply_engine_message(snapshot: &mut DaemonSnapshot, message: &WsServerMsg) {
    match message {
        WsServerMsg::TasksSnapshot { tasks } => {
            snapshot.tasks.clone_from(tasks);
            snapshot
                .task_runtime
                .retain(|id, _| tasks.iter().any(|task| task.task_id == *id));
            for task in tasks {
                if !matches!(task.status, 1 | 5) {
                    clear_active_runtime(snapshot, &task.task_id);
                }
            }
        }
        WsServerMsg::TaskProgress {
            task_id,
            status,
            downloaded_bytes,
            total_bytes,
            file_name,
            save_dir,
            url,
            error_message,
            uploaded_bytes,
            seeding_status,
            seeding_message,
            seeding_time_secs,
            ..
        } => {
            if *status == 4 && error_message == "deleted" {
                snapshot.task_runtime.remove(task_id);
                snapshot.tasks.retain(|task| task.task_id != *task_id);
                return;
            }
            if !matches!(*status, 1 | 5) {
                clear_active_runtime(snapshot, task_id);
            }
            if let Some(task) = snapshot
                .tasks
                .iter_mut()
                .find(|task| task.task_id == *task_id)
            {
                task.status = *status;
                task.downloaded_bytes = *downloaded_bytes;
                task.total_bytes = *total_bytes;
                if !file_name.is_empty() {
                    task.file_name.clone_from(file_name);
                }
                if !save_dir.is_empty() {
                    task.save_dir.clone_from(save_dir);
                }
                if !url.is_empty() {
                    task.url.clone_from(url);
                }
                task.error_message.clone_from(error_message);
                task.uploaded_bytes = *uploaded_bytes;
                task.seeding_status = *seeding_status;
                task.seeding_message.clone_from(seeding_message);
                task.seeding_time_secs = *seeding_time_secs;
            }
        }
        WsServerMsg::SegmentProgress {
            task_id,
            total_bytes,
            segments,
            ..
        } => {
            let Some(task) = snapshot.tasks.iter().find(|task| task.task_id == *task_id) else {
                return;
            };
            let inactive = !matches!(task.status, 1 | 5);
            let runtime = snapshot
                .task_runtime
                .entry(task_id.clone())
                .or_insert_with(|| crate::TaskRuntimeDto {
                    task_id: task_id.clone(),
                    active_transfers: inactive.then_some(0),
                    ..Default::default()
                });
            // 持久几何无传输采样；真实采样存在时不得覆盖它的分段/活跃读数。
            if runtime.sample_sequence == 0 {
                runtime.total_bytes = *total_bytes;
                runtime.segments = segments
                    .iter()
                    .map(|segment| crate::TaskSegmentDto {
                        index: segment.index,
                        start_byte: segment.start_byte,
                        end_byte: segment.end_byte,
                        downloaded_bytes: segment.downloaded_bytes,
                        active: inactive.then_some(false),
                    })
                    .collect();
            }
        }
        WsServerMsg::TaskMetaProbed {
            task_id,
            file_name,
            total_bytes,
        } => {
            if let Some(task) = snapshot
                .tasks
                .iter_mut()
                .find(|task| task.task_id == *task_id)
            {
                if !file_name.is_empty() {
                    task.file_name.clone_from(file_name);
                }
                task.total_bytes = *total_bytes;
            }
        }
        WsServerMsg::TaskQueueChanged { task_id, queue_id } => {
            if let Some(task) = snapshot
                .tasks
                .iter_mut()
                .find(|task| task.task_id == *task_id)
            {
                task.queue_id.clone_from(queue_id);
            }
        }
        WsServerMsg::TaskRouteChanged { task_id, route } => {
            if let Some(task) = snapshot
                .tasks
                .iter_mut()
                .find(|task| task.task_id == *task_id)
            {
                task.auto_route.clone_from(route);
            }
        }
        WsServerMsg::QueuesChanged { queues } => snapshot.queues.clone_from(queues),
        WsServerMsg::QueuePositionsChanged { positions } => {
            snapshot.queue_positions.clone_from(positions);
        }
        WsServerMsg::GroupsChanged { groups } => snapshot.groups.clone_from(groups),
        WsServerMsg::RssSourcesChanged { sources } => snapshot.rss_sources.clone_from(sources),
        WsServerMsg::RssItemsChanged { source_id, .. } => {
            let revision = snapshot
                .rss_item_revisions
                .entry(source_id.clone())
                .or_default();
            *revision = revision.saturating_add(1);
        }
        WsServerMsg::WebhookDeliveriesChanged { deliveries } => {
            snapshot.webhook_deliveries.clone_from(deliveries);
        }
        WsServerMsg::FileMissingChanged { updates } => {
            for update in updates {
                if let Some(task) = snapshot
                    .tasks
                    .iter_mut()
                    .find(|task| task.task_id == update.task_id)
                {
                    task.file_missing = update.missing;
                }
            }
        }
        WsServerMsg::PriorityTaskChanged {
            priority_task_id, ..
        } => {
            snapshot.priority.clear();
            if !priority_task_id.is_empty() {
                snapshot.priority.push(priority_task_id.clone());
            }
        }
        WsServerMsg::PluginAutoDisabled { identity, reason } => {
            if let Some(plugin) = snapshot
                .plugins
                .iter_mut()
                .find(|plugin| plugin.identity == *identity)
            {
                plugin.enabled = false;
                plugin.disabled_reason.clone_from(reason);
            }
        }
        _ => {}
    }
}

fn clear_active_runtime(snapshot: &mut DaemonSnapshot, task_id: &str) {
    if let Some(runtime) = snapshot.task_runtime.get_mut(task_id) {
        runtime.active_transfers = Some(0);
        runtime.connected_peers = Some(0);
        for segment in &mut runtime.segments {
            segment.active = Some(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{DaemonEvent, EventFrame, ServiceEvent, Snapshot, SnapshotBody};

    #[test]
    fn snapshot_carries_atomic_epoch_and_sequence() -> Result<(), serde_json::Error> {
        let snapshot = Snapshot {
            epoch: "epoch-a".to_owned(),
            sequence: 41,
            body: SnapshotBody::Daemon(Box::default()),
        };
        let wire = serde_json::to_value(&snapshot)?;
        assert_eq!(wire["epoch"], "epoch-a");
        assert_eq!(wire["sequence"], 41);
        assert_eq!(wire["body"]["role"], "daemon");
        assert!(wire["body"]["snapshot"]["tasks"].is_array());
        Ok(())
    }

    #[test]
    fn event_frame_uses_strict_monotonic_cursor_shape() -> Result<(), serde_json::Error> {
        let frame = EventFrame {
            epoch: "epoch-a".to_owned(),
            sequence: 42,
            event: ServiceEvent::Daemon(DaemonEvent::TaskDeleted {
                task_id: "task-1".to_owned(),
            }),
        };
        let wire = serde_json::to_value(frame)?;
        assert_eq!(
            wire,
            json!({
                "epoch": "epoch-a",
                "sequence": 42,
                "event": {
                    "service": "daemon",
                    "event": {
                        "type": "taskDeleted",
                        "data": { "taskId": "task-1" }
                    }
                }
            })
        );
        Ok(())
    }

    #[test]
    fn restart_geometry_never_invents_active_reads_or_overrides_live_sample()
    -> Result<(), serde_json::Error> {
        let task: super::TaskDto = serde_json::from_value(json!({
            "taskId":"t", "url":"https://example.com/t", "fileName":"t", "saveDir":"/tmp",
            "status":2, "downloadedBytes":25, "totalBytes":100, "errorMessage":"",
            "createdAt":"1", "proxyUrl":"", "queueId":"main", "checksum":""
        }))?;
        let mut snapshot = super::DaemonSnapshot {
            tasks: vec![task],
            ..Default::default()
        };
        let geometry = DaemonEvent::Engine(super::WsServerMsg::SegmentProgress {
            task_id: "t".into(),
            total_bytes: 100,
            segment_count: 1,
            segments: vec![crate::daemon::SegmentDetailDto {
                index: 0,
                start_byte: 0,
                end_byte: 99,
                downloaded_bytes: 25,
            }],
        });
        super::apply_daemon_event(&mut snapshot, &geometry);
        assert_eq!(snapshot.task_runtime["t"].segments[0].downloaded_bytes, 25);
        assert_eq!(snapshot.task_runtime["t"].active_transfers, Some(0));
        assert_eq!(snapshot.task_runtime["t"].segments[0].active, Some(false));
        let mut active = snapshot.tasks[0].clone();
        active.status = 1;
        super::apply_daemon_event(&mut snapshot, &DaemonEvent::TaskChanged(active.clone()));
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::TaskRuntimeChanged(crate::TaskRuntimeDto {
                task_id: "t".into(),
                sampled_at_ms: 1234,
                sample_sequence: 10,
                active_transfers: Some(2),
                total_bytes: 100,
                segments: vec![crate::TaskSegmentDto {
                    index: 0,
                    start_byte: 0,
                    end_byte: 99,
                    downloaded_bytes: 40,
                    active: Some(true),
                }],
                ..Default::default()
            }),
        );
        super::apply_daemon_event(&mut snapshot, &geometry);
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::TaskRuntimeChanged(crate::TaskRuntimeDto {
                task_id: "t".into(),
                sampled_at_ms: 5000,
                sample_sequence: 9,
                active_transfers: Some(1),
                segments: vec![crate::TaskSegmentDto {
                    downloaded_bytes: 20,
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        assert_eq!(snapshot.task_runtime["t"].segments[0].downloaded_bytes, 40);
        assert_eq!(snapshot.task_runtime["t"].active_transfers, Some(2));
        active.status = 3;
        super::apply_daemon_event(&mut snapshot, &DaemonEvent::TaskChanged(active));
        assert_eq!(snapshot.task_runtime["t"].active_transfers, Some(0));
        assert_eq!(snapshot.task_runtime["t"].segments[0].active, Some(false));
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::TaskRuntimeChanged(crate::TaskRuntimeDto {
                task_id: "t".into(),
                sampled_at_ms: 1234,
                sample_sequence: 10,
                active_transfers: Some(1),
                segments: vec![crate::TaskSegmentDto {
                    downloaded_bytes: 20,
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        assert_eq!(snapshot.task_runtime["t"].segments[0].downloaded_bytes, 40);
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::TaskRuntimeChanged(crate::TaskRuntimeDto {
                task_id: "t".into(),
                sampled_at_ms: 100,
                sample_sequence: 11,
                active_transfers: Some(0),
                segments: vec![crate::TaskSegmentDto {
                    downloaded_bytes: 100,
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        assert_eq!(snapshot.task_runtime["t"].segments[0].downloaded_bytes, 100);
        assert_eq!(snapshot.task_runtime["t"].active_transfers, Some(0));
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::TaskRuntimeChanged(crate::TaskRuntimeDto {
                task_id: "t".into(),
                sampled_at_ms: 9000,
                sample_sequence: 0,
                active_transfers: Some(1),
                ..Default::default()
            }),
        );
        assert_eq!(snapshot.task_runtime["t"].segments[0].downloaded_bytes, 100);
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::TaskDeleted {
                task_id: "t".into(),
            },
        );
        assert!(snapshot.task_runtime.is_empty());
        super::apply_daemon_event(
            &mut snapshot,
            &DaemonEvent::TaskRuntimeChanged(crate::TaskRuntimeDto {
                task_id: "t".into(),
                active_transfers: Some(3),
                ..Default::default()
            }),
        );
        assert!(
            snapshot.task_runtime.is_empty(),
            "late sample must not resurrect deleted task"
        );
        Ok(())
    }
}
