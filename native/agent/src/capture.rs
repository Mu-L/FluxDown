//! 浏览器/NMH 捕获确认事务：内存有界、单次消费且不持久化敏感请求上下文。
//!
//! Cookie / 请求头 / 请求体只留在事务里；官方 UI 只拿到 [`PendingCaptureDto`] 摘要，
//! 确认时提交表单产出的 [`CreateTaskRequest`]，由 [`merge_confirmed_request`] 以捕获原请求为底合并。

use std::collections::VecDeque;
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

    /// 静默策略直接提交 daemon；否则排入确认队列并在首项时唤起官方 UI。
    pub async fn submit(
        &self,
        request: DownloadRequest,
        silent: bool,
    ) -> Result<Value, CaptureError> {
        if silent {
            return self
                .create(captured_create_request(request), None, true)
                .await;
        }
        let public = pending_capture_dto(&request);
        let first = {
            let mut pending = self.pending.lock().await;
            if pending.len() >= CAPTURE_CAPACITY {
                return Err(CaptureError::Full);
            }
            let first = pending.is_empty();
            pending.push_back(CaptureTransaction {
                public: public.clone(),
                request,
            });
            first
        };
        self.publish().await;
        if first {
            self.shell.launch_for_prompt();
        }
        Ok(json!({ "transactionId": public.transaction_id }))
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

    use fluxdown_protocol::{CreateTaskRequest, RequestBody};
    use serde_json::json;

    use super::{DownloadRequest, merge_confirmed_request, pending_capture_dto};

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
