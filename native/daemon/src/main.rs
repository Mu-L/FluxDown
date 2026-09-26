//! `fluxdownd` 常驻下载核心进程。
// 发布构建为 GUI 子系统：由 agent 在后台拉起，任何路径下都不弹控制台窗口。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cancel = CancellationToken::new();
    let signal_cancel = cancel.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        signal_cancel.cancel();
    });
    fluxdown_daemon::runtime::run(cancel).await
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let Ok(mut terminate) = signal(SignalKind::terminate()) else {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
        return;
    };
    tokio::select! {
        result = tokio::signal::ctrl_c() => {
            if result.is_err() {
                terminate.recv().await;
            }
        }
        _ = terminate.recv() => {}
    }
}

/// 没有控制台时 Ctrl-C 处理器可能注册失败：失败即永不触发，而不是立刻退出。
/// 正常关停走 agent 的 `system.shutdown`。
#[cfg(not(unix))]
async fn shutdown_signal() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}
