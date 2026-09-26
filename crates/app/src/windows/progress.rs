//! 独立下载进度 / 完成窗口：每个任务一个，FluxDown 辅助窗口标题栏；标题随进度刷新，
//! 高度由视图按内容自行校正。开关时机由 [`crate::progress_windows`] 决定。
//!
//! 层级：界面常是后台应用（下载由浏览器捕获 / 静默建成时浏览器在前台），只在本应用内排序
//! 的窗口会压在浏览器下面。
//! - 进度窗口（用户刚开始的下载）：连同应用一起置前（`bring_to_front`）。
//! - 后台完成窗口：不抢焦点，macOS / Windows 用 `WindowKind::PopUp` 浮在所有应用之上
//!   （macOS 为不激活应用的浮动面板，Windows 为 `WS_EX_TOPMOST`）；Linux X11 的 PopUp 是
//!   不受窗口管理器管理的 override-redirect（不能拖动），这里保持普通窗口并请求注意。

use std::{sync::Arc, time::Duration};

use fluxdown_ui_downloads::{
    PROGRESS_WINDOW_INITIAL_HEIGHT, PROGRESS_WINDOW_WIDTH, ProgressWindowEvent, ProgressWindowView,
};
use fluxdown_ui_shell::{AuxiliaryWindowView, auxiliary_window_options};
use gpui::{
    App, AppContext as _, Bounds, Context, SharedString, Window, WindowBounds, WindowKind, point,
    px, size,
};
use gpui_component::Root;

use crate::{
    app::Desktop,
    downloads_port::AgentDownloadsPort,
    session::attach,
    windows::{WindowKey, WindowRegistry},
};

/// 同时打开多个窗口时逐个错开的距离，避免完全重叠。
const CASCADE_STEP: f32 = 28.;
/// 错开的最大档数：更多窗口回到同一位置循环。
const CASCADE_MAX: usize = 5;
/// 打开文件 / 文件夹后等待对方接管前台的上限。
const HANDOFF_CLOSE_TIMEOUT: Duration = Duration::from_millis(1500);

/// 打开（或置前）任务的进度窗口。`activate = false` 时不抢焦点（后台完成弹窗）。
pub fn open(cx: &mut App, task_id: String, activate: bool) {
    let desktop = Desktop::global(cx);
    let translator = desktop.translator.clone();
    let session = desktop.session.clone();
    let client = desktop.client.clone();
    let title = translator.read(cx).text("progressWindowTitle").to_owned();
    let show_completion = crate::progress_windows::show_completion(cx, &task_id);

    let cascade =
        WindowRegistry::count(cx, |key| matches!(key, WindowKey::Progress(_))) % (CASCADE_MAX + 1);
    let offset = px(CASCADE_STEP * cascade as f32);
    let display_id = WindowRegistry::main_display_id(cx);
    let mut bounds = Bounds::centered(
        display_id,
        size(
            px(PROGRESS_WINDOW_WIDTH),
            px(PROGRESS_WINDOW_INITIAL_HEIGHT),
        ),
        cx,
    );
    bounds.origin = point(bounds.origin.x + offset, bounds.origin.y + offset);

    let mut options = auxiliary_window_options(title);
    options.display_id = display_id;
    options.window_bounds = Some(WindowBounds::Windowed(bounds));
    // 辅助窗口默认最小尺寸（720×520）会阻止按内容收缩，这里高度完全由内容决定。
    options.window_min_size = None;
    options.is_resizable = false;
    options.is_minimizable = true;
    options.focus = activate;
    if !activate && cfg!(any(target_os = "macos", target_os = "windows")) {
        options.kind = WindowKind::PopUp;
    }

    let key = WindowKey::Progress(task_id.clone());
    let opened = WindowRegistry::open_or_focus(cx, key.clone(), options, move |window, cx| {
        let port = Arc::new(AgentDownloadsPort::new(client.clone()));
        let view = cx.new(|cx| {
            ProgressWindowView::new(
                translator.clone(),
                task_id.clone(),
                port,
                show_completion,
                cx,
            )
        });
        attach(&session, &view, cx);
        let window_view = cx.new(|cx| {
            AuxiliaryWindowView::new(
                translator.clone(),
                "progressWindowTitle",
                view.clone().into(),
                cx,
            )
        });

        cx.new(|cx| {
            let root = Root::new(window_view.clone(), window, cx);
            cx.observe_in(&view, window, move |_, view, window, cx| {
                let Some(title) = view.read(cx).title() else {
                    return;
                };
                window.set_window_title(&title);
                window_view.update(cx, |chrome, cx| {
                    chrome.set_title(Some(SharedString::from(title)), cx);
                });
            })
            .detach();
            cx.subscribe_in(&view, window, move |_, _, event, window, cx| match event {
                ProgressWindowEvent::Close => window.remove_window(),
                ProgressWindowEvent::HandedOff => close_after_handoff(window, cx),
                ProgressWindowEvent::ShowCompletionChanged(value) => {
                    crate::progress_windows::set_completion_override(cx, &task_id, *value);
                }
            })
            .detach();
            root
        })
    });
    let handle = opened
        .map(Into::into)
        .or_else(|| WindowRegistry::handle(cx, &key));
    if let Some(handle) = handle {
        let _ = handle.update(cx, |_, window, cx| {
            if activate {
                crate::windows::bring_to_front(window, cx);
            } else {
                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                window.request_attention();
            }
        });
    }
}

/// 打开的文件 / 文件夹接管前台后再关窗。本应用仍在前台时直接关闭当前窗口，系统会把本应用
/// 的下一个窗口（通常是主窗口）提到前面、压住刚打开的程序；等窗口失活（对方已成为前台）再关，
/// 超时兜底（对方未抢前台时也不让窗口滞留）。
fn close_after_handoff(window: &mut Window, cx: &mut Context<Root>) {
    if !window.is_window_active() {
        window.remove_window();
        return;
    }
    cx.observe_window_activation(window, |_, window, _| {
        if !window.is_window_active() {
            window.remove_window();
        }
    })
    .detach();
    cx.spawn_in(window, async move |_, cx| {
        cx.background_executor().timer(HANDOFF_CLOSE_TIMEOUT).await;
        let _ = cx.update(|window, _| window.remove_window());
    })
    .detach();
}
