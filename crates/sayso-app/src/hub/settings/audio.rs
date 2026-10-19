//! Settings › Audio: input device, a live level meter, live preview, max length.

use super::kit::{self, group, row};
use crate::devices::ViewWatch;
use crate::hub::{Route, SettingsPage};
use crate::live::LiveAudio;
use crate::model::AppModel;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_i18n::t;
use sayso_platform::{CaptureHandle, Permission, PermissionState};
use sayso_ui::ActivePaper;
use sayso_ui::a11y::{A11yExt as _, Pressable, Role};
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::text;
use std::sync::Arc;
use std::time::Instant;

const LENGTHS: [u64; 7] = [30, 60, 120, 300, 600, 900, 1800];

pub struct AudioSettings {
    model: Entity<AppModel>,
    live: Arc<LiveAudio>,
    meter: Option<(Option<String>, Box<dyn CaptureHandle>)>,
    meter_error: Option<String>,
    started: Instant,
    /// The device list stays fresh while this page shows.
    devices: ViewWatch,
    _observe: Subscription,
}

impl AudioSettings {
    pub fn new(model: Entity<AppModel>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Stop the microphone as soon as the page is not shown.
        let observe = cx.observe(&model, |this: &mut Self, model, cx| {
            let shown = model.read(cx).route == Route::Settings(SettingsPage::Audio);
            if !shown {
                this.stop_meter();
            }
            this.devices.set(shown, &model, cx);
            cx.notify();
        });
        let mut devices = ViewWatch::default();
        devices.set(model.read(cx).route == Route::Settings(SettingsPage::Audio), &model, cx);
        Self { model, live: Arc::new(LiveAudio::default()), meter: None, meter_error: None, started: Instant::now(), devices, _observe: observe }
    }

    fn stop_meter(&mut self) {
        if let Some((_, handle)) = self.meter.take() {
            handle.stop();
        }
        self.live.clear();
    }

    /// Keep the meter running on the chosen device while the page is visible.
    fn ensure_meter(&mut self, cx: &mut Context<Self>) {
        let m = self.model.read(cx);
        let busy = !matches!(m.state(), sayso_core::dictation::State::Idle);
        if m.permission(Permission::Microphone) != PermissionState::Granted || busy {
            self.stop_meter();
            return;
        }
        let device = m.config.audio.input_device.clone();
        if self.meter.as_ref().is_some_and(|(d, _)| *d == device) {
            return;
        }
        self.stop_meter();
        match m.start_level_meter(device.as_deref(), self.live.clone()) {
            Ok(handle) => {
                self.meter = Some((device, handle));
                self.meter_error = None;
            }
            Err(e) => self.meter_error = Some(e),
        }
    }
}

impl Drop for AudioSettings {
    fn drop(&mut self) {
        self.stop_meter();
    }
}

