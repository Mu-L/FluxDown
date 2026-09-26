//! 浏览器/NMH 捕获确认事务：内存有界、单次消费且不持久化敏感请求上下文。
//!
//! Cookie / 请求头 / 请求体只留在事务里；官方 UI 只拿到 [`PendingCaptureDto`] 摘要，
//! 确认时提交表单产出的 [`CreateTaskRequest`]，由 [`merge_confirmed_request`] 以捕获原请求为底合并。

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use fluxdown_protocol::{
    AgentEvent, CreateTaskRequest, DaemonCreateTaskParams, DownloadRequest, PendingCaptureDto,
};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::daemon_client::DaemonClient;
use crate::event_hub::AgentEventHub;
use crate::shell::ShellState;

const CAPTURE_CAPACITY: usize = 64;

/// 「免打扰下载」：外部接管请求不弹确认，直接按默认设置建任务（设置页 `download.rs`）。
pub(crate) const SILENT_DOWNLOAD_PREF: &str = "download.silent_download";
/// 免打扰子开关：静默建的任务跳过 BT 文件 / HLS·DASH 画质 / 插件变体二次选择。设备本地偏好。
pub(crate) const SILENT_SKIP_SELECTION_PREF: &str = "download.silent_skip_selection";
/// 「跟随上次保存位置」与其记录（官方 UI 新建下载写入）。
const REMEMBER_LAST_SAVE_DIR_PREF: &str = "download.remember_last_save_dir";
const LAST_SAVE_DIR_PREF: &str = "download.last_save_dir";

/// 捕获来源，决定是否需要用户确认。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureOrigin {
    /// 本机用户显式交来的链接（系统打开链接、拖入）：直接建任务。
    Direct,
    /// 浏览器扩展 / 用户脚本 / NMH 等外部接管：按「免打扰下载」偏好静默或确认。
    External,
    /// 需要用户过目的探测结果（剪贴板监听）：恒入确认队列。
    Prompt,
}

/// 外部接管的静默策略；由偏好现算，改动即生效。
#[derive(Clone, Debug, Eq, PartialEq)]
struct ExternalPolicy {
    silent: bool,
    skip_selection: bool,
    /// 「跟随上次保存位置」开启且有记录时的目录；捕获方未指定目录时使用。
    remembered_save_dir: Option<String>,
}

impl ExternalPolicy {
    fn from_preferences(preferences: &BTreeMap<String, Value>) -> Self {
        let flag = |key: &str| {
            preferences
                .get(key)
                .and_then(Value::as_bool)
                .unwrap_or(false)
        };
        let remembered_save_dir = flag(REMEMBER_LAST_SAVE_DIR_PREF)
            .then(|| preferences.get(LAST_SAVE_DIR_PREF).and_then(Value::as_str))
            .flatten()
            .map(str::trim)
            .filter(|dir| !dir.is_empty())
            .map(str::to_owned);
        Self {
            silent: flag(SILENT_DOWNLOAD_PREF),
            skip_selection: flag(SILENT_SKIP_SELECTION_PREF),
            remembered_save_dir,
        }
    }
}

struct CaptureTransaction {
    public: PendingCaptureDto,
    request: DownloadRequest,
}

pub struct CaptureService {
    daemon: Arc<DaemonClient>,
    events: AgentEventHub,
    pending: Mutex<VecDeque<CaptureTransaction>>,
    /// 待确认捕获入队时按需拉起官方 UI。
    shell: Arc<ShellState>,
}

impl CaptureService {
    #[must_use]
    pub fn new(daemon: Arc<DaemonClient>, events: AgentEventHub, shell: Arc<ShellState>) -> Self {
        Self {
            daemon,
            events,
            pending: Mutex::new(VecDeque::with_capacity(CAPTURE_CAPACITY)),
            shell,
        }
    }

