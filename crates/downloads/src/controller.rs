use std::{
    collections::{BTreeMap, HashMap},
    future::Future,
    pin::Pin,
    rc::Rc,
    sync::Arc,
};

use fluxdown_protocol::{
    AgentEvent, AgentSnapshot, CloudDevice, DaemonEvent, DaemonRuntimeStatsDto, DaemonSnapshot,
    GroupDto, LinkDeviceInfo, QueueDto, RemoteTaskDto, ServiceEvent, TaskActivityPage,
    TaskActivityQuery, TaskDto, TaskRuntimeDto, WsServerMsg,
};

use crate::model::{CategoryIndex, DownloadTaskView, TaskState, TaskStore};

/// 本机偏好：新建下载对话框上次使用的保存目录（设备本地，不进云同步）。
pub const LAST_SAVE_DIR_PREF: &str = "download.last_save_dir";
/// 偏好：新建下载默认沿用上次保存目录。
pub const REMEMBER_LAST_SAVE_DIR_PREF: &str = "download.remember_last_save_dir";

pub type PortFuture<T> =
    Pin<Box<dyn Future<Output = Result<T, fluxdown_protocol::RpcErrorData>> + Send + 'static>>;

/// 队列创建 / 更新表单字段（与 `CreateQueueRequest` 一一对应）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueueFields {
    pub name: String,
    pub speed_limit_kbps: i64,
    pub upload_limit_kbps: i64,
    pub max_concurrent: i32,
    pub default_save_dir: String,
    pub default_segments: i32,
    pub default_user_agent: String,
}

/// BT 做种上限（`daemon.task.setSeedLimits`）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SeedLimits {
    pub ratio_limit_milli: i64,
    pub post_ratio_limit_milli: i64,
    pub seed_time_limit_minutes: i64,
    pub inactive_time_limit_minutes: i64,
    pub upload_limit_bps: i64,
}

pub enum DownloadsCommand {
    /// 分页读取 daemon 持久活动历史。
    TaskActivity(TaskActivityQuery),
    Create(Box<fluxdown_protocol::DaemonCreateTaskParams>),
    Pause {
        task_id: String,
    },
    Resume {
        task_id: String,
    },
    Rename {
        task_id: String,
        file_name: String,
    },
    Delete {
        task_id: String,
        delete_files: bool,
    },
    PauseAll,
    ResumeAll,
    /// 插件重试挂起 → 忽略并按普通任务继续。
    IgnorePluginRetry {
        task_id: String,
    },
    /// `daemon.queue.boost`：同 id 再次调用即取消。
    ToggleBoost {
        task_id: String,
    },
    /// 删除（含文件）后按原参数重建。
    Redownload(Box<fluxdown_protocol::CreateTaskRequest>, String),
    MoveToQueue {
        task_id: String,
        queue_id: String,
    },
    SetSeedLimits {
        task_id: String,
        limits: SeedLimits,
    },
    /// daemon 公开配置增量写入（乐观并发：`expected_revision`）。
    PatchConfig {
        values: BTreeMap<String, String>,
        expected_revision: u64,
    },
    QueueCreate(QueueFields),
    QueueUpdate {
        queue_id: String,
        fields: QueueFields,
    },
    QueueSchedule {
        queue_id: String,
        enabled: bool,
        start_time: String,
        stop_time: String,
        days: i32,
    },
    QueueStart {
        queue_id: String,
    },
    QueueStop {
        queue_id: String,
    },
    QueueDelete {
        queue_id: String,
    },
    GroupPause {
        group_id: String,
    },
    GroupResume {
        group_id: String,
    },
    GroupDelete {
        group_id: String,
        delete_files: bool,
    },
    ResolveSelection(fluxdown_protocol::SelectionResolutionDto),
    /// 外部捕获确认 / 忽略；确认时携带表单产出的建任务参数，由 agent 与捕获原请求合并。
    CaptureResolve(Box<fluxdown_protocol::CaptureResolveParams>),
    /// 按下载链接匹配已保存的站点 HTTP 凭据（新建下载表单自动回填）。
    SiteAuthMatch {
        url: String,
    },
    RemoteDispatch(serde_json::Value),
    RemoteCommand(serde_json::Value),
    OpenTask {
        task_id: String,
    },
    RevealTask {
        task_id: String,
    },
    /// 本机 `.torrent` 文件：agent 读取、上传 blob 后按捕获路径建任务。
    SubmitTorrentFile {
        path: String,
    },
    /// 设备本地偏好写入（`sync:false`，不进云同步）。
    SetLocalPreference {
        key: &'static str,
        value: serde_json::Value,
    },
    /// 云同步偏好写入（`ui.*` 等）。
    SetSyncedPreference {
        key: &'static str,
        value: serde_json::Value,
    },
}

