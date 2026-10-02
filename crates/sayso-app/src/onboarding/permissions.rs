//! Step 4: microphone and accessibility on one screen, with live detection
//! and the mic test inline.

use super::{OnboardingView, card, heading, ring};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::stt::ModelStatus;
use sayso_platform::{Permission, PermissionState};
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::{ActivePaper, Colors, text};

impl OnboardingView {
    pub(super) fn permissions_step(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let mic = m.permission(Permission::Microphone);
        let ax = m.permission(Permission::Accessibility);
        let info = m.active_model();
        let status = m.status_of(&info.id);
        let devices = m.input_devices();
        let device_id = m.resolve_input_device(m.config.audio.input_device.as_deref());
        let device_name = match device_id {
            Some(id) => devices.iter().find(|d| d.id == id).map(|d| d.name.clone()),
            None => devices.iter().find(|d| d.is_default).map(|d| d.name.clone()),
        }
        .unwrap_or_else(|| "Default microphone".into());
        // The ring marks the card that needs you now.
        let focus_mic = mic != PermissionState::Granted;
        let focus_ax = !focus_mic && ax != PermissionState::Granted;

        // Microphone card.
        let mut mic_card = card(&c)
            .when(focus_mic, |d| d.shadow(ring(&c)))
            .child(card_head("Microphone", mic, &c))
            .child(text::ui(
                match mic {
                    PermissionState::Granted => "Say something. The ink should move with your voice.",
                    PermissionState::Denied => "Microphone access is off for Sayso. Turn on Sayso in Privacy and Security › Microphone.",
                    PermissionState::NotDetermined => "Sayso records only while you dictate. macOS asks you once.",
                },
                13.,
                FontWeight::NORMAL,
                c.graphite,
            )
            .line_height(px(19.)));
        mic_card = match mic {
            PermissionState::Granted => {
                let well = div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .py(px(12.))
                    .px(px(14.))
                    .rounded(px(12.))
                    .debossed(&c);
                if let Some(e) = &self.meter_error {
                    mic_card.child(well.child(text::ui(format!("The microphone did not start: {e}."), 12., FontWeight::NORMAL, c.danger)))
                } else {
                    let reduce = cx.paper().reduce_motion;
                    if self.meter.is_some() && !reduce {
                        window.request_animation_frame();
                    }
                    let t = if reduce { 0.0 } else { self.started.elapsed().as_secs_f32() };
                    mic_card.child(
                        well.child(ink_waveform(self.live.smoothed(28), t, c.ink).w(px(200.)).h(px(34.)).flex_none()).child(
                            text::ui(device_name, 12., FontWeight::NORMAL, c.graphite).flex_1().min_w_0().text_right().truncate(),
                        ),
                    )
                }
            }
            PermissionState::NotDetermined => mic_card.child(
                div().flex().pt(px(6.)).child(Button::new("allow-mic", "Allow microphone").primary().on_click(cx.listener(|this, _, _, cx| {
                    this.model.update(cx, |m, _| m.request_permission(Permission::Microphone));
                }))),
            ),
            PermissionState::Denied => mic_card.child(
                div().flex().pt(px(6.)).child(Button::new("open-mic", "Open System Settings").primary().on_click(cx.listener(|this, _, _, cx| {
                    this.model.update(cx, |m, _| m.open_permission_settings(Permission::Microphone));
                }))),
            ),
        };

        // Accessibility card.
        let mut ax_card = card(&c)
            .when(focus_ax, |d| d.shadow(ring(&c)))
            // macOS has no "denied" for Accessibility; until it is on, Sayso waits.
            .child(card_head("Accessibility", if ax == PermissionState::Granted { ax } else { PermissionState::NotDetermined }, &c))
            .child(text::ui(
                if ax == PermissionState::Granted {
                    "Sayso can paste text into the app you are using."
                } else {
                    "Lets Sayso paste text into the app you are using. Turn on Sayso in Privacy and Security › Accessibility."
                },
                13.,
                FontWeight::NORMAL,
                c.graphite,
            )
            .line_height(px(19.)));
        if ax != PermissionState::Granted {
            ax_card = ax_card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .pt(px(6.))
                    .child(Button::new("open-ax", "Open System Settings").primary().on_click(cx.listener(|this, _, _, cx| {
                        // `request` shows the macOS prompt, which adds Sayso to the list and opens Settings.
                        this.model.update(cx, |m, _| m.request_permission(Permission::Accessibility));
                    })))
                    .child(Button::new("why-ax", "Why is this needed?").ghost().on_click(cx.listener(|this, _, _, cx| {
                        this.show_why = !this.show_why;
                        cx.notify();
                    }))),
            );
            if self.show_why {
                ax_card = ax_card.child(
                    text::ui(
                        "macOS lets an app send Command+V to other apps only with Accessibility access. Sayso uses it to paste your words, and does not read your screen.",
                        12.,
                        FontWeight::NORMAL,
                        c.graphite,
                    )
                    .line_height(px(17.)),
                );
            }
        }

