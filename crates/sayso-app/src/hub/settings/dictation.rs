//! Settings › Dictation: hotkeys, cancel, insertion, sounds, AI timeouts.

use super::kit::{self, banner, group, row};
use super::recorder::{self, Recorder, Slot};
use crate::model::AppModel;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::config::CancelMode;
use sayso_core::hotkey::{Hotkey, SoloModifier};
use sayso_platform::{AppInfo, Permission, PermissionState};
use sayso_ui::ActivePaper;
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
}

fn rec(v: &mut DictationSettings) -> &mut Recorder {
    &mut v.recorder
}

impl DictationSettings {
    pub fn new(model: Entity<AppModel>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self { model, recorder: Recorder::new(cx), picker: None, volume: None }
    }

    fn start(&mut self, slot: Slot, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.clone();
        recorder::start(rec, self, &model, slot, window, cx);
    }

    fn hotkeys_group(&mut self, cx: &mut Context<Self>) -> Div {
        let m = self.model.read(cx);
        let config = m.config.clone();
        let toggle = config.hotkeys.toggle;
        let ptt = config.hotkeys.push_to_talk;
        let im_missing = ptt.is_some() && m.permission(Permission::InputMonitoring) != PermissionState::Granted;
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

        let toggle_field = kit::key_field(Slot::Toggle, toggle, &self.recorder, false, None, start, cancel, clear, cx);
        let right_option = (ptt.is_none() && !self.recorder.is_recording(Slot::PushToTalk)).then(|| {
            Button::new("ptt-right-option", "Use Right Option")
                .small()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.model.update(cx, |m, cx| Slot::PushToTalk.apply(m, Some(Hotkey::Solo(SoloModifier::RightOption)), cx));
                }))
                .into_any_element()
        });
        let ptt_field = kit::key_field(Slot::PushToTalk, ptt, &self.recorder, true, right_option, start, cancel, clear, cx);
        let paste_field = kit::key_field(Slot::PasteLast, config.hotkeys.paste_last, &self.recorder, true, None, start, cancel, clear, cx);
        let cycle_field = kit::key_field(Slot::CycleStyle, config.hotkeys.cycle_style, &self.recorder, true, None, start, cancel, clear, cx);

        let cancel_index = if config.hotkeys.cancel == CancelMode::SingleEscape { 1 } else { 0 };
        let model = self.model.clone();
        let cancel_control = Segmented::new("cancel-mode", ["Double Esc", "Single Esc"], cancel_index).on_select(move |i, _, cx| {
            let mode = if i == 1 { CancelMode::SingleEscape } else { CancelMode::DoubleEscape };
            model.update(cx, |m, cx| m.edit_config(cx, |c| c.hotkeys.cancel = mode));
        });

        let mut g = group("Hotkeys", cx)
            .child(row("Start and stop dictation", "Press once to start, again to stop.", toggle_field, cx))
            .child(row("Push to talk", "Hold to record, release to insert. A single key such as Right Option works.", ptt_field, cx));
        for c in &conflicts {
            g = g.child(banner(BannerKind::Warning, recorder::conflict_text(c, false), cx));
        }
        if im_missing {
            g = g.child(
                div().py(px(8.)).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(div().flex_1().child(super::kit::notice(
                            BannerKind::Warning,
                            "Push to talk needs Input Monitoring. Turn on Sayso in Privacy and Security › Input Monitoring.", cx)))
                        .child(Button::new("open-im", "Open System Settings").small().on_click(cx.listener(|this, _, _, cx| {
                            this.model.update(cx, |m, _| m.open_permission_settings(Permission::InputMonitoring));
                        }))),
                ),
            );
        }
        if self.recorder.window_only && self.recorder.slot.is_some() {
            g = g.child(banner(
                BannerKind::Info,
                "Sayso cannot listen for keys in other apps without Input Monitoring. Press the keys while this window is in front. Single keys such as Right Option need Input Monitoring.",
                cx,
            ));
        }
        if let Some(note) = &self.recorder.note {
            g = g.child(banner(BannerKind::Warning, note.clone(), cx));
        }
        g.child(row("Cancel", "Only while recording.", cancel_control, cx))
            .child(row("Paste last text", "Pastes your last dictation again.", paste_field, cx))
            .child(row("Next style", "Switches to the next style.", cycle_field, cx))
    }

    fn insertion_group(&mut self, cx: &mut Context<Self>) -> Div {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let restore = m.config.insertion.restore_clipboard;
        let apps = m.config.insertion.type_in_apps.clone();
        let running = self.picker.clone().unwrap_or_else(|| m.running_apps());
        let names: Vec<(String, String, _)> =
            apps.iter().map(|b| (b.clone(), m.app_name_for(b, &running), m.icons.get(b))).collect();
        let model = self.model.clone();
        let restore_switch = Switch::new("restore-clipboard", restore).on_toggle(move |on, _, cx| {
            model.update(cx, |m, cx| m.edit_config(cx, |c| c.insertion.restore_clipboard = on));
        });

        let mut chips = div().flex().flex_wrap().justify_end().gap(px(6.)).max_w(px(460.));
        for (i, (bundle, name, app_icon)) in names.into_iter().enumerate() {
            let b = bundle.clone();
            chips = chips.child(
                div()
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
                    .child(text::ui(name, 13., FontWeight::NORMAL, c.ink))
                    .child(
                        div()
                            .id(("remove-app", i))
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
                .h(px(28.))
                .px(px(10.))
                .rounded(px(7.))
                .when(open, |d| d.bg(c.deboss.opacity(0.5)))
                .child(text::ui(if open { "Close list" } else { "Add app" }, 13., FontWeight::NORMAL, c.graphite))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.picker = if this.picker.is_some() { None } else { Some(this.model.read(cx).running_apps()) };
                    cx.notify();
                })),
        );

        let mut g = group("Inserting text", cx)
            .child(row(
                "Restore the clipboard after pasting",
                "Only if you did not copy something new in the meantime.",
                restore_switch,
                cx,
            ))
            .child(row("Type instead of paste in these apps", "For apps that block paste. Slower for long text.", chips, cx));
        if let Some(list) = &self.picker {
            let candidates: Vec<AppInfo> = list.iter().filter(|a| !apps.contains(&a.bundle_id)).cloned().collect();
            let mut panel = div()
                .id("app-picker")
                .flex()
                .flex_col()
                .max_h(px(240.))
                .overflow_y_scroll()
                .p(px(4.))
                .rounded(px(10.))
                .debossed(&c);
            if candidates.is_empty() {
                panel = panel.child(div().p(px(10.)).child(text::ui(
                    "No other apps are open. Open the app, then try again.",
                    13.,
                    FontWeight::NORMAL,
                    c.graphite,
                )));
            }
            let app_icons: Vec<_> = candidates.iter().map(|a| self.model.read(cx).icons.get(&a.bundle_id)).collect();
            for (i, (app, app_icon)) in candidates.into_iter().zip(app_icons).enumerate() {
                let bundle = app.bundle_id.clone();
                panel = panel.child(
                    div()
                        .id(("pick-app", i))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .h(px(34.))
                        .px(px(10.))
                        .rounded(px(7.))
                        .cursor_pointer()
                        .hover(|s| s.bg(c.sheet_raised))
                        .child(AppBadge::new(app.name.clone()).icon(app_icon).size(18.))
                        .child(text::ui(app.name.clone(), 13., FontWeight::MEDIUM, c.ink))
                        .child(text::mono(app.bundle_id.clone(), 11., c.pencil).font_weight(FontWeight::NORMAL))
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
        let enabled = Switch::new("sounds-on", s.enabled).on_toggle(move |on, _, cx| {
            model.update(cx, |m, cx| m.edit_config(cx, |c| c.sounds.enabled = on));
        });
        let view = cx.entity().downgrade();
        let view2 = view.clone();
        let slider = kit::slider(
            "volume",
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

        let events = [("Start", s.start), ("Stop", s.stop), ("Cancel", s.cancel), ("Insert", s.insert)];
        let mut chips = div().flex().flex_none().gap(px(6.));
        for (i, (label, on)) in events.into_iter().enumerate() {
            chips = chips.child(Chip::new(("sound-event", i), label, on).on_click(cx.listener(move |this, _, _, cx| {
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
        group("Sounds", cx)
            .child(row(
                "Sounds",
                "A soft pen tap when recording starts and stops.",
                div().flex().items_center().gap(px(24.)).child(slider).child(enabled),
                cx,
            ))
            .child(row("Play a sound for", "Choose the moments that make a sound.", chips.when(!s.enabled, |d| d.opacity(0.45)), cx))
    }

    fn ai_group(&mut self, cx: &mut Context<Self>) -> Div {
        let ai = self.model.read(cx).config.ai.clone();
        let http_s = ai.http_timeout_ms / 1000;
        let cli_s = ai.cli_timeout_ms / 1000;
        let m1 = self.model.clone();
        let m2 = self.model.clone();
        let http = kit::stepper("http-timeout", format!("{http_s} s"), move |d, _, cx| {
            let v = (http_s as i64 + d as i64).clamp(1, 30) as u64 * 1000;
            m1.update(cx, |m, cx| m.edit_config(cx, |c| c.ai.http_timeout_ms = v));
        }, cx);
        let cli = kit::stepper("cli-timeout", format!("{cli_s} s"), move |d, _, cx| {
            let v = (cli_s as i64 + d as i64).clamp(2, 60) as u64 * 1000;
            m2.update(cx, |m, cx| m.edit_config(cx, |c| c.ai.cli_timeout_ms = v));
        }, cx);
        group("AI", cx)
            .child(row("AI timeout", "After this, Sayso inserts the transcript without the style.", http, cx))
            .child(row("AI timeout for command-line tools", "Claude CLI and Codex CLI are slower, so they get more time.", cli, cx))
    }
}

impl Render for DictationSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = kit::body()
            .child(self.hotkeys_group(cx))
            .child(self.insertion_group(cx))
            .child(self.sounds_group(cx))
            .child(self.ai_group(cx));
        div()
            .size_full()
            .track_focus(&self.recorder.focus)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                let model = this.model.clone();
                if recorder::on_key_down(rec, this, &model, e, cx) {
                    cx.stop_propagation();
                }
            }))
            .child(kit::page("dictation-page", "Dictation", "Hotkeys, cancel, and how text goes into your apps.", body, cx))
    }
}
