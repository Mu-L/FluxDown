//! 系统通知投递（下载完成）。三个平台「由谁来发这条通知」各不相同，不显式处理就会出错：
//!
//! - **macOS**：不用 `notify-rust`。它的 macOS 后端走已废弃的 `NSUserNotification` 并伪装
//!   成某个 bundle 投递：未指定时会用 AppleScript 查找名为 `use_default` 的应用（弹出
//!   「Where is use_default?」选择框）；指定了也会被新版 macOS 拒收——`usernoted` 对已接入
//!   `UNUserNotificationCenter` 的 bundle（Finder、Flutter 版 `com.fluxdown.app`）记录
//!   「Legacy client … connecting to modern client … Denying message」，通知静默丢失。
//!   agent 是裸二进制 / 与界面同处未签名包内，没有可用的现代通知身份，改由系统自带的
//!   `osascript` `display notification` 投递（由「脚本编辑器」身份显示，各版本 macOS 可靠送达）。
//! - **Windows**：WinRT toast 必须带 AppUserModelID；`notify-rust` 缺省借用 PowerShell 的
//!   AUMID，通知会显示成「Windows PowerShell」。在
//!   `HKCU\Software\Classes\AppUserModelId\<AUMID>` 登记 `DisplayName` + `IconUri`
//!   （未打包 Win32 应用发送本地 toast 的官方做法，不需要开始菜单快捷方式）。
//! - **Linux**：freedesktop 通知的 `app_name` 缺省是可执行文件名 `fluxdown-agent`。设置
//!   应用名、`desktop-entry` hint（GNOME / KDE 据此归组通知设置并取应用图标）与图标
//!   （数据目录下 PNG 的绝对路径：AppImage / 调试包没有安装主题图标时同样能显示）。
//!
//! 所有函数同步阻塞（子进程 / 注册表 / D-Bus / 文件写入），调用方放进 `spawn_blocking`。

use std::path::PathBuf;
#[cfg(not(target_os = "macos"))]
use std::{path::Path, sync::OnceLock};

/// 通知显示的应用名。
#[cfg(not(target_os = "macos"))]
const APP_NAME: &str = "FluxDown";

/// 通知图标（写入 agent 数据目录，供 Windows `IconUri` 与 Linux 通知图标使用）。
#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
const ICON_BYTES: &[u8] = include_bytes!("../../../assets/logo/fluxdown_logo.png");
#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
const ICON_FILE_NAME: &str = "notification_icon.png";

/// Windows toast 的 AppUserModelID。刻意不与 Flutter 版（`Com.FluxDown.App`，注册表键
/// 不区分大小写）共用：那个键上挂着 Flutter 的 COM 激活器，共用会让点击本通知去拉起
/// Flutter 应用。
#[cfg(windows)]
const WINDOWS_AUMID: &str = "dev.zerx.fluxdown";

/// Linux 打包安装的桌面入口 id（`linux/com.fluxdown.app.desktop`，不含后缀）。
#[cfg(all(unix, not(target_os = "macos")))]
const LINUX_DESKTOP_ENTRY: &str = "com.fluxdown.app";

/// 发送系统通知；平台应用身份在首次发送时一次性准备。
pub struct Notifier {
    #[cfg(not(target_os = "macos"))]
    data_dir: PathBuf,
    #[cfg(not(target_os = "macos"))]
    prepared: OnceLock<Prepared>,
}

/// 首次发送前准备好的平台身份。
#[cfg(not(target_os = "macos"))]
#[derive(Default)]
struct Prepared {
    /// 已落盘的通知图标（写入失败为 `None`，此时退回主题图标名）。
    #[cfg(all(unix, not(target_os = "macos")))]
    icon: Option<PathBuf>,
}

impl Notifier {
    /// `data_dir`：Windows / Linux 通知图标的落盘目录（macOS 经 osascript 投递，不用图标）。
    #[must_use]
    pub fn new(data_dir: PathBuf) -> Self {
        #[cfg(target_os = "macos")]
        let _ = data_dir;
        Self {
            #[cfg(not(target_os = "macos"))]
            data_dir,
            #[cfg(not(target_os = "macos"))]
            prepared: OnceLock::new(),
        }
    }

    /// 阻塞发送一条通知；失败只记日志（通知是尽力而为的旁路效果）。
    #[cfg(not(target_os = "macos"))]
    pub fn show(&self, title: &str, body: &str) {
        let prepared = self.prepared.get_or_init(|| prepare(&self.data_dir));
        let mut notification = notify_rust::Notification::new();
        notification.appname(APP_NAME).summary(title).body(body);
        apply_platform_identity(&mut notification, prepared);
        if let Err(error) = notification.show() {
            tracing::warn!(error = %error, "could not show system notification");
        }
    }

    /// 阻塞发送一条通知；失败只记日志（通知是尽力而为的旁路效果）。
    #[cfg(target_os = "macos")]
    pub fn show(&self, title: &str, body: &str) {
        match std::process::Command::new(OSASCRIPT)
            .args(osascript_notification_args(title, body))
            .stdin(std::process::Stdio::null())
            .output()
        {
            Ok(output) if output.status.success() => {}
            Ok(output) => tracing::warn!(
                status = %output.status,
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "osascript notification failed"
            ),
            Err(error) => tracing::warn!(error = %error, "could not spawn osascript"),
        }
    }
}

