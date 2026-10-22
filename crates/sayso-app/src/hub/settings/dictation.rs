//! Settings › Dictation: language, hotkeys, cancel, insertion, sounds, AI timeouts.

use super::kit::{self, banner, group, row};
use super::recorder::{self, Recorder, Slot};
use crate::model::AppModel;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::config::CancelMode;
use sayso_core::hotkey::{Hotkey, SoloModifier};
use sayso_i18n::t;
use sayso_platform::{AppInfo, Permission, PermissionState};
use sayso_ui::ActivePaper;
use sayso_ui::a11y::{Pressable, Role};
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::text;

pub struct DictationSettings {
    model: Entity<AppModel>,
    recorder: Recorder,
    /// Running apps while the "Add app" list is open.
    picker: Option<Vec<AppInfo>>,
    /// Volume while the slider is dragged, so the file is written once.
    volume: Option<f32>,
    /// The language list is open.
    language_menu: bool,
}

fn rec(v: &mut DictationSettings) -> &mut Recorder {
    &mut v.recorder
}

impl DictationSettings {
    pub fn new(model: Entity<AppModel>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self { model, recorder: Recorder::new(cx), picker: None, volume: None, language_menu: false }
    }

    fn start(&mut self, slot: Slot, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.clone();
        recorder::start(rec, self, &model, slot, window, cx);
    }

