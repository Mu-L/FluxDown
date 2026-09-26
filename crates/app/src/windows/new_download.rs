//! 「新建下载」窗口：菜单 / 快捷键 / 拖放入口，以及外部捕获（浏览器扩展 / NMH）确认。
//!
//! 外部捕获不单独开确认弹窗：待确认队列里新出现的事务追加进同一个新建下载表单（没有就开
//! 一个，无主窗口时也可以）。提交时普通链接经主窗口下载页建任务（无主窗口时直接走 agent
//! 端口），捕获条目经 `agent.capture.resolve` 确认；表单释放时未确认的捕获一并忽略。
//! 成功后关窗并在主窗口 toast。

use std::{collections::HashSet, rc::Rc, sync::Arc};

use fluxdown_protocol::{AgentEvent, CaptureResolveParams, PendingCaptureDto, ServiceEvent};
use fluxdown_ui_downloads::{
    DownloadsCommand, DownloadsPort, NewDownloadContext, NewDownloadSubmission, NewDownloadView,
    new_download_context_from_snapshot,
};
use fluxdown_ui_i18n::keys;
use fluxdown_ui_shell::{AuxiliaryWindowView, auxiliary_window_options};
use gpui::{App, AppContext as _, Bounds, Global, WeakEntity, WindowBounds, px, size};
use gpui_component::{Root, WindowExt as _, notification::Notification};

use crate::{
    app::Desktop,
    downloads_port::AgentDownloadsPort,
    lifecycle,
    session::{SessionSignal, agent_body},
    windows::{WindowKey, WindowRegistry},
};

const NEW_DOWNLOAD_WINDOW_SIZE: gpui::Size<gpui::Pixels> = size(px(640.), px(530.));
const NEW_DOWNLOAD_WINDOW_MIN_SIZE: gpui::Size<gpui::Pixels> = size(px(560.), px(440.));

/// 外部捕获派发状态：当前表单 + 已交给表单的事务（agent 移除前不重复追加，
/// 关窗忽略后也不会因列表尚未刷新而重开）。
#[derive(Default)]
struct CaptureDispatch {
    form: Option<WeakEntity<NewDownloadView>>,
    handed: HashSet<String>,
}

impl Global for CaptureDispatch {}

/// 菜单 / 快捷键入口：上下文取自主窗口下载页；主窗口不存在时先重建主窗口。
pub fn open_default(cx: &mut App) {
    let downloads = main_downloads(cx);
    let Some(downloads) = downloads else {
        return;
    };
    let context = downloads.read(cx).new_download_context();
    open(cx, context);
}

fn main_downloads(cx: &mut App) -> Option<gpui::Entity<fluxdown_ui_downloads::DownloadView>> {
    if let Some(downloads) = Desktop::global(cx)
        .main_downloads
        .as_ref()
        .and_then(|weak| weak.upgrade())
    {
        return Some(downloads);
    }
    crate::windows::main::open(cx);
    Desktop::global(cx)
        .main_downloads
        .as_ref()
        .and_then(|weak| weak.upgrade())
}

pub fn open(cx: &mut App, context: NewDownloadContext) {
    open_with(cx, context, Vec::new());
}

/// 订阅会话：待确认捕获出现 → 追加进新建下载表单（必要时打开）；启动时回放快照。
pub fn install_captures(cx: &mut App) {
    cx.set_global(CaptureDispatch::default());
    let session = Desktop::global(cx).session.clone();
    let pending = session
        .read(cx)
        .agent_snapshot()
        .map(|body| body.pending_captures.clone());
    if let Some(pending) = pending {
        sync_captures(cx, &pending);
    }
    cx.subscribe(&session, |_, signal, cx| match signal {
        SessionSignal::Snapshot(snapshot) => {
            if let Some(body) = agent_body(snapshot) {
                sync_captures(cx, &body.pending_captures);
            }
        }
        SessionSignal::Event(frame) => {
            if let ServiceEvent::Agent(AgentEvent::PendingCapturesChanged(pending)) = &frame.event {
                sync_captures(cx, pending);
            }
        }
        SessionSignal::Stale | SessionSignal::Fatal(_) | SessionSignal::ServiceStopped => {}
    })
    .detach();
}

/// 对齐 agent 的待确认列表：已消失的事务从表单移除（链接行保留为普通链接），新事务追加
/// 进现有表单并置前；没有表单时以快照投影的环境开一个。
fn sync_captures(cx: &mut App, pending: &[PendingCaptureDto]) {
    let (form, fresh) = {
        let dispatch = cx.global_mut::<CaptureDispatch>();
        dispatch.handed.retain(|transaction_id| {
            pending
                .iter()
                .any(|capture| &capture.transaction_id == transaction_id)
        });
        let fresh = pending
            .iter()
            .filter(|capture| dispatch.handed.insert(capture.transaction_id.clone()))
            .cloned()
            .collect::<Vec<_>>();
        (dispatch.form.as_ref().and_then(WeakEntity::upgrade), fresh)
    };
    let mut fresh = Some(fresh);
    if let (Some(form), Some(handle)) = (form, WindowRegistry::handle(cx, &WindowKey::NewDownload))
    {
        let delivered = handle.update(cx, |_, window, cx| {
            let fresh = fresh.take().unwrap_or_default();
            let arrived = !fresh.is_empty();
            form.update(cx, |form, cx| {
                form.retain_captures(pending, window, cx);
                if arrived {
                    form.add_captures(fresh, window, cx);
                }
            });
            if arrived {
                crate::windows::bring_to_front(window, cx);
            }
        });
        if delivered.is_ok() {
            return;
        }
    }
    let fresh = fresh.unwrap_or_default();
    if fresh.is_empty() {
        return;
    }
    let context = Desktop::global(cx)
        .session
        .read(cx)
        .agent_snapshot()
        .map(new_download_context_from_snapshot)
        .unwrap_or_default();
    open_with(cx, context, fresh);
}

