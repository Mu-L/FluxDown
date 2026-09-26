//! agent 对同级 `fluxdownd` 的单飞启动与异步回收。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Mutex;

/// daemon 启动错误。
#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("could not locate sibling fluxdownd: {0}")]
    Locate(#[from] std::io::Error),
    #[error("failed to spawn fluxdownd: {0}")]
    Spawn(String),
}

#[derive(Default)]
struct SupervisorState {
    generation: u64,
    running: bool,
    reapers: Vec<tokio::task::JoinHandle<()>>,
}

/// 只在连接拒绝路径调用的 daemon 单飞启动器。
pub struct DaemonSupervisor {
    state: Arc<Mutex<SupervisorState>>,
    bind_addr: SocketAddr,
    /// 完全退出流程中置位：此后连接拒绝不再拉起 daemon。
    stopped: AtomicBool,
}

impl DaemonSupervisor {
    #[must_use]
    pub fn new(bind_addr: SocketAddr) -> Self {
        Self {
            state: Arc::new(Mutex::new(SupervisorState::default())),
            bind_addr,
            stopped: AtomicBool::new(false),
        }
    }

    /// 永久停止监管（不可恢复）：随后的 [`Self::ensure_running`] 均为空操作。
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }

    /// 启动同级 daemon；短时间内并发/重复调用只产生一个子进程。
    ///
    /// 返回本进程所监管、仍存活的 daemon 子进程代际（刚拉起或早先拉起）；已停止监管时为
    /// `None`。代际让调用方区分「同一个子进程仍在初始化」与「子进程已退出又被重新拉起」。
    pub async fn ensure_running(&self) -> Result<Option<u64>, SupervisorError> {
        let mut state = self.state.lock().await;
        state.reapers.retain(|task| !task.is_finished());
        if self.stopped.load(Ordering::Acquire) {
            return Ok(None);
        }
        if state.running {
            return Ok(Some(state.generation));
        }
        let executable = daemon_executable()?;
        let mut command = std::process::Command::new(&executable);
        command
            .env("FLUXDOWN_DAEMON_BIND", self.bind_addr.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        detach_background_process(&mut command);
        let mut child = tokio::process::Command::from(command)
            .spawn()
            .map_err(|error| SupervisorError::Spawn(format!("{error:#}")))?;
        state.generation = state.generation.saturating_add(1);
        let generation = state.generation;
        state.running = true;
        let supervisor_state = self.state.clone();
        state.reapers.push(tokio::spawn(async move {
            match child.wait().await {
                Ok(status) => tracing::info!(%status, "supervised fluxdownd exited"),
                Err(error) => tracing::warn!(error = %error, "failed to reap fluxdownd"),
            }
            let mut state = supervisor_state.lock().await;
            if state.generation == generation {
                state.running = false;
            }
        }));
        Ok(Some(generation))
    }
}

fn daemon_executable() -> Result<PathBuf, std::io::Error> {
    if let Some(path) = std::env::var_os("FLUXDOWN_DAEMON_BIN") {
        return Ok(PathBuf::from(path));
    }
    let current = std::env::current_exe()?;
    let name = if cfg!(windows) {
        "fluxdownd.exe"
    } else {
        "fluxdownd"
    };
    Ok(current.with_file_name(name))
}

/// 后台服务与拉起者解耦：Windows 不弹控制台窗；Unix 进入独立进程组，终端里对拉起者的
/// Ctrl-C（SIGINT 发给前台进程组）不会连带终止常驻 daemon。
#[cfg(windows)]
fn detach_background_process(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000);
}

#[cfg(unix)]
fn detach_background_process(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(any(windows, unix)))]
fn detach_background_process(_command: &mut std::process::Command) {}
