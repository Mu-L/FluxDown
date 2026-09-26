//! `fluxdown-desktop` 的薄入口；应用装配集中在本 crate。
// 发布构建为 GUI 子系统：双击 / 由 agent 拉起时不弹控制台窗口。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod account_port;
mod actions;
mod agent_client;
mod app;
mod app_icon;
mod assets;
mod capability_ports;
mod downloads_port;
mod instance_ipc;
mod launch;
mod lifecycle;
mod menus;
mod power;
mod service_bootstrap;
mod session;
mod settings_port;
mod windows;

use std::process::ExitCode;

fn main() -> ExitCode {
    exit_code(app::run())
}

fn exit_code(result: Result<app::RunOutcome, app::AppError>) -> ExitCode {
    match result {
        Ok(app::RunOutcome::Completed) => ExitCode::SUCCESS,
        Ok(app::RunOutcome::NoPrimary) => ExitCode::from(3),
        Err(error) => {
            eprintln!("failed to start FluxDown desktop client: {error:#}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activate_existing_without_a_primary_uses_exit_code_three() {
        assert_eq!(exit_code(Ok(app::RunOutcome::NoPrimary)), ExitCode::from(3));
    }
}