pub enum DownloadsResult {
    Unit,
    Value(serde_json::Value),
    TaskActivity(TaskActivityPage),
}

impl DownloadsResult {
    /// 建任务类命令（`daemon.task.create` → `{taskId}`、`agent.capture.resolve` /
    /// 种子上传 → `{taskIds}`）返回的新任务 ID；其它结果为空。
    #[must_use]
    pub fn created_task_ids(&self) -> Vec<String> {
        let Self::Value(value) = self else {
            return Vec::new();
        };
        if let Some(id) = value.get("taskId").and_then(serde_json::Value::as_str) {
            return vec![id.to_owned()];
        }
        value
            .get("taskIds")
            .and_then(serde_json::Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }
}

pub trait DownloadsPort: Send + Sync {
    fn execute(&self, command: DownloadsCommand) -> PortFuture<DownloadsResult>;
}

/// 任务组聚合（由任务行 + `snapshot.daemon.groups` 计算）。
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct GroupSummary {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) origin_url: String,
    pub(crate) save_dir: String,
    pub(crate) total: usize,
    pub(crate) completed: usize,
    pub(crate) failed: usize,
    pub(crate) downloading: usize,
    pub(crate) progress: f32,
}

pub struct DownloadsController {
    port: Arc<dyn DownloadsPort>,
    local: Vec<TaskDto>,
    remote: Vec<RemoteTaskDto>,
    store: Rc<TaskStore>,
    live_speeds: HashMap<String, i64>,
    task_runtime: BTreeMap<String, Rc<TaskRuntimeDto>>,
    boosted: Option<String>,
    queues: Vec<QueueDto>,
    groups: Vec<GroupDto>,
    group_summaries: Vec<GroupSummary>,
    group_summaries_generation: u64,
    cloud_devices: Vec<CloudDevice>,
    linked_devices: Vec<LinkDeviceInfo>,
    config: BTreeMap<String, String>,
    config_revision: u64,
    runtime_stats: DaemonRuntimeStatsDto,
    preferences: BTreeMap<String, serde_json::Value>,
    categories: Rc<CategoryIndex>,
    stale: bool,
}

impl DownloadsController {
    #[must_use]
    pub fn new(port: Arc<dyn DownloadsPort>) -> Self {
        Self {
            port,
            local: Vec::new(),
            remote: Vec::new(),
            store: Rc::new(TaskStore::default()),
            live_speeds: HashMap::new(),
            task_runtime: BTreeMap::new(),
            boosted: None,
            queues: Vec::new(),
            groups: Vec::new(),
            group_summaries: Vec::new(),
            group_summaries_generation: u64::MAX,
            cloud_devices: Vec::new(),
            linked_devices: Vec::new(),
            config: BTreeMap::new(),
            config_revision: 0,
            runtime_stats: DaemonRuntimeStatsDto::default(),
            preferences: BTreeMap::new(),
            categories: Rc::new(CategoryIndex::from_preference(None)),
            stale: true,
        }
    }

    pub fn replace_snapshot(&mut self, snapshot: &AgentSnapshot) {
        self.live_speeds.clear();
        self.local.clone_from(&snapshot.daemon.tasks);
        self.remote.clone_from(&snapshot.remote_tasks);
        self.cloud_devices.clone_from(&snapshot.cloud_devices);
        self.linked_devices.clone_from(&snapshot.linked_devices);
        self.absorb_daemon_context(&snapshot.daemon);
        self.set_preferences(&snapshot.preferences.values);
        self.stale = !snapshot.daemon_connected;
        if self.stale {
            self.task_runtime.clear();
        }
        self.rebuild_all();
        self.rebuild_remote();
    }