fn open_with(cx: &mut App, context: NewDownloadContext, captures: Vec<PendingCaptureDto>) {
    let desktop = Desktop::global(cx);
    let translator = desktop.translator.clone();
    let client = desktop.client.clone();
    let title = translator.read(cx).text(keys::NEW_DOWNLOAD).to_owned();
    let display_id = WindowRegistry::main_display_id(cx);
    let bounds = Bounds::centered(display_id, NEW_DOWNLOAD_WINDOW_SIZE, cx);
    let mut options = auxiliary_window_options(title);
    options.display_id = display_id;
    options.window_bounds = Some(WindowBounds::Windowed(bounds));
    options.window_min_size = Some(NEW_DOWNLOAD_WINDOW_MIN_SIZE);
    options.is_resizable = true;

    let handle =
        WindowRegistry::open_or_focus(cx, WindowKey::NewDownload, options, move |window, cx| {
            let port = Arc::new(AgentDownloadsPort::new(client));
            let submit_port = Arc::clone(&port);
            let on_submit = Rc::new(
                move |submission, _window: &mut gpui::Window, cx: &mut App| {
                    submit(submission, &submit_port, cx);
                },
            );
            let form_port: Arc<dyn DownloadsPort> = port.clone();
            let form = cx.new(|cx| {
                NewDownloadView::new(
                    translator.clone(),
                    context,
                    form_port,
                    on_submit,
                    window,
                    cx,
                )
            });
            if !captures.is_empty() {
                form.update(cx, |form, cx| form.add_captures(captures, window, cx));
            }
            cx.observe_release(&form, move |form, cx| {
                ignore_captures(form.take_captures(), &port, cx);
            })
            .detach();
            cx.default_global::<CaptureDispatch>().form = Some(form.downgrade());
            let window_view = cx.new(|cx| {
                AuxiliaryWindowView::new(translator, keys::NEW_DOWNLOAD, form.into(), cx)
            });
            cx.new(|cx| Root::new(window_view, window, cx))
        });
    // 菜单入口与外部捕获都是需要用户立即处理的操作：macOS 后台时也要将窗口及应用置前。
    if let Some(handle) = handle {
        let _ = handle.update(cx, |_, window, cx| {
            crate::windows::bring_to_front(window, cx)
        });
    } else if let Some(handle) = WindowRegistry::handle(cx, &WindowKey::NewDownload) {
        let _ = handle.update(cx, |_, window, cx| {
            crate::windows::bring_to_front(window, cx)
        });
    }
}

/// 表单关闭时仍未确认的捕获：逐条忽略，界面退出前等它们送达 agent。
fn ignore_captures(captures: Vec<PendingCaptureDto>, port: &AgentDownloadsPort, cx: &mut App) {
    for capture in captures {
        let ignore = port.execute(DownloadsCommand::CaptureResolve(Box::new(
            CaptureResolveParams {
                transaction_id: capture.transaction_id,
                accepted: false,
                request: None,
            },
        )));
        let task = cx.spawn(async move |_| {
            let _ = ignore.await;
        });
        lifecycle::keep_alive(cx, task).detach();
    }
}

/// 执行提交：有主窗口时经下载页（失败进横幅），否则直接走 agent 端口；完成后在主窗口
/// toast。界面退出前等提交送达（外部捕获拉起的界面没有主窗口，关表单即最后一个窗口）。
fn submit(submission: NewDownloadSubmission, port: &AgentDownloadsPort, cx: &mut App) {
    let main = Desktop::global(cx)
        .main_downloads
        .as_ref()
        .and_then(WeakEntity::upgrade);
    let created = match main {
        Some(downloads) => downloads.update(cx, |downloads, cx| {
            downloads.create_download(submission, cx)
        }),
        None => {
            let remember = submission
                .remember_save_dir_command()
                .map(|command| port.execute(command));
            let starts_immediately = submission.starts_immediately();
            let commands = submission
                .into_commands()
                .into_iter()
                .map(|command| port.execute(command))
                .collect::<Vec<_>>();
            cx.spawn(async move |cx| {
                if let Some(remember) = remember {
                    let _ = remember.await;
                }
                let mut ok = true;
                let mut created = Vec::new();
                for command in commands {
                    match command.await {
                        Ok(result) => created.extend(result.created_task_ids()),
                        Err(_) => ok = false,
                    }
                }
                // 与主窗口下载页同一规则：恰好一个任务立即开始才弹进度窗口。
                if let ([task_id], true) = (created.as_slice(), starts_immediately) {
                    let task_id = task_id.clone();
                    cx.update(|cx| crate::progress_windows::user_started(task_id, cx));
                }
                ok
            })
        }
    };
    let translator = Desktop::global(cx).translator.clone();
    let task = cx.spawn(async move |cx| {
        let ok = created.await;
        cx.update(|cx| {
            let Some(main) = WindowRegistry::handle(cx, &WindowKey::Main) else {
                return;
            };
            let translator = translator.read(cx);
            let notification = if ok {
                Notification::from(translator.text("taskCreatedToast").to_owned())
            } else {
                Notification::error(translator.text("localServiceActionFailed").to_owned())
            };
            let _ = main.update(cx, |_, window, cx| {
                window.push_notification(notification, cx)
            });
        });
    });
    lifecycle::keep_alive(cx, task).detach();
}
