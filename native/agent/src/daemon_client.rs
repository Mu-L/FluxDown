//! agent 到 daemon 的单连接 JSON-RPC 客户端与重连快照恢复。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use fluxdown_protocol::method;
use fluxdown_protocol::{
    ApplicationErrorCode, EventFrame, RequestId, RpcErrorData, RpcNotification, RpcRequest,
    RpcResponse, ServiceHello, ServiceRole, Snapshot, SnapshotBody,
};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, header};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;

use crate::supervisor::DaemonSupervisor;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// daemon 客户端配置。
#[derive(Clone)]
pub struct DaemonClientConfig {
    pub rpc_url: String,
    pub bearer: String,
}

impl DaemonClientConfig {
    /// 拒绝非 loopback daemon URL。
    pub fn validate(&self) -> Result<(), DaemonClientError> {
        let url = reqwest::Url::parse(&self.rpc_url)
            .map_err(|error| DaemonClientError::Configuration(error.to_string()))?;
        let host = url
            .host_str()
            .ok_or_else(|| DaemonClientError::Configuration("daemon URL has no host".to_owned()))?;
        let loopback = host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback());
        if loopback {
            Ok(())
        } else {
            Err(DaemonClientError::Configuration(
                "daemon URL must be loopback".to_owned(),
            ))
        }
    }
}

/// daemon 连接产生的有序状态流。
pub enum DaemonClientEvent {
    Snapshot(Snapshot),
    Event(Box<EventFrame>),
    Stale,
    Fatal(RpcErrorData),
}

struct ClientCommand {
    method: String,
    params: Option<Value>,
    ack: oneshot::Sender<Result<Value, RpcErrorData>>,
}

/// 首次连上 daemon 之前（agent 冷启动，Gateway 已先行服务）的调用等待上限，与 agent 启动
/// 就绪预算一致：期间到达的捕获 / 兼容 API / 迁移等调用等 daemon 就绪后送达，而不是直接失败。
const STARTUP_CALL_WAIT: Duration = Duration::from_secs(30);

/// 可克隆的 daemon 调用入口。
#[derive(Clone)]
pub struct DaemonClient {
    commands: mpsc::Sender<ClientCommand>,
    connected: Arc<AtomicBool>,
    /// 首次连接已有结论（连上过，或重连任务已终止）：此后断线期间的调用立即失败。
    settled: Arc<AtomicBool>,
    ready: Arc<Notify>,
    cancel: CancellationToken,
}

impl DaemonClient {
    /// 启动重连任务与有界事件流；`cancel` 触发后不再为首次连接等待。
    pub fn start(
        config: DaemonClientConfig,
        supervisor: Arc<DaemonSupervisor>,
        cancel: CancellationToken,
    ) -> Result<(Self, mpsc::Receiver<DaemonClientEvent>), DaemonClientError> {
        config.validate()?;
        let (commands, command_rx) = mpsc::channel(64);
        let (events, event_rx) = mpsc::channel(1024);
        let connected = Arc::new(AtomicBool::new(false));
        let settled = Arc::new(AtomicBool::new(false));
        let ready = Arc::new(Notify::new());
        tokio::spawn(run_client(
            config,
            supervisor,
            command_rx,
            events,
            connected.clone(),
            SettleOnExit {
                settled: settled.clone(),
                ready: ready.clone(),
            },
        ));
        Ok((
            Self {
                commands,
                connected,
                settled,
                ready,
                cancel,
            },
            event_rx,
        ))
    }

