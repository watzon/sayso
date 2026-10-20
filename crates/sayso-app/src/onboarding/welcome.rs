//! Step 1: Welcome. It shows the interface language and the dictation
//! language from the system, and lets the user change both.

use super::OnboardingView;
use crate::hub::pages::kit;
use crate::hub::settings::general::{InterfaceLanguages, interface_languages};
use gpui_kit::*;
use sayso_core::languages;
use sayso_i18n::t;
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::ActivePaper;
use sayso_ui::components::ink_waveform;
use sayso_ui::text;

/// A language list of the welcome step.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LanguageMenu {
    Interface,
    Dictation,
}

/// A calm stroke for the hero: high in the middle, two smaller swells.
const HERO: [f32; 12] = [0.35, 0.55, 0.8, 0.62, 0.9, 1.0, 0.86, 0.66, 0.84, 0.6, 0.42, 0.3];

impl OnboardingView {
    pub(super) fn welcome(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let reduce = cx.paper().reduce_motion;
        let t = if reduce { 0.0 } else { self.started.elapsed().as_secs_f32() * 0.35 };
        if !reduce {
            window.request_animation_frame();
        }
        let levels: Vec<f32> = HERO.iter().enumerate().map(|(i, l)| l * (0.92 + 0.08 * (t * 2.0 + i as f32 * 0.7).sin())).collect();
        let droplet = |x: f32, y: f32, r: f32, a: f32| div().absolute().left(px(x)).top(px(y)).size(px(r * 2.)).rounded_full().bg(c.ink.opacity(a));
        let hero = div()
            .relative()
            .w(px(520.))
            .h(px(84.))
            .child(ink_waveform(levels, t, c.ink).size_full())
            .child(droplet(484., 21., 2.6, 0.4))
            .child(droplet(86., 60., 2.1, 0.3))
            .child(droplet(313., 74., 1.6, 0.3));
        let title = t!("onboarding.welcome.title");
        let fact = |label: String| {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().size(px(6.)).rounded_full().bg(c.ink))
                .child(text::ui(label, 14., FontWeight::NORMAL, c.ink).line_height(px(18.)))
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(px(28.))
            .px(px(96.))
            .pb(px(24.))
            .child(hero)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(14.))
                    .child(text::letterpress(title, 56., 59., 1, &c))
                    .child(
                        text::ui(
                            t!(crate::shell::os_text!("onboarding.welcome.lead_mac", "onboarding.welcome.lead_windows", "onboarding.welcome.lead_other")),
                            17.,
                            FontWeight::NORMAL,
                            c.graphite,
                        )
                        .line_height(px(26.))
                        .text_center()
                        .w(px(520.)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(28.))
                    .pt(px(8.))
                    .child(fact(t!(crate::shell::os_text!("onboarding.welcome.fact_engine_mac", "onboarding.welcome.fact_engine_windows", "onboarding.welcome.fact_engine_other"))))
                    .child(fact(t!("onboarding.welcome.fact_every_app")))
                    .child(fact(t!("onboarding.welcome.fact_setup_time"))),
            )
            .child(self.languages(cx))
            .into_any_element()
    }

    /// The interface language and the language the user speaks. A new
    /// dictation language also selects the model suggested for it.
    fn languages(&mut self, cx: &mut Context<Self>) -> Div {
        let config = &self.model.read(cx).config;
        let (setting, spoken) = (config.general.language.clone(), config.dictation.language.clone());
        let InterfaceLanguages { codes, items, label, detail } = interface_languages(&setting);
        let value = match &detail {
            Some(detail) => format!("{label}, {detail}"),
            None => label.clone(),
        };
        let mut button = Button::new("welcome-interface-language", label).a11y_label(t!(
            "onboarding.welcome.language_button",
            name = t!("onboarding.welcome.interface_language"),
            value = value
        ));
        if let Some(detail) = detail {
            button = button.detail(detail);
        }
        let interface = self.language_picker(
            LanguageMenu::Interface,
            t!("onboarding.welcome.interface_language"),
            button,
            items,
            move |code, m, cx| m.edit_config(cx, move |c| c.general.language = code),
            codes,
            cx,
        );

        let mut codes: Vec<String> = languages::all().iter().map(|c| c.to_string()).collect();
        codes.sort_by_cached_key(|code| languages::display(code));
        let items = codes.iter().map(|code| kit::MenuItem::new(languages::display(code)).selected(*code == spoken)).collect();
        let dictation = self.language_picker(
            LanguageMenu::Dictation,
            t!("onboarding.welcome.dictation_language"),
            Button::new("welcome-dictation-language", languages::display(&spoken)).a11y_label(t!(
                "onboarding.welcome.language_button",
                name = t!("onboarding.welcome.dictation_language"),
                value = languages::display(&spoken)
            )),
            items,
            move |code, m, cx| {
                let model = super::model_step::suggested(&code);
                m.edit_config(cx, move |c| {
                    c.dictation.language = code;
                    c.dictation.model = model;
                })
            },
            codes,
            cx,
        );
        div().flex().flex_wrap().justify_center().items_center().gap(px(24.)).child(interface).child(dictation)
    }

    #[allow(clippy::too_many_arguments)]
    fn language_picker(
        &mut self,
        which: LanguageMenu,
        label: String,
        button: Button,
        items: Vec<kit::MenuItem>,
        pick: impl Fn(String, &mut crate::model::AppModel, &mut Context<crate::model::AppModel>) + 'static,
        codes: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Div {
        let c = cx.paper().colors;
        let open = self.language_menu == Some(which);
        let mut control = div()
            .relative()
            .flex_none()
            // Esc closes the list.
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" && this.language_menu == Some(which) {
                    cx.stop_propagation();
                    this.language_menu = None;
                    cx.notify();
                }
            }))
            .child(button.trailing_icon(Icon::ChevronDown).expanded(open).on_click(cx.listener(move |this, _, _, cx| {
                this.language_menu = if this.language_menu == Some(which) { None } else { Some(which) };
                cx.notify();
            })));
        if open {
            let (picked, close) = (cx.entity(), cx.entity());
            control = control.child(div().absolute().top_full().left_0().w(px(260.)).child(kit::menu_items(
                match which {
                    LanguageMenu::Interface => "welcome-interface-language-menu",
                    LanguageMenu::Dictation => "welcome-dictation-language-menu",
                },
                items,
                move |i, _, cx| {
                    let Some(code) = codes.get(i).cloned() else { return };
                    picked.update(cx, |this, cx| {
                        this.language_menu = None;
                        this.model.update(cx, |m, cx| pick(code, m, cx));
                        cx.notify();
                    });
                },
                move |_, cx| {
                    close.update(cx, |this, cx| {
                        this.language_menu = None;
                        cx.notify();
                    })
                },
                cx,
            )));
        }
        div().flex().items_center().gap(px(10.)).child(text::ui(label, 13., FontWeight::NORMAL, c.graphite)).child(control)
    }
}
