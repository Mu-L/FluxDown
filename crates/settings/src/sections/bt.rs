//! BitTorrent：基础（DHT/UPnP/端口）、Tracker（列表 + 订阅）、做种。

use fluxdown_ui_components::FluxIcon;
use gpui::{App, SharedString};

use super::{
    SectionContext,
    subscription::{self, ListFormat},
};
use crate::ui::{SettingsPage, SettingsSection, SettingsTab};

pub(crate) fn page(ctx: &SectionContext, cx: &mut App) -> SettingsPage {
    SettingsPage::new(
        "bt",
        ctx.t("settingsCatBt"),
        ctx.t("settingsCatBtDesc"),
        FluxIcon::Magnet,
    )
    .tab(SettingsTab::new("basic", ctx.t("settingsTabGeneral")).section(basic_section(ctx)))
    .tab(SettingsTab::new("tracker", ctx.t("settingsTabTracker")).section(tracker_section(ctx, cx)))
    .tab(SettingsTab::new("seeding", ctx.t("settingsTabSeeding")).section(seeding_section(ctx, cx)))
}

fn basic_section(ctx: &SectionContext) -> SettingsSection {
    SettingsSection::new()
        .title(ctx.t("settingsTabGeneral"))
        .subtitle(ctx.t("btSettingsRestartHint"))
        .row(ctx.item(
            "btEnableDht",
            Some("btEnableDhtDesc"),
            ctx.daemon_switch("bt_enable_dht"),
        ))
        .row(ctx.item(
            "btEnableUpnp",
            Some("btEnableUpnpDesc"),
            ctx.daemon_switch("bt_enable_upnp"),
        ))
        .row(ctx.item(
            "btListenPortStart",
            Some("btListenPortDesc"),
            ctx.daemon_number("bt_port_start"),
        ))
        .row(ctx.item("btListenPortEnd", None, ctx.daemon_number("bt_port_end")))
        .row(ctx.item(
            "btMseMode",
            Some("btMseModeDesc"),
            ctx.daemon_enum_dropdown("bt_mse_mode", "btMseMode"),
        ))
}

fn tracker_section(ctx: &SectionContext, cx: &mut App) -> SettingsSection {
    let sub_enabled = ctx.store.read(cx).daemon_bool("bt_tracker_sub_enabled");
    SettingsSection::new()
        .title(ctx.t("settingsTabTracker"))
        .row(subscription::list_item(
            ctx,
            "btTrackerList",
            "btTrackerListDesc",
            "btTrackerPlaceholder",
            "bt_custom_trackers",
            ListFormat::Lines,
        ))
        .row(ctx.item(
            "btTrackerSub",
            Some("btTrackerSubDesc"),
            ctx.daemon_switch("bt_tracker_sub_enabled"),
        ))
        .row(
            subscription::list_item(
                ctx,
                "btTrackerSubUrls",
                "btTrackerSubUrlsDesc",
                "btTrackerSubPlaceholder",
                "bt_tracker_sub_urls",
                ListFormat::Lines,
            )
            .disabled(!sub_enabled),
        )
        .row(subscription::status_item(
            ctx,
            subscription::SubscriptionKind::BtTrackers,
            cx,
        ))
}

fn seeding_section(ctx: &SectionContext, cx: &mut App) -> SettingsSection {
    let store = ctx.store.read(cx);
    let seed_enabled = store.daemon_bool("bt_seed_enabled");
    let mut group = SettingsSection::new()
        .title(ctx.t("settingsTabSeeding"))
        .row(ctx.item(
            "btSeedEnabled",
            Some("btSeedEnabledDesc"),
            ctx.daemon_switch("bt_seed_enabled"),
        ));
    if !seed_enabled {
        return group;
    }
    group = group
        .row(ctx.item(
            "btSeedMaxActive",
            Some("btSeedMaxActiveDesc"),
            ctx.daemon_number("bt_seed_max_active"),
        ))
        .row(ctx.item(
            "btAutoReseed",
            Some("btAutoReseedDesc"),
            ctx.daemon_switch("bt_auto_reseed"),
        ))
        .row(ctx.item(
            "btSeedRatioLimit",
            None,
            ctx.daemon_number_with("bt_seed_ratio_limit", 0.1),
        ))
        .row(ctx.item(
            "btSeedPostRatioLimit",
            None,
            ctx.daemon_number_with("bt_seed_post_ratio_limit", 0.1),
        ))
        .row(ctx.item(
            "btSeedTimeLimit",
            None,
            ctx.daemon_number("bt_seed_time_limit_minutes"),
        ))
        .row(ctx.item(
            "btSeedTimeLimitUnit",
            None,
            ctx.daemon_dropdown("bt_seed_time_limit_unit", time_units(ctx)),
        ))
        .row(ctx.item(
            "btSeedInactiveTimeLimit",
            None,
            ctx.daemon_number("bt_seed_inactive_time_limit_minutes"),
        ))
        .row(ctx.item(
            "btSeedInactiveTimeLimitUnit",
            None,
            ctx.daemon_dropdown("bt_seed_inactive_time_limit_unit", time_units(ctx)),
        ))
        .row(ctx.item(
            "btSeedConditionsOperator",
            None,
            ctx.daemon_dropdown(
                "bt_seed_limit_operator",
                vec![
                    (SharedString::from("or"), ctx.t("btSeedOperatorOr")),
                    (SharedString::from("and"), ctx.t("btSeedOperatorAnd")),
                ],
            ),
        ))
        .row(ctx.item(
            "btSeedThenAction",
            None,
            ctx.daemon_dropdown(
                "bt_seed_then_action",
                vec![
                    (SharedString::from("stop"), ctx.t("btSeedStopSeeding")),
                    (SharedString::from("delete"), ctx.t("btSeedDeleteTask")),
                    (
                        SharedString::from("delete_files"),
                        ctx.t("btSeedDeleteTaskAndFiles"),
                    ),
                ],
            ),
        ));
    group
}

fn time_units(ctx: &SectionContext) -> Vec<(SharedString, SharedString)> {
    vec![
        (SharedString::from("minutes"), ctx.t("timeUnitMinutes")),
        (SharedString::from("hours"), ctx.t("timeUnitHours")),
        (SharedString::from("days"), ctx.t("timeUnitDays")),
    ]
}