    /// 按来源分流：`Direct` 与开启免打扰的 `External` 直接提交 daemon，其余排入确认队列并在
    /// 队列由空变非空时唤起官方 UI。批量请求（换行连接的多 URL）先拆成逐条请求。
    ///
    /// 保存目录优先级与 Flutter 一致：捕获方指定 > 分类目录 > （仅静默）跟随上次 > daemon 默认。
    /// 确认路径把分类目录写进待确认摘要，官方表单据此预填。
    pub async fn submit(
        &self,
        request: DownloadRequest,
        origin: CaptureOrigin,
    ) -> Result<Value, CaptureError> {
        let requests = split_batch(request);
        if origin == CaptureOrigin::Direct {
            return self.create_all(requests, None, true).await;
        }
        let (policy, requests) = self.events.inspect(|snapshot| {
            let preferences = &snapshot.preferences.values;
            let requests = requests
                .into_iter()
                .map(|request| with_category_dir(request, preferences))
                .collect::<Vec<_>>();
            (ExternalPolicy::from_preferences(preferences), requests)
        });
        if origin == CaptureOrigin::External && policy.silent {
            let save_dir = policy.remembered_save_dir.as_deref();
            return self
                .create_all(requests, save_dir, policy.skip_selection)
                .await;
        }
        self.enqueue(requests).await
    }

    async fn create_all(
        &self,
        requests: Vec<DownloadRequest>,
        fallback_save_dir: Option<&str>,
        unattended: bool,
    ) -> Result<Value, CaptureError> {
        let mut task_ids = Vec::with_capacity(requests.len());
        for request in requests {
            let mut create = captured_create_request(request);
            if let Some(dir) = fallback_save_dir {
                fill_if_blank(&mut create.save_dir, dir.to_owned());
            }
            let created = self.create(create, None, unattended).await?;
            task_ids.push(created.get("taskId").cloned().unwrap_or(Value::Null));
        }
        Ok(json!({ "taskIds": task_ids }))
    }

    async fn enqueue(&self, requests: Vec<DownloadRequest>) -> Result<Value, CaptureError> {
        let (first, transaction_ids) = {
            let mut pending = self.pending.lock().await;
            if pending.len() + requests.len() > CAPTURE_CAPACITY {
                return Err(CaptureError::Full);
            }
            let first = pending.is_empty();
            let mut transaction_ids = Vec::with_capacity(requests.len());
            for request in requests {
                let public = pending_capture_dto(&request);
                transaction_ids.push(public.transaction_id.clone());
                pending.push_back(CaptureTransaction { public, request });
            }
            (first, transaction_ids)
        };
        self.publish().await;
        if first {
            self.shell.launch_for_prompt();
        }
        Ok(json!({ "transactionIds": transaction_ids }))
    }

    /// 用户选定的本机 `.torrent`（已上传为 daemon blob）直接建任务；
    /// `unattended` 为 true 时全选文件不弹选择框。
    pub async fn create_torrent(
        &self,
        request: DownloadRequest,
        torrent_blob_id: String,
        unattended: bool,
    ) -> Result<Value, CaptureError> {
        self.create(
            captured_create_request(request),
            Some(torrent_blob_id),
            unattended,
        )
        .await
    }

    pub async fn list(&self) -> Vec<PendingCaptureDto> {
        self.pending
            .lock()
            .await
            .iter()
            .map(|transaction| transaction.public.clone())
            .collect()
    }

    /// 确认/拒绝均只消费一次；确认时 `confirmed`（官方 UI 表单结果）经
    /// [`merge_confirmed_request`] 合并进捕获原请求，`None` 按原请求建任务。
    pub async fn resolve(
        &self,
        transaction_id: &str,
        accepted: bool,
        confirmed: Option<CreateTaskRequest>,
    ) -> Result<Value, CaptureError> {
        let transaction = {
            let mut pending = self.pending.lock().await;
            let index = pending
                .iter()
                .position(|transaction| transaction.public.transaction_id == transaction_id)
                .ok_or(CaptureError::NotFound)?;
            pending.remove(index).ok_or(CaptureError::NotFound)?
        };
        self.publish().await;
        if !accepted {
            return Ok(json!({ "accepted": false }));
        }
        let request = match confirmed {
            Some(confirmed) => merge_confirmed_request(transaction.request, confirmed),
            None => captured_create_request(transaction.request),
        };
        self.create(request, None, false).await
    }