impl Render for AudioSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_meter(cx);
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let devices = m.input_devices();
        let current = m.config.audio.input_device.clone();
        let current_id = m.resolve_input_device(current.as_deref());
        let mic = m.permission(Permission::Microphone);
        let live_preview = m.config.dictation.live_preview;
        let pause_media = m.config.audio.pause_media;
        let max_s = m.config.dictation.max_duration_s;
        // With the setting off, ask what the preview would use if it were on.
        let has_preview_model = m.preview_candidate().is_some();
        let recording = !matches!(m.state(), sayso_core::dictation::State::Idle);
        let default_name = devices.iter().find(|d| d.is_default).map(|d| d.name.clone());
        let meter_name = match &current_id {
            Some(id) => devices.iter().find(|d| d.id == *id).map(|d| d.name.clone()).unwrap_or_default(),
            None => default_name.clone().unwrap_or_else(|| t!("settings.audio.system_default")),
        };

        // Device list.
        let mut options: Vec<(Option<String>, String, String)> = vec![(
            None,
            t!("settings.audio.system_default"),
            match default_name.clone() {
                Some(name) => t!(crate::shell::os_text!("settings.audio.follows_now_mac", "settings.audio.follows_now_other"), name = name),
                None => t!(crate::shell::os_text!("settings.audio.follows_mac", "settings.audio.follows_other")),
            },
        )];
        options.extend(devices.iter().map(|d| (Some(d.id.clone()), d.name.clone(), String::new())));
        // One radio group. The arrow keys change the device, as in `Segmented`.
        let count = options.len();
        let selected_index = options.iter().position(|(v, _, _)| *v == current_id).unwrap_or(0);
        let values: Vec<Option<String>> = options.iter().map(|(v, _, _)| v.clone()).collect();
        let mut list = div()
            .id("devices")
            .role(Role::RadioGroup)
            .aria_label(t!("settings.audio.input"))
            .flex()
            .flex_col()
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, window, cx| {
                let next = match e.keystroke.key.as_str() {
                    "right" | "down" if selected_index + 1 < count => selected_index + 1,
                    "left" | "up" if selected_index > 0 => selected_index - 1,
                    _ => return,
                };
                cx.stop_propagation();
                let v = values[next].clone();
                this.model.update(cx, |m, cx| m.edit_config(cx, |c| c.audio.input_device = v));
                if next > selected_index { window.focus_next(cx) } else { window.focus_prev(cx) }
            }));
        for (i, (value, name, desc)) in options.into_iter().enumerate() {
            let selected = value == current_id;
            // The row id comes from the device id, so it survives a change of the list.
            let row_id = SharedString::from(format!("device-{}", value.as_deref().unwrap_or("default")));
            let label = if desc.is_empty() { name.clone() } else { format!("{name}, {desc}") };
            list = list.child(
                Pressable::new(row_id)
                    .a11y_radio(label, selected, i, count)
                    .flex()
                    .items_center()
                    .gap(px(14.))
                    .py(px(12.))
                    .border_b_1()
                    .border_color(c.rule)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let v = value.clone();
                        this.model.update(cx, |m, cx| m.edit_config(cx, |c| c.audio.input_device = v));
                    }))
                    .child(radio_dot(selected, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .font_family(sayso_ui::fonts::UI)
                                    .text_size(px(15.))
                                    .line_height(px(18.))
                                    .font_weight(if selected { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                                    .text_color(c.ink)
                                    .child(name),
                            )
                            .when(!desc.is_empty(), |d| {
                                d.child(
                                    div()
                                        .font_family(sayso_ui::fonts::UI)
                                        .text_size(px(13.))
                                        .line_height(px(16.))
                                        .font_weight(FontWeight::NORMAL)
                                        .text_color(c.graphite)
                                        .child(desc),
                                )
                            }),
                    ),
            );
        }
        if devices.is_empty() {
            list = list.child(kit::banner(BannerKind::Warning, t!("settings.audio.no_microphone", settings = crate::shell::sound_settings()), cx));
        }

        // Level meter.
        let meter: AnyElement = if mic != PermissionState::Granted {
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .py(px(8.))
                .child(div().flex_1().child(super::kit::notice(BannerKind::Info, t!("settings.audio.allow_to_test"), cx)))
                .child(Button::new("allow-mic", if mic == PermissionState::Denied { crate::shell::open_permission_settings() } else { t!("settings.audio.allow_microphone") }).small().on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.model.update(cx, |m, _| {
                            if mic == PermissionState::Denied {
                                m.open_permission_settings(Permission::Microphone)
                            } else {
                                m.request_permission(Permission::Microphone)
                            }
                        });
                    }),
                ))
                .into_any_element()
        } else if let Some(e) = &self.meter_error {
            kit::banner(BannerKind::Warning, t!("settings.audio.meter_failed", error = e, settings = crate::shell::sound_settings()), cx).into_any_element()
        } else if recording {
            kit::banner(BannerKind::Info, t!("settings.audio.test_paused"), cx).into_any_element()
        } else {
            if self.meter.is_some() && !cx.paper().reduce_motion {
                window.request_animation_frame();
            }
            let t = self.started.elapsed().as_secs_f32();
            div()
                .py(px(8.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(16.))
                        .py(px(12.))
                        .px(px(16.))
                        .rounded(px(12.))
                        .debossed(&c)
                        .child(ink_waveform(self.live.smoothed(28), t, c.ink).w(px(320.)).h(px(40.)))
                        .child(div().flex_1())
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_end()
                                .gap(px(2.))
                                .child(text::ui(meter_name, 13., FontWeight::SEMIBOLD, c.ink))
                                .child(text::ui(t!("settings.audio.say_something"), 12., FontWeight::NORMAL, c.graphite)),
                        ),
                )
                .into_any_element()
        };

        let (m1, m2, m3) = (self.model.clone(), self.model.clone(), self.model.clone());
        let preview = Switch::new("live-preview", live_preview).label(t!("settings.audio.live_preview")).on_toggle(move |on, _, cx| {
            m1.update(cx, |m, cx| m.edit_config(cx, |c| c.dictation.live_preview = on));
        });
        let pause = Switch::new("pause-media", pause_media).label(t!("settings.audio.pause_media")).on_toggle(move |on, _, cx| {
            m3.update(cx, |m, cx| m.edit_config(cx, |c| c.audio.pause_media = on));
        });
        let index = LENGTHS.iter().position(|l| *l >= max_s).unwrap_or(LENGTHS.len() - 1);
        let length = kit::stepper("max-length", format_length(max_s), move |d, _, cx| {
            let i = (index as i32 + d).clamp(0, LENGTHS.len() as i32 - 1) as usize;
            m2.update(cx, |m, cx| m.edit_config(cx, |c| c.dictation.max_duration_s = LENGTHS[i]));
        }, cx).label(t!("settings.audio.max_length"));
        let preview_desc = if has_preview_model {
            t!("settings.audio.live_preview_detail")
        } else {
            t!("settings.audio.live_preview_unavailable")
        };

        let input = group(&t!("settings.audio.input"), cx).child(list).child(meter);
        let rec = group(&t!("settings.audio.recording"), cx)
            .child(row(&t!("settings.audio.live_preview"), &preview_desc, preview, cx))
            .child(row(&t!("settings.audio.pause_media"), &t!("settings.audio.pause_media_detail"), pause, cx))
            .child(row(&t!("settings.audio.max_length"), &t!("settings.audio.max_length_detail"), length, cx));
        kit::page("audio-page", &t!("settings.audio.title"), &t!("settings.audio.subtitle"), kit::body().child(input).child(rec), cx)
    }
}

fn format_length(s: u64) -> String {
    if s < 60 { t!("settings.audio.length_seconds", seconds = s) } else { t!("settings.audio.length_minutes", minutes = s / 60) }
}
