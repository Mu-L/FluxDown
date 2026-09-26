//! 桌面进程的两种退出，与 agent 驻留策略（`AgentSnapshot.shell.resident`）配合：
//!
//! - **只退出界面**：托盘驻留时关闭主窗口 / 关闭最后一个窗口。agent + daemon 继续运行，
//!   托盘由 agent 承载，点击托盘再拉起本进程。
//! - **完全退出**：菜单「退出」、⌘Q，或非驻留模式下关闭主窗口。先请求 agent
//!   `system.shutdown`（agent 先关停 daemon 再退出），再退出本进程。
//!
//! agent 以 `service-quit` 关闭连接（例如托盘「退出」）时本进程同样立即退出且不再重拉 agent。

use std::time::Duration;

use fluxdown_protocol::method;
use gpui::{App, Global, Task};
use serde_json::Value;

use crate::{app::Desktop, windows::WindowRegistry};

/// 关窗后仍须送达 agent 的请求（新建下载提交、外部捕获确认 / 忽略）数量。未完成前
/// 「只退出界面」推迟到它们结束，避免最后一个窗口关闭后进程先退出、请求丢失。
#[derive(Default)]
struct InFlight {
    count: usize,
    quit_pending: bool,
}

impl Global for InFlight {}

/// 登记一个关窗后仍须完成的请求；返回的任务完成时解除登记（期间推迟的退出随之执行）。
pub fn keep_alive<R: 'static>(cx: &mut App, task: Task<R>) -> Task<R> {
    cx.default_global::<InFlight>().count += 1;
    cx.spawn(async move |cx| {
        let output = task.await;
        cx.update(|cx| {
            let in_flight = cx.default_global::<InFlight>();
            in_flight.count = in_flight.count.saturating_sub(1);
            let quit = in_flight.count == 0 && std::mem::take(&mut in_flight.quit_pending);
            if quit && WindowRegistry::open_count(cx) == 0 {
                quit_ui(cx);
            }
        });
        output
    })
}

/// 等 agent 受理 `system.shutdown` 的上限；agent 未连接时不能让界面卡在退出中。
const SHUTDOWN_REQUEST_BUDGET: Duration = Duration::from_secs(3);

/// 完全退出：停止后台（agent + daemon）后退出界面；重复调用无副作用。
pub fn quit_everything(cx: &mut App) {
    let desktop = Desktop::global_mut(cx);
    if desktop.quitting {
        return;
    }
    desktop.quitting = true;
    let client = desktop.client.clone();
    client.stop_service_bootstrap();
    let request = client.call::<Value, Value>(method::SYSTEM_SHUTDOWN, None);
    let timer = cx.background_executor().timer(SHUTDOWN_REQUEST_BUDGET);
    cx.spawn(async move |cx| {
        let _ = futures_util::future::select(request, timer).await;
        cx.update(|cx| cx.quit());
    })
    .detach();
}

/// 只退出界面（后台驻留）。延后一轮执行：同一轮里随窗口释放的视图（例如关窗时忽略
/// 外部捕获）先经 [`keep_alive`] 登记在途请求，登记了就等它们结束再退出。
pub fn quit_ui(cx: &mut App) {
    cx.defer(|cx| {
        let in_flight = cx.default_global::<InFlight>();
        if in_flight.count > 0 {
            in_flight.quit_pending = true;
            return;
        }
        let desktop = Desktop::global_mut(cx);
        if desktop.quitting {
            return;
        }
        desktop.quitting = true;
        cx.quit();
    });
}

/// agent 已完全退出：界面随之退出，且不再拉起 agent。
pub fn service_stopped(cx: &mut App) {
    let desktop = Desktop::global_mut(cx);
    desktop.client.stop_service_bootstrap();
    desktop.quitting = true;
    cx.quit();
}

/// 退出流程外的应用退出（macOS Dock「退出」、系统注销等）：非驻留模式下尽力通知 agent
/// 一起退出。GPUI 只给 `on_app_quit` 很短的收尾时间，这里只发请求不保证送达；送达失败时
/// 由 agent 的闲置检测兜底。
pub fn shutdown_request_on_app_quit(
    cx: &mut App,
) -> Option<crate::agent_client::AgentFuture<Value>> {
    let known = Desktop::global(cx).session.read(cx).latest().is_some();
    let desktop = Desktop::global_mut(cx);
    if desktop.quitting || desktop.shell.resident || !known {
        return None;
    }
    desktop.quitting = true;
    desktop.client.stop_service_bootstrap();
    Some(
        desktop
            .client
            .call::<Value, Value>(method::SYSTEM_SHUTDOWN, None),
    )
}