    /// 当前是否已连上 daemon（不等待）。
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    /// 提交类型化 RPC 调用。首次连上之前等待就绪（有上限）；之后断线期间立即失败。
    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: Option<P>,
    ) -> Result<R, RpcErrorData> {
        if !self.is_connected() && !self.settled.load(Ordering::Acquire) {
            tokio::select! {
                _ = self.cancel.cancelled() => {}
                _ = self.wait_until(STARTUP_CALL_WAIT, || {
                    self.is_connected() || self.settled.load(Ordering::Acquire)
                }) => {}
            }
        }
        if !self.is_connected() {
            return Err(unavailable_error());
        }
        let params = match params {
            Some(params) => Some(serde_json::to_value(params).map_err(|_| internal_error())?),
            None => None,
        };
        let (ack, response) = oneshot::channel();
        self.commands
            .send(ClientCommand {
                method: method.to_owned(),
                params,
                ack,
            })
            .await
            .map_err(|_| unavailable_error())?;
        let value = response.await.map_err(|_| unavailable_error())??;
        serde_json::from_value(value).map_err(|_| internal_error())
    }

    pub async fn wait_ready(&self, timeout: Duration) -> Result<(), RpcErrorData> {
        self.wait_until(timeout, || self.is_connected()).await
    }

    /// 等 `done` 成立；先登记唤醒再检查条件，连接恰在检查与等待之间建立也不会漏掉通知。
    async fn wait_until(
        &self,
        timeout: Duration,
        done: impl Fn() -> bool,
    ) -> Result<(), RpcErrorData> {
        tokio::time::timeout(timeout, async {
            loop {
                let notified = self.ready.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if done() {
                    return;
                }
                notified.await;
            }
        })
        .await
        .map_err(|_| unavailable_error())
    }
}

/// 重连任务退出（协议不兼容 / 致命错误）时结束首次连接等待，调用方不再空等到上限。
struct SettleOnExit {
    settled: Arc<AtomicBool>,
    ready: Arc<Notify>,
}

impl SettleOnExit {
    fn settle(&self) {
        self.settled.store(true, Ordering::Release);
        self.ready.notify_waiters();
    }
}

impl Drop for SettleOnExit {
    fn drop(&mut self) {
        self.settle();
    }
}

#[cfg(test)]
impl DaemonClient {
    /// 已断线（首次连接已有结论）的客户端：调用立即失败。
    pub(crate) fn disconnected() -> Self {
        let (commands, receiver) = mpsc::channel(1);
        drop(receiver);
        Self {
            commands,
            connected: Arc::new(AtomicBool::new(false)),
            settled: Arc::new(AtomicBool::new(true)),
            ready: Arc::new(Notify::new()),
            cancel: CancellationToken::new(),
        }
    }
}

/// 本进程刚拉起、仍存活但尚未监听的 daemon 按短间隔轮询（约 10s）：冷启动 daemon 通常
/// 数百毫秒内就绪，若按 1s 起步的退避等待，整条启动链（界面首个快照）会被白白拖长约 1s。
/// 只对同一个子进程代际生效；子进程退出后被重新拉起则回到指数退避，避免崩溃循环高频拉起。
const STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(50);
const STARTUP_POLL_ATTEMPTS: usize = 200;

