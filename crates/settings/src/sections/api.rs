//! API 服务：本机网关的功能开关、端口、访问令牌与 LAN 暴露。

use fluxdown_protocol::GatewayPatchParams;
use fluxdown_ui_components::{
    ButtonVariant, ControlExt as _, FluxIcon, button, icon_button, loading_icon_button,
    tabular_numbers,
};
use fluxdown_ui_theme::active_theme;
use gpui::{
    App, AppContext as _, ClipboardItem, Entity, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, Window, px,
};
use gpui_component::{
    Icon, h_flex,
    input::{Input, InputEvent, InputState},
    tooltip::Tooltip,
};

use super::SectionContext;
use crate::ui::{Control, INPUT_WIDTH, SettingsPage, SettingsSection, body_text};

pub(crate) fn page(ctx: &SectionContext, cx: &mut App) -> SettingsPage {
    SettingsPage::new(
        "api",
        ctx.t("settingsCatApiService"),
        ctx.t("settingsCatApiServiceDesc"),
        FluxIcon::Code,
    )
    .sections([service_section(ctx, cx), features_section(ctx)])
}

fn service_section(ctx: &SectionContext, cx: &mut App) -> SettingsSection {
    let gateway = ctx.store.read(cx).gateway().clone();
    let port_text = SharedString::from(gateway.port.to_string());
    let address = SharedString::from(format!("http://127.0.0.1:{}", gateway.port));
    let address_for_copy = address.clone();
    let copied = ctx.t("apiServiceCopied");
    let copy_label = ctx.t("apiServiceCopy");

    SettingsSection::new()
        .title(ctx.t("settingsCatApiService"))
        .row(ctx.item(
            "apiServicePort",
            Some("apiServicePortDesc"),
            Control::custom(move |_, _, _, cx: &mut App| {
                body_text(cx)
                    .font_features(tabular_numbers())
                    .child(port_text.clone())
            }),
        ))
        .row(
            ctx.item(
                "apiServiceAddress",
                None,
                Control::custom(move |_, _, _, cx: &mut App| {
                    let tokens = active_theme(cx).tokens();
                    let address = address_for_copy.clone();
                    h_flex()
                        .gap(tokens.spacing.sm)
                        .items_center()
                        .child(body_text(cx).child(address.clone()))
                        .child(
                            button(
                                "api-copy-address",
                                copy_label.clone(),
                                ButtonVariant::Secondary,
                                cx,
                            )
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    address.to_string(),
                                ));
                            }),
                        )
                }),
            )
            .keywords([copied.clone()]),
        )
        .row(ctx.item(
            "apiServiceLanEnable",
            Some("apiServiceLanEnableDesc"),
            gateway_switch(ctx, GatewayFlag::Lan),
        ))
        .row(ctx.item(
            "apiServiceToken",
            Some("apiServiceTokenDesc"),
            token_field(ctx),
        ))
}

fn features_section(ctx: &SectionContext) -> SettingsSection {
    SettingsSection::new()
        .title(ctx.t("apiServiceFeaturesTitle"))
        .subtitle(ctx.t("apiServiceFeaturesDesc"))
        .row(ctx.item(
            "apiServiceTakeover",
            Some("apiServiceTakeoverDesc"),
            gateway_switch(ctx, GatewayFlag::Takeover),
        ))
        .row(ctx.item(
            "apiServiceJsonrpc",
            Some("apiServiceJsonrpcDesc"),
            gateway_switch(ctx, GatewayFlag::Jsonrpc),
        ))
        .row(ctx.item(
            "apiServiceApi",
            Some("apiServiceApiDesc"),
            gateway_switch(ctx, GatewayFlag::Api),
        ))
        .row(ctx.item(
            "apiServiceMcp",
            Some("apiServiceMcpDesc"),
            gateway_switch(ctx, GatewayFlag::Mcp),
        ))
        .row(ctx.item(
            "apiServiceCorsAllowAll",
            Some("apiServiceCorsAllowAllDesc"),
            gateway_switch(ctx, GatewayFlag::Cors),
        ))
}

#[derive(Clone, Copy)]
enum GatewayFlag {
    Takeover,
    Jsonrpc,
    Api,
    Mcp,
    Cors,
    Lan,
}

fn gateway_switch(ctx: &SectionContext, flag: GatewayFlag) -> Control {
    let get = ctx.store();
    let set = ctx.store();
    Control::switch(
        move |cx: &App| {
            let gateway = get.read(cx).gateway();
            match flag {
                GatewayFlag::Takeover => gateway.takeover_enabled,
                GatewayFlag::Jsonrpc => gateway.jsonrpc_enabled,
                GatewayFlag::Api => gateway.api_enabled,
                GatewayFlag::Mcp => gateway.mcp_enabled,
                GatewayFlag::Cors => gateway.cors_enabled,
                GatewayFlag::Lan => gateway.lan_enabled,
            }
        },
        move |value, cx: &mut App| {
            let mut patch = GatewayPatchParams::default();
            match flag {
                GatewayFlag::Takeover => patch.takeover_enabled = Some(value),
                GatewayFlag::Jsonrpc => patch.jsonrpc_enabled = Some(value),
                GatewayFlag::Api => patch.api_enabled = Some(value),
                GatewayFlag::Mcp => patch.mcp_enabled = Some(value),
                GatewayFlag::Cors => patch.cors_enabled = Some(value),
                GatewayFlag::Lan => patch.lan_enabled = Some(value),
            }
            set.update(cx, |store, cx| store.patch_gateway(patch, cx));
        },
    )
}

struct TokenSlot {
    input: Entity<InputState>,
    last_synced: SharedString,
    _subscription: gpui::Subscription,
}