        // The background download.
        let (fraction, label) = match &status {
            ModelStatus::Downloading { fraction, .. } => (*fraction, format!("{:.0}%", fraction * 100.0)),
            ModelStatus::Ready => (1.0, "Ready".into()),
            ModelStatus::Optimizing => (1.0, "Optimizing".into()),
            ModelStatus::Downloaded => (1.0, "Downloaded".into()),
            ModelStatus::NotDownloaded => (0.0, "Not started".into()),
            ModelStatus::Failed { .. } => (0.0, "Stopped".into()),
        };
        let download = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(4.))
            .child(text::ui(info.name.clone(), 13., FontWeight::NORMAL, c.graphite).flex_none())
            .child(
                div()
                    .flex()
                    .flex_1()
                    .h(px(4.))
                    .rounded_full()
                    .bg(c.deboss)
                    .shadow(vec![BoxShadow::new(px(0.), px(1.), c.shadow(0.25)).blur_radius(px(1.)).inset()])
                    .child(div().h_full().w(relative(fraction.clamp(0.0, 1.0))).rounded_full().bg(c.ink_fill)),
            )
            .child(text::mono(label, 12., c.graphite).font_weight(FontWeight::NORMAL).flex_none());

        div()
            .flex()
            .flex_col()
            .gap(px(22.))
            .pt(px(28.))
            .px(px(72.))
            .pb(px(24.))
            .child(heading(
                "Two permissions",
                "Sayso needs to hear you and to type into other apps. This page updates by itself when you allow each one.",
                &c,
            ))
            .when(!crate::dev::running_from_bundle(), |d| {
                d.child(crate::hub::settings::kit::notice(BannerKind::Warning, crate::dev::UNBUNDLED_NOTE, cx))
            })
            .child(div().flex().items_start().gap(px(14.)).child(mic_card).child(ax_card))
            .child(download)
            .child(crate::hub::settings::kit::notice(
                BannerKind::Info,
                "A push-to-talk key needs one more permission, Input Monitoring. Sayso asks for it only if you set one in the next step.", cx))
            .into_any_element()
    }
}

fn card_head(title: &str, state: PermissionState, c: &Colors) -> Div {
    let badge = match state {
        PermissionState::Granted => super::check_line_w("Allowed", FontWeight::SEMIBOLD, c),
        PermissionState::Denied => div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(crate::hub::settings::kit::halo_dot(8., c.danger, 3., 0.16))
            .child(text::ui("Blocked", 13., FontWeight::SEMIBOLD, c.danger)),
        PermissionState::NotDetermined => div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(crate::hub::settings::kit::halo_dot(8., c.accent, 3., 0.16))
            .child(text::ui("Waiting", 13., FontWeight::SEMIBOLD, c.accent)),
    };
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(text::title(title.to_string(), 20., c).line_height(px(24.)))
        .child(badge)
}