async fn run_client(
    config: DaemonClientConfig,
    supervisor: Arc<DaemonSupervisor>,
    mut commands: mpsc::Receiver<ClientCommand>,
    events: mpsc::Sender<DaemonClientEvent>,
    connected: Arc<AtomicBool>,
    settle: SettleOnExit,
) {
    let backoff = [1_u64, 2, 5, 15, 30];
    let mut attempt = 0_usize;
    // 本轮断连中快速轮询的 daemon 子进程代际与已用次数；连上即清零。
    let mut polled_child: Option<u64> = None;
    let mut startup_polls = 0_usize;
    // 每个 agent 进程只尝试替换一次协议不兼容的 daemon，避免同目录二进制错配时反复互杀。
    let mut replaced_incompatible = false;
    loop {
        match connect(&config).await {
            Ok((socket, snapshot, buffered)) => {
                attempt = 0;
                polled_child = None;
                startup_polls = 0;
                connected.store(true, Ordering::Release);
                settle.settle();
                let snapshot_cursor = (snapshot.epoch.clone(), snapshot.sequence);
                if events
                    .send(DaemonClientEvent::Snapshot(snapshot))
                    .await
                    .is_err()
                {
                    connected.store(false, Ordering::Release);
                    return;
                }
                if run_connected(socket, &mut commands, &events, snapshot_cursor, buffered)
                    .await
                    .is_err()
                {
                    let _ = events.send(DaemonClientEvent::Stale).await;
                }
                connected.store(false, Ordering::Release);
                fail_queued_commands(&mut commands);
            }
            Err(ConnectError::Refused) => match supervisor.ensure_running().await {
                Ok(Some(child))
                    if startup_polls < STARTUP_POLL_ATTEMPTS
                        && *polled_child.get_or_insert(child) == child =>
                {
                    startup_polls += 1;
                    tokio::time::sleep(STARTUP_POLL_INTERVAL).await;
                    continue;
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(error = %error, "could not supervise fluxdownd");
                }
            },
            Err(ConnectError::Incompatible) if !replaced_incompatible => {
                replaced_incompatible = true;
                match request_shutdown(&config).await {
                    Ok(()) => {
                        tracing::info!("asked protocol-incompatible fluxdownd to exit");
                        wait_until_stopped(&config, Duration::from_secs(30)).await;
                        attempt = 0;
                        continue;
                    }
                    Err(error) => {
                        tracing::warn!(%error, "protocol-incompatible fluxdownd refused shutdown");
                        let _ = events
                            .send(DaemonClientEvent::Fatal(protocol_error()))
                            .await;
                        connected.store(false, Ordering::Release);
                        return;
                    }
                }
            }
            Err(ConnectError::Incompatible) => {
                let _ = events
                    .send(DaemonClientEvent::Fatal(protocol_error()))
                    .await;
                connected.store(false, Ordering::Release);
                return;
            }
            Err(ConnectError::Fatal(error)) => {
                let _ = events.send(DaemonClientEvent::Fatal(error)).await;
                connected.store(false, Ordering::Release);
                return;
            }
            Err(ConnectError::Transient(error)) => {
                tracing::warn!(%error, "daemon connection failed");
            }
        }
        connected.store(false, Ordering::Release);
        let delay = backoff[attempt.min(backoff.len() - 1)];
        attempt = attempt.saturating_add(1);
        tokio::time::sleep(Duration::from_secs(delay)).await;
    }
}

fn fail_queued_commands(commands: &mut mpsc::Receiver<ClientCommand>) {
    while let Ok(command) = commands.try_recv() {
        let _ = command.ack.send(Err(unavailable_error()));
    }
}

async fn open_socket(config: &DaemonClientConfig) -> Result<Socket, ConnectError> {
    let mut request = config
        .rpc_url
        .clone()
        .into_client_request()
        .map_err(|error| ConnectError::Fatal(invalid_argument(error.to_string())))?;
    let authorization = HeaderValue::from_str(&format!("Bearer {}", config.bearer))
        .map_err(|error| ConnectError::Fatal(invalid_argument(error.to_string())))?;
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, authorization);
    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(classify_connect_error)?;
    Ok(socket)
}

/// 握手前 `system.shutdown`：只有支持该首帧的 daemon（v4 起）会受理。
async fn request_shutdown(config: &DaemonClientConfig) -> Result<(), String> {
    let mut socket = open_socket(config)
        .await
        .map_err(|_| "daemon unreachable".to_owned())?;
    let mut buffered = Vec::new();
    call_on_socket(&mut socket, 1, method::SYSTEM_SHUTDOWN, None, &mut buffered)
        .await
        .map(|_| ())
        .map_err(|error| match error {
            ConnectError::Fatal(data) => format!("{:?}", data.code),
            ConnectError::Transient(message) => message,
            ConnectError::Refused | ConnectError::Incompatible => "daemon unreachable".to_owned(),
        })
}