    /// 应用事件；返回 `true` 表示任务行 / 队列 / 组等表格可见数据有变化。
    pub fn apply_event(&mut self, event: &ServiceEvent) -> bool {
        let ServiceEvent::Agent(event) = event else {
            return false;
        };
        match event {
            AgentEvent::DaemonSnapshotReplaced(snapshot) => {
                self.live_speeds.clear();
                self.local.clone_from(&snapshot.tasks);
                self.absorb_daemon_context(snapshot);
                self.stale = false;
                self.rebuild_all();
                true
            }
            AgentEvent::DaemonConnectionChanged(connected) => {
                self.stale = !connected;
                if !connected {
                    self.live_speeds.clear();
                    self.task_runtime.clear();
                }
                self.rebuild_all();
                true
            }
            AgentEvent::RemoteTasksChanged(tasks) => {
                self.remote.clone_from(tasks);
                self.rebuild_remote();
                true
            }
            AgentEvent::CloudDevicesChanged(devices) => {
                self.cloud_devices.clone_from(devices);
                true
            }
            AgentEvent::LinkedDevicesChanged(devices) => {
                self.linked_devices.clone_from(devices);
                true
            }
            AgentEvent::PreferencesChanged(preferences) => {
                self.set_preferences(&preferences.values);
                true
            }
            AgentEvent::Daemon(event) => self.apply_daemon_event(event),
            _ => false,
        }
    }

    pub fn mark_stale(&mut self) {
        self.stale = true;
        self.task_runtime.clear();
        self.live_speeds.clear();
        self.rebuild_all();
    }

    #[must_use]
    pub fn is_stale(&self) -> bool {
        self.stale
    }

    /// 共享任务行存储（表格 / 详情按下标读取）。
    #[must_use]
    pub(crate) fn store(&self) -> &Rc<TaskStore> {
        &self.store
    }

    /// 本地任务的原始 DTO（重新下载等需要完整参数）。
    #[must_use]
    pub(crate) fn task_dto(&self, task_id: &str) -> Option<&TaskDto> {
        self.store
            .find_local(task_id)
            .and_then(|ix| self.local.get(ix))
    }

    #[must_use]
    pub(crate) fn task_runtime(&self, task_id: &str) -> Option<&TaskRuntimeDto> {
        self.task_runtime.get(task_id).map(Rc::as_ref)
    }
    #[must_use]
    pub(crate) fn categories(&self) -> &Rc<CategoryIndex> {
        &self.categories
    }

    /// daemon 队列清单（快照顺序）。
    #[must_use]
    pub(crate) fn queues(&self) -> &[QueueDto] {
        &self.queues
    }

    #[must_use]
    pub(crate) fn queue_name(&self, queue_id: &str) -> Option<&str> {
        self.queues
            .iter()
            .find(|queue| queue.queue_id == queue_id)
            .map(|queue| queue.name.as_str())
    }

    #[must_use]
    pub(crate) fn groups(&self) -> &[GroupDto] {
        &self.groups
    }

    /// 任务组聚合；按存储 generation 缓存。
    pub(crate) fn group_summaries(&mut self) -> &[GroupSummary] {
        let generation = self.store.generation();
        if self.group_summaries_generation != generation {
            self.group_summaries = compute_group_summaries(&self.groups, &self.store.local());
            self.group_summaries_generation = generation;
        }
        &self.group_summaries
    }

    #[must_use]
    pub(crate) fn cloud_devices(&self) -> &[CloudDevice] {
        &self.cloud_devices
    }

    #[must_use]
    pub(crate) fn linked_devices(&self) -> &[LinkDeviceInfo] {
        &self.linked_devices
    }

    #[must_use]
    pub(crate) fn runtime_stats(&self) -> &DaemonRuntimeStatsDto {
        &self.runtime_stats
    }

    /// daemon 公开配置（字符串编码）。
    #[must_use]
    pub(crate) fn config(&self) -> &BTreeMap<String, String> {
        &self.config
    }

    #[must_use]
    pub(crate) fn config_revision(&self) -> u64 {
        self.config_revision
    }

    /// 配置值（缺省为空串），已去首尾空白。
    #[must_use]
    pub(crate) fn config_str(&self, key: &str) -> &str {
        self.config.get(key).map_or("", |value| value.trim())
    }

