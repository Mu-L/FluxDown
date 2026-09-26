//! 托盘宿主：主线程运行 tao 事件循环并持有托盘图标，agent runtime 在工作线程运行。
//!
//! - macOS 的状态栏图标必须在主线程、随 NSApp 事件循环运行；Windows / Linux 同样放主线程，
//!   三个平台只有一套线程模型。进程以 Accessory 策略运行（无 Dock 图标）。
//! - 托盘菜单 / 点击事件直接经 [`TrayAction`] 通道交给 runtime；runtime 经 [`TrayPort`]
//!   把展示状态投递回事件循环。
//! - 系统注销 / 关机（Windows `WM_ENDSESSION`、macOS `applicationWillTerminate`）时 tao 发出
//!   `LoopDestroyed` 后随即结束进程：在这里同步执行有时限的完全退出。
//! - Linux 需要图形会话与 StatusNotifier 宿主；缺失时不建托盘，后台随界面退出。

use std::panic::AssertUnwindSafe;
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;

use fluxdown_protocol::TrayUnavailableReason;
use tao::event::{Event, StartCause};
use tao::event_loop::{ControlFlow, EventLoop, EventLoopBuilder, EventLoopProxy};
use tao::platform::run_return::EventLoopExtRunReturn;
use tokio::sync::mpsc;
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use super::{ShellHost, TrayAction, TrayAvailability, TrayModel, TrayPort};
use crate::runtime::AgentResult;

/// 系统注销时等待完全退出（daemon 优雅关停）的上限；Windows 约 5s 后强制结束进程。
const SESSION_END_BUDGET: Duration = Duration::from_millis(4500);
/// 菜单位置：显示窗口、分隔线、全部暂停、全部恢复之后插入「取消完成后关机」。
const CANCEL_SHUTDOWN_POSITION: usize = 4;
const DEFAULT_TOOLTIP: &str = "FluxDown";

#[cfg(target_os = "macos")]
const ICON_BYTES: &[u8] = include_bytes!("../../../../assets/logo/tray_iconTemplate.png");
#[cfg(target_os = "linux")]
const ICON_BYTES: &[u8] = include_bytes!("../../../../assets/logo/fluxdown_logo.png");
/// Windows 没有模板图标机制：深色任务栏用浅色图标，浅色任务栏用原色图标。
#[cfg(windows)]
const DARK_TASKBAR_ICON_BYTES: &[u8] = include_bytes!("../../../../assets/logo/logo_on_dark.png");
#[cfg(windows)]
const LIGHT_TASKBAR_ICON_BYTES: &[u8] = include_bytes!("../../../../assets/logo/fluxdown_logo.png");
#[cfg(windows)]
const THEME_POLL_INTERVAL: Duration = Duration::from_secs(5);

enum HostEvent {
    Apply(TrayModel),
    RuntimeFinished,
}

struct ProxyPort(Mutex<EventLoopProxy<HostEvent>>);

impl TrayPort for ProxyPort {
    fn apply(&self, model: &TrayModel) {
        if let Ok(proxy) = self.0.lock() {
            let _ = proxy.send_event(HostEvent::Apply(model.clone()));
        }
    }
}