/// 等旧 daemon 关闭监听（连接被拒）；超时后交回重连循环自愈。
async fn wait_until_stopped(config: &DaemonClientConfig, timeout: Duration) {
    let Ok(url) = reqwest::Url::parse(&config.rpc_url) else {
        return;
    };
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return;
    };
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        match TcpStream::connect((host.as_str(), port)).await {
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => return,
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

async fn connect(
    config: &DaemonClientConfig,
) -> Result<(Socket, Snapshot, Vec<EventFrame>), ConnectError> {
    let mut socket = open_socket(config).await?;
    let mut buffered = Vec::new();
    let hello = serde_json::json!({
        "clientName": "fluxdown-agent",
        "clientVersion": env!("CARGO_PKG_VERSION"),
        "minProtocolVersion": fluxdown_protocol::MIN_PROTOCOL_VERSION,
        "maxProtocolVersion": fluxdown_protocol::PROTOCOL_VERSION,
        "requestedRole": "daemon",
        "capabilities": []
    });
    let hello_value = call_on_socket(
        &mut socket,
        1,
        method::SYSTEM_HELLO,
        Some(hello),
        &mut buffered,
    )
    .await
    .map_err(|error| match error {
        ConnectError::Fatal(data) if data.code == ApplicationErrorCode::ProtocolIncompatible => {
            ConnectError::Incompatible
        }
        other => other,
    })?;
    let service = serde_json::from_value::<ServiceHello>(hello_value)
        .map_err(|_| ConnectError::Fatal(protocol_error()))?;
    if service.role != ServiceRole::Daemon {
        return Err(ConnectError::Fatal(protocol_error()));
    }
    if service.protocol_version != fluxdown_protocol::PROTOCOL_VERSION {
        return Err(ConnectError::Incompatible);
    }
    let snapshot_value =
        call_on_socket(&mut socket, 2, method::SYSTEM_SNAPSHOT, None, &mut buffered).await?;
    let snapshot = serde_json::from_value::<Snapshot>(snapshot_value)
        .map_err(|_| ConnectError::Fatal(protocol_error()))?;
    if !matches!(snapshot.body, SnapshotBody::Daemon(_)) {
        return Err(ConnectError::Fatal(protocol_error()));
    }
    Ok((socket, snapshot, buffered))
}

async fn run_connected(
    mut socket: Socket,
    commands: &mut mpsc::Receiver<ClientCommand>,
    events: &mpsc::Sender<DaemonClientEvent>,
    snapshot_cursor: (String, u64),
    buffered: Vec<EventFrame>,
) -> Result<(), ()> {
    let mut next_id = 10_i64;
    let mut pending = HashMap::<i64, oneshot::Sender<Result<Value, RpcErrorData>>>::new();
    let mut cursor = snapshot_cursor;
    for frame in buffered {
        if frame.epoch == cursor.0 {
            forward_event(frame, &mut cursor, events).await?;
        }
    }
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { return Ok(()); };
                let id = next_id;
                next_id = next_id.saturating_add(1);
                let request = RpcRequest::new(RequestId::Integer(id), command.method, command.params);
                let text = serde_json::to_string(&request).map_err(|_| ())?;
                pending.insert(id, command.ack);
                if socket.send(Message::Text(text.into())).await.is_err() { break; }
            }
            incoming = socket.next() => {
                let Some(Ok(Message::Text(text))) = incoming else { break; };
                if let Ok(notification) = serde_json::from_str::<RpcNotification>(&text)
                    && notification.method == method::SERVICE_EVENT
                {
                    let Some(params) = notification.params else { break; };
                    let Ok(frame) = serde_json::from_value::<EventFrame>(params) else { break; };
                    forward_event(frame, &mut cursor, events).await?;
                    continue;
                }
                let Ok(response) = serde_json::from_str::<RpcResponse>(&text) else { break; };
                match response {
                    RpcResponse::Success(success) => {
                        if let RequestId::Integer(id) = success.id
                            && let Some(ack) = pending.remove(&id)
                        {
                            let _ = ack.send(Ok(success.result));
                        }
                    }
                    RpcResponse::Failure(failure) => {
                        if let Some(RequestId::Integer(id)) = failure.id
                            && let Some(ack) = pending.remove(&id)
                        {
                            let error = failure.error.data.unwrap_or_else(internal_error);
                            let _ = ack.send(Err(error));
                        }
                    }
                }
            }
        }
    }
    for (_, ack) in pending {
        let _ = ack.send(Err(unavailable_error()));
    }
    Err(())
}

