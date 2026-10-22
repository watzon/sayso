//! Settings › General: the interface language, your name, launch at login,
//! the Dock icon, updates, config problems, About.

use super::kit::{self, group, row};
use crate::model::AppModel;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::config::ConfigProblem;
use sayso_platform::LoginItemState;
use sayso_ui::ActivePaper;
use sayso_ui::a11y::{LiveStatus, Pressable, Role};
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::text;
use sayso_i18n::t;
use sayso_ui::assets::Icon;
use crate::hub::pages::kit::MenuItem;

pub struct GeneralSettings {
    model: Entity<AppModel>,
    /// The login item state. The system call for it takes more than 100 ms,
    /// so the page reads it when the state can change, not in each frame.
    login_state: LoginItemState,
    /// A login item error from the last toggle.
    login_error: Option<String>,
    name: Entity<InputState>,
    /// The Language list is open.
    language_menu: bool,
    /// The locale of the text in `name`'s placeholder.
    locale: String,
    _name_sub: Subscription,
    _activation: Subscription,
}

impl GeneralSettings {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let current = model.read(cx).config.general.name().unwrap_or_default().to_string();
        // The placeholder shows what Home uses when the field is empty.
        let placeholder = super::super::pages::kit::first_name().unwrap_or_else(|| t!("settings.general.name_placeholder"));
        let name = crate::widgets::input_state(&placeholder, &current, window, cx);
        let sub = cx.subscribe_in(&name, window, |this, state, ev: &InputEvent, _, cx| {
            if !matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                return;
            }
            let value = state.read(cx).value().trim().to_string();
            let v = (!value.is_empty()).then_some(value);
            if this.model.read(cx).config.general.name.as_deref().map(str::trim) != v.as_deref() {
                this.model.update(cx, |m, cx| m.edit_config(cx, |c| c.general.name = v));
            }
        });
        // The user approves the login item in System Settings, then comes back.
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.refresh_login_state(cx);
            }
        });
        let login_state = model.read(cx).login_item_state();
        Self { model, login_state, login_error: None, name, language_menu: false, locale: sayso_i18n::locale(), _name_sub: sub, _activation: activation }
    }

    fn refresh_login_state(&mut self, cx: &mut Context<Self>) {
        let state = self.model.read(cx).login_item_state();
        if state != self.login_state {
            self.login_state = state;
            cx.notify();
        }
    }
}

/// The lines of an interface language menu, for Settings and onboarding.
/// Each language shows its name in that language, so a user who cannot read
/// the current language still finds their own.
pub(crate) struct InterfaceLanguages {
    /// The `general.language` setting of each line.
    pub codes: Vec<String>,
    pub items: Vec<MenuItem>,
    /// The text of the button: the current choice and the language "system" gives.
    pub label: String,
    pub detail: Option<String>,
}

pub(crate) fn interface_languages(setting: &str) -> InterfaceLanguages {
    let system = sayso_i18n::resolve(sayso_i18n::SYSTEM, &sayso_i18n::system_locales());
    let system_name = sayso_i18n::find(system).map_or("English", |l| l.name);
    // Each line: the setting, the name in its own language, and the name in
    // the interface language when that is another text.
    let mut choices: Vec<(String, String, Option<String>)> = vec![(sayso_i18n::SYSTEM.into(), t!("settings.general.language_system"), Some(system_name.into()))];
    choices.extend(sayso_i18n::available().map(|l| {
        let here = sayso_i18n::language_name(l.code);
        (l.code.to_string(), l.name.to_string(), (here.to_lowercase() != l.name.to_lowercase()).then_some(here))
    }));
    if sayso_i18n::HAS_PSEUDO {
        choices.push((sayso_i18n::PSEUDO.into(), "Pseudo".into(), Some(sayso_i18n::PSEUDO.into())));
    }
    // An unknown setting acts as "system" (see `resolve`), so it shows so.
    let current = choices.iter().position(|(code, _, _)| code == setting).unwrap_or(0);
    let items = choices
        .iter()
        .enumerate()
        .map(|(i, (code, name, here))| {
            let mut item = MenuItem::new(name.clone()).selected(i == current);
            if let Some(here) = here {
                item = item.detail(here.clone());
            }
            if let Some(locale) = sayso_i18n::find(code) {
                item = item.locale(locale.code);
            }
            if i == 0 { item.rule_after() } else { item }
        })
        .collect();
    let label = choices[current].1.clone();
    InterfaceLanguages { codes: choices.into_iter().map(|(code, _, _)| code).collect(), items, label, detail: (current == 0).then(|| system_name.to_string()) }
}

