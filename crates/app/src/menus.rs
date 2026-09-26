//! 应用菜单（macOS 原生 / Windows·Linux 标题栏菜单栏）、全局键位与应用级动作处理。

use fluxdown_ui_downloads::actions as dl;
use fluxdown_ui_i18n::Translator;
use gpui::{App, Entity, KeyBinding, Menu, MenuItem, SharedString, Window};
use gpui_component::{GlobalState, WindowExt as _, menu::AppMenuBar};

use crate::{
    actions::{
        About, BringAllToFront, CheckUpdate, CloseWindow, Hide, HideOthers, MinimizeWindow,
        OpenLogsFolder, OpenSettings, OpenWebsite, Quit, ShowAll, ShowMainWindow, ToggleFullScreen,
        ZoomWindow,
    },
    app::Desktop,
    windows::{WindowKey, WindowRegistry, confirm_active_tasks},
};

const WEBSITE_URL: &str = "https://fluxdown.zerx.dev";

fn primary() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    }
}

/// 注册全局键位。字母键只在下载页上下文生效（页面内再按聚焦输入框二次拦截）。
///
/// macOS 的 ⌘W / ⌘M / ⌘H / ⌃⌘F 由 AppKit 经菜单 key equivalent 分发，gpui 不会自动
/// 提供；须像 Zed 一样显式绑定并在菜单树里给出对应项。
pub fn bind_keys(cx: &mut App) {
    let p = primary();
    let dl_ctx = Some(dl::KEY_CONTEXT);
    let mut bindings = vec![
        KeyBinding::new(&format!("{p}-n"), dl::NewDownload, None),
        KeyBinding::new(&format!("{p}-,"), OpenSettings, None),
        KeyBinding::new(&format!("{p}-f"), dl::FocusSearch, dl_ctx),
        KeyBinding::new(&format!("{p}-a"), dl::SelectAllTasks, dl_ctx),
        KeyBinding::new("delete", dl::DeleteSelected, dl_ctx),
        KeyBinding::new("backspace", dl::DeleteSelected, dl_ctx),
        KeyBinding::new(&format!("{p}-backspace"), dl::DeleteSelected, dl_ctx),
        KeyBinding::new("shift-delete", dl::DeleteSelectedWithFiles, dl_ctx),
        KeyBinding::new("space", dl::TogglePauseSelected, dl_ctx),
        KeyBinding::new("shift-d", dl::CycleDensity, dl_ctx),
        KeyBinding::new("g", dl::CycleGroupBy, dl_ctx),
        KeyBinding::new("s", dl::CycleSort, dl_ctx),
        KeyBinding::new(&format!("{p}-i"), dl::ToggleDetailPanel, dl_ctx),
        KeyBinding::new("escape", dl::ClearSelection, dl_ctx),
    ];
    if cfg!(target_os = "macos") {
        bindings.extend([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-w", CloseWindow, None),
            KeyBinding::new("cmd-m", MinimizeWindow, None),
            KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
            KeyBinding::new("cmd-h", Hide, None),
            KeyBinding::new("alt-cmd-h", HideOthers, None),
        ]);
    }
    cx.bind_keys(bindings);
}

fn t(translator: &Translator, key: &str) -> SharedString {
    SharedString::from(translator.text(key).to_owned())
}

