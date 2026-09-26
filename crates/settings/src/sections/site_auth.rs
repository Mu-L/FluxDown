//! 已保存的站点 HTTP Basic 凭据管理（只列站点与用户名，可逐条删除或清空）。

use fluxdown_ui_components::{ButtonVariant, loading_button};
use fluxdown_ui_theme::active_theme;
use gpui::{App, IntoElement as _, ParentElement, SharedString, Styled};
use gpui_component::{h_flex, v_flex};

use super::SectionContext;
use crate::ui::{SettingsRow, SettingsSection, body_text, meta_text, row_loading_danger_button};

pub(crate) fn group(ctx: &SectionContext, cx: &mut App) -> SettingsSection {
    if ctx.store.read(cx).site_auth().is_empty()
        && ctx.store.read(cx).transient("site_auth_loaded").is_none()
    {
        ctx.store.update(cx, |store, cx| {
            store.set_transient("site_auth_loaded", serde_json::json!(true), cx);
            store.load_site_auth(cx);
        });
    }
    SettingsSection::new()
        .title(ctx.t("settingsSiteAuthTitle"))
        .subtitle(ctx.t("settingsSiteAuthDesc"))
        .row(list_item(ctx))
}

fn list_item(ctx: &SectionContext) -> SettingsRow {
    let store = ctx.store();
    let empty = ctx.t("settingsSiteAuthEmpty");
    let delete = ctx.t("settingsSiteAuthDelete");
    let clear_all = ctx.t("settingsSiteAuthClearAll");
    SettingsRow::custom(move |_disabled, _key, _window, cx: &mut App| {
        let theme = active_theme(cx);
        let tokens = theme.tokens();
        let extended = theme.extended().colors;
        let entries = store.read(cx).site_auth().to_vec();
        let busy = store.read(cx).is_busy("siteAuth");
        let clearing = store.read(cx).is_busy_tagged("siteAuth", "clearAll");
        let clear_store = store.clone();
        let mut column = v_flex().w_full().gap(tokens.spacing.xs);
        if entries.is_empty() {
            column = column.child(meta_text(cx).child(empty.clone()));
        }
        for entry in entries {
            let site = entry.site.clone();
            let delete_store = store.clone();
            let deleting = store.read(cx).is_busy_tagged("siteAuth", &site);
            column = column.child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap(tokens.spacing.md)
                    .py(tokens.spacing.xs)
                    .border_b_1()
                    .border_color(extended.hairline)
                    .child(
                        v_flex()
                            .gap(tokens.spacing.xxs)
                            .child(body_text(cx).child(SharedString::from(entry.site.clone())))
                            .child(meta_text(cx).child(SharedString::from(entry.user.clone()))),
                    )
                    .child(
                        row_loading_danger_button(
                            SharedString::from(format!("site-auth-delete-{}", entry.site)),
                            delete.clone(),
                            deleting,
                            cx,
                        )
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            let site = site.clone();
                            delete_store.update(cx, |store, cx| {
                                store.delete_site_auth(&site, cx);
                                store.tag_busy("siteAuth", site);
                            });
                        }),
                    ),
            );
        }
        column
            .child(
                h_flex().w_full().justify_end().child(
                    loading_button(
                        "site-auth-clear",
                        clear_all.clone(),
                        ButtonVariant::Secondary,
                        clearing,
                        cx,
                    )
                    .disabled(busy || store.read(cx).site_auth().is_empty())
                    .on_click(move |_, _, cx| {
                        clear_store.update(cx, |store, cx| {
                            store.clear_site_auth(cx);
                            store.tag_busy("siteAuth", "clearAll");
                        });
                    }),
                ),
            )
            .into_any_element()
    })
    .keywords([ctx.t("settingsSiteAuthTitle")])
}