/// 在主线程运行托盘宿主；`run_runtime` 在工作线程阻塞运行 agent 并返回其结果。
pub fn run<F>(autostart: bool, run_runtime: F) -> AgentResult
where
    F: FnOnce(ShellHost) -> AgentResult + Send + 'static,
{
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return run_runtime(ShellHost::without_tray(
            TrayUnavailableReason::NoDisplay,
            autostart,
        ));
    }
    // Linux 下 GTK 初始化失败时 tao 直接 panic；降级为无托盘运行而不是让 agent 起不来。
    let built =
        std::panic::catch_unwind(|| EventLoopBuilder::<HostEvent>::with_user_event().build());
    let Ok(mut event_loop) = built else {
        tracing::warn!("tray event loop unavailable; running without tray");
        return run_runtime(ShellHost::without_tray(
            TrayUnavailableReason::InitFailed,
            autostart,
        ));
    };
    configure_platform(&mut event_loop);

    let proxy = event_loop.create_proxy();
    let (actions_tx, actions_rx) = mpsc::unbounded_channel();
    let mut pending = Some((run_runtime, actions_rx));
    let mut ui: Option<TrayUi> = None;
    let mut runtime: Option<std::thread::JoinHandle<AgentResult>> = None;
    let mut done_rx: Option<std_mpsc::Receiver<()>> = None;
    let mut finished = false;
    let mut spawn_error: Option<std::io::Error> = None;
    #[cfg(windows)]
    let mut theme_deadline = Instant::now() + THEME_POLL_INTERVAL;

    event_loop.run_return(|event, _, control_flow| {
        if !matches!(
            *control_flow,
            ControlFlow::Exit | ControlFlow::ExitWithCode(_)
        ) {
            #[cfg(windows)]
            {
                *control_flow = ControlFlow::WaitUntil(theme_deadline);
            }
            #[cfg(not(windows))]
            {
                *control_flow = ControlFlow::Wait;
            }
        }
        match event {
            Event::NewEvents(StartCause::Init) => {
                let Some((run_runtime, actions_rx)) = pending.take() else {
                    return;
                };
                let availability = match TrayUi::create(&actions_tx) {
                    Ok(created) => {
                        ui = Some(created);
                        TrayAvailability::Available
                    }
                    Err(reason) => TrayAvailability::Unavailable(reason),
                };
                let port = (availability == TrayAvailability::Available)
                    .then(|| Arc::new(ProxyPort(Mutex::new(proxy.clone()))) as Arc<dyn TrayPort>);
                let host = ShellHost {
                    availability,
                    port,
                    actions: Some(actions_rx),
                    autostart,
                };
                let (done_tx, rx) = std_mpsc::channel();
                let finished_proxy = proxy.clone();
                let spawned = std::thread::Builder::new()
                    .name("fluxdown-agent-runtime".to_owned())
                    .spawn(move || {
                        let result = run_runtime(host);
                        let _ = done_tx.send(());
                        let _ = finished_proxy.send_event(HostEvent::RuntimeFinished);
                        result
                    });
                match spawned {
                    Ok(handle) => {
                        runtime = Some(handle);
                        done_rx = Some(rx);
                    }
                    Err(error) => {
                        spawn_error = Some(error);
                        finished = true;
                        ui = None;
                        *control_flow = ControlFlow::Exit;
                    }
                }
            }
            #[cfg(windows)]
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => {
                theme_deadline = Instant::now() + THEME_POLL_INTERVAL;
                *control_flow = ControlFlow::WaitUntil(theme_deadline);
                if let Some(ui) = ui.as_mut() {
                    ui.sync_taskbar_theme();
                }
            }
            Event::UserEvent(HostEvent::Apply(model)) => {
                if let Some(ui) = ui.as_mut() {
                    ui.apply(&model);
                }
            }
            Event::UserEvent(HostEvent::RuntimeFinished) => {
                finished = true;
                // 先销毁托盘图标（Windows 需要 NIM_DELETE，否则任务栏残留幽灵图标）再结束循环。
                ui = None;
                *control_flow = ControlFlow::Exit;
            }
            Event::Opened { urls } => {
                let urls = urls.iter().map(ToString::to_string).collect::<Vec<_>>();
                let _ = actions_tx.send(TrayAction::OpenUrls(urls));
            }
            Event::Reopen { .. } => {
                let _ = actions_tx.send(TrayAction::ShowWindow);
            }
            Event::LoopDestroyed => {
                if !finished {
                    // 系统结束会话：tao 在本回调返回后即结束进程，这里同步等完全退出。
                    let _ = actions_tx.send(TrayAction::SessionEnd);
                    if let Some(rx) = done_rx.as_ref() {
                        let _ = rx.recv_timeout(SESSION_END_BUDGET);
                    }
                }
                ui = None;
            }
            _ => {}
        }
    });

    if let Some(error) = spawn_error {
        return Err(Box::new(error));
    }
    match runtime.map(std::thread::JoinHandle::join) {
        Some(Ok(result)) => result,
        Some(Err(_)) => Err("fluxdown-agent runtime thread panicked".into()),
        None => Ok(()),
    }
}