impl GeneralSettings {
    /// The Language group. It shows only when there is more than English to
    /// choose, and in a debug build, which also has the pseudo-locale.
    fn language(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        if sayso_i18n::available().count() < 2 && !sayso_i18n::HAS_PSEUDO {
            return None;
        }
        let setting = self.model.read(cx).config.general.language.clone();
        let InterfaceLanguages { codes, items, label, detail } = interface_languages(&setting);
        let mut button = Button::new("interface-language", label.clone())
            .a11y_label(format!("{}, {label}", t!("settings.general.interface_language")))
            .expanded(self.language_menu);
        if let Some(detail) = detail {
            button = button.detail(detail);
        }

        let mut control = div().relative().flex_none().child(
            button.trailing_icon(Icon::ChevronDown).on_click(cx.listener(|this, _, _, cx| {
                this.language_menu = !this.language_menu;
                cx.notify();
            })),
        );
        if self.language_menu {
            let (pick, close) = (cx.entity(), cx.entity());
            control = control.child(div().absolute().top_full().right_0().w(px(280.)).child(crate::hub::pages::kit::menu_items(
                "interface-language-menu",
                items,
                move |i, _, cx| {
                    let Some(code) = codes.get(i).cloned() else { return };
                    pick.update(cx, |this, cx| {
                        this.language_menu = false;
                        this.model.update(cx, |m, cx| m.edit_config(cx, |c| c.general.language = code));
                        cx.notify();
                    });
                },
                move |_, cx| {
                    close.update(cx, |this, cx| {
                        this.language_menu = false;
                        cx.notify();
                    })
                },
                cx,
            )));
        }
        Some(group(&t!("settings.general.language"), cx).child(row(&t!("settings.general.interface_language"), &t!("settings.general.interface_language_detail"), control, cx)))
    }

    /// The Updates group: the status row and the two switches.
    fn updates(&self, cx: &mut Context<Self>) -> Div {
        use crate::update_ui::{Action, Tone};
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        if !m.updates_enabled() {
            return group(&t!("settings.general.updates"), cx).child(row(
                &t!("settings.general.no_self_update"),
                &t!("settings.general.no_self_update_detail", version = sayso_update::VERSION),
                div(),
                cx,
            ));
        }
        let v = crate::update_ui::view(&m.update);
        let (check, automatic) = (m.config.updates.check, m.config.updates.automatic);
        let checking = m.update == sayso_update::Status::Checking;
        let can_restart = m.can_restart_to_update();
        let dot = match v.tone {
            Tone::Accent => Some(c.accent),
            Tone::Danger => Some(c.danger),
            Tone::Plain => None,
        };
        let mut control = div().flex().flex_none().items_center().gap(px(12.));
        if let Some(progress) = v.progress {
            control = control.child(div().w(px(180.)).child(Progress::new(progress).label(v.title.clone())));
        }
        if checking {
            control = control.child(Button::new("update-action", t!("settings.general.checking")).small().disabled(true));
        }
        if let Some((label, action)) = v.action {
            let model = self.model.clone();
            let button = Button::new("update-action", label).small().on_click(move |_, _, cx| crate::update_ui::perform(action, &model, cx));
            control = control.child(match action {
                Action::Install => button.primary(),
                Action::Restart => button.primary().disabled(!can_restart),
                Action::Cancel => button.ghost(),
                _ => button,
            });
        }
        let detail = if v.action.is_some_and(|(_, a)| a == Action::Restart) && !can_restart {
            t!("settings.general.restart_after_dictation")
        } else {
            v.detail
        };
        let notes_url = v.notes_url;
        let whats_new = notes_url.is_none() && crate::whats_new::current().is_some();
        let status_row = div()
            .flex()
            .items_center()
            .w_full()
            .gap(px(24.))
            .py(px(14.))
            .border_b_1()
            .border_color(c.rule)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        // The title changes with the update state, so it is live.
                        LiveStatus::new("update-status", v.title.clone())
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .when_some(dot, |d, color| d.child(kit::halo_dot(8., color, 3., 0.16)))
                            .child(text::ui(v.title, 15., FontWeight::SEMIBOLD, c.ink).line_height(px(18.))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .child(text::ui(detail, 13., FontWeight::NORMAL, c.graphite).line_height(px(16.)))
                            .when_some(notes_url, |d, url| {
                                d.child(
                                    Pressable::new("update-notes")
                                        .a11y_role(Role::Link)
                                        .label(t!("settings.general.whats_new"))
                                        .cursor_pointer()
                                        .on_click(move |_, _, _| crate::shell::open(&url))
                                        .child(link_text(t!("settings.general.whats_new"), c.accent)),
                                )
                            })
                            // The notes of this version, when no newer version has the link.
                            .when(whats_new, |d| {
                                let model = self.model.clone();
                                d.child(
                                    Pressable::new("update-whats-new")
                                        .label(t!("settings.general.whats_new"))
                                        .cursor_pointer()
                                        .on_click(move |_, _, cx| crate::whats_new::open(&model, cx))
                                        .child(link_text(t!("settings.general.whats_new"), c.accent)),
                                )
                            }),
                    ),
            )
            .child(control);
        let model = self.model.clone();
        let check_switch = Switch::new("update-check-switch", check)
            .label(t!("settings.general.check_updates"))
            .on_toggle(move |on, _, cx| model.update(cx, |m, cx| m.edit_config(cx, |c| c.updates.check = on)));
        let model = self.model.clone();
        let automatic_switch = Switch::new("update-automatic-switch", automatic)
            .label(t!("settings.general.install_automatically"))
            .on_toggle(move |on, _, cx| model.update(cx, |m, cx| m.edit_config(cx, |c| c.updates.automatic = on)));
        group(&t!("settings.general.updates"), cx)
            .child(status_row)
            .child(row(
                &t!("settings.general.check_updates"),
                &t!("settings.general.check_updates_detail"),
                check_switch,
                cx,
            ))
            // Only macOS installs an update by itself now.
            .when(cfg!(target_os = "macos"), |g| {
                g.child(row(
                    &t!("settings.general.install_automatically"),
                    &t!("settings.general.install_automatically_detail"),
                    automatic_switch,
                    cx,
                ))
            })
    }
}

