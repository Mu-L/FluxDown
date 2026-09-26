use std::sync::Arc;

use fluxdown_protocol::method;
use fluxdown_ui_downloads::{DownloadsCommand, DownloadsPort, DownloadsResult, PortFuture};
use serde_json::{Value, json};

use crate::agent_client::AgentClient;

pub struct AgentDownloadsPort {
    client: Arc<AgentClient>,
}

impl AgentDownloadsPort {
    #[must_use]
    pub fn new(client: Arc<AgentClient>) -> Self {
        Self { client }
    }
}

fn queue_fields(fields: &fluxdown_ui_downloads::QueueFields) -> Value {
    json!({
        "name": fields.name,
        "speedLimitKbps": fields.speed_limit_kbps,
        "uploadLimitKbps": fields.upload_limit_kbps,
        "maxConcurrent": fields.max_concurrent,
        "defaultSaveDir": fields.default_save_dir,
        "defaultSegments": fields.default_segments,
        "defaultUserAgent": fields.default_user_agent,
    })
}

impl DownloadsPort for AgentDownloadsPort {
    fn execute(&self, command: DownloadsCommand) -> PortFuture<DownloadsResult> {
        let client = self.client.clone();
        Box::pin(async move {
            let (method, params) = match command {
                DownloadsCommand::TaskActivity(query) => {
                    let page = client
                        .call(method::DAEMON_TASK_ACTIVITY, Some(query))
                        .await?;
                    return Ok(DownloadsResult::TaskActivity(page));
                }
                DownloadsCommand::Create(params) => {
                    (method::DAEMON_TASK_CREATE, serialize(params)?)
                }
                DownloadsCommand::Pause { task_id } => {
                    (method::DAEMON_TASK_PAUSE, json!({ "taskId": task_id }))
                }
                DownloadsCommand::Resume { task_id } => {
                    (method::DAEMON_TASK_RESUME, json!({ "taskId": task_id }))
                }
                DownloadsCommand::Rename { task_id, file_name } => (
                    method::DAEMON_TASK_RENAME,
                    json!({ "taskId": task_id, "fileName": file_name }),
                ),
                DownloadsCommand::Delete {
                    task_id,
                    delete_files,
                } => (
                    method::DAEMON_TASK_DELETE,
                    json!({ "taskId": task_id, "deleteFiles": delete_files }),
                ),
                DownloadsCommand::PauseAll => (method::DAEMON_TASK_PAUSE_ALL, json!({})),
                DownloadsCommand::ResumeAll => (method::DAEMON_TASK_RESUME_ALL, json!({})),
                DownloadsCommand::IgnorePluginRetry { task_id } => (
                    method::DAEMON_PLUGIN_IGNORE_RETRY,
                    json!({ "taskId": task_id }),
                ),
                DownloadsCommand::ToggleBoost { task_id } => {
                    (method::DAEMON_QUEUE_BOOST, json!({ "id": task_id }))
                }
                DownloadsCommand::Redownload(request, task_id) => {
                    client
                        .call::<Value, Value>(
                            method::DAEMON_TASK_DELETE,
                            Some(json!({ "taskId": task_id, "deleteFiles": true })),
                        )
                        .await?;
                    (
                        method::DAEMON_TASK_CREATE,
                        serialize(fluxdown_protocol::DaemonCreateTaskParams {
                            request: *request,
                            torrent_blob_id: None,
                            unattended: false,
                        })?,
                    )
                }
                DownloadsCommand::MoveToQueue { task_id, queue_id } => (
                    method::DAEMON_QUEUE_MOVE_TASK,
                    json!({ "taskId": task_id, "queueId": queue_id }),
                ),
                DownloadsCommand::SetSeedLimits { task_id, limits } => (
                    method::DAEMON_TASK_SET_SEED_LIMITS,
                    json!({
                        "taskId": task_id,
                        "ratioLimitMilli": limits.ratio_limit_milli,
                        "postRatioLimitMilli": limits.post_ratio_limit_milli,
                        "seedTimeLimitMinutes": limits.seed_time_limit_minutes,
                        "inactiveTimeLimitMinutes": limits.inactive_time_limit_minutes,
                        "uploadLimitBps": limits.upload_limit_bps,
                    }),
                ),
                DownloadsCommand::PatchConfig {
                    values,
                    expected_revision,
                } => (
                    method::DAEMON_CONFIG_PATCH,
                    json!({ "expectedRevision": expected_revision, "values": values }),
                ),
                DownloadsCommand::QueueCreate(fields) => {
                    (method::DAEMON_QUEUE_CREATE, queue_fields(&fields))
                }
                DownloadsCommand::QueueUpdate { queue_id, fields } => {
                    let mut params = queue_fields(&fields);
                    if let Value::Object(map) = &mut params {
                        map.insert("queueId".to_owned(), Value::String(queue_id));
                    }
                    (method::DAEMON_QUEUE_UPDATE, params)
                }
                DownloadsCommand::QueueSchedule {
                    queue_id,
                    enabled,
                    start_time,
                    stop_time,
                    days,
                } => (
                    method::DAEMON_QUEUE_SCHEDULE,
                    json!({
                        "queueId": queue_id,
                        "enabled": enabled,
                        "startTime": start_time,
                        "stopTime": stop_time,
                        "days": days,
                    }),
                ),
                DownloadsCommand::QueueStart { queue_id } => {
                    (method::DAEMON_QUEUE_START, json!({ "queueId": queue_id }))
                }
                DownloadsCommand::QueueStop { queue_id } => {
                    (method::DAEMON_QUEUE_STOP, json!({ "queueId": queue_id }))
                }
                DownloadsCommand::QueueDelete { queue_id } => {
                    (method::DAEMON_QUEUE_DELETE, json!({ "queueId": queue_id }))
                }
                DownloadsCommand::GroupPause { group_id } => {
                    (method::DAEMON_GROUP_PAUSE, json!({ "groupId": group_id }))
                }
                DownloadsCommand::GroupResume { group_id } => {
                    (method::DAEMON_GROUP_RESUME, json!({ "groupId": group_id }))
                }
                DownloadsCommand::GroupDelete {
                    group_id,
                    delete_files,
                } => (
                    method::DAEMON_GROUP_DELETE,
                    json!({ "groupId": group_id, "deleteFiles": delete_files }),
                ),
                DownloadsCommand::ResolveSelection(params) => {
                    (method::DAEMON_SELECTION_RESOLVE, serialize(params)?)
                }
                DownloadsCommand::CaptureResolve(params) => {
                    (method::AGENT_CAPTURE_RESOLVE, serialize(params)?)
                }
                DownloadsCommand::SiteAuthMatch { url } => (
                    method::DAEMON_SITE_AUTH_MATCH,
                    serialize(fluxdown_protocol::SiteAuthMatchParams { url })?,
                ),
                DownloadsCommand::RemoteDispatch(params) => (method::AGENT_REMOTE_DISPATCH, params),
                DownloadsCommand::RemoteCommand(params) => (method::AGENT_REMOTE_COMMAND, params),
                DownloadsCommand::OpenTask { task_id } => (
                    method::AGENT_PLATFORM_OPEN_TASK,
                    json!({ "taskId": task_id }),
                ),
                DownloadsCommand::RevealTask { task_id } => (
                    method::AGENT_PLATFORM_REVEAL_TASK,
                    json!({ "taskId": task_id }),
                ),
                DownloadsCommand::SubmitTorrentFile { path } => (
                    method::AGENT_CAPTURE_SUBMIT_TORRENT_FILE,
                    json!({ "path": path, "silent": true }),
                ),
                DownloadsCommand::SetLocalPreference { key, value } => (
                    method::AGENT_PREFERENCES_PATCH,
                    json!({ "values": { key: value }, "sync": false }),
                ),
                DownloadsCommand::SetSyncedPreference { key, value } => (
                    method::AGENT_PREFERENCES_PATCH,
                    json!({ "values": { key: value } }),
                ),
            };
            let value: Value = client.call(method, Some(params)).await?;
            Ok(if value == json!({ "ok": true }) {
                DownloadsResult::Unit
            } else {
                DownloadsResult::Value(value)
            })
        })
    }
}

fn serialize<T: serde::Serialize>(value: T) -> Result<Value, fluxdown_protocol::RpcErrorData> {
    serde_json::to_value(value).map_err(|_| {
        fluxdown_protocol::RpcErrorData::new(
            fluxdown_protocol::ApplicationErrorCode::Internal,
            false,
        )
    })
}