    /// The language of the dictation, limited to what the active model understands.
    fn language_group(&mut self, cx: &mut Context<Self>) -> Div {
        use sayso_core::languages;
        let m = self.model.read(cx);
        let active = m.active_model();
        let current = active.language_for(&m.config.dictation.language);
        let description = if active.is_multilingual() {
            t!("settings.dictation.language_detail_multi", model = active.name, languages = active.language_label().to_lowercase())
        } else {
            t!("settings.dictation.language_detail_single", model = active.name, language = languages::display(&current))
        };
        let mut codes: Vec<String> = active.languages.iter().map(|c| c.to_string()).collect();
        codes.sort_by_key(|code| languages::display(code));
        if active.is_multilingual() {
            codes.insert(0, languages::AUTO.to_string());
        }
        let items: Vec<(SharedString, bool)> = codes.iter().map(|code| (languages::display(code).into(), *code == current)).collect();

        let mut control = div().relative().flex_none().child(
            Button::new("language", languages::display(&current))
                .a11y_label(format!("{}, {}", t!("settings.dictation.dictation_language"), languages::display(&current)))
                .expanded(self.language_menu)
                .trailing_icon(Icon::ChevronDown)
                .disabled(codes.len() < 2)
                .on_click(cx.listener(
                |this, _, _, cx| {
                    this.language_menu = !this.language_menu;
                    cx.notify();
                },
            )),
        );
        if self.language_menu {
            let (pick, close) = (cx.entity(), cx.entity());
            control = control.child(div().absolute().top_full().right_0().w(px(240.)).child(crate::hub::pages::kit::menu(
                "language-menu",
                items,
                move |i, _, cx| {
                    let Some(code) = codes.get(i).cloned() else { return };
                    pick.update(cx, |this, cx| {
                        this.language_menu = false;
                        this.model.update(cx, |m, cx| m.edit_config(cx, |c| c.dictation.language = code));
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
        let spoken = self.model.read(cx).config.dictation.spoken_punctuation;
        let model = self.model.clone();
        let spoken_switch = Switch::new("spoken-punctuation", spoken).label(t!("settings.dictation.spoken_punctuation")).on_toggle(move |on, _, cx| {
            model.update(cx, |m, cx| m.edit_config(cx, |c| c.dictation.spoken_punctuation = on));
        });
        group(&t!("settings.dictation.language"), cx).child(kit::row_s(t!("settings.dictation.dictation_language"), description, control, cx)).child(row(
            &t!("settings.dictation.spoken_punctuation"),
            &t!("settings.dictation.spoken_punctuation_detail"),
            spoken_switch,
            cx,
        ))
    }

    fn hotkeys_group(&mut self, cx: &mut Context<Self>) -> Div {
        let m = self.model.read(cx);
        let config = m.config.clone();
        let toggle = config.hotkeys.toggle;
        let ptt = config.hotkeys.push_to_talk;
        let im_missing = (config.hotkeys.needs_input_monitoring() || self.recorder.window_only) && m.permission(Permission::InputMonitoring) != PermissionState::Granted;
        let mut conflicts = self.recorder.conflicts(m, toggle);
        conflicts.extend(self.recorder.conflicts(m, ptt));
        let start = |this: &mut Self, slot: Slot, window: &mut Window, cx: &mut Context<Self>| this.start(slot, window, cx);
        let cancel = |this: &mut Self, cx: &mut Context<Self>| {
            this.recorder.cancel();
            cx.notify();
        };
        let clear = |this: &mut Self, slot: Slot, cx: &mut Context<Self>| {
            this.model.update(cx, |m, cx| slot.apply(m, None, cx));
        };

        let toggle_field = kit::key_field(Slot::Toggle, &t!("settings.dictation.toggle"), toggle, &self.recorder, false, None, start, cancel, clear, cx);
        let right_option = (ptt.is_none() && !self.recorder.is_recording(Slot::PushToTalk)).then(|| {
            Button::new("ptt-right-option", t!("settings.dictation.use_right_option"))
                .small()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.model.update(cx, |m, cx| Slot::PushToTalk.apply(m, Some(Hotkey::Solo(SoloModifier::RightOption)), cx));
                }))
                .into_any_element()
        });
        let ptt_field = kit::key_field(Slot::PushToTalk, &t!("settings.dictation.push_to_talk"), ptt, &self.recorder, true, right_option, start, cancel, clear, cx);
        let paste_field = kit::key_field(Slot::PasteLast, &t!("settings.dictation.paste_last"), config.hotkeys.paste_last, &self.recorder, true, None, start, cancel, clear, cx);
        let cycle_field = kit::key_field(Slot::CycleStyle, &t!("settings.dictation.next_style"), config.hotkeys.cycle_style, &self.recorder, true, None, start, cancel, clear, cx);
        let incognito_field = kit::key_field(Slot::Incognito, &t!("settings.dictation.incognito"), config.hotkeys.incognito, &self.recorder, true, None, start, cancel, clear, cx);

        let cancel_index = if config.hotkeys.cancel == CancelMode::SingleEscape { 1 } else { 0 };
        let model = self.model.clone();
        let cancel_control = Segmented::new("cancel-mode", [t!("settings.dictation.cancel_double"), t!("settings.dictation.cancel_single")], cancel_index)
            .label(t!("settings.dictation.cancel"))
            .on_select(move |i, _, cx| {
            let mode = if i == 1 { CancelMode::SingleEscape } else { CancelMode::DoubleEscape };
            model.update(cx, |m, cx| m.edit_config(cx, |c| c.hotkeys.cancel = mode));
        });

        let model = self.model.clone();
        let take_escape_switch = Switch::new("take-escape", config.hotkeys.take_escape).label(t!("settings.dictation.take_escape")).on_toggle(move |on, _, cx| {
            model.update(cx, |m, cx| m.edit_config(cx, |c| c.hotkeys.take_escape = on));
        });

        let model = self.model.clone();
        let enter_switch = Switch::new("enter-sends", config.hotkeys.enter_sends).label(t!("settings.dictation.enter_sends")).on_toggle(move |on, _, cx| {
            model.update(cx, |m, cx| m.edit_config(cx, |c| c.hotkeys.enter_sends = on));
        });

        let mut g = group(&t!("settings.dictation.hotkeys"), cx)
            .child(row(&t!("settings.dictation.toggle"), &t!("settings.dictation.toggle_detail"), toggle_field, cx))
            .child(row(&t!("settings.dictation.push_to_talk"), &t!("settings.dictation.push_to_talk_detail"), ptt_field, cx));
        for c in &conflicts {
            g = g.child(banner(BannerKind::Warning, recorder::conflict_text(c, false), cx));
        }
        if im_missing {
            let note = if ptt.is_some() {
                t!(crate::shell::os_text!("settings.dictation.input_monitoring_note_mac", "settings.dictation.input_monitoring_note_other"))
            } else {
                t!(crate::shell::os_text!("onboarding.hotkeys.im_off_mac", "onboarding.hotkeys.im_off_other"))
            };
            g = g.child(
                div().py(px(8.)).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(div().flex_1().child(super::kit::notice(
                            BannerKind::Warning,
                            note,
                            cx)))
                        .child(Button::new("open-im", crate::shell::open_permission_settings()).small().on_click(cx.listener(|this, _, _, cx| {
                            this.model.update(cx, |m, _| m.open_permission_settings(Permission::InputMonitoring));
                        }))),
                ),
            );
        }
        if self.recorder.window_only && self.recorder.slot.is_some() {
            g = g.child(banner(
                BannerKind::Info,
                t!("settings.dictation.window_only_note"),
                cx,
            ));
        }
        if let Some(note) = &self.recorder.note {
            g = g.child(banner(BannerKind::Warning, note.text(), cx));
        }
        g = g.child(row(&t!("settings.dictation.cancel"), &t!("settings.dictation.cancel_detail"), cancel_control, cx));
        // The key sources on Linux cannot keep Esc or Enter from the app. For
        // Enter, it has the --push-to-talk-up-submit command instead.
        if cfg!(any(target_os = "macos", windows)) {
            g = g.child(row(&t!("settings.dictation.take_escape"), &t!("settings.dictation.take_escape_detail"), take_escape_switch, cx));
            g = g.child(row(&t!("settings.dictation.enter_sends"), &t!("settings.dictation.enter_sends_detail"), enter_switch, cx));
        }
        g.child(row(&t!("settings.dictation.paste_last"), &t!("settings.dictation.paste_last_detail"), paste_field, cx))
            .child(row(&t!("settings.dictation.next_style"), &t!("settings.dictation.next_style_detail"), cycle_field, cx))
            .child(row(&t!("settings.dictation.incognito"), &t!("settings.dictation.incognito_detail"), incognito_field, cx))
    }

    fn insertion_group(&mut self, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let restore = m.config.insertion.restore_clipboard;
        let trailing_space = m.config.insertion.trailing_space;
        let apps = m.config.insertion.type_in_apps.clone();
        let running = self.picker.clone().unwrap_or_else(|| m.running_apps());
        let names: Vec<(String, String, _)> =
            apps.iter().map(|b| (b.clone(), m.app_name_for(b, &running), m.icons.get(b))).collect();
        let model = self.model.clone();
        let restore_switch = Switch::new("restore-clipboard", restore).label(t!("settings.dictation.restore_clipboard")).on_toggle(move |on, _, cx| {
            model.update(cx, |m, cx| m.edit_config(cx, |c| c.insertion.restore_clipboard = on));
        });
        let model = self.model.clone();
        let space_switch = Switch::new("trailing-space", trailing_space).label(t!("settings.dictation.trailing_space")).on_toggle(move |on, _, cx| {
            model.update(cx, |m, cx| m.edit_config(cx, |c| c.insertion.trailing_space = on));
        });

        let mut chips = div().flex().flex_wrap().justify_end().gap(px(6.)).max_w(px(460.));
        for (bundle, name, app_icon) in names {
            let b = bundle.clone();
            // The id comes from the bundle id, so it survives a change of the list.
            chips = chips.child(
                div()
                    .id(SharedString::from(format!("app-{bundle}")))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(28.))
                    .pl(px(6.))
                    .pr(px(4.))
                    .rounded(px(7.))
                    .bg(c.sheet_raised)
                    .shadow(vec![
                        BoxShadow::new(px(0.), px(1.), c.highlight(1.0)).inset(),
                        BoxShadow::new(px(0.), px(1.), c.shadow(0.16)).blur_radius(px(2.)),
                    ])
                    .child(AppBadge::new(name.clone()).icon(app_icon).size(16.))
                    .child(text::ui(name.clone(), 13., FontWeight::NORMAL, c.ink))
                    .child(
                        Pressable::new(SharedString::from(format!("remove-app-{bundle}")))
                            .label(t!("settings.dictation.remove_app", name = name))
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(20.))
                            .rounded(px(5.))
                            .cursor_pointer()
                            .hover(|s| s.bg(c.deboss))
                            .child(icon(Icon::Close, 10., c.graphite))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let b = b.clone();
                                this.model.update(cx, |m, cx| m.edit_config(cx, |c| c.insertion.type_in_apps.retain(|x| *x != b)));
                            })),
                    ),
            );
        }
        let open = self.picker.is_some();
        chips = chips.child(
            kit::dashed("add-app", cx)
                .aria_expanded(open)
                .h(px(28.))
                .px(px(10.))
                .rounded(px(7.))
                .when(open, |d| d.bg(c.deboss.opacity(0.5)))
                .child(text::ui(if open { t!("settings.dictation.close_list") } else { t!("settings.dictation.add_app") }, 13., FontWeight::NORMAL, c.graphite))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.picker = if this.picker.is_some() { None } else { Some(this.model.read(cx).running_apps()) };
                    cx.notify();
                })),
        );

        let mut g = group(&t!("settings.dictation.inserting"), cx)
            .child(row(&t!("settings.dictation.restore_clipboard"), &t!("settings.dictation.restore_clipboard_detail"), restore_switch, cx))
            .child(row(&t!("settings.dictation.trailing_space"), &t!("settings.dictation.trailing_space_detail"), space_switch, cx))
            .child(row(&t!("settings.dictation.type_in_apps"), &t!("settings.dictation.type_in_apps_detail"), chips, cx));
        if let Some(list) = &self.picker {
            let candidates: Vec<AppInfo> = list.iter().filter(|a| !apps.contains(&a.bundle_id)).cloned().collect();
            let mut panel = div()
                .id("app-picker")
                .role(Role::Group)
                .aria_label(t!("settings.dictation.type_in_apps"))
                // Escape closes the list.
                .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                    if e.keystroke.key == "escape" {
                        cx.stop_propagation();
                        this.picker = None;
                        cx.notify();
                    }
                }))
                .flex()
                .flex_col()
                .max_h(px(240.))
                .overflow_y_scroll()
                .p(px(4.))
                .rounded(px(10.))
                .debossed(&c);
            if candidates.is_empty() {
                panel = panel.child(div().p(px(10.)).child(text::ui(
                    t!("settings.dictation.no_other_apps"),
                    13.,
                    FontWeight::NORMAL,
                    c.graphite,
                )));
            }
            let app_icons: Vec<_> = candidates.iter().map(|a| self.model.read(cx).icons.get(&a.bundle_id)).collect();
            for (app, app_icon) in candidates.into_iter().zip(app_icons) {
                let bundle = app.bundle_id.clone();
                panel = panel.child(
                    Pressable::new(SharedString::from(format!("pick-app-{}", app.bundle_id)))
                        .label(app.name.clone())
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .h(px(34.))
                        .px(px(10.))
                        .rounded(px(7.))
                        .cursor_pointer()
                        .hover(|s| s.bg(c.sheet_raised))
                        .child(AppBadge::new(app.name.clone()).icon(app_icon).size(18.))
                        .child(plain_text(app.name.clone(), 13., FontWeight::MEDIUM, c.ink))
                        .child(plain_text(app.bundle_id.clone(), 11., FontWeight::NORMAL, c.pencil).font_family(sayso_ui::fonts::MONO))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let b = bundle.clone();
                            this.picker = None;
                            this.model.update(cx, |m, cx| {
                                m.edit_config(cx, |c| {
                                    if !c.insertion.type_in_apps.contains(&b) {
                                        c.insertion.type_in_apps.push(b)
                                    }
                                })
                            });
                        })),
                );
            }
            g = g.child(div().py(px(8.)).child(panel));
        }
        g
    }

    fn sounds_group(&mut self, cx: &mut Context<Self>) -> Div {
        let s = self.model.read(cx).config.sounds.clone();
        let volume = self.volume.unwrap_or(s.volume);
        let model = self.model.clone();
        let enabled = Switch::new("sounds-on", s.enabled).label(t!("settings.dictation.sounds_row")).on_toggle(move |on, _, cx| {
            model.update(cx, |m, cx| m.edit_config(cx, |c| c.sounds.enabled = on));
        });
        let view = cx.entity().downgrade();
        let view2 = view.clone();
        let slider = kit::slider(
            "volume",
            &t!("settings.common.volume"),
            volume,
            160.,
            move |v, _, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.volume = Some((v * 20.).round() / 20.);
                    cx.notify();
                });
            },
            move |_, cx| {
                let _ = view2.update(cx, |this, cx| {
                    if let Some(v) = this.volume.take() {
                        this.model.update(cx, |m, cx| {
                            m.edit_config(cx, |c| c.sounds.volume = v);
                            m.preview_sound();
                        });
                    }
                });
            },
            cx,
        )
        .when(!s.enabled, |d| d.opacity(0.45));

        let events = [
            (t!("settings.dictation.sound_start"), s.start),
            (t!("settings.dictation.sound_stop"), s.stop),
            (t!("settings.dictation.sound_cancel"), s.cancel),
            (t!("settings.dictation.sound_insert"), s.insert),
        ];
        let mut chips = div().id("sound-events").role(Role::Group).aria_label(t!("settings.dictation.sound_events")).flex().flex_none().gap(px(6.));
        for (i, (label, on)) in events.into_iter().enumerate() {
            chips = chips.child(Chip::new(("sound-event", i), label, on).role(ChipRole::Filter).on_click(cx.listener(move |this, _, _, cx| {
                this.model.update(cx, |m, cx| {
                    m.edit_config(cx, |c| match i {
                        0 => c.sounds.start = !on,
                        1 => c.sounds.stop = !on,
                        2 => c.sounds.cancel = !on,
                        _ => c.sounds.insert = !on,
                    })
                });
            })));
        }
        group(&t!("settings.dictation.sounds"), cx)
            .child(row(
                &t!("settings.dictation.sounds_row"),
                &t!("settings.dictation.sounds_detail"),
                div().flex().items_center().gap(px(24.)).child(slider).child(enabled),
                cx,
            ))
            .child(row(&t!("settings.dictation.sound_events"), &t!("settings.dictation.sound_events_detail"), chips.when(!s.enabled, |d| d.opacity(0.45)), cx))
    }

    fn styles_group(&mut self, cx: &mut Context<Self>) -> Div {
        let on = self.model.read(cx).config.ai.style_rules.enabled;
        let model = self.model.clone();
        let sites = self.model.read(cx).services.platform.context.reads_page_url();
        let (title, description) = if sites {
            (
                t!("settings.dictation.styles_by_app_site"),
                t!("settings.dictation.styles_by_app_site_detail"),
            )
        } else {
            (
                t!("settings.dictation.styles_by_app"),
                t!("settings.dictation.styles_by_app_detail"),
            )
        };
        let switch = Switch::new("style-rules", on).label(title.clone()).on_toggle(move |on, _, cx| model.update(cx, |m, cx| m.set_style_rules(on, cx)));
        group(&t!("settings.dictation.styles"), cx).child(row(&title, &description, switch, cx))
    }

    fn ai_group(&mut self, cx: &mut Context<Self>) -> Div {
        let ai = self.model.read(cx).config.ai.clone();
        let http_s = ai.http_timeout_ms / 1000;
        let cli_s = ai.cli_timeout_ms / 1000;
        let m1 = self.model.clone();
        let m2 = self.model.clone();
        let http = kit::stepper("http-timeout", t!("settings.dictation.seconds", seconds = http_s), move |d, _, cx| {
            let v = (http_s as i64 + d as i64).clamp(1, 30) as u64 * 1000;
            m1.update(cx, |m, cx| m.edit_config(cx, |c| c.ai.http_timeout_ms = v));
        }, cx).label(t!("settings.dictation.ai_timeout"));
        let cli = kit::stepper("cli-timeout", t!("settings.dictation.seconds", seconds = cli_s), move |d, _, cx| {
            let v = (cli_s as i64 + d as i64).clamp(2, 60) as u64 * 1000;
            m2.update(cx, |m, cx| m.edit_config(cx, |c| c.ai.cli_timeout_ms = v));
        }, cx).label(t!("settings.dictation.ai_timeout_cli"));
        group(&t!("settings.dictation.ai"), cx)
            .child(row(&t!("settings.dictation.ai_timeout"), &t!("settings.dictation.ai_timeout_detail"), http, cx))
            .child(row(&t!("settings.dictation.ai_timeout_cli"), &t!("settings.dictation.ai_timeout_cli_detail"), cli, cx))
    }
}

impl Render for DictationSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = kit::body()
            .child(self.language_group(cx))
            .child(self.hotkeys_group(cx))
            .child(self.insertion_group(cx))
            .child(self.sounds_group(cx))
            .child(self.styles_group(cx))
            .child(self.ai_group(cx));
        div()
            // The key recorder takes focus while it listens.
            .size_full()
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                let model = this.model.clone();
                if recorder::on_key_down(rec, this, &model, e, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(kit::key_listener("dictation-keys", &self.recorder.focus))
            .child(kit::page("dictation-page", &t!("settings.dictation.title"), &t!("settings.dictation.subtitle"), body, cx))
    }
}

/// Text inside a control. It has no node: the control carries the name.
fn plain_text(text: impl Into<SharedString>, size: f32, weight: FontWeight, color: Hsla) -> Div {
    div().font_family(sayso_ui::fonts::UI).text_size(px(size)).font_weight(weight).text_color(color).child(text.into())
}
