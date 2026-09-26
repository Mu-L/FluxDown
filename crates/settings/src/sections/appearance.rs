//! 外观：语言、明暗模式、内置主题、强调色、界面缩放。
//!
//! 每个控件同时写入偏好（走 `agent.preferences.patch`）并立即通过主题 crate 生效；
//! 偏好快照回流时 app 调用 `fluxdown_ui_theme::apply_appearance_preferences` 幂等对齐。

use fluxdown_ui_theme::{
    AccentScheme, AppearancePreferences, BuiltinThemeId, COLOR_SCHEME_KEY, CUSTOM_COLOR_KEY,
    DARK_THEME_KEY, ExtendedTokens, LIGHT_THEME_KEY, THEME_MODE_KEY, ThemePreference, UI_SCALE_KEY,
    UI_SCALE_PERCENTS, active_theme, argb_color, color_argb, foreground_for, set_appearance,
    set_theme_preference, set_ui_scale,
};
use gpui::{
    App, AppContext as _, Entity, Hsla, InteractiveElement as _, IntoElement as _, ParentElement,
    Pixels, SharedString, StatefulInteractiveElement as _, Styled, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    Icon,
    color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState},
    h_flex,
    tooltip::Tooltip,
    v_flex,
};

use super::SectionContext;
use crate::ui::{Control, SettingsPage, SettingsSection, meta_text};
use crate::{component_locale, store::SettingsStore};
use fluxdown_ui_components::FluxIcon;

pub(crate) const LOCALE_KEY: &str = "general.locale";

const THEME_CARD_WIDTH: f32 = 120.;
const THEME_PREVIEW_HEIGHT: f32 = 52.;
const THEME_PREVIEW_SIDEBAR_WIDTH: f32 = 28.;
const THEME_PREVIEW_BAR_HEIGHT: f32 = 3.;
const COLOR_DOT_SIZE: f32 = 28.;

pub(crate) fn page(ctx: &SectionContext, _cx: &mut App) -> SettingsPage {
    SettingsPage::new(
        "appearance",
        ctx.t("settingsCatAppearance"),
        ctx.t("settingsCatAppearanceDesc"),
        FluxIcon::Palette,
    )
    .sections([
        SettingsSection::new().row(ctx.item("language", Some("languageDesc"), language_field(ctx))),
        SettingsSection::new()
            .title(ctx.t("settingsGroupTheme"))
            .row(ctx.item("themeMode", Some("themeModeDesc"), theme_mode_field(ctx)))
            .row(
                ctx.item(
                    "themeSelection",
                    Some("themeSelectionDesc"),
                    theme_cards_field(ctx),
                )
                .vertical(),
            )
            .row(
                ctx.item(
                    "themeColor",
                    Some("themeColorDesc"),
                    color_scheme_field(ctx),
                )
                .vertical(),
            ),
        SettingsSection::new()
            .title(ctx.t("settingsGroupInterface"))
            .row(ctx.item("uiScale", Some("uiScaleDesc"), ui_scale_field(ctx))),
    ])
}

fn language_field(ctx: &SectionContext) -> Control {
    let mut options = vec![(SharedString::from("system"), ctx.t("languageSystem"))];
    options.extend(ctx.translator.available_locales().iter().map(|locale| {
        (
            SharedString::from(locale.clone()),
            SharedString::from(ctx.translator.native_name_of(locale).to_owned()),
        )
    }));
    let store = ctx.store();
    let set_store = ctx.store();
    let translator = ctx.translator_entity.clone();
    Control::dropdown(
        options,
        move |cx: &App| SharedString::from(store.read(cx).pref_str(LOCALE_KEY, "system")),
        move |value: SharedString, cx: &mut App| {
            set_store.update(cx, |store, cx| {
                store.set_pref_str(LOCALE_KEY, value.to_string(), cx)
            });
            let target = if value.as_ref() == "system" {
                fluxdown_ui_i18n::system_locale()
            } else {
                value.to_string()
            };
            translator.update(cx, |translator, cx| {
                if translator.set_locale(&target) {
                    gpui_component::set_locale(component_locale(translator.locale()));
                    cx.notify();
                }
            });
        },
    )
}