#[cfg(target_os = "macos")]
const OSASCRIPT: &str = "/usr/bin/osascript";

/// `osascript` 参数：标题 / 正文经 `argv` 传入，不拼进脚本源码（文件名里的引号、反斜杠
/// 不会破坏脚本，也无从注入）。标题在前且非空，osascript 在它之后停止解析选项，正文以
/// `-` 开头也安全。
#[cfg(any(target_os = "macos", test))]
fn osascript_notification_args<'a>(title: &'a str, body: &'a str) -> [&'a str; 8] {
    [
        "-e",
        "on run argv",
        "-e",
        "display notification (item 2 of argv) with title (item 1 of argv)",
        "-e",
        "end run",
        title,
        body,
    ]
}

#[cfg(windows)]
fn prepare(data_dir: &Path) -> Prepared {
    let icon = write_icon(data_dir);
    if let Err(error) = register_windows_aumid(icon.as_deref()) {
        tracing::warn!(error = %error, "could not register notification AppUserModelID");
    }
    Prepared::default()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn prepare(data_dir: &Path) -> Prepared {
    Prepared {
        icon: write_icon(data_dir),
    }
}

#[cfg(windows)]
fn apply_platform_identity(notification: &mut notify_rust::Notification, _prepared: &Prepared) {
    // 图标由注册表 `IconUri` 提供；toast 正文不再附图。
    notification.app_id(WINDOWS_AUMID);
}

#[cfg(all(unix, not(target_os = "macos")))]
fn apply_platform_identity(notification: &mut notify_rust::Notification, prepared: &Prepared) {
    let icon = prepared.icon.as_deref().map_or_else(
        || LINUX_DESKTOP_ENTRY.to_owned(),
        |path| path.display().to_string(),
    );
    notification
        .icon(&icon)
        .hint(notify_rust::Hint::DesktopEntry(
            LINUX_DESKTOP_ENTRY.to_owned(),
        ));
}

/// 把内嵌图标写到数据目录（内容相同则不重写）；失败返回 `None`。
#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
fn write_icon(data_dir: &Path) -> Option<PathBuf> {
    let path = data_dir.join(ICON_FILE_NAME);
    if std::fs::read(&path).is_ok_and(|existing| existing == ICON_BYTES) {
        return Some(path);
    }
    match std::fs::create_dir_all(data_dir).and_then(|()| std::fs::write(&path, ICON_BYTES)) {
        Ok(()) => Some(path),
        Err(error) => {
            tracing::warn!(path = %path.display(), error = %error, "could not write notification icon");
            None
        }
    }
}

/// 登记 toast 的显示名与图标（每次启动覆盖一次：图标路径随数据目录变化而更新）。
#[cfg(windows)]
fn register_windows_aumid(icon: Option<&Path>) -> std::io::Result<()> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_WRITE};

    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey_with_flags(
        format!("Software\\Classes\\AppUserModelId\\{WINDOWS_AUMID}"),
        KEY_WRITE,
    )?;
    key.set_value("DisplayName", &APP_NAME)?;
    if let Some(icon) = icon {
        key.set_value("IconUri", &icon.display().to_string())?;
    }
    Ok(())
}

/// 下载完成通知文案（与 Flutter `NotificationService._showSystemBatch` 同规则）：单个任务
/// 标题「下载完成」、正文文件名；多个任务标题「N 个任务下载完成」、正文「最后一个文件名
/// 等 N-1 个文件」。`text(key, count)` 按 en / zh 基线键取文案并替换 `{count}`。
pub fn completion_text(
    file_names: &[String],
    text: impl Fn(&str, Option<usize>) -> String,
) -> Option<(String, String)> {
    let last = file_names.last()?;
    if file_names.len() == 1 {
        return Some((text("downloadCompleted", None), last.clone()));
    }
    let count = file_names.len();
    Some((
        text("batchDownloadCompleted", Some(count)),
        format!("{last} {}", text("andMoreFiles", Some(count - 1))),
    ))
}

/// 英文基线文案（headless 构建没有文案目录 / 目录加载失败时）；与 `assets/i18n/en.json` 同文。
#[must_use]
pub fn english_text(key: &str, count: Option<usize>) -> String {
    let template = match key {
        "downloadCompleted" => "Download Complete",
        "batchDownloadCompleted" => "{count} Downloads Complete",
        "andMoreFiles" => "and {count} more",
        other => other,
    };
    count.map_or_else(
        || template.to_owned(),
        |count| template.replace("{count}", &count.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::{completion_text, english_text as english};

    #[test]
    fn single_and_batch_completion_text_match_flutter_rules() {
        assert_eq!(completion_text(&[], english), None);
        assert_eq!(
            completion_text(&["a.bin".to_owned()], english),
            Some(("Download Complete".to_owned(), "a.bin".to_owned()))
        );
        assert_eq!(
            completion_text(
                &["a.bin".to_owned(), "b.bin".to_owned(), "c.bin".to_owned()],
                english
            ),
            Some((
                "3 Downloads Complete".to_owned(),
                "c.bin and 2 more".to_owned()
            ))
        );
    }
}