    async fn create(
        &self,
        request: CreateTaskRequest,
        torrent_blob_id: Option<String>,
        unattended: bool,
    ) -> Result<Value, CaptureError> {
        self.daemon
            .call(
                fluxdown_protocol::method::DAEMON_TASK_CREATE,
                Some(DaemonCreateTaskParams {
                    request,
                    torrent_blob_id,
                    unattended,
                }),
            )
            .await
            .map_err(CaptureError::Daemon)
    }

    async fn publish(&self) {
        self.events
            .publish(AgentEvent::PendingCapturesChanged(self.list().await));
    }
}

/// 捕获请求 → UI 可见摘要：只暴露头名与是否带 Cookie，不含任何值。
fn pending_capture_dto(request: &DownloadRequest) -> PendingCaptureDto {
    let mut header_names = request
        .headers
        .iter()
        .flatten()
        .map(|(name, _)| name.clone())
        .filter(|name| !name.eq_ignore_ascii_case("cookie"))
        .collect::<Vec<_>>();
    header_names.sort_by_key(|name| name.to_ascii_lowercase());
    let cookie_header = request
        .headers
        .iter()
        .flatten()
        .any(|(name, value)| name.eq_ignore_ascii_case("cookie") && !value.trim().is_empty());
    PendingCaptureDto {
        transaction_id: Uuid::new_v4().to_string(),
        url: request.url.clone(),
        file_name: request.filename.clone(),
        created_at_unix_ms: now_unix_ms(),
        file_size: request.file_size.unwrap_or(0).max(0),
        referrer: request.referrer.clone(),
        save_dir: request.save_dir.clone(),
        has_cookies: !request.cookies.trim().is_empty() || cookie_header,
        header_names,
    }
}

/// 捕获方未指定保存目录时按分类规则补上（命中且该分类配置了目录）。
fn with_category_dir(
    mut request: DownloadRequest,
    preferences: &BTreeMap<String, Value>,
) -> DownloadRequest {
    if request.save_dir.trim().is_empty()
        && let Some(dir) =
            crate::category_dir::category_save_dir(preferences, &request.filename, &request.url)
    {
        request.save_dir = dir;
    }
    request
}

/// `/download/batch` 把多个 URL 以换行连接成单个请求（`fluxdown_api::takeover::parse_batch`）；
/// 建任务与确认事务都必须逐 URL 进行，否则 daemon 收到多行 URL，确认表单也会把它们拆成与事务
/// 对不上的普通链接而丢掉 Cookie / Referer / 请求头。单 URL 原样保留；多 URL 只共享
/// Cookie / Referer / 请求头 / 保存目录，文件名 / method / body / 音频轨 / 大小是单请求语义，丢弃。
fn split_batch(request: DownloadRequest) -> Vec<DownloadRequest> {
    let urls = request
        .url
        .lines()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if urls.len() <= 1 {
        return vec![request];
    }
    urls.into_iter()
        .map(|url| DownloadRequest {
            url,
            filename: String::new(),
            save_dir: request.save_dir.clone(),
            referrer: request.referrer.clone(),
            cookies: request.cookies.clone(),
            headers: request.headers.clone(),
            file_size: None,
            mime_type: None,
            method: None,
            body: None,
            audio_url: None,
        })
        .collect()
}

/// 按捕获原请求建任务（静默提交 / 未带表单结果的确认）；队列与分段走 daemon 默认。
fn captured_create_request(request: DownloadRequest) -> CreateTaskRequest {
    CreateTaskRequest {
        url: request.url,
        file_name: request.filename,
        save_dir: request.save_dir,
        segments: 0,
        cookies: request.cookies,
        referrer: request.referrer,
        proxy_url: String::new(),
        user_agent: String::new(),
        queue_id: String::new(),
        checksum: String::new(),
        ignore_tls_errors: false,
        headers: request.headers,
        torrent_b64: None,
        method: request.method,
        body: request.body,
        audio_url: request.audio_url,
        start_paused: false,
        http_user: String::new(),
        http_password: String::new(),
        save_site_auth: false,
    }
}

