//! 通用：启动与托盘、系统集成、侧边栏与活动栏可见性、自定义分类。

use fluxdown_protocol::{ShellStatusDto, TrayUnavailableReason};
use fluxdown_ui_components::FluxIcon;
use gpui::App;

use super::{SectionContext, categories};
use crate::ui::{Control, SettingsPage, SettingsSection};

pub(crate) fn page(ctx: &SectionContext, cx: &mut App) -> SettingsPage {
    // 首次进入拉取系统集成状态（自启 / 文件关联 / URL scheme）。
    if ctx.store.read(cx).integration().is_none() && !ctx.store.read(cx).is_busy("integration") {
        ctx.store.update(cx, |store, cx| store.load_integration(cx));
    }

    SettingsPage::new(
        "general",
        ctx.t("settingsCatGeneral"),
        ctx.t("settingsCatGeneralDesc"),
        FluxIcon::Settings,
    )
    .sections([
        startup_section(ctx, cx),
        system_section(ctx, cx),
        sidebar_section(ctx),
        activity_bar_section(ctx),
        categories::group(ctx, cx),
    ])
}

fn startup_section(ctx: &SectionContext, cx: &mut App) -> SettingsSection {
    // 托盘由 agent 承载：可用性在运行期探测（Linux 取决于 StatusNotifier 宿主与 appindicator）。
    let shell = ctx.store.read(cx).shell().clone();
    SettingsSection::new()
        .title(ctx.t("settingsGroupStartupTray"))
        .row(
            ctx.item(
                "autoStartup",
                Some("autoStartupDesc"),
                integration_switch(ctx, IntegrationKind::Autostart),
            )
            .disabled(!integration_supported(ctx, IntegrationKind::Autostart, cx)),
        )
        .row(
            ctx.item(
                "closeToTray",
                Some(tray_desc(&shell, "closeToTrayDesc")),
                ctx.pref_switch("close_to_tray", true),
            )
            .disabled(!shell.tray_available),
        )
        .row(
            ctx.item(
                "startMinimizedToTray",
                Some(tray_desc(&shell, "startMinimizedToTrayDesc")),
                ctx.pref_switch("start_minimized_to_tray", false),
            )
            .disabled(!shell.tray_available),
        )
}