/// 菜单树（macOS 原生菜单与标题栏菜单栏共用一份）。
pub fn build_menus(translator: &Translator) -> Vec<Menu> {
    let mut menus = Vec::with_capacity(7);
    if cfg!(target_os = "macos") {
        menus.push(Menu::new("FluxDown").items([
            MenuItem::action(t(translator, "menuAbout"), About),
            MenuItem::separator(),
            MenuItem::action(t(translator, "menuSettings"), OpenSettings),
            MenuItem::separator(),
            MenuItem::action(t(translator, "menuHide"), Hide),
            MenuItem::action(t(translator, "menuHideOthers"), HideOthers),
            MenuItem::action(t(translator, "menuShowAll"), ShowAll),
            MenuItem::separator(),
            MenuItem::action(t(translator, "menuQuit"), Quit),
        ]));
    }
    let mut file = vec![
        MenuItem::action(t(translator, "menuNewDownload"), dl::NewDownload),
        MenuItem::action(t(translator, "openTorrentFile"), dl::OpenTorrentFile),
    ];
    file.push(MenuItem::separator());
    if cfg!(target_os = "macos") {
        file.push(MenuItem::action(
            t(translator, "menuCloseWindow"),
            CloseWindow,
        ));
    } else {
        file.push(MenuItem::action(t(translator, "menuQuit"), Quit));
    }
    menus.push(Menu::new(t(translator, "menuFile")).items(file));
    menus.push(Menu::new(t(translator, "menuTasks")).items([
        MenuItem::action(t(translator, "pause"), dl::PauseSelected),
        MenuItem::action(t(translator, "resume"), dl::ResumeSelected),
        MenuItem::action(t(translator, "deleteTask"), dl::DeleteSelected),
        MenuItem::action(
            t(translator, "deleteTaskAndFile"),
            dl::DeleteSelectedWithFiles,
        ),
        MenuItem::separator(),
        MenuItem::action(t(translator, "selectAll"), dl::SelectAllTasks),
        MenuItem::action(t(translator, "pauseAll"), dl::PauseAll),
        MenuItem::action(t(translator, "resumeAll"), dl::ResumeAll),
        MenuItem::action(t(translator, "menuClearFinished"), dl::ClearFinished),
        MenuItem::separator(),
        MenuItem::action(t(translator, "openFile"), dl::OpenSelected),
        MenuItem::action(t(translator, "openFolder"), dl::RevealSelected),
        MenuItem::action(t(translator, "copyUrl"), dl::CopySelectedUrl),
        MenuItem::action(t(translator, "renameTask"), dl::RenameSelected),
        MenuItem::action(t(translator, "redownloadTask"), dl::RedownloadSelected),
        MenuItem::action(t(translator, "boostDownload"), dl::ToggleBoostSelected),
        MenuItem::action(t(translator, "menuOpenInWindow"), dl::OpenSelectedInWindow),
    ]));
    let mut view = vec![
        MenuItem::action(t(translator, "menuCycleDensity"), dl::CycleDensity),
        MenuItem::action(t(translator, "menuCycleGroupBy"), dl::CycleGroupBy),
        MenuItem::action(t(translator, "menuCycleSort"), dl::CycleSort),
        MenuItem::separator(),
        MenuItem::action(t(translator, "menuDetailPanel"), dl::ToggleDetailPanel),
    ];
    if cfg!(target_os = "macos") {
        view.push(MenuItem::separator());
        view.push(MenuItem::action(
            t(translator, "menuToggleFullScreen"),
            ToggleFullScreen,
        ));
    }
    menus.push(Menu::new(t(translator, "menuView")).items(view));
    let mut tools = Vec::with_capacity(4);
    if !cfg!(target_os = "macos") {
        tools.push(MenuItem::action(
            t(translator, "menuSettings"),
            OpenSettings,
        ));
    }
    tools.extend([
        MenuItem::action(t(translator, "manageQueueAction"), dl::OpenQueueManager),
        MenuItem::action(t(translator, "menuCheckForUpdates"), CheckUpdate),
        MenuItem::action(t(translator, "menuOpenLogsFolder"), OpenLogsFolder),
    ]);
    menus.push(Menu::new(t(translator, "menuTools")).items(tools));
    if cfg!(target_os = "macos") {
        menus.push(Menu::new(t(translator, "menuWindow")).items([
            MenuItem::action(t(translator, "menuMinimize"), MinimizeWindow),
            MenuItem::action(t(translator, "menuZoom"), ZoomWindow),
            MenuItem::separator(),
            MenuItem::action(t(translator, "menuBringAllToFront"), BringAllToFront),
        ]));
    }
    let mut help = vec![MenuItem::action(t(translator, "menuWebsite"), OpenWebsite)];
    if !cfg!(target_os = "macos") {
        help.push(MenuItem::action(t(translator, "menuAbout"), About));
    }
    menus.push(Menu::new(t(translator, "menuHelp")).items(help));
    menus
}

