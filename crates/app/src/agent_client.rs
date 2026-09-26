//! GPUI composition root 唯一 agent JSON-RPC 会话。

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use fluxdown_protocol::method;
use fluxdown_protocol::{
    ApplicationErrorCode, CLOSE_REASON_SERVICE_QUIT, EventFrame, RequestId, RpcErrorData,
    RpcNotification, RpcRequest, RpcResponse, ServiceHello, ServiceRole, Snapshot, SnapshotBody,
};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, header};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::service_bootstrap::ServiceBootstrap;

pub type AgentFuture<T> = Pin<Box<dyn Future<Output = Result<T, RpcErrorData>> + Send + 'static>>;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub enum AgentClientEvent {
    /// 连接 / 重连后的全量快照（带 `epoch`/`sequence` 游标）。
    Snapshot(Box<Snapshot>),
    Event(Box<EventFrame>),
    Stale,
    Fatal(RpcErrorData),
    /// agent 执行了完全退出（`system.shutdown`）：不再重连，界面随之退出。
    ServiceStopped,
}

#[derive(Clone)]
pub struct AgentClientConfig {
    pub rpc_url: String,
    pub bearer_path: PathBuf,
}

struct ClientCommand {
    method: String,
    params: Option<Value>,
    ack: oneshot::Sender<Result<Value, RpcErrorData>>,
}

pub struct AgentClient {
    commands: mpsc::Sender<ClientCommand>,
    runtime: Arc<tokio::runtime::Runtime>,
    bootstrap: Arc<ServiceBootstrap>,
}

impl AgentClient {
    pub fn start(
        config: AgentClientConfig,
        bootstrap: Arc<ServiceBootstrap>,
    ) -> Result<(Arc<Self>, mpsc::Receiver<AgentClientEvent>), AgentClientError> {
        validate_url(&config.rpc_url)?;
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("fluxdown-agent-client")
                .build()
                .map_err(AgentClientError::Runtime)?,
        );
        let (commands, command_rx) = mpsc::channel(64);
        let (events, event_rx) = mpsc::channel(1024);
        runtime.spawn(run_client(config, bootstrap.clone(), command_rx, events));
        Ok((
            Arc::new(Self {
                commands,
                runtime,
                bootstrap,
            }),
            event_rx,
        ))
    }

    /// 界面发起完全退出后调用：连接断开时不再拉起新的 agent。
    pub fn stop_service_bootstrap(&self) {
        self.bootstrap.stop();
    }

    pub fn call<P, R>(&self, method_name: &str, params: Option<P>) -> AgentFuture<R>
    where
        P: Serialize + Send + 'static,
        R: DeserializeOwned + Send + 'static,
    {
        let commands = self.commands.clone();
        let method_name = method_name.to_owned();
        Box::pin(async move {
            let params = params
                .map(serde_json::to_value)
                .transpose()
                .map_err(|_| internal_error())?;
            let (ack, response) = oneshot::channel();
            commands
                .send(ClientCommand {
                    method: method_name,
                    params,
                    ack,
                })
                .await
                .map_err(|_| unavailable_error())?;
            let value = response.await.map_err(|_| unavailable_error())??;
            serde_json::from_value(value).map_err(|_| internal_error())
        })
    }

    /// 在客户端自有 tokio 运行时上运行后台任务（单实例激活监听等）。
    pub fn spawn_background<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.runtime.spawn(future);
    }
}

