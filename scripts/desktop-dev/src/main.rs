use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus};

const BUILD: &[&str] = &[
    "build",
    "-p",
    "fluxdown_ui_app",
    "-p",
    "fluxdown_agent",
    "-p",
    "fluxdown_daemon",
    // 官方桌面包的 agent 承载托盘与剪贴板监听；不带该 feature 时是 headless agent。
    "--features",
    "fluxdown_agent/desktop",
    "--bins",
];
const PROBE_MARKER: &str = ".desktop-dev-activation-v1";

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("desktop-dev: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> io::Result<u8> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let build_only = match args.as_slice() {
        [] => false,
        [arg] if arg == "--build-only" => true,
        [arg] if arg == "--help" || arg == "-h" => {
            println!(
                "Usage: cargo desktop-dev [--build-only]\n\
                Activate the running desktop, or build UI + agent + daemon and start it.\n\
                --build-only builds without launching or activating anything.\n\
                Existing services are reused, never forcibly stopped. Quit the UI and\n\
                stop the services explicitly before testing changes to running code."
            );
            return Ok(0);
        }
        _ => {
            eprintln!("Usage: cargo desktop-dev [--build-only]");
            return Ok(2);
        }
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    // The launcher is built by the same workspace/config/profile as its siblings.
    // This also respects CARGO_TARGET_DIR without parsing Cargo's configuration.
    let executable = std::env::current_exe()?;
    let output = executable
        .parent()
        .ok_or_else(|| io::Error::other("launcher has no parent directory"))?;
    let desktop = output.join(format!("fluxdown-desktop{}", std::env::consts::EXE_SUFFIX));
    let marker = output.join(PROBE_MARKER);
    let guard = startup_lock(output)?;
    // Older desktop binaries ignore unknown flags. Only probe binaries built by
    // this launcher, which implement the activation-only exit-code contract.
    if !build_only && probe_is_supported(&desktop, &marker) && activate_existing(&desktop)? {
        eprintln!("desktop-dev: activated the running UI; reused existing services (no rebuild).");
        return Ok(0);
    }
    eprintln!("desktop-dev: building UI, agent and daemon...");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut build = Command::new(cargo);
    build.current_dir(&root).args(BUILD);
    no_console_window(&mut build);
    let status = build.status()?;
    if !status.success() {
        eprintln!(
            "desktop-dev: build failed; no UI launched. On Windows, exit running binaries before rebuilding them."
        );
        return Ok(exit_code(status));
    }
    fs::write(&marker, binary_stamp(&desktop)?)?;
    if build_only {
        return Ok(0);
    }
    if activate_existing(&desktop)? {
        eprintln!("desktop-dev: activated the running UI; running code was not replaced.");
        return Ok(0);
    }
    eprintln!("desktop-dev: starting UI; existing agent/daemon will be reused, not restarted.");
    let mut launch = Command::new(&desktop);
    launch.current_dir(root);
    no_console_window(&mut launch);
    let mut child = launch.spawn()?;
    // Do not retain the development lock for the lifetime of the UI: a second
    // invocation must be able to activate it. The UI owns its own instance lock.
    drop(guard);
    child.wait().map(exit_code)
}

fn startup_lock(output: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(output.join(".desktop-dev-startup.lock"))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            eprintln!("desktop-dev: waiting for another development build/startup...");
            file.lock()?;
        }
        Err(std::fs::TryLockError::Error(error)) => return Err(error),
    }
    Ok(file)
}

fn binary_stamp(path: &Path) -> io::Result<String> {
    let metadata = fs::metadata(path)?;
    let modified = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?;
    Ok(format!("{}:{}", metadata.len(), modified.as_nanos()))
}

fn probe_is_supported(desktop: &Path, marker: &Path) -> bool {
    match (binary_stamp(desktop), fs::read_to_string(marker)) {
        (Ok(current), Ok(built)) => current == built,
        _ => false,
    }
}

fn activate_existing(desktop: &PathBuf) -> io::Result<bool> {
    if !desktop.is_file() {
        return Ok(false);
    }
    let mut command = Command::new(desktop);
    command.arg("--activate-existing");
    no_console_window(&mut command);
    match command.status()?.code() {
        Some(0) => Ok(true),
        Some(3) => Ok(false),
        code => Err(io::Error::other(format!(
            "existing UI activation failed ({code:?}); refusing to start another instance"
        ))),
    }
}

fn exit_code(status: ExitStatus) -> u8 {
    status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1)
}

#[cfg(windows)]
fn no_console_window(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;
    command.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn no_console_window(_command: &mut Command) {}