    /// agent 偏好值。
    #[must_use]
    pub(crate) fn preference(&self, key: &str) -> Option<&serde_json::Value> {
        self.preferences.get(key)
    }

    /// 全部 agent 偏好。
    #[must_use]
    pub(crate) fn preferences(&self) -> &BTreeMap<String, serde_json::Value> {
        &self.preferences
    }

    #[must_use]
    pub(crate) fn preference_bool(&self, key: &str, default: bool) -> bool {
        self.preference(key)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(default)
    }

    pub fn execute(&self, command: DownloadsCommand) -> PortFuture<DownloadsResult> {
        if self.stale {
            return Box::pin(async {
                Err(fluxdown_protocol::RpcErrorData::new(
                    fluxdown_protocol::ApplicationErrorCode::Unavailable,
                    true,
                ))
            });
        }
        self.port.execute(command)
    }

    fn set_preferences(&mut self, values: &BTreeMap<String, serde_json::Value>) {
        let categories_changed = self
            .preferences
            .get(fluxdown_protocol::CUSTOM_CATEGORIES_PREF_KEY)
            != values.get(fluxdown_protocol::CUSTOM_CATEGORIES_PREF_KEY);
        self.preferences.clone_from(values);
        if categories_changed || self.categories.rules().is_empty() {
            self.categories = Rc::new(CategoryIndex::from_preference(
                values.get(fluxdown_protocol::CUSTOM_CATEGORIES_PREF_KEY),
            ));
        }
    }

    fn absorb_daemon_context(&mut self, snapshot: &DaemonSnapshot) {
        self.queues.clone_from(&snapshot.queues);
        self.groups.clone_from(&snapshot.groups);
        self.config.clone_from(&snapshot.config.values);
        self.config_revision = snapshot.config.revision;
        self.runtime_stats.clone_from(&snapshot.runtime_stats);
        self.task_runtime = snapshot
            .task_runtime
            .iter()
            .map(|(id, runtime)| (id.clone(), Rc::new(runtime.clone())))
            .collect();
        for task in &snapshot.tasks {
            if !matches!(task.status, 1 | 5) {
                self.stop_runtime(&task.task_id);
            }
        }
        self.boosted = snapshot.priority.first().cloned();
    }

