//! 主窗口：shell + 下载页 + RSS 页；关闭策略与边界持久化。

use std::{rc::Rc, sync::Arc};

use fluxdown_ui_components::FluxIcon;
use fluxdown_ui_downloads::{DownloadHostActions, DownloadView};
use fluxdown_ui_i18n::keys;
use fluxdown_ui_rss::RssView;
use fluxdown_ui_settings::WebhookView;
use fluxdown_ui_shell::{RouteId, ShellAction, ShellRoute, ShellView, main_window_options};
use fluxdown_ui_theme::{active_theme, toggle_theme};
use gpui::{App, AppContext as _, Window, WindowHandle, px, size};
use gpui_component::{Icon, Root};

use crate::{
    app::Desktop,
    capability_ports::AgentRssPort,
    downloads_port::AgentDownloadsPort,
    session::attach,
    windows::{WindowKey, WindowRegistry, confirm_active_tasks},
};

const MAIN_WINDOW_SIZE: gpui::Size<gpui::Pixels> = size(px(1120.), px(760.));

/// 显示 / 恢复 / 聚焦主窗口；窗口已关闭时重建。
pub fn reveal(cx: &mut App) {
    open(cx);
    cx.activate(true);
}

/// 打开或聚焦主窗口。返回新建窗口句柄（已开时 `None`）。
pub fn open(cx: &mut App) -> Option<WindowHandle<Root>> {
    let desktop = Desktop::global(cx);
    let translator = desktop.translator.clone();
    let session = desktop.session.clone();
    let client = desktop.client.clone();
    let settings_store = desktop.settings_store.clone();
    let menu_bar = desktop.menu_bar.clone();
    let stored = Desktop::pref(cx, "desktop.window.main");
    let mut options = main_window_options();
    options.window_bounds = Some(WindowRegistry::restore_bounds(
        &WindowKey::Main,
        stored.as_ref(),
        MAIN_WINDOW_SIZE,
        cx,
    ));

    WindowRegistry::open_or_focus(cx, WindowKey::Main, options, move |window, cx| {
        let downloads_port = Arc::new(AgentDownloadsPort::new(client.clone()));
        let downloads =
            cx.new(|cx| DownloadView::new(translator.clone(), downloads_port, window, cx));
        let rss_port = Arc::new(AgentRssPort::new(client.clone()));
        let rss = cx.new(|cx| RssView::new(translator.clone(), rss_port, window, cx));
        let webhooks =
            cx.new(|cx| WebhookView::new(translator.clone(), settings_store.clone(), cx));
        let downloads_title_bar = downloads.update(cx, |downloads, cx| downloads.new_title_bar(cx));

        // RSS / Webhook 暂无顶栏插槽：统一顶栏保持空白拖拽区。
        let routes = vec![
            ShellRoute::new(
                RouteId::new("downloads"),
                "activity-downloads",
                "activity-downloads-tooltip",
                keys::MOBILE_NAV_DOWNLOADS,
                Icon::new(FluxIcon::Download),
                downloads.clone().into(),
            )
            .with_title_bar(downloads_title_bar),
            ShellRoute::new(
                RouteId::new("rss"),
                "activity-rss",
                "activity-rss-tooltip",
                "sidebarRss",
                Icon::new(FluxIcon::Rss),
                rss.clone().into(),
            )
            .optional(true),
            ShellRoute::new(
                RouteId::new("webhooks"),
                "activity-webhooks",
                "activity-webhooks-tooltip",
                "webhookNavTitle",
                Icon::new(FluxIcon::Webhook),
                webhooks.into(),
            )
            .optional(true),
        ];
        let actions = vec![
            ShellAction::with_dynamic_icon(
                "activity-theme",
                "activity-theme-tooltip",
                "activityThemeToggle",
                |cx| {
                    if active_theme(cx).mode().is_dark() {
                        Icon::new(FluxIcon::Sun)
                    } else {
                        Icon::new(FluxIcon::Moon)
                    }
                },
                toggle_theme,
            )
            .optional(true),
            ShellAction::new(
                "activity-settings",
                "activity-settings-tooltip",
                keys::SETTINGS,
                Icon::new(FluxIcon::Settings),
                move |_, cx| crate::windows::settings::open(cx),
            ),
        ];
        let shell =
            cx.new(|cx| ShellView::new(translator.clone(), routes, actions, Some(menu_bar), cx));
        // 活动栏可选项的初始可见性：偏好缺省视同 true（与设置页默认值一致）。
        let show_activity_rss = Desktop::pref(cx, "ui.show_activity_rss")
            .and_then(|value| value.as_bool())
            .unwrap_or(true);
        let show_activity_theme = Desktop::pref(cx, "ui.show_activity_theme")
            .and_then(|value| value.as_bool())
            .unwrap_or(true);
        shell.update(cx, |shell, cx| {
            shell.set_route_visible(RouteId::new("rss"), show_activity_rss, cx);
            shell.set_action_visible("activity-theme", show_activity_theme, cx);
        });

        let settings_for_categories = settings_store.clone();
        let translator_for_categories = translator.clone();
        let shutdown_status = crate::power::status(cx);
        let shutdown_port = crate::power::shutdown_port(cx);
        downloads.update(cx, |downloads, _| {
            downloads.set_host_actions(DownloadHostActions {
                open_new_download: Some(Rc::new(|context, _, cx| {
                    crate::windows::new_download::open(cx, context);
                })),
                open_task_window: Some(Rc::new(|task_id, _, cx| {
                    crate::windows::task_detail::open(cx, task_id);
                })),
                open_group_window: Some(Rc::new(|group_id, _, cx| {
                    crate::windows::group_detail::open(cx, group_id);
                })),
                open_queue_manager: Some(Rc::new(|_, cx| {
                    crate::windows::queue_manager::open(cx);
                })),
                open_category_editor: Some(Rc::new(move |id, window, cx| {
                    let translator = translator_for_categories.read(cx).clone();
                    fluxdown_ui_settings::open_category_editor(
                        settings_for_categories.clone(),
                        &translator,
                        id,
                        window,
                        cx,
                    );
                })),
                shutdown_status,
                shutdown: shutdown_port,
                on_user_started: Some(Rc::new(crate::progress_windows::user_started)),
            });
        });

        attach(&session, &downloads, cx);
        attach(&session, &rss, cx);
        {
            let desktop = Desktop::global_mut(cx);
            desktop.main_downloads = Some(downloads.downgrade());
            desktop.main_shell = Some(shell.downgrade());
        }

        let root = cx.new(|cx| Root::new(shell, window, cx));
        WindowRegistry::persist_bounds(&WindowKey::Main, client, &root, window, cx);
        install_close_policy(window, cx);
        root
    })
}

/// 关闭策略：原生关闭按钮（`windowShouldClose:`）与 ⌘W 共用一份判定。
fn install_close_policy(window: &mut Window, cx: &mut App) {
    window.on_window_should_close(cx, should_close);
}

/// 主窗口关闭：agent 托盘驻留 → 只关窗（最后一个窗口关闭后界面进程退出，下载与队列在后台
/// 继续）；否则等同「退出」——有活跃任务时先确认，随后完全退出（后台随界面一起停止）。
/// 尚未收到首个快照时驻留策略未知，按只关窗处理，绝不误停后台。
pub fn should_close(window: &mut Window, cx: &mut App) -> bool {
    let desktop = Desktop::global(cx);
    if desktop.shell.resident || desktop.session.read(cx).latest().is_none() {
        return true;
    }
    if Desktop::active_task_count(cx) == 0 {
        crate::lifecycle::quit_everything(cx);
    } else {
        confirm_active_tasks(window, cx, |_, cx| crate::lifecycle::quit_everything(cx));
    }
    false
}