/// 快照以前的通知可跳过；快照以后的缺口或 epoch 更换必须重新握手。
async fn forward_event(
    frame: EventFrame,
    cursor: &mut (String, u64),
    events: &mpsc::Sender<DaemonClientEvent>,
) -> Result<(), ()> {
    if frame.epoch != cursor.0 {
        return Err(());
    }
    if frame.sequence <= cursor.1 {
        return Ok(());
    }
    if frame.sequence != cursor.1.saturating_add(1) {
        return Err(());
    }
    cursor.1 = frame.sequence;
    events
        .send(DaemonClientEvent::Event(Box::new(frame)))
        .await
        .map_err(|_| ())
}

async fn call_on_socket(
    socket: &mut Socket,
    id: i64,
    method_name: &str,
    params: Option<Value>,
    buffered: &mut Vec<EventFrame>,
) -> Result<Value, ConnectError> {
    let request = RpcRequest::new(RequestId::Integer(id), method_name, params);
    let text = serde_json::to_string(&request)
        .map_err(|error| ConnectError::Transient(error.to_string()))?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|error| ConnectError::Transient(error.to_string()))?;
    while let Some(message) = socket.next().await {
        let message = message.map_err(|error| ConnectError::Transient(error.to_string()))?;
        let Message::Text(text) = message else {
            continue;
        };
        if let Ok(notification) = serde_json::from_str::<RpcNotification>(&text)
            && notification.method == method::SERVICE_EVENT
        {
            let Some(params) = notification.params else {
                return Err(ConnectError::Fatal(protocol_error()));
            };
            let frame = serde_json::from_value::<EventFrame>(params)
                .map_err(|_| ConnectError::Fatal(protocol_error()))?;
            if buffered.len() >= 1024 {
                return Err(ConnectError::Transient(
                    "handshake event backlog".to_owned(),
                ));
            }
            buffered.push(frame);
            continue;
        }
        let response = serde_json::from_str::<RpcResponse>(&text)
            .map_err(|error| ConnectError::Transient(error.to_string()))?;
        match response {
            RpcResponse::Success(success) if success.id == RequestId::Integer(id) => {
                return Ok(success.result);
            }
            RpcResponse::Failure(failure) if failure.id == Some(RequestId::Integer(id)) => {
                return Err(ConnectError::Fatal(
                    failure.error.data.unwrap_or_else(internal_error),
                ));
            }
            _ => {}
        }
    }
    Err(ConnectError::Transient("daemon socket closed".to_owned()))
}

fn classify_connect_error(error: tokio_tungstenite::tungstenite::Error) -> ConnectError {
    match &error {
        tokio_tungstenite::tungstenite::Error::Io(io)
            if matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
            ) =>
        {
            ConnectError::Refused
        }
        tokio_tungstenite::tungstenite::Error::Http(response) if response.status() == 401 => {
            ConnectError::Fatal(RpcErrorData::new(ApplicationErrorCode::Unauthorized, false))
        }
        _ => ConnectError::Transient(format!("{error:#}")),
    }
}

enum ConnectError {
    Refused,
    Transient(String),
    /// 对端协议版本不兼容：可尝试让旧进程退出后由监管拉起同版本 daemon。
    Incompatible,
    Fatal(RpcErrorData),
}

/// daemon 客户端启动错误。
#[derive(Debug, thiserror::Error)]
pub enum DaemonClientError {
    #[error("daemon client configuration is invalid: {0}")]
    Configuration(String),
}

fn unavailable_error() -> RpcErrorData {
    RpcErrorData::new(ApplicationErrorCode::Unavailable, true)
}