    fn apply_daemon_event(&mut self, event: &DaemonEvent) -> bool {
        match event {
            DaemonEvent::SnapshotReplaced(snapshot) => {
                self.live_speeds.clear();
                self.local.clone_from(&snapshot.tasks);
                self.absorb_daemon_context(snapshot);
                self.rebuild_all();
                true
            }
            DaemonEvent::QueuesChanged(queues) => {
                self.queues.clone_from(queues);
                true
            }
            DaemonEvent::GroupsChanged(groups) => {
                self.groups.clone_from(groups);
                self.group_summaries_generation = u64::MAX;
                true
            }
            DaemonEvent::ConfigChanged(config) => {
                self.config.clone_from(&config.values);
                self.config_revision = config.revision;
                false
            }
            DaemonEvent::RuntimeStatsChanged(stats) => {
                self.runtime_stats.clone_from(stats);
                false
            }
            DaemonEvent::RssChanged { .. } => false,
            DaemonEvent::TaskChanged(task) => {
                if !matches!(task.status, 1 | 5) {
                    self.live_speeds.remove(&task.task_id);
                    self.stop_runtime(&task.task_id);
                }
                self.upsert(task.clone());
                true
            }
            DaemonEvent::TaskRuntimeChanged(runtime) => {
                let task_id = &runtime.task_id;
                if self.task_runtime.get(task_id).is_some_and(|previous| {
                    previous.sample_sequence != 0
                        && runtime.sample_sequence <= previous.sample_sequence
                }) {
                    return false;
                }
                let row = self.store.find_local(task_id);
                let mut runtime = runtime.clone();
                if runtime.segments.is_empty()
                    && let Some(previous) = self.task_runtime.get(task_id)
                {
                    runtime.segments.clone_from(&previous.segments);
                }
                self.task_runtime.insert(task_id.clone(), Rc::new(runtime));
                if let Some(ix) = row {
                    if !matches!(self.local[ix].status, 1 | 5) {
                        self.stop_runtime(task_id);
                    }
                    self.rebuild_row(ix);
                    true
                } else {
                    // 任务元数据事件可能紧随其后；先保留样本供新行直接显示。
                    false
                }
            }
            DaemonEvent::TaskActivityAdded(_) => false,
            DaemonEvent::TaskDeleted { task_id } => {
                self.live_speeds.remove(task_id);
                self.task_runtime.remove(task_id);
                if let Some(ix) = self.store.find_local(task_id) {
                    self.local.swap_remove(ix);
                    self.store.swap_remove_local(ix);
                }
                true
            }
            DaemonEvent::Engine(WsServerMsg::TasksSnapshot { tasks }) => {
                self.local.clone_from(tasks);
                self.live_speeds
                    .retain(|task_id, _| tasks.iter().any(|task| task.task_id == *task_id));
                self.task_runtime
                    .retain(|task_id, _| tasks.iter().any(|task| task.task_id == *task_id));
                for task in tasks.iter().filter(|task| !matches!(task.status, 1 | 5)) {
                    self.stop_runtime(&task.task_id);
                }
                self.rebuild_all();
                true
            }
            DaemonEvent::Engine(WsServerMsg::TaskQueueChanged { task_id, queue_id }) => {
                let Some(ix) = self.store.find_local(task_id) else {
                    return false;
                };
                self.local[ix].queue_id.clone_from(queue_id);
                self.rebuild_row(ix);
                true
            }
            DaemonEvent::Engine(WsServerMsg::TaskProgress {
                task_id,
                status,
                downloaded_bytes,
                total_bytes,
                file_name,
                speed,
                error_message,
                uploaded_bytes,
                seeding_status,
                ..
            }) => {
                self.live_speeds
                    .insert(task_id.clone(), if *status == 1 { *speed } else { 0 });
                let Some(ix) = self.store.find_local(task_id) else {
                    return false;
                };
                if let Some(task) = self.local.get_mut(ix) {
                    task.status = *status;
                    task.downloaded_bytes = *downloaded_bytes;
                    task.total_bytes = *total_bytes;
                    task.uploaded_bytes = *uploaded_bytes;
                    task.seeding_status = *seeding_status;
                    if !file_name.is_empty() {
                        task.file_name.clone_from(file_name);
                    }
                    task.error_message.clone_from(error_message);
                }
                if !matches!(*status, 1 | 5) {
                    self.stop_runtime(task_id);
                }
                self.rebuild_row(ix);
                true
            }
            DaemonEvent::Engine(WsServerMsg::TaskMetaProbed {
                task_id,
                file_name,
                total_bytes,
            }) => {
                let Some(ix) = self.store.find_local(task_id) else {
                    return false;
                };
                if let Some(task) = self.local.get_mut(ix) {
                    if !file_name.is_empty() {
                        task.file_name.clone_from(file_name);
                    }
                    task.total_bytes = *total_bytes;
                }
                self.rebuild_row(ix);
                true
            }
            _ => false,
        }
    }

    fn view_of(&self, task: &TaskDto) -> DownloadTaskView {
        let mut view = DownloadTaskView::local(
            task,
            self.live_speeds.get(&task.task_id).copied(),
            self.boosted.as_deref() == Some(task.task_id.as_str()),
        );
        view.runtime = self.task_runtime.get(&task.task_id).cloned();
        view.runtime_connected = !self.stale;
        view
    }

    fn stop_runtime(&mut self, task_id: &str) {
        if let Some(runtime) = self.task_runtime.get_mut(task_id) {
            if runtime.active_transfers == Some(0)
                && runtime.connected_peers == Some(0)
                && runtime
                    .segments
                    .iter()
                    .all(|segment| segment.active == Some(false))
            {
                return;
            }
            let runtime = Rc::make_mut(runtime);
            runtime.active_transfers = Some(0);
            runtime.connected_peers = Some(0);
            for segment in &mut runtime.segments {
                segment.active = Some(false);
            }
        }
    }

    fn upsert(&mut self, task: TaskDto) {
        if let Some(ix) = self.store.find_local(&task.task_id) {
            let view = self.view_of(&task);
            self.local[ix] = task;
            self.store.set_local(ix, view);
        } else {
            let view = self.view_of(&task);
            self.local.push(task);
            self.store.push_local(view);
        }
    }