#[cfg(target_os = "macos")]
fn configure_platform(event_loop: &mut EventLoop<HostEvent>) {
    use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
}

#[cfg(not(target_os = "macos"))]
fn configure_platform(_event_loop: &mut EventLoop<HostEvent>) {}

#[derive(Clone)]
struct MenuIds {
    show: MenuId,
    pause: MenuId,
    resume: MenuId,
    cancel_shutdown: MenuId,
    quit: MenuId,
}

impl MenuIds {
    fn action(&self, id: &MenuId) -> Option<TrayAction> {
        if *id == self.show {
            Some(TrayAction::ShowWindow)
        } else if *id == self.pause {
            Some(TrayAction::PauseAll)
        } else if *id == self.resume {
            Some(TrayAction::ResumeAll)
        } else if *id == self.cancel_shutdown {
            Some(TrayAction::CancelShutdown)
        } else if *id == self.quit {
            Some(TrayAction::Quit)
        } else {
            None
        }
    }
}

struct TrayUi {
    icon: TrayIcon,
    menu: Menu,
    show: MenuItem,
    pause: MenuItem,
    resume: MenuItem,
    cancel_shutdown: MenuItem,
    cancel_inserted: bool,
    quit: MenuItem,
    applied: Option<TrayModel>,
    #[cfg(windows)]
    dark_taskbar: bool,
}

