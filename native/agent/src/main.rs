//! `fluxdown-agent` 官方客户端常驻后端。
// 发布构建由开机自启直接拉起：GUI 子系统，登录时不弹控制台窗口。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> fluxdown_agent::runtime::AgentResult {
    let autostart = std::env::args()
        .skip(1)
        .any(|arg| arg == fluxdown_agent::platform::AUTOSTART_ARG);
    #[cfg(feature = "desktop")]
    {
        fluxdown_agent::shell::host::run(autostart, fluxdown_agent::runtime::run_blocking)
    }
    #[cfg(not(feature = "desktop"))]
    {
        fluxdown_agent::runtime::run_blocking(fluxdown_agent::shell::ShellHost::without_tray(
            fluxdown_protocol::TrayUnavailableReason::NotBuilt,
            autostart,
        ))
    }
}
