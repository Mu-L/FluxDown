//! 下载：保存位置、行为、连接与性能、自动重试、高级。

use fluxdown_ui_components::{ButtonVariant, FluxIcon, button, loading_button, tabular_numbers};
use fluxdown_ui_theme::active_theme;
use gpui::{App, ParentElement, SharedString, Styled, px};
use gpui_component::h_flex;

use super::{SectionContext, user_agent};
use crate::ui::{Control, SettingsPage, SettingsSection, body_text, meta_text};

pub(crate) fn page(ctx: &SectionContext, cx: &mut App) -> SettingsPage {
    if ctx.store.read(cx).conn_policy().is_none() && !ctx.store.read(cx).is_busy("connPolicy") {
        ctx.store.update(cx, |store, cx| store.load_conn_policy(cx));
    }
    SettingsPage::new(
        "download",
        ctx.t("settingsCatDownload"),
        ctx.t("settingsCatDownloadDesc"),
        FluxIcon::Download,
    )
    .sections([
        save_location_section(ctx),
        behavior_section(ctx, cx),
        connection_section(ctx, cx),
        retry_section(ctx),
        advanced_section(ctx),
    ])
}

fn save_location_section(ctx: &SectionContext) -> SettingsSection {
    SettingsSection::new()
        .title(ctx.t("settingsGroupSaveLocation"))
        .row(ctx.item(
            "defaultSaveDir",
            Some("defaultSaveDirDesc"),
            save_dir_control(ctx),
        ))
        .row(ctx.item(
            "rememberLastSaveDir",
            Some("rememberLastSaveDirDesc"),
            ctx.pref_switch("download.remember_last_save_dir", false),
        ))
}

/// 目录选择：文本输入 + 系统目录选择器。
fn save_dir_control(ctx: &SectionContext) -> Control {
    let store = ctx.store();
    let browse = ctx.t("browse");
    Control::custom(move |disabled, _key, _window, cx: &mut App| {
        let tokens = active_theme(cx).tokens();
        let current = store.read(cx).daemon_str("default_save_dir");
        let pick_store = store.clone();
        h_flex()
            .gap(tokens.spacing.sm)
            .items_center()
            .child(
                body_text(cx)
                    .max_w(px(320.))
                    .truncate()
                    .text_color(if current.is_empty() {
                        tokens.colors.muted_foreground
                    } else {
                        tokens.colors.foreground
                    })
                    .child(SharedString::from(current)),
            )
            .child(
                button(
                    "download-pick-save-dir",
                    browse.clone(),
                    ButtonVariant::Secondary,
                    cx,
                )
                .on_click(move |_, _, cx| {
                    let store = pick_store.clone();
                    let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
                        files: false,
                        directories: true,
                        multiple: false,
                        prompt: None,
                    });
                    cx.spawn(async move |cx| {
                        if let Ok(Ok(Some(paths))) = receiver.await
                            && let Some(path) = paths.first()
                        {
                            let text = path.display().to_string();
                            store.update(cx, |store, cx| {
                                store.set_daemon("default_save_dir", text, cx);
                            });
                        }
                    })
                    .detach();
                })
                .disabled(disabled),
            )
    })
}

fn behavior_section(ctx: &SectionContext, cx: &mut App) -> SettingsSection {
    let silent = ctx
        .store
        .read(cx)
        .pref_bool("download.silent_download", false);
    let mut section = SettingsSection::new()
        .title(ctx.t("settingsGroupBehavior"))
        .row(ctx.item(
            "silentDownload",
            Some("silentDownloadDesc"),
            ctx.pref_switch("download.silent_download", false),
        ));
    if silent {
        section = section.row(ctx.item(
            "silentSkipSelection",
            Some("silentSkipSelectionDesc"),
            ctx.pref_switch("download.silent_skip_selection", false),
        ));
    }
    section
        .row(ctx.item(
            "useServerTime",
            Some("useServerTimeDesc"),
            ctx.daemon_switch("use_server_time"),
        ))
        .row(ctx.item(
            "fileExistsBehavior",
            Some("fileExistsBehaviorDesc"),
            ctx.daemon_enum_dropdown("file_exists_behavior", "fileExists"),
        ))
        .row(ctx.item(
            "fileMissingAction",
            Some("fileMissingActionDesc"),
            ctx.daemon_enum_dropdown("file_missing_action", "fileMissing"),
        ))
        .row(ctx.item(
            "defaultQueueSetting",
            Some("defaultQueueSettingDesc"),
            default_queue_control(ctx, cx),
        ))
}

fn default_queue_control(ctx: &SectionContext, cx: &mut App) -> Control {
    let mut options = vec![(SharedString::from(""), ctx.t("defaultQueue"))];
    options.extend(ctx.store.read(cx).queues().iter().map(|queue| {
        (
            SharedString::from(queue.queue_id.clone()),
            SharedString::from(queue.name.clone()),
        )
    }));
    ctx.daemon_dropdown("default_queue_id", options)
}