fn theme_mode_field(ctx: &SectionContext) -> Control {
    let options = vec![
        (SharedString::from("system"), ctx.t("themeModeSystem")),
        (SharedString::from("light"), ctx.t("themeModeLight")),
        (SharedString::from("dark"), ctx.t("themeModeDark")),
    ];
    let store = ctx.store();
    Control::dropdown(
        options,
        move |cx: &App| {
            SharedString::from(match active_theme(cx).preference() {
                ThemePreference::System => "system",
                ThemePreference::Light => "light",
                ThemePreference::Dark => "dark",
            })
        },
        move |value: SharedString, cx: &mut App| {
            let preference = theme_preference(&value);
            store.update(cx, |store, cx| {
                store.set_pref_str(THEME_MODE_KEY, value.to_string(), cx)
            });
            set_theme_preference(preference, None, cx);
        },
    )
}

/// 偏好字符串 → 主题偏好；未知值按系统处理。
#[must_use]
pub fn theme_preference(value: &str) -> ThemePreference {
    match value {
        "light" => ThemePreference::Light,
        "dark" => ThemePreference::Dark,
        _ => ThemePreference::System,
    }
}

// ───────────────────────── 内置主题卡片 ─────────────────────────

/// 与 Flutter `_ThemeSelector` 一致：只展示与当前明暗模式同外观的预设卡片。
fn theme_cards_field(ctx: &SectionContext) -> Control {
    let store = ctx.store();
    let dark_label = ctx.t("themeDarkTheme");
    let light_label = ctx.t("themeLightTheme");
    let labels: Vec<(BuiltinThemeId, SharedString)> = BuiltinThemeId::ALL
        .into_iter()
        .map(|id| (id, ctx.t(id.label_key())))
        .collect();
    Control::custom(
        move |disabled: bool, _key: &SharedString, _window: &mut Window, cx: &mut App| {
            let state = active_theme(cx);
            let mode = state.mode();
            let selected = state.appearance().builtin_theme(mode);
            let tokens = state.tokens().clone();
            let extended = state.extended().clone();
            let group_label = if mode.is_dark() {
                dark_label.clone()
            } else {
                light_label.clone()
            };

            v_flex()
                .w_full()
                .gap(tokens.spacing.xs)
                .child(meta_text(cx).child(group_label))
                .child(h_flex().gap(tokens.spacing.sm).flex_wrap().children(
                    BuiltinThemeId::presets_for(mode).map(|id| {
                        let label = labels
                            .iter()
                            .find(|(candidate, _)| *candidate == id)
                            .map_or_else(
                                || SharedString::from(id.wire_name()),
                                |(_, label)| label.clone(),
                            );
                        theme_card(
                            id,
                            label,
                            id == selected,
                            disabled,
                            &tokens,
                            &extended,
                            store.clone(),
                        )
                    }),
                ))
                .into_any_element()
        },
    )
}