fn internal_error() -> RpcErrorData {
    RpcErrorData::new(ApplicationErrorCode::Internal, false)
}

fn protocol_error() -> RpcErrorData {
    RpcErrorData::new(ApplicationErrorCode::ProtocolIncompatible, false)
}

fn invalid_argument(_message: String) -> RpcErrorData {
    RpcErrorData::new(ApplicationErrorCode::InvalidArgument, false)
}

#[cfg(test)]
mod tests {
    use fluxdown_protocol::ApplicationErrorCode;

    use super::DaemonClient;

    #[tokio::test]
    async fn disconnected_client_rejects_without_queueing_for_replay() {
        let client = DaemonClient::disconnected();
        let result = client
            .call::<serde_json::Value, serde_json::Value>(
                fluxdown_protocol::method::DAEMON_TASK_CREATE,
                Some(serde_json::json!({})),
            )
            .await;
        let error = result.expect_err("disconnected command must fail");
        assert_eq!(error.code, ApplicationErrorCode::Unavailable);
        assert!(error.retryable);
    }

    fn pending_client() -> (
        DaemonClient,
        tokio::sync::mpsc::Receiver<super::ClientCommand>,
        super::SettleOnExit,
    ) {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;

        let (commands, receiver) = tokio::sync::mpsc::channel(1);
        let settled = Arc::new(AtomicBool::new(false));
        let ready = Arc::new(tokio::sync::Notify::new());
        let client = DaemonClient {
            commands,
            connected: Arc::new(AtomicBool::new(false)),
            settled: settled.clone(),
            ready: ready.clone(),
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        (client, receiver, super::SettleOnExit { settled, ready })
    }

    #[tokio::test]
    async fn call_before_first_connection_is_delivered_once_connected() {
        let (client, mut commands, settle) = pending_client();
        let caller = client.clone();
        let call = tokio::spawn(async move {
            caller
                .call::<serde_json::Value, serde_json::Value>("task.create", None)
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            !call.is_finished(),
            "call must wait for the first connection"
        );
        client
            .connected
            .store(true, std::sync::atomic::Ordering::Release);
        settle.settle();
        let command = commands.recv().await.expect("command delivered");
        assert_eq!(command.method, "task.create");
        let _ = command.ack.send(Ok(serde_json::json!({"ok": true})));
        let result = call.await.expect("join").expect("call succeeds");
        assert_eq!(result, serde_json::json!({"ok": true}));
    }

    #[tokio::test]
    async fn call_fails_fast_when_client_gives_up_or_agent_cancels() {
        let (client, _commands, settle) = pending_client();
        let caller = client.clone();
        let call = tokio::spawn(async move {
            caller
                .call::<serde_json::Value, serde_json::Value>("task.create", None)
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        drop(settle);
        let error = tokio::time::timeout(std::time::Duration::from_secs(1), call)
            .await
            .expect("must not wait for the startup budget")
            .expect("join")
            .expect_err("never connected");
        assert_eq!(error.code, ApplicationErrorCode::Unavailable);

        let (client, _commands, _settle) = pending_client();
        client.cancel.cancel();
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            client.call::<serde_json::Value, serde_json::Value>("task.create", None),
        )
        .await
        .expect("cancelled agent must not wait")
        .expect_err("never connected");
        assert_eq!(error.code, ApplicationErrorCode::Unavailable);
    }

    #[tokio::test]
    async fn handshake_buffers_notifications_filters_old_frames_and_reconnects_after_gap() {
        use fluxdown_protocol::{
            DaemonEvent, DaemonSnapshot, EventFrame, RequestId, RpcNotification, RpcResponse,
            ServiceEvent, ServiceHello, ServiceRole, Snapshot, SnapshotBody, TaskRuntimeDto,
        };
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        fn notification(epoch: &str, sequence: u64) -> Message {
            let frame = EventFrame {
                epoch: epoch.into(),
                sequence,
                event: ServiceEvent::Daemon(DaemonEvent::TaskRuntimeChanged(TaskRuntimeDto {
                    task_id: "task".into(),
                    active_transfers: Some(sequence as u32),
                    ..Default::default()
                })),
            };
            let notification = RpcNotification::new(
                fluxdown_protocol::method::SERVICE_EVENT,
                Some(serde_json::to_value(frame).unwrap()),
            );
            Message::Text(serde_json::to_string(&notification).unwrap().into())
        }

        async fn serve_once(
            listener: &tokio::net::TcpListener,
            epoch: &str,
            sequence: u64,
            gap: bool,
        ) {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            ws.next().await.expect("hello request").unwrap();
            if gap {
                ws.send(notification("previous", 3)).await.unwrap();
            }
            let hello =
                ServiceHello::new(ServiceRole::Daemon, "daemon", "test", "instance", vec![]);
            ws.send(Message::Text(
                serde_json::to_string(&RpcResponse::success(
                    RequestId::Integer(1),
                    serde_json::to_value(hello).unwrap(),
                ))
                .unwrap()
                .into(),
            ))
            .await
            .unwrap();
            ws.next().await.expect("snapshot request").unwrap();
            ws.send(notification(epoch, sequence.saturating_sub(1)))
                .await
                .unwrap();
            ws.send(notification(epoch, sequence + 1)).await.unwrap();
            let snapshot = Snapshot {
                epoch: epoch.into(),
                sequence,
                body: SnapshotBody::Daemon(Box::new(DaemonSnapshot::default())),
            };
            ws.send(Message::Text(
                serde_json::to_string(&RpcResponse::success(
                    RequestId::Integer(2),
                    serde_json::to_value(snapshot).unwrap(),
                ))
                .unwrap()
                .into(),
            ))
            .await
            .unwrap();
            if gap {
                ws.send(notification(epoch, sequence + 2)).await.unwrap();
                ws.send(notification(epoch, sequence + 4)).await.unwrap();
            }
            ws.close(None).await.unwrap();
        }

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}/rpc", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                serve_once(&listener, "a", 5, true).await;
                serve_once(&listener, "b", 20, false).await;
            });
            let config = super::DaemonClientConfig {
                rpc_url: url,
                bearer: "test".into(),
            };
            let (tx, mut rx) = tokio::sync::mpsc::channel(16);
            let (command_tx, mut commands) = tokio::sync::mpsc::channel(1);
            let (socket, snapshot, buffered) = super::connect(&config)
                .await
                .unwrap_or_else(|_| panic!("connect"));
            assert_eq!((snapshot.epoch.as_str(), snapshot.sequence), ("a", 5));
            assert!(
                super::run_connected(
                    socket,
                    &mut commands,
                    &tx,
                    (snapshot.epoch, snapshot.sequence),
                    buffered
                )
                .await
                .is_err()
            );
            let first = rx.recv().await.expect("buffered next event");
            let second = rx.recv().await.expect("live next event");
            let [
                super::DaemonClientEvent::Event(first),
                super::DaemonClientEvent::Event(second),
            ] = [first, second]
            else {
                panic!("events")
            };
            assert_eq!((first.sequence, second.sequence), (6, 7));
            assert!(rx.try_recv().is_err(), "gap event must not be delivered");
            let (socket, snapshot, buffered) = super::connect(&config)
                .await
                .unwrap_or_else(|_| panic!("reconnect"));
            assert_eq!((snapshot.epoch.as_str(), snapshot.sequence), ("b", 20));
            super::run_connected(
                socket,
                &mut commands,
                &tx,
                (snapshot.epoch, snapshot.sequence),
                buffered,
            )
            .await
            .ok();
            let super::DaemonClientEvent::Event(recovered) =
                rx.recv().await.expect("recovered event")
            else {
                panic!("event")
            };
            assert_eq!((recovered.epoch.as_str(), recovered.sequence), ("b", 21));
            drop(command_tx);
            server.await.unwrap();
        })
        .await
        .expect("websocket handshake and recovery");
    }
}