async fn run_client(
    config: AgentClientConfig,
    bootstrap: Arc<ServiceBootstrap>,
    mut commands: mpsc::Receiver<ClientCommand>,
    events: mpsc::Sender<AgentClientEvent>,
) {
    let mut attempt = 0_usize;
    // 每个桌面进程只尝试替换一次协议不兼容的 agent，避免同目录二进制错配时反复互杀。
    let mut replaced_incompatible = false;
    loop {
        match connect(&config).await {
            Ok((socket, snapshot, buffered)) => {
                attempt = 0;
                let cursor = (snapshot.epoch.clone(), snapshot.sequence);
                if events
                    .send(AgentClientEvent::Snapshot(Box::new(snapshot)))
                    .await
                    .is_err()
                {
                    return;
                }
                match run_connected(socket, &mut commands, &events, cursor, buffered).await {
                    Ok(SessionEnd::ClientDropped) => return,
                    Ok(SessionEnd::ServiceQuit) => {
                        bootstrap.stop();
                        let _ = events.send(AgentClientEvent::ServiceStopped).await;
                        return;
                    }
                    Err(()) => {
                        if events.send(AgentClientEvent::Stale).await.is_err() {
                            return;
                        }
                    }
                }
            }
            Err(ConnectError::Refused) => {
                if bootstrap.ensure_running(&config.rpc_url).await.is_err()
                    && events.send(AgentClientEvent::Stale).await.is_err()
                {
                    return;
                }
            }
            Err(ConnectError::Incompatible) if !replaced_incompatible => {
                replaced_incompatible = true;
                if request_shutdown(&config).await {
                    // 旧 agent 关停 daemon 后退出；连接被拒时由 bootstrap 拉起同级新版本。
                    crate::service_bootstrap::wait_until_stopped(
                        &config.rpc_url,
                        Duration::from_secs(30),
                    )
                    .await;
                    attempt = 0;
                    continue;
                }
                let _ = events.send(AgentClientEvent::Fatal(protocol_error())).await;
                return;
            }
            Err(ConnectError::Incompatible) => {
                let _ = events.send(AgentClientEvent::Fatal(protocol_error())).await;
                return;
            }
            Err(ConnectError::Fatal(error)) => {
                let _ = events.send(AgentClientEvent::Fatal(error)).await;
                return;
            }
            Err(ConnectError::Transient) => {
                if events.send(AgentClientEvent::Stale).await.is_err() {
                    return;
                }
            }
        }
        tokio::time::sleep(retry_delay(attempt)).await;
        attempt = attempt.saturating_add(1);
    }
}

/// 连续快速重试的次数（100ms 间隔，约 5s）：覆盖回环重连、agent 冷启动与重启，让界面在
/// 连接宽限内恢复而不显示连接态；之后指数退避，避免后台长期不可用时空转。
const FAST_RETRY_ATTEMPTS: usize = 50;
const FAST_RETRY_INTERVAL: Duration = Duration::from_millis(100);
const BACKOFF_SECS: [u64; 5] = [1, 2, 5, 15, 30];

/// 第 `attempt` 次（从 0 起）连接失败后的等待时间。
fn retry_delay(attempt: usize) -> Duration {
    match attempt.checked_sub(FAST_RETRY_ATTEMPTS) {
        None => FAST_RETRY_INTERVAL,
        Some(slow) => Duration::from_secs(BACKOFF_SECS[slow.min(BACKOFF_SECS.len() - 1)]),
    }
}

/// 握手前 `system.shutdown`：只有支持该首帧的 agent（协议 v4 起）会受理。
async fn request_shutdown(config: &AgentClientConfig) -> bool {
    let Ok(mut socket) = open_socket(config).await else {
        return false;
    };
    let mut buffered = Vec::new();
    call_on_socket(&mut socket, 1, method::SYSTEM_SHUTDOWN, None, &mut buffered)
        .await
        .is_ok()
}

async fn open_socket(config: &AgentClientConfig) -> Result<Socket, ConnectError> {
    let bearer = tokio::fs::read_to_string(&config.bearer_path)
        .await
        .map_err(|_| ConnectError::Refused)?;
    let bearer = bearer.trim();
    if bearer.is_empty() {
        return Err(ConnectError::Refused);
    }
    let mut request = config
        .rpc_url
        .clone()
        .into_client_request()
        .map_err(|_| ConnectError::Fatal(protocol_error()))?;
    let authorization = HeaderValue::from_str(&format!("Bearer {bearer}"))
        .map_err(|_| ConnectError::Fatal(protocol_error()))?;
    request
        .headers_mut()
        .insert(header::AUTHORIZATION, authorization);
    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(classify_connect_error)?;
    Ok(socket)
}

