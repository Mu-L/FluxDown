//! 引擎发起的交互选择请求：每个 `request_id` 一个独立 `Floating` 窗口，互不阻塞、
//! 可并存；到期由 daemon 自动按默认值解析，`SelectionResolved` 事件负责关窗。

use std::sync::Arc;

use fluxdown_protocol::{
    AgentEvent, DaemonEvent, SelectionKind, SelectionRequestDto, ServiceEvent,
};
use fluxdown_ui_downloads::SelectionView;
use fluxdown_ui_shell::{AuxiliaryWindowView, auxiliary_window_options};
use gpui::{App, AppContext as _, Bounds, WindowBounds, WindowKind, px, size};
use gpui_component::Root;

use crate::{
    app::Desktop,
    downloads_port::AgentDownloadsPort,
    session::{SessionSignal, agent_body},
    windows::{WindowKey, WindowRegistry},
};

const HLS_VARIANT_SIZE: gpui::Size<gpui::Pixels> = size(px(520.), px(440.));
const BT_SIZE: gpui::Size<gpui::Pixels> = size(px(760.), px(580.));

/// 订阅会话：回放启动时已挂起的选择，跟随新到达/已解决的请求开关窗口。
pub fn install(cx: &mut App) {
    let session = Desktop::global(cx).session.clone();
    for request in pending_selections(&session, cx) {
        open(cx, request);
    }
    cx.subscribe(&session, |_, signal, cx| match signal {
        SessionSignal::Snapshot(snapshot) => {
            if let Some(body) = agent_body(snapshot) {
                for request in body.daemon.pending_selections.clone() {
                    open(cx, request);
                }
            }
        }
        SessionSignal::Event(frame) => match &frame.event {
            ServiceEvent::Agent(AgentEvent::Daemon(DaemonEvent::SelectionPending(request)))
            | ServiceEvent::Daemon(DaemonEvent::SelectionPending(request)) => {
                open(cx, request.clone());
            }
            // daemon 晚于 agent 就绪 / 重连：挂起的选择随替换快照到达，而不是逐条事件。
            ServiceEvent::Agent(AgentEvent::DaemonSnapshotReplaced(snapshot))
            | ServiceEvent::Agent(AgentEvent::Daemon(DaemonEvent::SnapshotReplaced(snapshot)))
            | ServiceEvent::Daemon(DaemonEvent::SnapshotReplaced(snapshot)) => {
                for request in snapshot.pending_selections.clone() {
                    open(cx, request);
                }
            }
            ServiceEvent::Agent(AgentEvent::Daemon(DaemonEvent::SelectionResolved {
                request_id,
            }))
            | ServiceEvent::Daemon(DaemonEvent::SelectionResolved { request_id }) => {
                WindowRegistry::close(cx, &WindowKey::Selection(request_id.clone()));
            }
            _ => {}
        },
        SessionSignal::Stale | SessionSignal::Fatal(_) | SessionSignal::ServiceStopped => {}
    })
    .detach();
}

fn pending_selections(
    session: &gpui::Entity<crate::session::AgentSession>,
    cx: &App,
) -> Vec<SelectionRequestDto> {
    session
        .read(cx)
        .agent_snapshot()
        .map(|body| body.daemon.pending_selections.clone())
        .unwrap_or_default()
}

/// 已开则跳过（去重）；否则按 `kind` 定尺寸、居中主窗口所在显示器打开。
fn open(cx: &mut App, request: SelectionRequestDto) {
    let key = WindowKey::Selection(request.request_id.clone());
    if WindowRegistry::is_open(cx, &key) {
        return;
    }
    let desktop = Desktop::global(cx);
    let translator = desktop.translator.clone();
    let client = desktop.client.clone();
    let session = desktop.session.clone();
    let task_name = task_name_for(&session, cx, &request.task_id);
    let title_key: &'static str = match &request.kind {
        SelectionKind::Hls { .. } => "hlsQualityTitle",
        SelectionKind::Bt { .. } => "btFileSelectTitle",
        SelectionKind::Variant { .. } => "resolveVariantTitle",
    };
    let window_size = match &request.kind {
        SelectionKind::Bt { .. } => BT_SIZE,
        _ => HLS_VARIANT_SIZE,
    };
    let display_id = WindowRegistry::handle(cx, &WindowKey::Main)
        .and_then(|handle| {
            handle
                .update(cx, |_, window, cx| window.display(cx))
                .ok()
                .flatten()
        })
        .map(|display| display.id());
    let bounds = Bounds::centered(display_id, window_size, cx);
    let mut options = auxiliary_window_options(translator.read(cx).text(title_key).to_owned());
    options.display_id = display_id;
    options.window_bounds = Some(WindowBounds::Windowed(bounds));
    options.window_min_size = None;
    options.kind = WindowKind::Floating;
    options.is_resizable = matches!(request.kind, SelectionKind::Bt { .. });

    WindowRegistry::open_or_focus(cx, key, options, move |window, cx| {
        if !task_name.is_empty() {
            window.set_window_title(&task_name);
        }
        let port = Arc::new(AgentDownloadsPort::new(client.clone()));
        let content = cx.new(|cx| {
            SelectionView::new(
                translator.clone(),
                request.clone(),
                task_name.clone(),
                port,
                window,
                cx,
            )
        });
        let window_view = cx
            .new(|cx| AuxiliaryWindowView::new(translator.clone(), title_key, content.into(), cx));
        cx.new(|cx| Root::new(window_view, window, cx))
    });
}

/// 从最近快照按 `task_id` 找文件名；找不到（元数据未落地/已删除）返回空串，
/// 由调用方用标题文案兜底，不把 UUID 暴露给用户。
fn task_name_for(
    session: &gpui::Entity<crate::session::AgentSession>,
    cx: &App,
    task_id: &str,
) -> String {
    session
        .read(cx)
        .agent_snapshot()
        .and_then(|body| {
            body.daemon
                .tasks
                .iter()
                .find(|task| task.task_id == task_id)
                .map(|task| task.file_name.clone())
        })
        .filter(|name| !name.is_empty())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_default()
}
