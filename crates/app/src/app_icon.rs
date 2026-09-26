//! 应用图标（Dock / 任务栏 / 窗口管理器）：三个平台的来源各不相同。
//!
//! - Windows：`build.rs` 经 winresource 把 `app_icon.ico` 嵌进资源 ID 1，GPUI 窗口、任务栏与资源
//!   管理器自动读取，运行期无事可做。
//! - macOS：`.app` 包由 Info.plist + icns 提供；直接运行裸二进制（`cargo run`）时 Dock 只有通用
//!   exec 图标，[`install`] 用 `NSDockTile` 内容视图补上。嵌入 `__info_plist` + 同目录 icns 的方案
//!   已实测无效（Dock / LaunchServices 对裸可执行文件不读 `CFBundleIconFile`）。
//! - Linux：X11 经 [`window_options`] 逐窗口写 `_NET_WM_ICON`；Wayland 没有窗口图标协议，只能靠
//!   `app_id` 匹配打包安装的 `com.fluxdown.app.desktop`。

use gpui::WindowOptions;

/// 与 Linux 桌面入口、Windows 资源 `InternalName`、macOS bundle id 一致的应用 ID。
pub const APP_ID: &str = "com.fluxdown.app";

/// 为窗口补上平台图标信息；所有窗口创建都必须经过这里（`WindowRegistry` 统一调用）。
pub fn window_options(mut options: WindowOptions) -> WindowOptions {
    if options.app_id.is_none() {
        options.app_id = Some(APP_ID.to_owned());
    }
    #[cfg(target_os = "linux")]
    if options.icon.is_none() {
        options.icon = linux::window_icon();
    }
    options
}

/// 进程级图标（macOS Dock）；其他平台空操作。须在 GPUI `App` 启动后、主线程调用。
pub fn install() {
    #[cfg(target_os = "macos")]
    macos::install_dock_icon();
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::{Arc, LazyLock};

    use image::RgbaImage;
    use image::imageops::FilterType;

    /// 与 Linux 打包安装到 hicolor 的 `com.fluxdown.app.png` 同源。
    const LOGO_PNG: &[u8] = include_bytes!("../../../assets/logo/fluxdown_logo.png");
    /// `_NET_WM_ICON` 按 u32 逐像素塞进窗口属性，256² 足够任务栏 / 切换器使用。
    const ICON_SIZE: u32 = 256;

    pub(super) fn window_icon() -> Option<Arc<RgbaImage>> {
        static ICON: LazyLock<Option<Arc<RgbaImage>>> = LazyLock::new(|| {
            let decoded = image::load_from_memory(LOGO_PNG).ok()?;
            let resized = decoded.resize(ICON_SIZE, ICON_SIZE, FilterType::Triangle);
            Some(Arc::new(resized.into_rgba8()))
        });
        ICON.clone()
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2::{AnyThread as _, MainThreadMarker, MainThreadOnly as _};
    use objc2_app_kit::{NSApplication, NSImage, NSImageScaling, NSImageView};
    use objc2_foundation::{NSData, NSPoint, NSRect};

    /// macOS 风格（圆角 + 留白）的 Dock 素材，与 Flutter 壳 `AppIcon.appiconset` 同源。
    const DOCK_ICON_PNG: &[u8] =
        include_bytes!("../../../macos/Runner/Assets.xcassets/AppIcon.appiconset/app_icon_512.png");

    /// 可执行文件位于 `*.app/Contents/MacOS/` 之内时，Dock 图标由 bundle 的 icns 提供。
    fn running_inside_bundle() -> bool {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.ends_with("Contents/MacOS")))
            .unwrap_or(false)
    }

    pub(super) fn install_dock_icon() {
        if running_inside_bundle() {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(image) =
            NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(DOCK_ICON_PNG))
        else {
            return;
        };
        let dock_tile = NSApplication::sharedApplication(mtm).dockTile();
        let frame = NSRect::new(NSPoint::ZERO, dock_tile.size());
        let view = NSImageView::initWithFrame(NSImageView::alloc(mtm), frame);
        view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        view.setImage(Some(&image));
        dock_tile.setContentView(Some(&view));
        dock_tile.display();
    }
}