impl TrayUi {
    fn create(actions: &mpsc::UnboundedSender<TrayAction>) -> Result<Self, TrayUnavailableReason> {
        #[cfg(target_os = "linux")]
        if !linux::status_notifier_host_present() {
            tracing::info!("no StatusNotifier host on the session bus; tray disabled");
            return Err(TrayUnavailableReason::NoHost);
        }
        // Linux 缺少 (ayatana-)appindicator 运行库时 tray-icon 直接 panic。
        let created = std::panic::catch_unwind(AssertUnwindSafe(Self::build));
        let ui = match created {
            Ok(Ok(ui)) => ui,
            Ok(Err(error)) => {
                tracing::warn!(error = %error, "could not create tray icon");
                return Err(TrayUnavailableReason::InitFailed);
            }
            Err(_) => {
                tracing::warn!("tray icon backend panicked during initialisation");
                return Err(TrayUnavailableReason::InitFailed);
            }
        };
        let ids = MenuIds {
            show: ui.show.id().clone(),
            pause: ui.pause.id().clone(),
            resume: ui.resume.id().clone(),
            cancel_shutdown: ui.cancel_shutdown.id().clone(),
            quit: ui.quit.id().clone(),
        };
        let menu_actions = actions.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(action) = ids.action(&event.id) {
                let _ = menu_actions.send(action);
            }
        }));
        let click_actions = actions.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = click_actions.send(TrayAction::ShowWindow);
            }
        }));
        Ok(ui)
    }

    fn build() -> Result<Self, String> {
        let show = MenuItem::new("Show Window", true, None);
        let pause = MenuItem::new("Pause All", true, None);
        let resume = MenuItem::new("Resume All", true, None);
        let cancel_shutdown = MenuItem::new("Cancel Shutdown", true, None);
        let quit = MenuItem::new("Exit", true, None);
        let menu = Menu::with_items(&[
            &show,
            &PredefinedMenuItem::separator(),
            &pause,
            &resume,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .map_err(|error| error.to_string())?;
        #[cfg(windows)]
        let dark_taskbar = windows_theme::taskbar_is_dark();
        #[cfg(windows)]
        let bytes = if dark_taskbar {
            DARK_TASKBAR_ICON_BYTES
        } else {
            LIGHT_TASKBAR_ICON_BYTES
        };
        #[cfg(not(windows))]
        let bytes = ICON_BYTES;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu.clone()))
            .with_tooltip(DEFAULT_TOOLTIP)
            .with_menu_on_left_click(false)
            .with_icon(decode_icon(bytes)?)
            .with_icon_as_template(cfg!(target_os = "macos"))
            .build()
            .map_err(|error| error.to_string())?;
        // 首个展示状态到达前保持隐藏：偏好关闭托盘时不应闪现。
        icon.set_visible(false).map_err(|error| error.to_string())?;
        Ok(Self {
            icon,
            menu,
            show,
            pause,
            resume,
            cancel_shutdown,
            cancel_inserted: false,
            quit,
            applied: None,
            #[cfg(windows)]
            dark_taskbar,
        })
    }

    fn apply(&mut self, model: &TrayModel) {
        if self.applied.as_ref() == Some(model) {
            return;
        }
        self.show.set_text(&model.show_window);
        self.pause.set_text(&model.pause_all);
        self.resume.set_text(&model.resume_all);
        self.quit.set_text(&model.quit);
        match (&model.cancel_shutdown, self.cancel_inserted) {
            (Some(label), inserted) => {
                self.cancel_shutdown.set_text(label);
                if !inserted {
                    match self
                        .menu
                        .insert(&self.cancel_shutdown, CANCEL_SHUTDOWN_POSITION)
                    {
                        Ok(()) => self.cancel_inserted = true,
                        Err(error) => tracing::warn!(error = %error, "tray menu insert failed"),
                    }
                }
            }
            (None, true) => match self.menu.remove(&self.cancel_shutdown) {
                Ok(()) => self.cancel_inserted = false,
                Err(error) => tracing::warn!(error = %error, "tray menu remove failed"),
            },
            (None, false) => {}
        }
        if let Err(error) = self.icon.set_tooltip(Some(&model.tooltip)) {
            tracing::debug!(error = %error, "tray tooltip unsupported");
        }
        if let Err(error) = self.icon.set_visible(model.visible) {
            tracing::warn!(error = %error, "could not change tray visibility");
        }
        self.applied = Some(model.clone());
    }

    #[cfg(windows)]
    fn sync_taskbar_theme(&mut self) {
        let dark = windows_theme::taskbar_is_dark();
        if dark == self.dark_taskbar {
            return;
        }
        let bytes = if dark {
            DARK_TASKBAR_ICON_BYTES
        } else {
            LIGHT_TASKBAR_ICON_BYTES
        };
        match decode_icon(bytes) {
            Ok(icon) => {
                if self.icon.set_icon(Some(icon)).is_ok() {
                    self.dark_taskbar = dark;
                }
            }
            Err(error) => tracing::warn!(%error, "could not decode tray icon"),
        }
    }
}

fn decode_icon(bytes: &[u8]) -> Result<Icon, String> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|error| error.to_string())?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).map_err(|error| error.to_string())
}

#[cfg(windows)]
mod windows_theme {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};

    const PERSONALIZE_KEY: &str =
        "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";

    /// 任务栏（系统界面）是否深色；读不到时按 Windows 默认深色任务栏处理。
    pub fn taskbar_is_dark() -> bool {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(PERSONALIZE_KEY, KEY_READ)
            .and_then(|key| key.get_value::<u32, _>("SystemUsesLightTheme"))
            .map_or(true, |light| light == 0)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use gtk::gio;
    use gtk::glib;
    use gtk::glib::prelude::ToVariant;

    /// 会话总线上是否有 StatusNotifier 宿主（KDE、带 AppIndicator 扩展的 GNOME 等）；
    /// 没有时 appindicator 图标不可见，关闭窗口会让应用「消失」。
    pub fn status_notifier_host_present() -> bool {
        let Ok(connection) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        else {
            return false;
        };
        let Ok(reply_type) = glib::VariantTy::new("(b)") else {
            return false;
        };
        connection
            .call_sync(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "NameHasOwner",
                Some(&("org.kde.StatusNotifierWatcher",).to_variant()),
                Some(reply_type),
                gio::DBusCallFlags::NONE,
                1000,
                gio::Cancellable::NONE,
            )
            .ok()
            .and_then(|reply| reply.get::<(bool,)>())
            .is_some_and(|(present,)| present)
    }
}
