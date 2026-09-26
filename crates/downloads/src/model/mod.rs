pub(crate) mod categories;
pub(crate) mod new_download;
pub(crate) mod progress_window;
pub(crate) mod shutdown;
pub(crate) mod store;
pub(crate) mod view_prefs;

pub(crate) use categories::CategoryIndex;
use std::rc::Rc;
pub(crate) use store::{RowId, TaskStore};

use fluxdown_protocol::TaskRuntimeDto;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DownloadStatusFilter {
    All,
    Completed,
    Incomplete,
    Failed,
    Paused,
}

impl DownloadStatusFilter {
    pub(crate) const ALL: [Self; 5] = [
        Self::All,
        Self::Incomplete,
        Self::Completed,
        Self::Failed,
        Self::Paused,
    ];

    pub(crate) fn exclusive_toggle(current: Option<Self>, clicked: Self) -> Option<Self> {
        if current == Some(clicked) {
            None
        } else {
            Some(clicked)
        }
    }

    fn slot(self) -> usize {
        match self {
            Self::All => 0,
            Self::Completed => 1,
            Self::Incomplete => 2,
            Self::Failed => 3,
            Self::Paused => 4,
        }
    }

    pub(crate) fn matches(self, state: TaskState) -> bool {
        match self {
            Self::All => true,
            Self::Completed => state == TaskState::Completed,
            // 与 Flutter `StatusTab.downloading` 同义：下载中 + 等待中。
            Self::Incomplete => matches!(state, TaskState::Downloading | TaskState::Pending),
            Self::Failed => state == TaskState::Failed,
            Self::Paused => state == TaskState::Paused,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StatusFolderMotion {
    from: [f32; 5],
    to: [f32; 5],
}

impl StatusFolderMotion {
    pub(crate) fn settled(open: Option<DownloadStatusFilter>) -> Self {
        let mut amounts = [0.; 5];
        if let Some(status) = open {
            amounts[status.slot()] = 1.;
        }
        Self {
            from: amounts,
            to: amounts,
        }
    }

    pub(crate) fn retarget(self, progress: f32, next: Option<DownloadStatusFilter>) -> Self {
        let progress = progress.clamp(0., 1.);
        let mut from = [0.; 5];
        for (slot, amount) in from.iter_mut().enumerate() {
            *amount = self.from[slot] + (self.to[slot] - self.from[slot]) * progress;
        }
        let mut to = [0.; 5];
        if let Some(status) = next {
            to[status.slot()] = 1.;
        }
        Self { from, to }
    }

    pub(crate) fn amount(self, status: DownloadStatusFilter, progress: f32) -> f32 {
        let progress = progress.clamp(0., 1.);
        let slot = status.slot();
        self.from[slot] + (self.to[slot] - self.from[slot]) * progress
    }
}

/// 状态 + 分类（分类 id 来自 [`fluxdown_protocol::CustomCategoryDto::id`]）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct DownloadFilter {
    pub(crate) status: DownloadStatusFilter,
    pub(crate) category: Option<String>,
}

impl DownloadFilter {
    pub(crate) const ALL: Self = Self {
        status: DownloadStatusFilter::All,
        category: None,
    };

    pub(crate) const fn status(status: DownloadStatusFilter) -> Self {
        Self {
            status,
            category: None,
        }
    }

    pub(crate) fn with_category(status: DownloadStatusFilter, category: &str) -> Self {
        Self {
            status,
            category: Some(category.to_owned()),
        }
    }

    pub(crate) fn matches(&self, task: &DownloadTaskView, categories: &CategoryIndex) -> bool {
        self.status.matches(task.state)
            && self
                .category
                .as_deref()
                .is_none_or(|category| categories.matches(category, task))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SidebarSection {
    Status,
    Queues,
    Devices,
}

impl SidebarSection {
    pub(crate) const ALL: [Self; 3] = [Self::Status, Self::Queues, Self::Devices];

    /// 分区可见性偏好键（`sync:true`，与其余 `ui.*` 同规则）。
    pub(crate) fn visibility_pref(self) -> &'static str {
        match self {
            Self::Status => "ui.show_sidebar_status",
            Self::Queues => "ui.show_sidebar_queues",
            Self::Devices => "ui.show_sidebar_devices",
        }
    }
}

/// 侧栏当前选中项；决定表格的筛选来源。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum SidebarSelection {
    Download(DownloadFilter),
    Queue(String),
    /// `"local"` = 本机，其余为云设备 id / 配对设备指纹。
    Device(String),
}

impl SidebarSelection {
    pub(crate) const LOCAL_DEVICE: &'static str = "local";
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TaskState {
    Pending,
    Downloading,
    Paused,
    Completed,
    Failed,
}

impl TaskState {
    /// 智能排序优先级：下载中 > 等待 > 暂停 > 失败 > 完成。
    pub(crate) fn smart_rank(self) -> u8 {
        match self {
            Self::Downloading => 0,
            Self::Pending => 1,
            Self::Paused => 2,
            Self::Failed => 3,
            Self::Completed => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TaskProtocol {
    Http,
    Bt,
    Ed2k,
    Ftp,
    Hls,
}

impl TaskProtocol {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Http => "HTTP",
            Self::Bt => "BT",
            Self::Ed2k => "ED2K",
            Self::Ftp => "FTP",
            Self::Hls => "HLS",
        }
    }

    fn detect(url: &str, file_name: &str) -> Self {
        let lower = url.to_ascii_lowercase();
        if lower.starts_with("magnet:") || lower.starts_with("torrent-file://") {
            Self::Bt
        } else if lower.starts_with("ed2k://") {
            Self::Ed2k
        } else if lower.starts_with("ftp://") || lower.starts_with("ftps://") {
            Self::Ftp
        } else if lower.contains(".m3u8") || lower.contains(".mpd") || file_name.ends_with(".m3u8")
        {
            Self::Hls
        } else {
            Self::Http
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum TaskKind {
    Application,
    DiskImage,
    Mobile,
    Video,
    Audio,
    Document,
    Image,
    Archive,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum TaskSource {
    Local,
    Remote,
}

/// 稳定的行身份（跨快照重建仍有效）；选中集合与窗口 key 用它。
#[derive(Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub(crate) enum RowKey {
    Local(String),
    Remote(String),
}

impl RowKey {
    pub(crate) fn task_id(&self) -> &str {
        match self {
            Self::Local(id) | Self::Remote(id) => id,
        }
    }

    pub(crate) fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }
}

#[derive(Clone)]
pub(crate) struct DownloadTaskView {
    pub(crate) key: RowKey,
    pub(crate) source: TaskSource,
    pub(crate) queue_id: String,
    pub(crate) name: String,
    pub(crate) size: String,
    pub(crate) size_bytes: u64,
    pub(crate) downloaded_bytes: u64,
    pub(crate) speed_bytes_per_second: Option<u64>,
    pub(crate) runtime: Option<Rc<TaskRuntimeDto>>,
    pub(crate) runtime_connected: bool,
    pub(crate) eta_seconds: Option<u64>,
    pub(crate) created_at_secs: i64,
    pub(crate) completed_at_secs: i64,
    pub(crate) kind: TaskKind,
    pub(crate) protocol: TaskProtocol,
    pub(crate) progress: f32,
    pub(crate) progress_label: String,
    pub(crate) state: TaskState,
    pub(crate) metadata_pending: bool,
    pub(crate) url: String,
    pub(crate) origin_url: String,
    pub(crate) site: String,
    pub(crate) referrer: String,
    pub(crate) save_dir: String,
    pub(crate) group_id: String,
    pub(crate) error_message: String,
    pub(crate) file_missing: bool,
    pub(crate) seeding_status: i32,
    pub(crate) uploaded_bytes: i64,
    pub(crate) boosted: bool,
    /// 远程任务来源设备 id（本地任务为空）。
    pub(crate) from_device: String,
}

impl DownloadTaskView {
    pub(crate) fn local(
        task: &fluxdown_protocol::TaskDto,
        speed: Option<i64>,
        boosted: bool,
    ) -> Self {
        let mut view = Self::new(
            RowKey::Local(task.task_id.clone()),
            TaskSource::Local,
            task.queue_id.clone(),
            task.file_name.clone(),
            task.total_bytes,
            task.downloaded_bytes,
            speed.map(|value| value.max(0) as u64),
            task.created_at.parse().unwrap_or_default(),
            task.status,
            &task.url,
        );
        view.completed_at_secs = task.completed_at.parse().unwrap_or_default();
        view.origin_url.clone_from(&task.origin_url);
        view.referrer.clone_from(&task.referrer);
        view.save_dir.clone_from(&task.save_dir);
        view.group_id.clone_from(&task.group_id);
        view.error_message.clone_from(&task.error_message);
        view.file_missing = task.file_missing;
        view.seeding_status = task.seeding_status;
        view.uploaded_bytes = task.uploaded_bytes;
        view.boosted = boosted;
        view
    }

    pub(crate) fn remote(task: &fluxdown_protocol::RemoteTaskDto) -> Self {
        let mut view = Self::new(
            RowKey::Remote(task.id.clone()),
            TaskSource::Remote,
            String::new(),
            task.file_name.clone(),
            task.total_bytes.unwrap_or_default(),
            task.downloaded_bytes,
            Some(task.speed.max(0) as u64),
            task.created_at.parse().unwrap_or_default(),
            match task.status {
                fluxdown_protocol::RemoteTaskStatus::Pending
                | fluxdown_protocol::RemoteTaskStatus::Accepted => 0,
                fluxdown_protocol::RemoteTaskStatus::Downloading => 1,
                fluxdown_protocol::RemoteTaskStatus::Paused => 2,
                fluxdown_protocol::RemoteTaskStatus::Completed => 3,
                fluxdown_protocol::RemoteTaskStatus::Failed
                | fluxdown_protocol::RemoteTaskStatus::Canceled => 4,
            },
            &task.url,
        );
        view.save_dir = task.save_dir.clone().unwrap_or_default();
        view.error_message = task.error.clone().unwrap_or_default();
        view.from_device.clone_from(&task.from_device);
        view
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "normalizes the same compact projection from local and remote wire tasks"
    )]
    fn new(
        key: RowKey,
        source: TaskSource,
        queue_id: String,
        name: String,
        total_bytes: i64,
        downloaded_bytes: i64,
        speed_bytes_per_second: Option<u64>,
        created_at_secs: i64,
        status: i32,
        url: &str,
    ) -> Self {
        let metadata_pending = name.trim().is_empty();
        let kind = task_kind(&name);
        let protocol = TaskProtocol::detect(url, &name);
        let size_bytes = total_bytes.max(0) as u64;
        let downloaded = downloaded_bytes.max(0) as u64;
        let progress = if size_bytes == 0 {
            0.0
        } else {
            (downloaded as f64 / size_bytes as f64).clamp(0.0, 1.0) as f32
        };
        let eta_seconds = speed_bytes_per_second
            .filter(|speed| *speed > 0 && downloaded < size_bytes)
            .map(|speed| (size_bytes - downloaded) / speed);
        Self {
            key,
            source,
            queue_id,
            kind,
            protocol,
            name,
            size: format_bytes(size_bytes),
            size_bytes,
            downloaded_bytes: downloaded,
            speed_bytes_per_second,
            runtime: None,
            runtime_connected: false,
            eta_seconds,
            created_at_secs,
            completed_at_secs: 0,
            progress,
            progress_label: format!("{:.1}%", progress * 100.0),
            state: match status {
                0 | 5 => TaskState::Pending,
                1 => TaskState::Downloading,
                2 => TaskState::Paused,
                3 => TaskState::Completed,
                _ => TaskState::Failed,
            },
            metadata_pending,
            site: url_host(url).to_owned(),
            url: url.to_owned(),
            origin_url: String::new(),
            referrer: String::new(),
            save_dir: String::new(),
            group_id: String::new(),
            error_message: String::new(),
            file_missing: false,
            seeding_status: 0,
            uploaded_bytes: 0,
            boosted: false,
            from_device: String::new(),
        }
    }

    /// 「复制链接」用：`origin_url` 优先，空则回退 `url`（torrent 任务的 `url` 是哨兵）。
    pub(crate) fn share_url(&self) -> &str {
        if self.origin_url.is_empty() {
            &self.url
        } else {
            &self.origin_url
        }
    }

    /// 来源站点：`referrer` host 为主，空则 url host。
    pub(crate) fn source_site(&self) -> &str {
        let host = url_host(&self.referrer);
        if host.is_empty() { &self.site } else { host }
    }

    /// 文件扩展名（小写，无点）。
    pub(crate) fn extension(&self) -> Option<String> {
        self.name
            .rsplit_once('.')
            .map(|(_, extension)| extension.to_ascii_lowercase())
            .filter(|extension| !extension.is_empty() && !extension.contains('/'))
    }

    pub(crate) fn active_transfers(&self) -> Option<u32> {
        if !self.runtime_connected || self.source != TaskSource::Local {
            None
        } else if self.state != TaskState::Downloading {
            Some(0)
        } else {
            self.runtime
                .as_ref()
                .and_then(|runtime| runtime.active_transfers)
        }
    }
}

/// URL host（不含端口 / 凭据）；非 URL 返回空串。
pub(crate) fn url_host(url: &str) -> &str {
    let Some((_, rest)) = url.split_once("://") else {
        return "";
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if host.starts_with('[') {
        host.split_once(']').map_or(host, |(ipv6, _)| &ipv6[1..])
    } else {
        host.split_once(':').map_or(host, |(host, _)| host)
    }
}

fn task_kind(name: &str) -> TaskKind {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    match extension.as_deref() {
        Some("exe" | "msi" | "appimage") => TaskKind::Application,
        Some("apk" | "ipa") => TaskKind::Mobile,
        Some("iso" | "dmg") => TaskKind::DiskImage,
        Some("zip" | "rar" | "7z" | "tar" | "gz") => TaskKind::Archive,
        Some("mp4" | "mkv" | "webm" | "avi") => TaskKind::Video,
        Some("mp3" | "flac" | "wav" | "m4a") => TaskKind::Audio,
        Some("pdf" | "doc" | "docx" | "txt") => TaskKind::Document,
        Some("png" | "jpg" | "jpeg" | "gif" | "webp") => TaskKind::Image,
        _ => TaskKind::Other,
    }
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{DownloadTaskView, RowKey, TaskProtocol, TaskSource, TaskState, url_host};

    #[test]
    fn local_and_remote_wire_tasks_project_without_placeholders() {
        let local = serde_json::from_value::<fluxdown_protocol::TaskDto>(json!({
            "taskId":"local-1","url":"https://user@example.com:8443/a?x=1","fileName":"a.bin",
            "saveDir":"/tmp","status":1,"downloadedBytes":50,"totalBytes":100,
            "errorMessage":"","createdAt":"7","proxyUrl":"","queueId":"main","checksum":"",
            "originUrl":"https://origin.example/page"
        }))
        .expect("local task");
        let local = DownloadTaskView::local(&local, None, false);
        assert_eq!(local.key, RowKey::Local("local-1".to_owned()));
        assert_eq!(local.source, TaskSource::Local);
        assert_eq!(local.progress, 0.5);
        assert_eq!(local.state, TaskState::Downloading);
        assert_eq!(local.site, "example.com");
        assert_eq!(local.share_url(), "https://origin.example/page");
        assert_eq!(local.protocol, TaskProtocol::Http);
        assert!(!local.metadata_pending);

        let remote = serde_json::from_value::<fluxdown_protocol::RemoteTaskDto>(json!({
            "id":"remote-1","url":"https://example.com/b","fileName":"b.bin",
            "status":"paused","totalBytes":200,"downloadedBytes":20,"speed":0
        }))
        .expect("remote task");
        let remote = DownloadTaskView::remote(&remote);
        assert_eq!(remote.key, RowKey::Remote("remote-1".to_owned()));
        assert_eq!(remote.source, TaskSource::Remote);
        assert_eq!(remote.state, TaskState::Paused);

        let probing = serde_json::from_value::<fluxdown_protocol::TaskDto>(json!({
            "taskId":"local-2","url":"magnet:?xt=urn:btih:abc","fileName":"",
            "saveDir":"/tmp","status":0,"downloadedBytes":0,"totalBytes":0,
            "errorMessage":"","createdAt":"8","proxyUrl":"","queueId":"main","checksum":""
        }))
        .expect("probing task");
        let probing = DownloadTaskView::local(&probing, None, false);
        assert!(probing.metadata_pending);
        assert!(probing.name.is_empty());
        assert_eq!(probing.protocol, TaskProtocol::Bt);
    }

    #[test]
    fn url_host_strips_credentials_port_and_path() {
        assert_eq!(url_host("https://u:p@host.example:443/x"), "host.example");
        assert_eq!(url_host("http://[::1]:80/"), "::1");
        assert_eq!(url_host("magnet:?xt=abc"), "");
    }
}
