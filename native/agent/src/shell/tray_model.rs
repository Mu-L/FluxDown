//! 由 agent 快照推导托盘展示（可见性、文案、关机倒计时），只在输入变化时推送给宿主。

use std::sync::Arc;

use fluxdown_protocol::{AgentSnapshot, PowerStatusDto};
use fluxdown_ui_i18n::{I18nCatalog, Translator};
use serde_json::Value;

use super::{TrayModel, TrayPort};
use crate::event_hub::AgentEventHub;

const DEFAULT_TOOLTIP: &str = "FluxDown";

#[derive(Clone, Debug, PartialEq, Eq)]
struct Inputs {
    visible: bool,
    locale: Option<String>,
    power: PowerStatusDto,
}

impl Inputs {
    fn read(snapshot: &AgentSnapshot) -> Self {
        Self {
            visible: snapshot.shell.tray_available && snapshot.shell.resident,
            locale: snapshot
                .preferences
                .values
                .get("general.locale")
                .and_then(Value::as_str)
                .filter(|locale| *locale != "system")
                .map(str::to_owned),
            power: snapshot.power,
        }
    }
}

pub(super) struct TrayPresenter {
    port: Option<Arc<dyn TrayPort>>,
    translator: Option<Translator>,
    last: Option<Inputs>,
}

impl TrayPresenter {
    pub(super) fn new(port: Option<Arc<dyn TrayPort>>) -> Self {
        let translator = port
            .as_ref()
            .and_then(|_| match I18nCatalog::load_embedded() {
                Ok(catalog) => {
                    Some(Arc::new(catalog).translator(&fluxdown_ui_i18n::system_locale()))
                }
                Err(error) => {
                    tracing::warn!(error = %error, "tray translations unavailable");
                    None
                }
            });
        Self {
            port,
            translator,
            last: None,
        }
    }

    pub(super) fn update(&mut self, events: &AgentEventHub) {
        let (Some(port), Some(translator)) = (self.port.as_ref(), self.translator.as_mut()) else {
            return;
        };
        let inputs = events.inspect(Inputs::read);
        if self.last.as_ref() == Some(&inputs) {
            return;
        }
        let locale = inputs
            .locale
            .clone()
            .unwrap_or_else(fluxdown_ui_i18n::system_locale);
        translator.set_locale(&locale);
        port.apply(&build(translator, &inputs));
        self.last = Some(inputs);
    }
}

fn build(translator: &Translator, inputs: &Inputs) -> TrayModel {
    let tooltip = inputs.power.countdown_remaining_secs.map_or_else(
        || DEFAULT_TOOLTIP.to_owned(),
        |remaining| {
            translator.text_with(
                "shutdownCountdown",
                &[("time", &format_remaining(remaining))],
            )
        },
    );
    TrayModel {
        visible: inputs.visible,
        show_window: translator.text("trayShowWindow").to_owned(),
        pause_all: translator.text("pauseAll").to_owned(),
        resume_all: translator.text("resumeAll").to_owned(),
        cancel_shutdown: inputs
            .power
            .armed_delay_secs
            .map(|_| translator.text("trayCancelShutdown").to_owned()),
        quit: translator.text("trayExit").to_owned(),
        tooltip,
    }
}

/// `mm:ss`，与桌面状态栏 / Flutter `ShutdownService.remainingText` 一致。
fn format_remaining(total_secs: u64) -> String {
    format!("{:02}:{:02}", total_secs / 60, total_secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translator(locale: &str) -> Translator {
        Arc::new(I18nCatalog::load_embedded().expect("embedded catalog")).translator(locale)
    }

    #[test]
    fn countdown_drives_tooltip_and_cancel_item() {
        let inputs = Inputs {
            visible: true,
            locale: Some("zh".to_owned()),
            power: PowerStatusDto {
                armed_delay_secs: Some(60),
                countdown_remaining_secs: Some(65),
            },
        };
        let model = build(&translator("zh"), &inputs);
        assert!(model.tooltip.contains("01:05"), "{}", model.tooltip);
        assert!(model.cancel_shutdown.is_some());

        let idle = Inputs {
            power: PowerStatusDto::default(),
            ..inputs
        };
        let model = build(&translator("zh"), &idle);
        assert_eq!(model.tooltip, DEFAULT_TOOLTIP);
        assert_eq!(model.cancel_shutdown, None);
    }

    /// 查表缺键时 `Translator` 回退为键名；托盘文案必须全部存在于 en / zh 基线。
    #[test]
    fn menu_labels_exist_in_both_baselines() {
        let inputs = Inputs {
            visible: true,
            locale: None,
            power: PowerStatusDto {
                armed_delay_secs: Some(0),
                countdown_remaining_secs: Some(1),
            },
        };
        for locale in ["en", "zh"] {
            let model = build(&translator(locale), &inputs);
            for (label, key) in [
                (model.show_window.as_str(), "trayShowWindow"),
                (model.pause_all.as_str(), "pauseAll"),
                (model.resume_all.as_str(), "resumeAll"),
                (model.quit.as_str(), "trayExit"),
                (
                    model.cancel_shutdown.as_deref().unwrap_or_default(),
                    "trayCancelShutdown",
                ),
            ] {
                assert_ne!(label, key, "{locale} is missing {key}");
            }
            assert!(!model.tooltip.contains("shutdownCountdown"), "{locale}");
        }
    }
}
