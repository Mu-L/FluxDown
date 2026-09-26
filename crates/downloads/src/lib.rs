//! GPUI 下载能力页面、领域组件与视图状态。
//!
//! 本 crate 不负责窗口导航或应用初始化；宿主在 composition root 中创建
//! [`DownloadView`] 并作为路由内容注入 shell。

pub mod actions;
mod components;
mod controller;
mod model;
mod pages;
mod strings;

pub use components::title_bar::DownloadTitleBar;
pub use controller::{
    DownloadsCommand, DownloadsController, DownloadsPort, DownloadsResult, LAST_SAVE_DIR_PREF,
    PortFuture, QueueFields, REMEMBER_LAST_SAVE_DIR_PREF, SeedLimits,
};
pub use model::shutdown::*;
pub use pages::downloads::*;
pub use pages::group_detail::*;
pub use pages::new_download::*;
pub use pages::queue_manager::*;
pub use pages::selection::*;
pub use pages::task_detail::*;