/// 令牌：可见可编辑（回车/失焦提交自定义值）+ 复制 / 生成 / 清空。
/// 文本经 `agent.gateway.revealToken` 按需读取，不走快照。
fn token_field(ctx: &SectionContext) -> Control {
    let store = ctx.store();
    let generate = ctx.t("apiServiceTokenGenerate");
    let clear = ctx.t("apiServiceTokenClear");
    let copy = ctx.t("apiServiceCopy");
    let copied = ctx.t("apiServiceCopied");
    let placeholder = ctx.t("proxyNotConfigured");
    Control::custom(move |disabled, key, window: &mut Window, cx: &mut App| {
        let theme = active_theme(cx);
        let tokens = theme.tokens().clone();
        let icon_size = theme.extended().icon.md;
        let snapshot = store.read(cx);
        let token: SharedString = snapshot
            .transient("gateway_user_token")
            .and_then(serde_json::Value::as_str)
            .map(|token| SharedString::from(token.to_owned()))
            .unwrap_or_default();
        let needs_reveal = snapshot.gateway_token_needs_reveal();
        let busy = snapshot.is_busy("gateway") || snapshot.is_busy("gatewayToken");
        let regenerating = snapshot.is_busy_tagged("gateway", "regenerateToken");
        let clearing = snapshot.is_busy_tagged("gateway", "clearToken");
        let just_copied = snapshot
            .transient("gateway_token_copied")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if needs_reveal && !busy {
            store.update(cx, |store, cx| store.reveal_gateway_token(cx));
        }

        let slot = window.use_keyed_state(SharedString::from(format!("{key}-token")), cx, {
            let store = store.clone();
            let token = token.clone();
            let placeholder = placeholder.clone();
            move |window, cx| {
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .default_value(token.clone())
                        .placeholder(placeholder)
                });
                let _subscription = cx.subscribe(
                    &input,
                    move |slot: &mut TokenSlot, input, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                            let value =
                                SharedString::from(input.read(cx).value().trim().to_owned());
                            if value != slot.last_synced {
                                slot.last_synced = value.clone();
                                store.update(cx, |store, cx| {
                                    store.patch_gateway(
                                        GatewayPatchParams {
                                            user_token: Some(value.to_string()),
                                            ..Default::default()
                                        },
                                        cx,
                                    );
                                });
                            }
                        }
                    },
                );
                TokenSlot {
                    input,
                    last_synced: token,
                    _subscription,
                }
            }
        });
        slot.update(cx, |slot, cx| {
            if slot.last_synced != token {
                slot.last_synced = token.clone();
                slot.input
                    .update(cx, |input, cx| input.set_value(token.clone(), window, cx));
            }
        });
        let input = slot.read(cx).input.clone();

        let copy_store = store.clone();
        let copy_token = token.clone();
        let generate_store = store.clone();
        let clear_store = store.clone();
        let copy_tooltip = if just_copied {
            copied.clone()
        } else {
            copy.clone()
        };
        let generate_tooltip = generate.clone();
        let clear_tooltip = clear.clone();
        h_flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(tokens.spacing.sm)
            .child(
                Input::new(&input)
                    .control(cx)
                    .w(px(INPUT_WIDTH))
                    .disabled(disabled || busy),
            )
            .child(
                icon_button(
                    "api-token-copy",
                    copy.clone(),
                    Icon::new(FluxIcon::Copy).size(icon_size),
                    ButtonVariant::Secondary,
                    cx,
                )
                .disabled(token.is_empty())
                .tooltip(move |window, cx| Tooltip::new(copy_tooltip.clone()).build(window, cx))
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy_token.to_string()));
                    copy_store.update(cx, |store, cx| {
                        store.set_transient("gateway_token_copied", serde_json::json!(true), cx);
                    });
                    let reset = copy_store.clone();
                    cx.spawn(async move |cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(2))
                            .await;
                        reset.update(cx, |store, cx| {
                            store.set_transient(
                                "gateway_token_copied",
                                serde_json::json!(false),
                                cx,
                            );
                        });
                    })
                    .detach();
                }),
            )
            .child(
                loading_icon_button(
                    "api-token-generate",
                    generate.clone(),
                    Icon::new(FluxIcon::RotateCw).size(icon_size),
                    ButtonVariant::Secondary,
                    regenerating,
                    cx,
                )
                .disabled(disabled || busy)
                .tooltip(move |window, cx| Tooltip::new(generate_tooltip.clone()).build(window, cx))
                .on_click(move |_, _, cx| {
                    generate_store.update(cx, |store, cx| {
                        store.patch_gateway(
                            GatewayPatchParams {
                                regenerate_user_token: true,
                                ..Default::default()
                            },
                            cx,
                        );
                        store.tag_busy("gateway", "regenerateToken");
                    });
                }),
            )
            .child(
                loading_icon_button(
                    "api-token-clear",
                    clear.clone(),
                    Icon::new(FluxIcon::Trash2)
                        .size(icon_size)
                        .text_color(tokens.colors.destructive),
                    ButtonVariant::Secondary,
                    clearing,
                    cx,
                )
                .disabled(disabled || busy || token.is_empty())
                .tooltip(move |window, cx| Tooltip::new(clear_tooltip.clone()).build(window, cx))
                .on_click(move |_, _, cx| {
                    clear_store.update(cx, |store, cx| {
                        store.patch_gateway(
                            GatewayPatchParams {
                                user_token: Some(String::new()),
                                ..Default::default()
                            },
                            cx,
                        );
                        store.tag_busy("gateway", "clearToken");
                    });
                }),
            )
    })
}