/// 官方 UI 表单结果合并进捕获原请求（规则见 `CaptureResolveParams::request`）。
///
/// 表单看不到的请求上下文（method / body / 音频轨）与事务身份（url）恒取捕获值；
/// 表单留空的字段回退捕获值；请求头以捕获为底、表单同名覆盖，表单填了 UA 时去掉
/// 捕获的 `User-Agent` 头。表单的 HTTP 认证原样保留：引擎对非空 `httpUser` 注入的
/// `Authorization` 会覆盖捕获头，留空则沿用浏览器头 / 已保存站点凭据。
fn merge_confirmed_request(
    captured: DownloadRequest,
    mut confirmed: CreateTaskRequest,
) -> CreateTaskRequest {
    let base = captured_create_request(captured);
    confirmed.url = base.url;
    confirmed.method = base.method;
    confirmed.body = base.body;
    confirmed.audio_url = base.audio_url;
    confirmed.torrent_b64 = None;
    fill_if_blank(&mut confirmed.file_name, base.file_name);
    fill_if_blank(&mut confirmed.save_dir, base.save_dir);
    fill_if_blank(&mut confirmed.cookies, base.cookies);
    fill_if_blank(&mut confirmed.referrer, base.referrer);
    let mut headers = base.headers.unwrap_or_default();
    if !confirmed.user_agent.trim().is_empty() {
        headers.retain(|name, _| !name.eq_ignore_ascii_case("user-agent"));
    }
    for (name, value) in confirmed.headers.take().unwrap_or_default() {
        headers.retain(|existing, _| !existing.eq_ignore_ascii_case(&name));
        headers.insert(name, value);
    }
    confirmed.headers = (!headers.is_empty()).then_some(headers);
    confirmed
}

fn fill_if_blank(target: &mut String, fallback: String) {
    if target.trim().is_empty() {
        *target = fallback;
    }
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("capture confirmation queue is full")]
    Full,
    #[error("capture transaction not found")]
    NotFound,
    #[error("daemon capture create failed: {0:?}")]
    Daemon(fluxdown_protocol::RpcErrorData),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Platform(#[from] crate::platform::PlatformError),
}

/// daemon 专用二进制上传端点（`POST /blobs/{torrents|plugins}`）的客户端。
///
/// 与 RPC 共用 daemon 的 bearer；base URL 由 RPC URL 换成 http(s) 并去掉路径。
pub struct DaemonBlobClient {
    base_url: reqwest::Url,
    bearer: String,
    http: reqwest::Client,
}

/// 上传的 blob 类型，对应 daemon 端点路径段。
#[derive(Clone, Copy, Debug)]
pub enum BlobKind {
    Torrent,
    Plugin,
}

impl BlobKind {
    fn path(self) -> &'static str {
        match self {
            Self::Torrent => "/blobs/torrents",
            Self::Plugin => "/blobs/plugins",
        }
    }
}

