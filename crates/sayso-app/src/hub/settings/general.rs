//! Settings › General: your name, launch at login, the Dock icon, updates,
//! config problems, About.

use super::kit::{self, group, row};
use crate::model::AppModel;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::*;
use sayso_platform::LoginItemState;
use sayso_ui::ActivePaper;
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::text;

pub struct GeneralSettings {
    model: Entity<AppModel>,
    /// A login item error from the last toggle.
    login_error: Option<String>,
    name: Entity<InputState>,
    _name_sub: Subscription,
}

impl GeneralSettings {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let current = model.read(cx).config.general.name().unwrap_or_default().to_string();
        // The placeholder shows what Home uses when the field is empty.
        let placeholder = super::super::pages::kit::first_name().unwrap_or_else(|| "Your name".into());
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
        Self { model, login_error: None, name, _name_sub: sub }
    }
}

impl GeneralSettings {
    /// The Updates group: the status row and the two switches.
    fn updates(&self, cx: &mut Context<Self>) -> Div {
        use crate::update_ui::{Action, Tone};
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        if !m.updates_enabled() {
            return group("Updates", cx).child(row(
                "This build does not update itself",
                &format!("Version {}, built from source.", sayso_update::VERSION),
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
            control = control.child(div().w(px(180.)).child(Progress::new(progress)));
        }
        if checking {
            control = control.child(Button::new("update-action", "Checking…").small().disabled(true));
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
            "Finish the dictation first. Then Sayso closes and opens again.".to_string()
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
                        div()
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
                                    div()
                                        .id("update-notes")
                                        .cursor_pointer()
                                        .on_click(move |_, _, _| crate::shell::open(&url))
                                        .child(text::ui("See what is new", 13., FontWeight::SEMIBOLD, c.accent).line_height(px(16.))),
                                )
                            })
                            // The notes of this version, when no newer version has the link.
                            .when(whats_new, |d| {
                                let model = self.model.clone();
                                d.child(
                                    div()
                                        .id("update-whats-new")
                                        .cursor_pointer()
                                        .on_click(move |_, _, cx| crate::whats_new::open(&model, cx))
                                        .child(text::ui("See what is new", 13., FontWeight::SEMIBOLD, c.accent).line_height(px(16.))),
                                )
                            }),
                    ),
            )
            .child(control);
        let model = self.model.clone();
        let check_switch = Switch::new("update-check-switch", check)
            .on_toggle(move |on, _, cx| model.update(cx, |m, cx| m.edit_config(cx, |c| c.updates.check = on)));
        let model = self.model.clone();
        let automatic_switch = Switch::new("update-automatic-switch", automatic)
            .on_toggle(move |on, _, cx| model.update(cx, |m, cx| m.edit_config(cx, |c| c.updates.automatic = on)));
        group("Updates", cx)
            .child(status_row)
            .child(row(
                "Check for updates",
                "When Sayso starts, and one time each day. The check sends no data about you.",
                check_switch,
                cx,
            ))
            // Only macOS installs an update by itself now.
            .when(cfg!(target_os = "macos"), |g| {
                g.child(row(
                    "Install updates automatically",
                    "Sayso downloads each new version and installs it the next time Sayso starts.",
                    automatic_switch,
                    cx,
                ))
            })
    }
}

