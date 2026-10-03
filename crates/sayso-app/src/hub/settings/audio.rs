//! Settings › Audio: input device, a live level meter, live preview, max length.

use super::kit::{self, group, row};
use crate::hub::{Route, SettingsPage};
use crate::live::LiveAudio;
use crate::model::AppModel;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_platform::{CaptureHandle, Permission, PermissionState};
use sayso_ui::ActivePaper;
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::text;
use std::sync::Arc;
use std::time::Instant;

/// Where the default input comes from.
const FOLLOWS: &str = crate::shell::os_text!("Follows the input in System Settings › Sound.", "Follows the default input of the system.");

const LENGTHS: [u64; 7] = [30, 60, 120, 300, 600, 900, 1800];

pub struct AudioSettings {
    model: Entity<AppModel>,
    live: Arc<LiveAudio>,
    meter: Option<(Option<String>, Box<dyn CaptureHandle>)>,
    meter_error: Option<String>,
    started: Instant,
    _observe: Subscription,
}

impl AudioSettings {
    pub fn new(model: Entity<AppModel>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Stop the microphone as soon as the page is not shown.
        let observe = cx.observe(&model, |this: &mut Self, model, cx| {
            if model.read(cx).route != Route::Settings(SettingsPage::Audio) {
                this.stop_meter();
            }
            cx.notify();
        });
        Self { model, live: Arc::new(LiveAudio::default()), meter: None, meter_error: None, started: Instant::now(), _observe: observe }
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
        let max_s = m.config.dictation.max_duration_s;
        // With the setting off, ask what the preview would use if it were on.
        let has_preview_model = m.active_model().live_preview || m.preview_candidate().is_some();
        let recording = !matches!(m.state(), sayso_core::dictation::State::Idle);
        let default_name = devices.iter().find(|d| d.is_default).map(|d| d.name.clone());
        let meter_name = match &current_id {
            Some(id) => devices.iter().find(|d| d.id == *id).map(|d| d.name.clone()).unwrap_or_default(),
            None => default_name.clone().unwrap_or_else(|| "System default".into()),
        };

        // Device list.
        let mut options: Vec<(Option<String>, String, String)> = vec![(
            None,
            "System default".into(),
            default_name.clone().map(|n| format!("Now {n}. {}", FOLLOWS)).unwrap_or_else(|| FOLLOWS.into()),
        )];
        options.extend(devices.iter().map(|d| (Some(d.id.clone()), d.name.clone(), String::new())));
        let mut list = div().flex().flex_col();
        for (i, (value, name, desc)) in options.into_iter().enumerate() {
            let selected = value == current_id;
            list = list.child(
                div()
                    .id(("device", i))
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
                            .child(text::ui(name, 15., if selected { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM }, c.ink).line_height(px(18.)))
                            .when(!desc.is_empty(), |d| d.child(text::ui(desc, 13., FontWeight::NORMAL, c.graphite).line_height(px(16.)))),
                    ),
            );
        }
        if devices.is_empty() {
            list = list.child(kit::banner(BannerKind::Warning, crate::shell::os_text!("Sayso found no microphone. Connect one, or check System Settings › Sound.", "Sayso found no microphone. Connect one, or check the sound settings."), cx));
        }

        // Level meter.
        let meter: AnyElement = if mic != PermissionState::Granted {
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .py(px(8.))
                .child(div().flex_1().child(super::kit::notice(BannerKind::Info, "Allow microphone access to test your microphone here.", cx)))
                .child(Button::new("allow-mic", if mic == PermissionState::Denied { crate::shell::OPEN_PERMISSION_SETTINGS } else { "Allow microphone" }).small().on_click(
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
            kit::banner(BannerKind::Warning, format!("The microphone did not start: {e}. Choose another input, or check {}.", crate::shell::os_text!("System Settings › Sound", "the sound settings")), cx).into_any_element()
        } else if recording {
            kit::banner(BannerKind::Info, "The test pauses while you dictate.", cx).into_any_element()
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
                                .child(text::ui("Say something. The ink moves with your voice.", 12., FontWeight::NORMAL, c.graphite)),
                        ),
                )
                .into_any_element()
        };

        let (m1, m2) = (self.model.clone(), self.model.clone());
        let preview = Switch::new("live-preview", live_preview).on_toggle(move |on, _, cx| {
            m1.update(cx, |m, cx| m.edit_config(cx, |c| c.dictation.live_preview = on));
        });
        let index = LENGTHS.iter().position(|l| *l >= max_s).unwrap_or(LENGTHS.len() - 1);
        let length = kit::stepper("max-length", format_length(max_s), move |d, _, cx| {
            let i = (index as i32 + d).clamp(0, LENGTHS.len() as i32 - 1) as usize;
            m2.update(cx, |m, cx| m.edit_config(cx, |c| c.dictation.max_duration_s = LENGTHS[i]));
        }, cx);
        let preview_desc = if has_preview_model {
            "Streams your words to the overlay while you speak. Uses a little more power."
        } else {
            "Your model has no live preview in this language. Download a model that has one in Models, for example Parakeet EOU for English."
        };

        let input = group("Input", cx).child(list).child(meter);
        let rec = group("Recording", cx)
            .child(row("Live preview", preview_desc, preview, cx))
            .child(row("Maximum length", "Recording stops by itself after this long.", length, cx));
        kit::page("audio-page", "Audio", "Your microphone and how long Sayso records.", kit::body().child(input).child(rec), cx)
    }
}

fn format_length(s: u64) -> String {
    if s < 60 { format!("{s} s") } else { format!("{} min", s / 60) }
}