impl Render for GeneralSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The input keeps its placeholder, so it changes with the language.
        if self.locale != sayso_i18n::locale() {
            self.locale = sayso_i18n::locale();
            let placeholder = super::super::pages::kit::first_name().unwrap_or_else(|| t!("settings.general.name_placeholder"));
            self.name.update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
        }
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let launch = m.config.general.launch_at_login;
        let dock = m.config.general.dock_icon_with_hub;
        let state = self.login_state;
        let issues = m.config_issues.clone();
        let path = m.config_path_display();
        let (engine_title, engine_detail, engine_color) = m.engine_summary();

        let launch_control = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .when(launch && state == LoginItemState::RequiresApproval, |d| {
                d.child(Button::new("approve-login", t!("settings.general.approve_login_item")).small().on_click(|_, _, _| {
                    crate::shell::open("x-apple.systempreferences:com.apple.LoginItems-Settings.extension");
                }))
            })
            .child(Switch::new("launch-at-login", launch).label(t!("settings.general.launch_at_login")).on_toggle({
                let view = cx.entity().downgrade();
                move |on, _, cx| {
                let _ = view.update(cx, |this, cx| {
                this.model.update(cx, |m, cx| m.edit_config(cx, |c| c.general.launch_at_login = on));
                this.refresh_login_state(cx);
                this.login_error = match (on, this.login_state) {
                    (true, LoginItemState::Disabled) if cfg!(target_os = "macos") => {
                        Some(t!("settings.general.login_item_failed_macos"))
                    }
                    (true, LoginItemState::Disabled) => {
                        Some(t!("settings.general.login_item_failed_windows"))
                    }
                    _ => None,
                };
                cx.notify();
                });
            }}));
        let launch_desc = match (launch, state) {
            (true, LoginItemState::RequiresApproval) => t!("settings.general.launch_needs_approval"),
            _ => t!(crate::shell::os_text!("settings.general.launch_detail_mac", "settings.general.launch_detail_windows", "settings.general.launch_detail_other")),
        };

        let model = self.model.clone();
        let dock_switch = Switch::new("dock-icon", dock).label(t!("settings.general.dock_icon")).on_toggle(move |on, _, cx| {
            model.update(cx, |m, cx| m.edit_config(cx, |c| c.general.dock_icon_with_hub = on));
            // The Hub is open now, so apply the change at once.
            crate::shell::set_dock_icon_visible(on);
        });

        let name_field = div()
            .flex()
            .items_center()
            .w(px(220.))
            .h(px(36.))
            .px(px(12.))
            .rounded(px(9.))
            .debossed(&c)
            .text_size(px(14.))
            .child(Input::new(&self.name).aria_label(t!("settings.general.name")).appearance(false).w_full());
        let you = group(&t!("settings.general.you"), cx).child(row(
            &t!("settings.general.name"),
            &t!(crate::shell::os_text!("settings.general.name_detail_mac", "settings.general.name_detail_windows", "settings.general.name_detail_other")),
            name_field,
            cx,
        ));

        let mut startup = group(&t!("settings.general.startup"), cx)
            .child(row(&t!("settings.general.launch_at_login"), &launch_desc, launch_control, cx))
            // Only macOS has a Dock.
            .when(cfg!(target_os = "macos"), |g| {
                g.child(row(&t!("settings.general.dock_icon"), &t!("settings.general.dock_icon_detail"), dock_switch, cx))
            });
        if let Some(e) = &self.login_error {
            startup = startup.child(kit::banner(BannerKind::Warning, e.clone(), cx));
        }

        let mut body = kit::body().children(self.language(cx)).child(you).child(startup);

        if !issues.is_empty() {
            let mut list = div().flex().flex_col().gap(px(6.));
            for (i, issue) in issues.iter().enumerate() {
                list = list.child(
                    div()
                        .id(("config-issue", i))
                        .flex()
                        .gap(px(8.))
                        .child(text::mono(issue.field.clone(), 12., c.danger).flex_none())
                        .child(text::ui(issue_text(&issue.problem), 13., FontWeight::NORMAL, c.ink).line_height(px(18.))),
                );
            }
            let model = self.model.clone();
            body = body.child(
                group(&t!("settings.general.config_file"), cx).child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .py(px(8.))
                        .child(super::kit::notice(
                            BannerKind::Warning,
                            t!("settings.general.config_problems", count = issues.len()), cx))
                        .child(list.px(px(4.)))
                        .child(
                            div().flex().child(Button::new("open-config-issues", t!("settings.general.open_config_file")).small().on_click(move |_, _, cx| {
                                model.read(cx).reveal_config_file();
                            })),
                        ),
                ),
            );
        }

        body = body.child(self.updates(cx));

        let about = group(&t!("settings.general.about"), cx)
            .child(row(
                "Sayso",
                &t!("settings.general.version", version = env!("CARGO_PKG_VERSION")),
                text::ui(t!("settings.general.tagline", os = crate::shell::OS_NAME), 13., FontWeight::NORMAL, c.graphite),
                cx,
            ))
            .child(row(
                &t!("settings.general.speech_engine"),
                &engine_detail,
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(kit::halo_dot(8., engine_color(&c), 3., 0.16))
                    .child(text::ui(engine_title, 13., FontWeight::SEMIBOLD, c.ink)),
                cx,
            ))
            .child(row(&t!("settings.general.config_file"), &path, div(), cx));
        body = body.child(about);

        let title = t!("settings.general.title");
        let subtitle = t!(crate::shell::os_text!("settings.general.subtitle_mac", "settings.general.subtitle_other"));
        kit::page("general-page", &title, &subtitle, body, cx)
    }
}