/// 装配菜单：原生菜单 + 标题栏菜单栏；翻译变化时重建。
pub fn install(cx: &mut App, translator: &Entity<Translator>) -> Entity<AppMenuBar> {
    apply_menus(translator, cx);
    let menu_bar = AppMenuBar::new(cx);
    let observed_bar = menu_bar.clone();
    cx.observe(translator, move |translator, cx| {
        apply_menus(&translator, cx);
        observed_bar.update(cx, |bar, cx| bar.reload(cx));
    })
    .detach();
    menu_bar
}

fn apply_menus(translator: &Entity<Translator>, cx: &mut App) {
    let translator = translator.read(cx).clone();
    cx.set_menus(build_menus(&translator));
    GlobalState::global_mut(cx).set_app_menus(
        build_menus(&translator)
            .into_iter()
            .map(Menu::owned)
            .collect(),
    );
}

/// 应用级动作（不依赖具体窗口）。窗口级动作作用于当前活动窗口。
pub fn install_global_actions(cx: &mut App) {
    cx.on_action(|_: &OpenSettings, cx| crate::windows::settings::open(cx));
    cx.on_action(|_: &ShowMainWindow, cx| {
        crate::windows::main::open(cx);
        cx.activate(true);
    });
    cx.on_action(|_: &Quit, cx| request_quit(cx));
    cx.on_action(|_: &CloseWindow, cx| WindowRegistry::close_active_window(cx));
    cx.on_action(|_: &MinimizeWindow, cx| with_active_window(cx, |w| w.minimize_window()));
    cx.on_action(|_: &ZoomWindow, cx| with_active_window(cx, |w| w.zoom_window()));
    cx.on_action(|_: &ToggleFullScreen, cx| {
        with_active_window(cx, |w| w.toggle_fullscreen());
    });
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &BringAllToFront, cx| cx.activate(true));
    cx.on_action(|_: &OpenWebsite, cx| cx.open_url(WEBSITE_URL));
    cx.on_action(|_: &dl::NewDownload, cx| crate::windows::new_download::open_default(cx));
    cx.on_action(|_: &dl::OpenQueueManager, cx| crate::windows::queue_manager::open(cx));
    cx.on_action(|_: &CheckUpdate, cx| check_update(cx));
    cx.on_action(|_: &OpenLogsFolder, cx| open_logs_folder(cx));
    cx.on_action(|_: &About, cx| show_about(cx));
}

/// 全局动作里操作活动窗口必须 defer：键盘触发时正处于该窗口自己的 update 栈内，
/// 同步 `handle.update` 拿不到窗口会静默失败（菜单点击不在 update 栈内，所以只有键盘路径失效）。
fn with_active_window(cx: &mut App, f: impl FnOnce(&Window) + 'static) {
    cx.defer(move |cx| {
        if let Some(window) = WindowRegistry::focused_window(cx) {
            let _ = window.update(cx, |_, window, _| f(window));
        }
    });
}

/// 退出（菜单 / ⌘Q）：完全退出，后台 agent 与 daemon 一起停止。有活跃任务时先在当前窗口
/// 确认「下载将暂停」。只退出界面、保留后台，走关闭窗口（托盘驻留时）。
pub fn request_quit(cx: &mut App) {
    if Desktop::active_task_count(cx) == 0 {
        crate::lifecycle::quit_everything(cx);
        return;
    }
    cx.defer(|cx| {
        let Some(window) = WindowRegistry::focused_window(cx)
            .or_else(|| WindowRegistry::handle(cx, &WindowKey::Main))
        else {
            crate::lifecycle::quit_everything(cx);
            return;
        };
        let _ = window.update(cx, |_, window, cx| {
            confirm_active_tasks(window, cx, |_, cx| crate::lifecycle::quit_everything(cx));
        });
    });
}