fn theme_card(
    id: BuiltinThemeId,
    label: SharedString,
    selected: bool,
    disabled: bool,
    tokens: &fluxdown_ui_theme::SemanticThemeTokens,
    extended: &ExtendedTokens,
    store: Entity<SettingsStore>,
) -> impl gpui::IntoElement {
    let colors = tokens.colors;
    let row_hover = extended.colors.row_hover;
    let check_size = extended.icon.sm;
    let preview = id.colors();
    let bar = |width: gpui::DefiniteLength, color: Hsla| {
        div()
            .w(width)
            .h(px(THEME_PREVIEW_BAR_HEIGHT))
            .rounded_full()
            .bg(color)
    };
    let preview_element = h_flex()
        .w_full()
        .h(px(THEME_PREVIEW_HEIGHT))
        .rounded(tokens.radius.md)
        .border_1()
        .border_color(preview.border)
        .bg(preview.background)
        .overflow_hidden()
        .child(
            v_flex()
                .w(px(THEME_PREVIEW_SIDEBAR_WIDTH))
                .h_full()
                .items_center()
                .justify_center()
                .gap(px(THEME_PREVIEW_BAR_HEIGHT))
                .bg(preview.surface)
                .child(bar(px(16.).into(), preview.primary))
                .child(bar(px(16.).into(), preview.muted_foreground.opacity(0.35)))
                .child(bar(px(16.).into(), preview.muted_foreground.opacity(0.35))),
        )
        .child(
            v_flex()
                .flex_1()
                .h_full()
                .p(tokens.spacing.xs)
                .justify_center()
                .gap(px(THEME_PREVIEW_BAR_HEIGHT))
                .child(bar(gpui::relative(1.), preview.foreground.opacity(0.4)))
                .child(bar(
                    gpui::relative(1.),
                    preview.muted_foreground.opacity(0.4),
                ))
                .child(bar(
                    gpui::relative(0.6),
                    preview.muted_foreground.opacity(0.4),
                )),
        );

    div()
        .id(SharedString::from(format!("theme-card-{}", id.wire_name())))
        .w(px(THEME_CARD_WIDTH))
        .p(tokens.spacing.sm)
        .rounded(tokens.radius.lg)
        .border_1()
        .border_color(if selected {
            colors.primary
        } else {
            colors.border
        })
        .bg(colors.surface)
        .when(!disabled, |this| {
            this.cursor_pointer()
                .when(!selected, |this| {
                    this.hover(move |style| style.bg(row_hover))
                })
                .on_click(move |_, _, cx| {
                    let mut appearance = *active_theme(cx).appearance();
                    let mode = active_theme(cx).mode();
                    if appearance.builtin_theme(mode) == id {
                        return;
                    }
                    appearance.set_builtin_theme(mode, id);
                    set_appearance(appearance, None, cx);
                    let key = if mode.is_dark() {
                        DARK_THEME_KEY
                    } else {
                        LIGHT_THEME_KEY
                    };
                    store.update(cx, |store, cx| store.set_pref_str(key, id.pref_value(), cx));
                })
        })
        .child(preview_element)
        .child(
            h_flex()
                .mt(tokens.spacing.xs)
                .justify_between()
                .items_center()
                .text_size(tokens.typography.xs.size)
                .line_height(tokens.typography.xs.line_height)
                .text_color(colors.foreground)
                .child(label)
                .when(selected, |this| {
                    this.child(
                        Icon::new(FluxIcon::Check)
                            .size(check_size)
                            .text_color(colors.primary),
                    )
                }),
        )
}

// ───────────────────────── 强调色 ─────────────────────────

struct CustomColorSlot {
    picker: Entity<ColorPickerState>,
    last_synced: u32,
    _subscription: Subscription,
}

/// 与 Flutter `_ColorSchemeSelector` 一致：4 个预设色点 + 自定义；选中自定义时展开取色器。
fn color_scheme_field(ctx: &SectionContext) -> Control {
    let store = ctx.store();
    let labels: Vec<(AccentScheme, SharedString)> = AccentScheme::ALL
        .into_iter()
        .map(|scheme| (scheme, ctx.t(scheme.label_key())))
        .collect();
    let custom_label = ctx.t("colorCustom");
    Control::custom(
        move |disabled: bool, _key: &SharedString, window: &mut Window, cx: &mut App| {
            let state = active_theme(cx);
            let appearance = *state.appearance();
            let tokens = state.tokens().clone();
            let icon_size = state.extended().icon.md;

            let dots = h_flex()
                .gap(tokens.spacing.sm)
                .flex_wrap()
                .children(labels.iter().map(|(scheme, label)| {
                    color_dot(
                        *scheme,
                        label.clone(),
                        appearance,
                        disabled,
                        &tokens,
                        icon_size,
                        store.clone(),
                    )
                }));

            let mut column = v_flex().w_full().gap(tokens.spacing.md).child(dots);
            if appearance.color_scheme == AccentScheme::Custom {
                column = column.child(custom_color_picker(
                    appearance.custom_color,
                    custom_label.clone(),
                    disabled,
                    store.clone(),
                    window,
                    cx,
                ));
            }
            column.into_any_element()
        },
    )
}