/// The text of a problem in config.toml. The text of the TOML parser and of
/// the system stays as it is.
fn issue_text(problem: &ConfigProblem) -> String {
    match problem {
        ConfigProblem::Unreadable { path, error } => t!("settings.general.issue.unreadable", path = path, error = error),
        ConfigProblem::Syntax(message) => message.clone(),
        ConfigProblem::BadSection(message) => t!("settings.general.issue.bad_section", message = message),
        ConfigProblem::VolumeRange => t!("settings.general.issue.volume_range"),
        ConfigProblem::AiTimeoutTooShort => t!("settings.general.issue.ai_timeout_too_short"),
        ConfigProblem::PushToTalkIsToggle => t!("settings.general.issue.push_to_talk_is_toggle"),
        ConfigProblem::HotkeyConflictsWithToggle(hotkey) => super::recorder::Note::UsedBy(*hotkey, super::recorder::Slot::Toggle).text(),
        ConfigProblem::UnknownProvider(id) => t!("settings.general.issue.unknown_provider", id = id),
        ConfigProblem::UnknownModel(id) => t!("settings.general.issue.unknown_model", id = id),
        ConfigProblem::NotAStreamingModel(id) => t!("settings.general.issue.not_a_streaming_model", id = id),
        ConfigProblem::UnknownInterfaceLanguage(code) => t!("settings.general.issue.unknown_interface_language", code = code),
        ConfigProblem::UnknownDictationLanguage(code) => t!("settings.general.issue.unknown_dictation_language", code = code),
        ConfigProblem::SpeechTimeoutTooShort => t!("settings.general.issue.speech_timeout_too_short"),
        ConfigProblem::BadProviderId(id) => t!("settings.general.issue.bad_provider_id", id = id),
        ConfigProblem::DuplicateProviderId(id) => t!("settings.general.issue.duplicate_provider_id", id = id),
        ConfigProblem::NeedsBaseUrl(id) => t!("settings.general.issue.needs_base_url", id = id),
    }
}

/// The text of a link inside a `Pressable`, which carries the name.
fn link_text(text: String, color: Hsla) -> Div {
    div().font_family(sayso_ui::fonts::UI).text_size(px(13.)).line_height(px(16.)).font_weight(FontWeight::SEMIBOLD).text_color(color).child(text)
}