async fn connect(
    config: &AgentClientConfig,
) -> Result<(Socket, Snapshot, Vec<EventFrame>), ConnectError> {
    let mut socket = open_socket(config).await?;
    let mut buffered = Vec::new();
    let hello = serde_json::json!({
        "clientName": "fluxdown-desktop",
        "clientVersion": env!("CARGO_PKG_VERSION"),
        "minProtocolVersion": fluxdown_protocol::MIN_PROTOCOL_VERSION,
        "maxProtocolVersion": fluxdown_protocol::PROTOCOL_VERSION,
        "requestedRole": "agent",
        "capabilities": [method::CAPABILITY_CLIENT_SELECTIONS]
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
    if service.role != ServiceRole::Agent {
        return Err(ConnectError::Fatal(protocol_error()));
    }
    if service.protocol_version != fluxdown_protocol::PROTOCOL_VERSION {
        return Err(ConnectError::Incompatible);
    }
    let snapshot_value =
        call_on_socket(&mut socket, 2, method::SYSTEM_SNAPSHOT, None, &mut buffered).await?;
    let snapshot = serde_json::from_value::<Snapshot>(snapshot_value)
        .map_err(|_| ConnectError::Fatal(protocol_error()))?;
    if !matches!(snapshot.body, SnapshotBody::Agent(_)) {
        return Err(ConnectError::Fatal(protocol_error()));
    }
    Ok((socket, snapshot, buffered))
}

/// 连接正常结束的原因；`Err(())` 表示断线（需重连）。
enum SessionEnd {
    /// 本进程已丢弃客户端（命令通道关闭）。
    ClientDropped,
    /// agent 以 `service-quit` 关闭：完全退出，不再重连。
    ServiceQuit,
}

async fn run_connected(
    mut socket: Socket,
    commands: &mut mpsc::Receiver<ClientCommand>,
    events: &mpsc::Sender<AgentClientEvent>,
    snapshot_cursor: (String, u64),
    buffered: Vec<EventFrame>,
) -> Result<SessionEnd, ()> {
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
                let Some(command) = command else { return Ok(SessionEnd::ClientDropped); };
                let id = next_id;
                next_id = next_id.saturating_add(1);
                let request = RpcRequest::new(RequestId::Integer(id), command.method, command.params);
                let text = serde_json::to_string(&request).map_err(|_| ())?;
                pending.insert(id, command.ack);
                if socket.send(Message::Text(text.into())).await.is_err() { break; }
            }
            incoming = socket.next() => {
                let text = match incoming {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(Some(frame))))
                        if frame.reason.as_str() == CLOSE_REASON_SERVICE_QUIT =>
                    {
                        for (_, ack) in pending.drain() {
                            let _ = ack.send(Err(unavailable_error()));
                        }
                        return Ok(SessionEnd::ServiceQuit);
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => continue,
                    _ => break,
                };
                if let Ok(notification) = serde_json::from_str::<RpcNotification>(&text)
                    && notification.method == method::SERVICE_EVENT
                {
                    let Some(params) = notification.params else { break; };
                    let Ok(frame) = serde_json::from_value::<EventFrame>(params) else { break; };
                    forward_event(frame, &mut cursor, events).await?;
                    continue;
                }
                let response = serde_json::from_str::<RpcResponse>(&text).map_err(|_| ())?;
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
                            let _ = ack.send(Err(failure.error.data.unwrap_or_else(internal_error)));
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

async fn forward_event(
    frame: EventFrame,
    cursor: &mut (String, u64),
    events: &mpsc::Sender<AgentClientEvent>,
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
        .send(AgentClientEvent::Event(Box::new(frame)))
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
    let text = serde_json::to_string(&request).map_err(|_| ConnectError::Transient)?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|_| ConnectError::Transient)?;
    while let Some(message) = socket.next().await {
        let message = message.map_err(|_| ConnectError::Transient)?;
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
                return Err(ConnectError::Transient);
            }
            buffered.push(frame);
            continue;
        }
        let response =
            serde_json::from_str::<RpcResponse>(&text).map_err(|_| ConnectError::Transient)?;
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
    Err(ConnectError::Transient)
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
        _ => ConnectError::Transient,
    }
}

enum ConnectError {
    Refused,
    Transient,
    /// 对端协议版本不兼容：可尝试让旧 agent 退出后由 bootstrap 拉起同级新版本。
    Incompatible,
    Fatal(RpcErrorData),
}

fn validate_url(url: &str) -> Result<(), AgentClientError> {
    let url = reqwest_url(url)?;
    let host = url
        .host()
        .ok_or_else(|| AgentClientError::Configuration("agent URL has no host".to_owned()))?;
    let loopback = host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if loopback {
        Ok(())
    } else {
        Err(AgentClientError::Configuration(
            "agent URL must be loopback".to_owned(),
        ))
    }
}

fn reqwest_url(url: &str) -> Result<tokio_tungstenite::tungstenite::http::Uri, AgentClientError> {
    url.parse::<tokio_tungstenite::tungstenite::http::Uri>()
        .map_err(|error| AgentClientError::Configuration(error.to_string()))
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

#[derive(Debug, thiserror::Error)]
pub enum AgentClientError {
    #[error("agent client configuration is invalid: {0}")]
    Configuration(String),
    #[error("could not create agent client runtime: {0}")]
    Runtime(std::io::Error),
}