fn connection_section(ctx: &SectionContext, cx: &mut App) -> SettingsSection {
    let store = ctx.store.read(cx);
    let auto_segments = store.daemon_i64("default_segments") == 0;
    let cdn_multi = store.daemon_bool("cdn_multi_enabled");
    let mut section = SettingsSection::new()
        .title(ctx.t("settingsGroupConnection"))
        .row(ctx.item(
            "defaultThreads",
            Some("defaultThreadsDesc"),
            ctx.daemon_number("default_segments"),
        ));
    if auto_segments {
        section = section.row(ctx.item(
            "autoMaxConnections",
            Some("autoMaxConnectionsDesc"),
            ctx.daemon_number("auto_max_connections"),
        ));
    }
    section = section.row(ctx.item(
        "cdnMultiEnabled",
        Some("cdnMultiEnabledDesc"),
        ctx.daemon_switch("cdn_multi_enabled"),
    ));
    if cdn_multi {
        section = section.row(ctx.item(
            "cdnMaxNodes",
            Some("cdnMaxNodesDesc"),
            ctx.daemon_number("cdn_max_nodes"),
        ));
    }
    section
        .row(ctx.item(
            "connPolicyCache",
            Some("connPolicyCacheDesc"),
            conn_policy_control(ctx),
        ))
        .row(ctx.item(
            "maxConcurrent",
            Some("maxConcurrentDesc"),
            ctx.daemon_number("max_concurrent_tasks"),
        ))
        .row(ctx.item(
            "speedLimit",
            Some("speedLimitDesc"),
            bytes_per_second_control(ctx, "speed_limit_bytes"),
        ))
        .row(ctx.item(
            "uploadLimit",
            Some("uploadLimitDesc"),
            bytes_per_second_control(ctx, "upload_limit_bytes"),
        ))
}

/// 以 KB/s 显示与编辑字节速率键；0 = 不限。
fn bytes_per_second_control(ctx: &SectionContext, key: &'static str) -> Control {
    let get = ctx.store();
    let set = ctx.store();
    Control::number(
        0.0,
        (i64::MAX / 1024) as f64,
        64.0,
        move |cx: &App| (get.read(cx).daemon_i64(key) / 1024) as f64,
        move |value, cx: &mut App| {
            set.update(cx, |store, cx| {
                store.set_daemon_i64(key, (value.round() as i64).saturating_mul(1024), cx);
            });
        },
    )
}

fn conn_policy_control(ctx: &SectionContext) -> Control {
    let store = ctx.store();
    let clear = ctx.t("connPolicyCacheClear");
    let empty = ctx.t("connPolicyCacheEmpty");
    Control::custom(move |_disabled, _key, _window, cx: &mut App| {
        let tokens = active_theme(cx).tokens();
        let count = store
            .read(cx)
            .conn_policy()
            .map_or(0, |summary| summary.domain_count);
        let busy = store.read(cx).is_busy("connPolicy");
        let clearing = store.read(cx).is_busy_tagged("connPolicy", "clear");
        let clear_store = store.clone();
        h_flex()
            .gap(tokens.spacing.sm)
            .items_center()
            .child(
                meta_text(cx)
                    .font_features(tabular_numbers())
                    .child(if count == 0 {
                        empty.clone()
                    } else {
                        SharedString::from(count.to_string())
                    }),
            )
            .child(
                loading_button(
                    "download-clear-conn-policy",
                    clear.clone(),
                    ButtonVariant::Secondary,
                    clearing,
                    cx,
                )
                .disabled(busy || count == 0)
                .on_click(move |_, _, cx| {
                    clear_store.update(cx, |store, cx| {
                        store.clear_conn_policy(cx);
                        store.tag_busy("connPolicy", "clear");
                    });
                }),
            )
    })
}

fn retry_section(ctx: &SectionContext) -> SettingsSection {
    SettingsSection::new()
        .title(ctx.t("settingsGroupRetry"))
        .row(ctx.item(
            "autoRetryCount",
            Some("autoRetryCountDesc"),
            ctx.daemon_number("max_auto_retries"),
        ))
        .row(ctx.item(
            "autoRetryDelay",
            Some("autoRetryDelayDesc"),
            ctx.daemon_number("auto_retry_delay_secs"),
        ))
        .row(ctx.item(
            "autoResumeOnStart",
            Some("autoResumeOnStartDesc"),
            ctx.daemon_switch("auto_resume_on_start"),
        ))
}

fn advanced_section(ctx: &SectionContext) -> SettingsSection {
    SettingsSection::new()
        .title(ctx.t("settingsGroupAdvanced"))
        .row(ctx.item("userAgent", Some("userAgentDesc"), user_agent::field(ctx)))
        .row(
            ctx.item(
                "revealFileCmdLabel",
                Some("revealFileCmdShortDesc"),
                ctx.pref_input("reveal_file_cmd", ""),
            )
            .help(ctx.t("revealFileCmdDesc")),
        )
}