    fn rebuild_all(&mut self) {
        let rows = self.local.iter().map(|task| self.view_of(task)).collect();
        self.store.replace_local(rows);
    }

    fn rebuild_remote(&mut self) {
        let rows = self.remote.iter().map(DownloadTaskView::remote).collect();
        self.store.replace_remote(rows);
    }

    fn rebuild_row(&mut self, ix: usize) {
        if let Some(task) = self.local.get(ix) {
            let view = self.view_of(task);
            self.store.set_local(ix, view);
        }
    }
}

fn compute_group_summaries(groups: &[GroupDto], rows: &[DownloadTaskView]) -> Vec<GroupSummary> {
    let mut summaries: Vec<GroupSummary> = groups
        .iter()
        .map(|group| GroupSummary {
            id: group.group_id.clone(),
            name: group.name.clone(),
            origin_url: group.source_url.clone(),
            save_dir: group.save_dir.clone(),
            ..GroupSummary::default()
        })
        .collect();
    let mut totals: HashMap<String, (u64, u64)> = HashMap::new();
    for row in rows.iter().filter(|row| !row.group_id.is_empty()) {
        let Some(summary) = summaries
            .iter_mut()
            .find(|summary| summary.id == row.group_id)
        else {
            continue;
        };
        summary.total += 1;
        match row.state {
            TaskState::Completed => summary.completed += 1,
            TaskState::Failed => summary.failed += 1,
            TaskState::Downloading => summary.downloading += 1,
            TaskState::Pending | TaskState::Paused => {}
        }
        let entry = totals.entry(row.group_id.clone()).or_default();
        entry.0 += row.downloaded_bytes;
        entry.1 += row.size_bytes;
    }
    for summary in &mut summaries {
        let (downloaded, total) = totals.get(&summary.id).copied().unwrap_or((0, 0));
        summary.progress = if total > 0 {
            (downloaded as f64 / total as f64).clamp(0.0, 1.0) as f32
        } else if summary.total > 0 {
            summary.completed as f32 / summary.total as f32
        } else {
            0.0
        };
    }
    summaries
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::{
        DaemonEvent, DownloadsCommand, DownloadsController, DownloadsPort, DownloadsResult,
        PortFuture, WsServerMsg,
    };

    struct NullPort;

    impl DownloadsPort for NullPort {
        fn execute(&self, _command: DownloadsCommand) -> PortFuture<DownloadsResult> {
            Box::pin(async { Ok(DownloadsResult::Unit) })
        }
    }

    fn task(id: &str) -> fluxdown_protocol::TaskDto {
        serde_json::from_value(json!({
            "taskId":id,"url":"https://example.com/download","fileName":"",
            "saveDir":"/tmp","status":0,"downloadedBytes":0,"totalBytes":0,
            "errorMessage":"","createdAt":"1","proxyUrl":"","queueId":"main","checksum":""
        }))
        .expect("task")
    }

    #[test]
    fn created_task_ids_read_single_and_batch_create_results() {
        let single = DownloadsResult::Value(json!({ "taskId": "a" }));
        assert_eq!(single.created_task_ids(), ["a"]);
        // 捕获确认里失败的条目为 null，不计入新任务。
        let batch = DownloadsResult::Value(json!({ "taskIds": ["a", null, "b"] }));
        assert_eq!(batch.created_task_ids(), ["a", "b"]);
        assert!(DownloadsResult::Unit.created_task_ids().is_empty());
    }

    #[test]
    fn task_queue_delta_moves_visible_row_without_reloading_tasks() {
        let mut controller = DownloadsController::new(Arc::new(NullPort));
        controller.local.push(task("task-1"));
        controller.rebuild_all();

        assert!(controller.apply_daemon_event(&DaemonEvent::Engine(
            WsServerMsg::TaskQueueChanged {
                task_id: "task-1".to_owned(),
                queue_id: "work".to_owned(),
            },
        )));
        assert_eq!(controller.store().local()[0].queue_id, "work");
        assert_eq!(controller.task_dto("task-1").unwrap().queue_id, "work");
    }

    #[test]
    fn metadata_probe_replaces_loading_row_name_and_size() {
        let mut controller = DownloadsController::new(Arc::new(NullPort));
        controller.local.push(task("task-1"));
        controller.rebuild_all();
        assert!(controller.store().local()[0].metadata_pending);

        controller.apply_daemon_event(&DaemonEvent::Engine(WsServerMsg::TaskMetaProbed {
            task_id: "task-1".to_owned(),
            file_name: "resolved.bin".to_owned(),
            total_bytes: 4096,
        }));

        {
            let rows = controller.store().local();
            assert_eq!(rows[0].name, "resolved.bin");
            assert_eq!(rows[0].size_bytes, 4096);
            assert!(!rows[0].metadata_pending);
        }

        controller.apply_daemon_event(&DaemonEvent::Engine(WsServerMsg::TaskProgress {
            task_id: "task-1".to_owned(),
            status: 1,
            downloaded_bytes: 1024,
            total_bytes: 4096,
            speed: 1024,
            upload_speed: 0,
            file_name: "resolved.bin".to_owned(),
            save_dir: "/tmp".to_owned(),
            url: "https://example.com/download".to_owned(),
            error_message: String::new(),
            uploaded_bytes: 0,
            seeding_status: 0,
            seeding_message: String::new(),
            seeding_time_secs: 0,
        }));

        let rows = controller.store().local();
        assert_eq!(rows[0].speed_bytes_per_second, Some(1024));
        assert_eq!(rows[0].eta_seconds, Some(3));
        assert_eq!(rows[0].progress, 0.25);
    }

    #[test]
    fn progress_touches_only_its_row_and_delete_keeps_index_consistent() {
        let mut controller = DownloadsController::new(Arc::new(NullPort));
        for id in ["a", "b", "c"] {
            controller.local.push(task(id));
        }
        controller.rebuild_all();
        let before = controller.store().generation();

        controller.apply_daemon_event(&DaemonEvent::Engine(WsServerMsg::TaskMetaProbed {
            task_id: "b".to_owned(),
            file_name: "b.bin".to_owned(),
            total_bytes: 10,
        }));
        assert_eq!(controller.store().generation(), before + 1);
        assert_eq!(controller.store().find_local("b"), Some(1));
        assert!(controller.store().local()[0].name.is_empty());
        assert_eq!(controller.store().local()[1].name, "b.bin");

        controller.apply_daemon_event(&DaemonEvent::TaskDeleted {
            task_id: "a".to_owned(),
        });
        assert_eq!(controller.local.len(), 2);
        assert_eq!(controller.store().find_local("a"), None);
        assert_eq!(controller.store().find_local("c"), Some(0));
        assert_eq!(controller.store().find_local("b"), Some(1));
        assert_eq!(controller.local[0].task_id, "c");
        assert_eq!(controller.store().local()[0].key.task_id(), "c");
    }
    #[test]
    fn late_snapshot_contains_segments_and_pausing_clears_active_without_erasing_bytes() {
        let mut snapshot = fluxdown_protocol::AgentSnapshot::default();
        snapshot.daemon_connected = true;
        let mut initial = task("t");
        initial.status = 1;
        initial.total_bytes = 100;
        snapshot.daemon.tasks.push(initial);
        snapshot.daemon.task_runtime.insert(
            "t".to_owned(),
            fluxdown_protocol::TaskRuntimeDto {
                task_id: "t".to_owned(),
                total_bytes: 100,
                active_transfers: Some(1),
                segments: vec![fluxdown_protocol::TaskSegmentDto {
                    start_byte: 0,
                    end_byte: 99,
                    downloaded_bytes: 23,
                    active: Some(true),
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        let mut controller = DownloadsController::new(Arc::new(NullPort));
        controller.replace_snapshot(&snapshot);
        assert_eq!(controller.store().local()[0].active_transfers(), Some(1));
        assert_eq!(
            controller.store().local()[0]
                .runtime
                .as_ref()
                .unwrap()
                .segments[0]
                .downloaded_bytes,
            23
        );

        let mut paused = snapshot.daemon.tasks[0].clone();
        paused.status = 2;
        controller.apply_daemon_event(&DaemonEvent::TaskChanged(paused));
        {
            let row = &controller.store().local()[0];
            assert_eq!(row.active_transfers(), Some(0));
            assert_eq!(
                row.runtime.as_ref().unwrap().segments[0].downloaded_bytes,
                23
            );
            assert_eq!(
                row.runtime.as_ref().unwrap().segments[0].active,
                Some(false)
            );
        }

        controller.mark_stale();
        assert_eq!(controller.store().local()[0].active_transfers(), None);
        assert!(controller.store().local()[0].runtime.is_none());
    }
    #[test]
    fn runtime_source_sequence_prevents_regression_and_paused_transfer_revival() {
        let mut snapshot = fluxdown_protocol::AgentSnapshot::default();
        snapshot.daemon_connected = true;
        let mut task = task("t");
        task.status = 1;
        snapshot.daemon.tasks.push(task.clone());
        let current = fluxdown_protocol::TaskRuntimeDto {
            task_id: "t".to_owned(),
            sampled_at_ms: 100,
            sample_sequence: 10,
            total_bytes: 100,
            active_transfers: Some(2),
            segments: vec![fluxdown_protocol::TaskSegmentDto {
                start_byte: 0,
                end_byte: 99,
                downloaded_bytes: 30,
                active: Some(true),
                ..Default::default()
            }],
            ..Default::default()
        };
        snapshot
            .daemon
            .task_runtime
            .insert("t".to_owned(), current.clone());
        let mut controller = DownloadsController::new(Arc::new(NullPort));
        controller.replace_snapshot(&snapshot);

        let mut older = current.clone();
        older.sampled_at_ms = 10_000; // 墙上时间即使更新也不得决定顺序。
        older.sample_sequence = 9;
        older.segments[0].downloaded_bytes = 10;
        older.active_transfers = Some(7);
        assert!(!controller.apply_daemon_event(&DaemonEvent::TaskRuntimeChanged(older.clone())));
        older.sample_sequence = 10;
        assert!(!controller.apply_daemon_event(&DaemonEvent::TaskRuntimeChanged(older)));
        {
            let row = &controller.store().local()[0];
            assert_eq!(row.active_transfers(), Some(2));
            assert_eq!(
                row.runtime.as_ref().unwrap().segments[0].downloaded_bytes,
                30
            );
        }
        let mut count_only = current.clone();
        count_only.sample_sequence = 11;
        count_only.segments.clear();
        count_only.active_transfers = Some(3);
        assert!(controller.apply_daemon_event(&DaemonEvent::TaskRuntimeChanged(count_only)));
        {
            let row = &controller.store().local()[0];
            assert_eq!(row.active_transfers(), Some(3));
            assert_eq!(
                row.runtime.as_ref().unwrap().segments[0].downloaded_bytes,
                30
            );
        }

        task.status = 2;
        controller.apply_daemon_event(&DaemonEvent::TaskChanged(task));
        let mut later = current;
        later.sample_sequence = 12;
        later.sampled_at_ms = 1;
        later.segments[0].downloaded_bytes = 45;
        later.active_transfers = Some(3);
        assert!(controller.apply_daemon_event(&DaemonEvent::TaskRuntimeChanged(later)));
        let row = &controller.store().local()[0];
        assert_eq!(row.active_transfers(), Some(0));
        let runtime = row.runtime.as_ref().unwrap();
        assert_eq!(runtime.segments[0].downloaded_bytes, 45);
        assert_eq!(runtime.segments[0].active, Some(false));
    }

    #[test]
    fn empty_progress_error_clears_previous_failure() {
        let mut controller = DownloadsController::new(Arc::new(NullPort));
        let mut initial = task("t");
        initial.status = 4;
        initial.error_message = "transient failure".to_owned();
        controller.local.push(initial);
        controller.rebuild_all();
        controller.apply_daemon_event(&DaemonEvent::Engine(WsServerMsg::TaskProgress {
            task_id: "t".to_owned(),
            status: 1,
            downloaded_bytes: 0,
            total_bytes: 100,
            speed: 0,
            upload_speed: 0,
            file_name: String::new(),
            save_dir: "/tmp".to_owned(),
            url: "https://example.com/download".to_owned(),
            error_message: String::new(),
            uploaded_bytes: 0,
            seeding_status: 0,
            seeding_message: String::new(),
            seeding_time_secs: 0,
        }));
        assert!(controller.store().local()[0].error_message.is_empty());
        assert!(controller.task_dto("t").unwrap().error_message.is_empty());
    }
}