impl DaemonBlobClient {
    pub fn new(config: &crate::daemon_client::DaemonClientConfig) -> Result<Self, BlobError> {
        let mut base_url = reqwest::Url::parse(&config.rpc_url)
            .map_err(|error| BlobError::Url(error.to_string()))?;
        let scheme = match base_url.scheme() {
            "ws" | "http" => "http",
            "wss" | "https" => "https",
            other => return Err(BlobError::Url(format!("unsupported scheme {other}"))),
        };
        base_url
            .set_scheme(scheme)
            .map_err(|()| BlobError::Url("scheme is not settable".to_owned()))?;
        base_url.set_path("");
        base_url.set_query(None);
        base_url.set_fragment(None);
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(60))
            .build()?;
        Ok(Self {
            base_url,
            bearer: config.bearer.clone(),
            http,
        })
    }

    /// 上传字节并返回 daemon 分配的 `blobId`。
    pub async fn upload(&self, kind: BlobKind, bytes: Vec<u8>) -> Result<String, BlobError> {
        let url = self
            .base_url
            .join(kind.path())
            .map_err(|error| BlobError::Url(error.to_string()))?;
        let response = self
            .http
            .post(url)
            .bearer_auth(&self.bearer)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(bytes)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(BlobError::Status(status.as_u16()));
        }
        let body = response.json::<Value>().await?;
        body.get("blobId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or(BlobError::Decode)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BlobError {
    #[error("daemon URL is invalid: {0}")]
    Url(String),
    #[error("daemon blob upload transport failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("daemon blob upload rejected with HTTP {0}")]
    Status(u16),
    #[error("daemon blob upload response has no blobId")]
    Decode,
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use fluxdown_protocol::{AgentSnapshot, CreateTaskRequest, RequestBody};
    use serde_json::json;

    use super::{
        CaptureOrigin, CaptureService, DownloadRequest, ExternalPolicy, merge_confirmed_request,
        pending_capture_dto, split_batch, with_category_dir,
    };

    fn preferences(
        value: serde_json::Value,
    ) -> std::collections::BTreeMap<String, serde_json::Value> {
        serde_json::from_value(value).expect("preference map")
    }

    /// 断开的 daemon：静默路径的建任务调用必然失败，从而可观察「没有入确认队列」。
    fn service(prefs: serde_json::Value) -> CaptureService {
        let daemon = std::sync::Arc::new(crate::daemon_client::DaemonClient::disconnected());
        let mut snapshot = AgentSnapshot::default();
        snapshot.preferences.values = preferences(prefs);
        let events = crate::event_hub::AgentEventHub::new(snapshot);
        let shell = crate::shell::ShellState::new(
            crate::shell::TrayAvailability::Unavailable(
                fluxdown_protocol::TrayUnavailableReason::NotBuilt,
            ),
            daemon.clone(),
            events.clone(),
        );
        CaptureService::new(daemon, events, shell)
    }

    #[test]
    fn external_policy_follows_silent_preferences_and_remembered_dir() {
        let off = ExternalPolicy::from_preferences(&preferences(json!({})));
        assert!(!off.silent);
        assert!(!off.skip_selection);
        assert_eq!(off.remembered_save_dir, None);

        let on = ExternalPolicy::from_preferences(&preferences(json!({
            "download.silent_download": true,
            "download.silent_skip_selection": true,
            "download.remember_last_save_dir": true,
            "download.last_save_dir": "  /last  ",
        })));
        assert!(on.silent);
        assert!(on.skip_selection);
        assert_eq!(on.remembered_save_dir.as_deref(), Some("/last"));

        // 「跟随上次」关闭时忽略记录；旧的无命名空间子开关键不生效。
        let not_remembered = ExternalPolicy::from_preferences(&preferences(json!({
            "download.silent_download": true,
            "silent_skip_selection": true,
            "download.last_save_dir": "/last",
        })));
        assert!(!not_remembered.skip_selection);
        assert_eq!(not_remembered.remembered_save_dir, None);
    }

    #[tokio::test]
    async fn silent_external_capture_creates_directly_instead_of_prompting() {
        let capture = service(json!({ "download.silent_download": true }));
        let result = capture.submit(captured(), CaptureOrigin::External).await;
        assert!(matches!(result, Err(super::CaptureError::Daemon(_))));
        assert!(capture.list().await.is_empty());
    }

    #[test]
    fn category_dir_fills_only_unspecified_save_dir() {
        let prefs = preferences(json!({
            "custom_categories": [{
                "id": "bin", "name": "bin", "extensions": ["bin"], "position": 1,
                "saveDir": "/category",
            }],
        }));
        assert_eq!(with_category_dir(captured(), &prefs).save_dir, "/captured");
        let mut unspecified = captured();
        unspecified.save_dir = String::new();
        assert_eq!(with_category_dir(unspecified, &prefs).save_dir, "/category");
    }

    #[test]
    fn batch_request_splits_per_url_sharing_only_request_context() {
        let mut batch = captured();
        batch.url = "https://example.com/a.bin\n\n  https://example.com/b.bin  \n".to_owned();
        let split = split_batch(batch);
        assert_eq!(split.len(), 2);
        assert_eq!(split[0].url, "https://example.com/a.bin");
        assert_eq!(split[1].url, "https://example.com/b.bin");
        for request in &split {
            assert_eq!(request.cookies, "sid=1");
            assert_eq!(request.referrer, "https://example.com/page");
            assert_eq!(request.save_dir, "/captured");
            assert_eq!(request.headers, captured().headers);
            assert!(request.filename.is_empty());
            assert!(request.method.is_none() && request.body.is_none());
            assert!(request.audio_url.is_none() && request.file_size.is_none());
        }

        let single = split_batch(captured());
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].url, "https://example.com/a.bin");
        assert_eq!(single[0].filename, "a.bin");
        assert_eq!(single[0].method.as_deref(), Some("POST"));
        assert!(single[0].audio_url.is_some());
    }

    fn captured() -> DownloadRequest {
        DownloadRequest {
            url: "https://example.com/a.bin".to_owned(),
            filename: "a.bin".to_owned(),
            save_dir: "/captured".to_owned(),
            referrer: "https://example.com/page".to_owned(),
            cookies: "sid=1".to_owned(),
            headers: Some(HashMap::from([
                ("User-Agent".to_owned(), "Browser/1".to_owned()),
                ("Authorization".to_owned(), "Basic YTpi".to_owned()),
                ("Accept".to_owned(), "*/*".to_owned()),
            ])),
            file_size: Some(-1),
            mime_type: None,
            method: Some("POST".to_owned()),
            body: Some(RequestBody::Urlencoded {
                raw: "k=v".to_owned(),
            }),
            audio_url: Some("https://example.com/a.m4a".to_owned()),
        }
    }

    fn form(value: serde_json::Value) -> CreateTaskRequest {
        serde_json::from_value(value).expect("form request")
    }

    #[test]
    fn pending_summary_exposes_header_names_but_no_secret_values() {
        let dto = pending_capture_dto(&captured());
        assert_eq!(dto.header_names, ["Accept", "Authorization", "User-Agent"]);
        assert!(dto.has_cookies);
        assert!(dto.has_authorization());
        assert_eq!(dto.file_size, 0);
        let wire = serde_json::to_string(&dto).expect("serialize summary");
        assert!(!wire.contains("sid=1"));
        assert!(!wire.contains("YTpi"));
    }

    #[test]
    fn blank_form_keeps_captured_context_and_form_choices() {
        let merged = merge_confirmed_request(
            captured(),
            form(json!({
                "url": "https://evil.example/other",
                "queueId": "later",
                "segments": 8,
                "startPaused": true,
            })),
        );
        assert_eq!(merged.url, "https://example.com/a.bin");
        assert_eq!(merged.file_name, "a.bin");
        assert_eq!(merged.save_dir, "/captured");
        assert_eq!(merged.cookies, "sid=1");
        assert_eq!(merged.referrer, "https://example.com/page");
        assert_eq!(merged.method.as_deref(), Some("POST"));
        assert!(matches!(merged.body, Some(RequestBody::Urlencoded { .. })));
        assert_eq!(
            merged.audio_url.as_deref(),
            Some("https://example.com/a.m4a")
        );
        assert_eq!(merged.queue_id, "later");
        assert_eq!(merged.segments, 8);
        assert!(merged.start_paused);
        let headers = merged.headers.expect("captured headers kept");
        assert_eq!(
            headers.get("User-Agent").map(String::as_str),
            Some("Browser/1")
        );
        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Basic YTpi")
        );
    }

    #[test]
    fn form_values_override_captured_context_case_insensitively() {
        let merged = merge_confirmed_request(
            captured(),
            form(json!({
                "url": "https://example.com/a.bin",
                "fileName": "renamed.bin",
                "saveDir": "/chosen",
                "cookies": "sid=2",
                "userAgent": "Custom/2",
                "headers": { "accept": "application/octet-stream", "X-Extra": "1" },
                "httpUser": "alice",
                "httpPassword": "secret",
                "saveSiteAuth": true,
            })),
        );
        assert_eq!(merged.file_name, "renamed.bin");
        assert_eq!(merged.save_dir, "/chosen");
        assert_eq!(merged.cookies, "sid=2");
        assert_eq!(merged.user_agent, "Custom/2");
        assert_eq!(merged.http_user, "alice");
        assert!(merged.save_site_auth);
        let headers = merged.headers.expect("merged headers");
        assert!(
            !headers
                .keys()
                .any(|name| name.eq_ignore_ascii_case("user-agent")),
            "form UA replaces captured User-Agent header"
        );
        assert!(!headers.contains_key("Accept"));
        assert_eq!(
            headers.get("accept").map(String::as_str),
            Some("application/octet-stream")
        );
        assert_eq!(headers.get("X-Extra").map(String::as_str), Some("1"));
        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Basic YTpi")
        );
    }
}