fn check_update(cx: &mut App) {
    let desktop = Desktop::global(cx);
    let client = desktop.client.clone();
    let translator = desktop.translator.clone();
    let future = client.call::<serde_json::Value, fluxdown_protocol::UpdateCheckResultDto>(
        fluxdown_protocol::method::AGENT_UPDATE_CHECK,
        Some(serde_json::json!({})),
    );
    cx.spawn(async move |cx| {
        let result = future.await;
        cx.update(|cx| {
            let translator = translator.read(cx).clone();
            let Some(window) = cx
                .active_window()
                .or_else(|| WindowRegistry::handle(cx, &WindowKey::Main))
            else {
                return;
            };
            let _ = window.update(cx, |_, window, cx| match result {
                Ok(result) if result.has_update => {
                    let url = if result.release_page_url.is_empty() {
                        result.download_url.clone()
                    } else {
                        result.release_page_url.clone()
                    };
                    let message = translator
                        .text("updateAvailableToast")
                        .replace("{v}", &result.latest_version);
                    let action_label = t(&translator, "goToDownload");
                    let note = gpui_component::notification::Notification::info(message).action(
                        move |_, _, _| {
                            let url = url.clone();
                            gpui_component::button::Button::new("update-go-download")
                                .label(action_label.clone())
                                .on_click(move |_, _, cx| cx.open_url(&url))
                        },
                    );
                    window.push_notification(note, cx);
                }
                Ok(_) => window.push_notification(t(&translator, "upToDate"), cx),
                Err(_) => window.push_notification(
                    gpui_component::notification::Notification::error(t(
                        &translator,
                        "localServiceActionFailed",
                    )),
                    cx,
                ),
            });
        });
    })
    .detach();
}

fn open_logs_folder(cx: &mut App) {
    let client = Desktop::global(cx).client.clone();
    let future = client.call::<(), fluxdown_protocol::LogPathsDto>(
        fluxdown_protocol::method::AGENT_DIAGNOSTICS_LOG_PATHS,
        None,
    );
    cx.spawn(async move |cx| {
        if let Ok(paths) = future.await {
            let dir = if paths.agent_log_dir.is_empty() {
                paths.daemon_log_dir
            } else {
                paths.agent_log_dir
            };
            cx.update(|cx| cx.reveal_path(std::path::Path::new(&dir)));
        }
    })
    .detach();
}

fn show_about(cx: &mut App) {
    let Some(window) = cx
        .active_window()
        .or_else(|| WindowRegistry::handle(cx, &WindowKey::Main))
    else {
        return;
    };
    let translator = Desktop::global(cx).translator.read(cx).clone();
    let title = t(&translator, "menuAbout");
    let version_label = t(&translator, "currentVersion");
    let protocol_label = t(&translator, "protocolVersionLabel");
    let website_label = t(&translator, "menuWebsite");
    let close_label = t(&translator, "close");
    let _ = window.update(cx, |_, window, cx| {
        window.open_dialog(cx, move |dialog, _, cx| {
            use gpui::{IntoElement as _, ParentElement as _, Styled as _};
            let version_label = version_label.clone();
            let protocol_label = protocol_label.clone();
            let website_label = website_label.clone();
            dialog
                .title(fluxdown_ui_components::dialog_title(title.clone(), cx))
                .w(gpui::px(520.))
                .content(move |content, _, cx| {
                    let tokens = fluxdown_ui_theme::active_theme(cx).tokens();
                    let value = |text: String| {
                        gpui::div()
                            .text_size(tokens.typography.sm.size)
                            .line_height(tokens.typography.sm.line_height)
                            .text_color(tokens.colors.muted_foreground)
                            .font_features(fluxdown_ui_components::tabular_numbers())
                            .child(text)
                    };
                    // 版本信息用分组卡片的「标签 — 值」行呈现，与设置页同一版式。
                    content.child(fluxdown_ui_components::option_group(
                        [
                            fluxdown_ui_components::option_row(
                                version_label.clone(),
                                None,
                                value(format!("v{}", env!("CARGO_PKG_VERSION"))),
                                cx,
                            )
                            .into_any_element(),
                            fluxdown_ui_components::option_row(
                                protocol_label.clone(),
                                None,
                                value(fluxdown_protocol::PROTOCOL_VERSION.to_string()),
                                cx,
                            )
                            .into_any_element(),
                            fluxdown_ui_components::option_row(
                                website_label.clone(),
                                None,
                                gpui_component::link::Link::new("about-website")
                                    .href(WEBSITE_URL)
                                    .text_size(tokens.typography.sm.size)
                                    .child(WEBSITE_URL),
                                cx,
                            )
                            .into_any_element(),
                        ],
                        cx,
                    ))
                })
                .footer(fluxdown_ui_components::dialog_footer(
                    None,
                    close_label.clone(),
                    fluxdown_ui_components::DialogIntent::Confirm,
                    cx,
                ))
        });
    });
}