fn color_dot(
    scheme: AccentScheme,
    label: SharedString,
    appearance: AppearancePreferences,
    disabled: bool,
    tokens: &fluxdown_ui_theme::SemanticThemeTokens,
    icon_size: Pixels,
    store: Entity<SettingsStore>,
) -> impl gpui::IntoElement {
    let colors = tokens.colors;
    let selected = appearance.color_scheme == scheme;
    let color = scheme.color(appearance.custom_color);
    let icon: Option<Icon> = if selected {
        Some(FluxIcon::Check.into())
    } else if scheme == AccentScheme::Custom {
        Some(FluxIcon::Palette.into())
    } else {
        None
    };
    let tooltip_label = label;
    div()
        .id(SharedString::from(format!(
            "accent-dot-{}",
            scheme.wire_name()
        )))
        .size(px(COLOR_DOT_SIZE))
        .rounded_full()
        .bg(color)
        .border_2()
        .border_color(if selected { colors.foreground } else { color })
        .flex()
        .items_center()
        .justify_center()
        .tooltip(move |window, cx| Tooltip::new(tooltip_label.clone()).build(window, cx))
        .when(!disabled, |this| {
            this.cursor_pointer()
                .when(!selected, |this| {
                    this.hover(move |style| style.border_color(colors.muted_foreground))
                })
                .on_click(move |_, _, cx| {
                    let mut appearance = *active_theme(cx).appearance();
                    if appearance.color_scheme == scheme {
                        return;
                    }
                    appearance.color_scheme = scheme;
                    set_appearance(appearance, None, cx);
                    store.update(cx, |store, cx| {
                        store.set_pref_str(COLOR_SCHEME_KEY, scheme.wire_name(), cx);
                    });
                })
        })
        .when_some(icon, |this, icon| {
            this.child(icon.size(icon_size).text_color(foreground_for(color)))
        })
}

/// gpui-component 取色器：色板 + HSLA 滑块 + 十六进制输入，提交即生效并写入偏好。
fn custom_color_picker(
    custom_color: u32,
    label: SharedString,
    disabled: bool,
    store: Entity<SettingsStore>,
    window: &mut Window,
    cx: &mut App,
) -> impl gpui::IntoElement {
    let slot = window.use_keyed_state(SharedString::from("settings-accent-custom"), cx, {
        let store = store.clone();
        move |window, cx| {
            let picker = cx.new(|cx| {
                ColorPickerState::new(window, cx).default_value(argb_color(custom_color))
            });
            let _subscription = cx.subscribe(
                &picker,
                move |slot: &mut CustomColorSlot, _, event: &ColorPickerEvent, cx| {
                    let ColorPickerEvent::Change(Some(color)) = event else {
                        return;
                    };
                    let argb = color_argb(*color);
                    if argb == slot.last_synced {
                        return;
                    }
                    slot.last_synced = argb;
                    let mut appearance = *active_theme(cx).appearance();
                    appearance.color_scheme = AccentScheme::Custom;
                    appearance.custom_color = argb;
                    set_appearance(appearance, None, cx);
                    // 云同步目录（Flutter `Color.toARGB32()`）约定整数 ARGB。
                    store.update(cx, |store, cx| {
                        store.set_pref_i64(CUSTOM_COLOR_KEY, i64::from(argb), cx);
                    });
                },
            );
            CustomColorSlot {
                picker,
                last_synced: custom_color,
                _subscription,
            }
        }
    });
    slot.update(cx, |slot, cx| {
        if slot.last_synced != custom_color {
            slot.last_synced = custom_color;
            slot.picker.update(cx, |picker, cx| {
                picker.set_value(argb_color(custom_color), window, cx);
            });
        }
    });
    let picker = slot.read(cx).picker.clone();
    // 不传 featured colors：gpui-component 用 `color-{hex}` 作为色块 ElementId，且特色行与
    // 调色板行同处一个无 id 的父级；预设色（如 #3b82f6 / #22c55e）与调色板色重复时会产生
    // 重复的 a11y 节点 id，调试构建直接 panic。预设色已由上方色点提供，这里只保留调色板。
    div().when(disabled, |this| this.opacity(0.5)).child(
        ColorPicker::new(&picker)
            .label(label)
            .featured_colors(Vec::new()),
    )
}

// ───────────────────────── 界面缩放 ─────────────────────────

fn ui_scale_field(ctx: &SectionContext) -> Control {
    let options: Vec<(SharedString, SharedString)> = UI_SCALE_PERCENTS
        .iter()
        .map(|percent| {
            (
                SharedString::from(percent.to_string()),
                SharedString::from(format!("{percent}%")),
            )
        })
        .collect();
    let store = ctx.store();
    Control::dropdown(
        options,
        move |cx: &App| SharedString::from(active_theme(cx).ui_scale_percent().to_string()),
        move |value: SharedString, cx: &mut App| {
            let Ok(percent) = value.parse::<u16>() else {
                return;
            };
            set_ui_scale(percent, cx);
            let scale = active_theme(cx).appearance().ui_scale_pref_value();
            store.update(cx, |store, cx| {
                store.set_pref(UI_SCALE_KEY, serde_json::Value::from(scale), cx);
            });
        },
    )
}
