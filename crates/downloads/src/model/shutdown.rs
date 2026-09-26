//! 完成后关机：跨 `downloads` / `app` 边界的最小共享契约。
//!
//! 状态机与实际关机执行归 agent（`agent.power.*` / `PowerChanged`），app 侧 `power` 只做
//! 投影与请求转发；本文件只定义状态栏渲染与请求发起所需的类型，避免 `downloads` 依赖 `app`。

use std::{cell::Cell, rc::Rc, time::Duration};

/// 状态栏发出的关机请求。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownRequest {
    /// 取消已排定的关机（无论是等待任务完成还是正在倒计时）。
    Disarm,
    /// 全部任务完成后延迟 `Duration` 关机；`Duration::ZERO` = 完成后立即关机。
    ArmAfter(Duration),
}

/// 关机调度器的只读状态投影，供任意窗口的状态栏渲染。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShutdownStatus {
    /// 已排定的延迟；`None` = 未启用。
    pub armed_delay: Option<Duration>,
    /// 全部任务完成、真正开始倒计时后的剩余时间；等待任务完成阶段为 `None`。
    pub countdown_remaining: Option<Duration>,
}

/// 跨窗口共享的关机状态（app 从 agent 快照投影写入，任意窗口只读渲染）。
pub type SharedShutdownStatus = Rc<Cell<ShutdownStatus>>;

/// 状态栏发起关机请求的端口（由 app 在主窗口装配时注入，转成 `agent.power.*`）。
pub type ShutdownPort = Rc<dyn Fn(ShutdownRequest, &mut gpui::App)>;