impl Render for GeneralSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let launch = m.config.general.launch_at_login;
        let dock = m.config.general.dock_icon_with_hub;
        let state = m.login_item_state();
        let issues = m.config_issues.clone();
        let path = m.config_path_display();
        let (engine_title, engine_detail, engine_color) = m.engine_summary();

        let launch_control = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .when(launch && state == LoginItemState::RequiresApproval, |d| {
                d.child(Button::new("approve-login", "Approve in System Settings").small().on_click(|_, _, _| {
                    crate::shell::open("x-apple.systempreferences:com.apple.LoginItems-Settings.extension");
                }))
            })
            .child(Switch::new("launch-at-login", launch).on_toggle({
                let view = cx.entity().downgrade();
                move |on, _, cx| {
                let _ = view.update(cx, |this, cx| {
                this.model.update(cx, |m, cx| m.edit_config(cx, |c| c.general.launch_at_login = on));
                let now = this.model.read(cx).login_item_state();
                this.login_error = match (on, now) {
                    (true, LoginItemState::Disabled) if cfg!(target_os = "macos") => {
                        Some("macOS did not add Sayso to the login items. Move Sayso to the Applications folder, then try again.".into())
                    }
                    (true, LoginItemState::Disabled) => {
                        Some("Windows did not add Sayso to the startup apps. Check Settings › Apps › Startup.".into())
                    }
                    _ => None,
                };
                cx.notify();
                });
            }}));
        let launch_desc = match (launch, state) {
            (true, LoginItemState::RequiresApproval) => "macOS needs your approval in Login Items before Sayso can start at login.",
            _ => crate::shell::os_text!("Start Sayso in the menu bar when you log in.", "Start Sayso in the taskbar when you sign in.", "Start Sayso in the background when you log in."),
        };

        let model = self.model.clone();
        let dock_switch = Switch::new("dock-icon", dock).on_toggle(move |on, _, cx| {
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
            .child(Input::new(&self.name).appearance(false).w_full());
        let you = group("You", cx).child(row(
            "Your name",
            crate::shell::os_text!("Home greets you with it. Leave it empty to use the first name of your Mac account.", "Home greets you with it. Leave it empty to use the first name of your Windows account.", "Home greets you with it. Leave it empty to use the name of your user account."),
            name_field,
            cx,
        ));

        let mut startup = group("Startup", cx)
            .child(row("Launch at login", launch_desc, launch_control, cx))
            // Only macOS has a Dock.
            .when(cfg!(target_os = "macos"), |g| {
                g.child(row("Show the Dock icon while the Hub is open", "At other times Sayso lives in the menu bar only.", dock_switch, cx))
            });
        if let Some(e) = &self.login_error {
            startup = startup.child(kit::banner(BannerKind::Warning, e.clone(), cx));
        }

        let mut body = kit::body().child(you).child(startup);

        if !issues.is_empty() {
            let mut list = div().flex().flex_col().gap(px(6.));
            for issue in &issues {
                list = list.child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(text::mono(issue.field.clone(), 12., c.danger).flex_none())
                        .child(text::ui(issue.message.clone(), 13., FontWeight::NORMAL, c.ink).line_height(px(18.))),
                );
            }
            let model = self.model.clone();
            body = body.child(
                group("Config file", cx).child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .py(px(8.))
                        .child(super::kit::notice(
                            BannerKind::Warning,
                            format!(
                                "config.toml has {} {}. Sayso uses the default for each value below until you fix the file.",
                                issues.len(),
                                if issues.len() == 1 { "problem" } else { "problems" }
                            ), cx))
                        .child(list.px(px(4.)))
                        .child(
                            div().flex().child(Button::new("open-config-issues", "Open config file").small().on_click(move |_, _, cx| {
                                model.read(cx).reveal_config_file();
                            })),
                        ),
                ),
            );
        }

        body = body.child(self.updates(cx));

        let about = group("About", cx)
            .child(row(
                "Sayso",
                &format!("Version {}", env!("CARGO_PKG_VERSION")),
                text::ui(format!("Local dictation for {}", crate::shell::OS_NAME), 13., FontWeight::NORMAL, c.graphite),
                cx,
            ))
            .child(row(
                "Speech engine",
                &engine_detail,
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(kit::halo_dot(8., engine_color(&c), 3., 0.16))
                    .child(text::ui(engine_title, 13., FontWeight::SEMIBOLD, c.ink)),
                cx,
            ))
            .child(row("Config file", &path, div(), cx));
        body = body.child(about);

        kit::page(
            "general-page",
            "General",
            crate::shell::os_text!("Your name, startup, the Dock icon, updates, and the config file.", "Your name, startup, updates, and the config file."),
            body,
            cx,
        )
    }
}

use gpui_kit::prelude::FluentBuilder as _;