/// 托盘不可用时用原因替换说明文案，告诉用户关闭窗口后会发生什么。
fn tray_desc(shell: &ShellStatusDto, key: &'static str) -> &'static str {
    match shell.tray_unavailable_reason {
        None => key,
        Some(TrayUnavailableReason::NotBuilt) => "trayUnavailableNotBuilt",
        Some(TrayUnavailableReason::NoDisplay) => "trayUnavailableNoDisplay",
        Some(TrayUnavailableReason::NoHost) => "trayUnavailableNoHost",
        Some(TrayUnavailableReason::InitFailed) => "trayUnavailableInitFailed",
    }
}

fn system_section(ctx: &SectionContext, cx: &mut App) -> SettingsSection {
    SettingsSection::new()
        .title(ctx.t("settingsGroupSystem"))
        .row(ctx.item(
            "clipboardWatch",
            Some("clipboardWatchDesc"),
            ctx.pref_switch("general.clipboard_watch", false),
        ))
        .row(
            ctx.item(
                "torrentFileAssociation",
                Some("torrentFileAssociationDesc"),
                integration_switch(ctx, IntegrationKind::Torrent),
            )
            .disabled(!integration_supported(ctx, IntegrationKind::Torrent, cx)),
        )
        .row(
            ctx.item(
                "magnetLinkAssociation",
                Some("magnetLinkAssociationDesc"),
                integration_switch(ctx, IntegrationKind::Scheme("magnet")),
            )
            .disabled(!integration_supported(
                ctx,
                IntegrationKind::Scheme("magnet"),
                cx,
            )),
        )
        .row(
            ctx.item(
                "ed2kLinkAssociation",
                Some("ed2kLinkAssociationDesc"),
                integration_switch(ctx, IntegrationKind::Scheme("ed2k")),
            )
            .disabled(!integration_supported(
                ctx,
                IntegrationKind::Scheme("ed2k"),
                cx,
            )),
        )
        .row(ctx.item(
            "keepAwakeWhileDownloading",
            Some("keepAwakeWhileDownloadingDesc"),
            ctx.pref_switch("download.keep_awake", false),
        ))
        .row(ctx.item(
            "analyticsEnabled",
            Some("analyticsEnabledDesc"),
            ctx.pref_switch("analytics_enabled", true),
        ))
}

fn sidebar_section(ctx: &SectionContext) -> SettingsSection {
    SettingsSection::new()
        .title(ctx.t("sidebarVisibility"))
        .subtitle(ctx.t("sidebarVisibilityDesc"))
        .row(ctx.item(
            "showSidebarStatus",
            Some("showSidebarStatusDesc"),
            ctx.pref_switch("ui.show_sidebar_status", true),
        ))
        .row(ctx.item(
            "showSidebarQueues",
            Some("showSidebarQueuesDesc"),
            ctx.pref_switch("ui.show_sidebar_queues", true),
        ))
        .row(ctx.item(
            "showSidebarCategory",
            Some("showSidebarCategoryNestedDesc"),
            ctx.pref_switch("ui.show_sidebar_category", true),
        ))
        .row(ctx.item(
            "showSidebarDevice",
            Some("showSidebarDeviceDesc"),
            show_sidebar_device_field(ctx),
        ))
}

/// `ui.show_sidebar_devices` 三态：未设置 = 登录后自动显示。开关显示有效值。
fn show_sidebar_device_field(ctx: &SectionContext) -> Control {
    let get = ctx.store();
    let set = ctx.store();
    Control::switch(
        move |cx: &App| {
            let store = get.read(cx);
            store
                .pref("ui.show_sidebar_devices")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or_else(|| store.session().is_some())
        },
        move |value, cx: &mut App| {
            set.update(cx, |store, cx| {
                store.set_pref_bool("ui.show_sidebar_devices", value, cx)
            });
        },
    )
}

fn activity_bar_section(ctx: &SectionContext) -> SettingsSection {
    SettingsSection::new()
        .title(ctx.t("activityBarSection"))
        .subtitle(ctx.t("activityBarSectionDesc"))
        .row(ctx.item(
            "showActivityRss",
            Some("showActivityRssDesc"),
            ctx.pref_switch("ui.show_activity_rss", true),
        ))
        .row(ctx.item(
            "showActivityTheme",
            Some("showActivityThemeDesc"),
            ctx.pref_switch("ui.show_activity_theme", true),
        ))
}

#[derive(Clone, Copy)]
enum IntegrationKind {
    Autostart,
    Torrent,
    Scheme(&'static str),
}

fn integration_supported(ctx: &SectionContext, kind: IntegrationKind, cx: &App) -> bool {
    let store = ctx.store.read(cx);
    if store.is_read_only() {
        return false;
    }
    store.integration().is_some_and(|dto| match kind {
        IntegrationKind::Autostart => dto.autostart_supported,
        IntegrationKind::Torrent => dto.file_association_supported,
        IntegrationKind::Scheme(_) => dto.url_protocol_supported,
    })
}

/// 系统集成开关：值来自 agent 探测结果，切换即调用 agent 注册/注销。
fn integration_switch(ctx: &SectionContext, kind: IntegrationKind) -> Control {
    let get = ctx.store();
    let set = ctx.store();
    Control::switch(
        move |cx: &App| {
            get.read(cx).integration().is_some_and(|dto| match kind {
                IntegrationKind::Autostart => dto.autostart_enabled,
                IntegrationKind::Torrent => dto.torrent_associated,
                IntegrationKind::Scheme(scheme) => {
                    dto.url_protocols.get(scheme).copied().unwrap_or(false)
                }
            })
        },
        move |value, cx: &mut App| {
            set.update(cx, |store, cx| match kind {
                IntegrationKind::Autostart => store.set_autostart(value, cx),
                IntegrationKind::Torrent => {
                    store.set_pref_bool("torrent_assoc_user_disabled", !value, cx);
                    store.set_file_association(value, cx);
                }
                IntegrationKind::Scheme(scheme) => {
                    let key: &'static str = match scheme {
                        "magnet" => "magnet_assoc_user_disabled",
                        _ => "ed2k_assoc_user_disabled",
                    };
                    store.set_pref_bool(key, !value, cx);
                    store.set_url_protocol(scheme, value, cx);
                }
            });
        },
    )
}
