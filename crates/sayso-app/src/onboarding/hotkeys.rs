//! Step 5: the toggle key and an optional push-to-talk key, with conflicts.

use super::{OnboardingView, card, heading, rec};
use crate::hub::settings::recorder::{self, Slot};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::hotkey::{Hotkey, SoloModifier};
use sayso_platform::{Permission, PermissionState};
use sayso_ui::components::*;
use sayso_ui::paper::PaperStyled;
use sayso_ui::{ActivePaper, Colors, text};

impl OnboardingView {
    fn record(&mut self, slot: Slot, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.clone();
        recorder::start(rec, self, &model, slot, window, cx);
    }

    pub(super) fn hotkeys_step(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let toggle = m.config.hotkeys.toggle;
        let ptt = m.config.hotkeys.push_to_talk;
        let im = m.permission(Permission::InputMonitoring);
        let mut conflicts = self.recorder.conflicts(m, toggle);
        conflicts.extend(self.recorder.conflicts(m, ptt));
        let rec_toggle = self.recorder.is_recording(Slot::Toggle);
        let rec_ptt = self.recorder.is_recording(Slot::PushToTalk);
        let window_only = self.recorder.window_only;

        // Start and stop.
        let well = div()
            .flex()
            .items_center()
            .justify_center()
            .h(px(88.))
            .rounded(px(12.))
            .bg(c.deboss)
            .shadow(vec![
                BoxShadow::new(px(0.), px(2.), c.shadow(0.2)).blur_radius(px(4.)).inset(),
                BoxShadow::new(px(0.), px(-1.), c.highlight(0.75)).inset(),
            ]);
        let well = match (rec_toggle, toggle) {
            (true, _) => well.child(listening(window_only, &c)),
            (false, Some(hk)) => well.child(Keycaps::hotkey(&hk).size(KeySize::Large).with_plus()),
            (false, None) => well.child(text::ui("No key set", 14., FontWeight::NORMAL, c.graphite)),
        };
        let toggle_status: AnyElement = if rec_toggle {
            text::ui("Press the new keys. Press Esc to stop.", 13., FontWeight::NORMAL, c.graphite).into_any_element()
        } else if toggle.is_some() && conflicts.iter().all(|x| Some(x.hotkey) != toggle) {
            super::check_line("No other app uses these keys.", &c).into_any_element()
        } else if toggle.is_some() {
            text::ui("Another app uses these keys. See below.", 13., FontWeight::NORMAL, c.danger).into_any_element()
        } else {
            text::ui("Set a key to start and stop dictation.", 13., FontWeight::NORMAL, c.graphite).into_any_element()
        };
        let change_label = if rec_toggle { "Cancel" } else { "Change" };
        let toggle_card = card(&c)
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(text::title("Start and stop", 20., &c).line_height(px(24.)))
                    .child(
                        div()
                            .id("change-toggle")
                            .cursor_pointer()
                            .child(text::ui(change_label, 13., FontWeight::MEDIUM, c.accent))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if rec_toggle {
                                    this.recorder.cancel();
                                    cx.notify();
                                } else {
                                    this.record(Slot::Toggle, window, cx);
                                }
                            })),
                    ),
            )
            .child(well)
            .child(toggle_status);

        // Push to talk.
        let body: AnyElement = match (rec_ptt, ptt) {
            (true, _) => dashed_box(&c).child(listening(window_only, &c)).into_any_element(),
            (false, Some(hk)) => div()
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .h(px(88.))
                        .rounded(px(12.))
                        .debossed(&c)
                        .child(Keycaps::hotkey(&hk).size(KeySize::Large).with_plus()),
                )
                .into_any_element(),
            (false, None) => dashed_box(&c)
                .child(text::ui("Hold a key to record. Release to insert.", 14., FontWeight::NORMAL, c.graphite))
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(Button::new("ptt-right-option", "Use Right Option").small().on_click(cx.listener(|this, _, _, cx| {
                            this.recorder.note = None;
                            this.model.update(cx, |m, cx| Slot::PushToTalk.apply(m, Some(Hotkey::Solo(SoloModifier::RightOption)), cx));
                        })))
                        .child(
                            Button::new("ptt-record", "Record a key")
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| this.record(Slot::PushToTalk, window, cx))),
                        ),
                )
                .into_any_element(),
        };
        let ptt_actions = if ptt.is_some() && !rec_ptt {
            div()
                .flex()
                .gap(px(14.))
                .child(
                    div()
                        .id("ptt-change")
                        .cursor_pointer()
                        .child(text::ui("Change", 13., FontWeight::MEDIUM, c.accent))
                        .on_click(cx.listener(|this, _, window, cx| this.record(Slot::PushToTalk, window, cx))),
                )
                .child(
                    div()
                        .id("ptt-clear")
                        .cursor_pointer()
                        .child(text::ui("Clear", 13., FontWeight::MEDIUM, c.graphite))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.update(cx, |m, cx| Slot::PushToTalk.apply(m, None, cx));
                        })),
                )
        } else if rec_ptt {
            div().child(
                div()
                    .id("ptt-cancel")
                    .cursor_pointer()
                    .child(text::ui("Cancel", 13., FontWeight::MEDIUM, c.accent))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.recorder.cancel();
                        cx.notify();
                    })),
            )
        } else {
            div().child(text::ui("Optional", 13., FontWeight::NORMAL, c.graphite))
        };
        let im_line: AnyElement = match (ptt.is_some(), im) {
            // Windows needs no permission to hear keys in other apps.
            _ if !crate::shell::HAS_INPUT_PERMISSIONS => {
                text::ui("Works in every app while you hold it.", 13., FontWeight::NORMAL, c.graphite).into_any_element()
            }
            (true, PermissionState::Granted) => super::check_line(crate::shell::os_text!("Input Monitoring is on.", "Keyboard access is on."), &c).into_any_element(),
            (true, _) => div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(text::ui(crate::shell::os_text!("Turn on Sayso in Input Monitoring to use this key.", "This key needs keyboard access. See the setup guide."), 13., FontWeight::NORMAL, c.danger).flex_1().min_w_0())
                .child(Button::new("open-im", crate::shell::OPEN_PERMISSION_SETTINGS).small().on_click(cx.listener(|this, _, _, cx| {
                    this.model.update(cx, |m, _| m.open_permission_settings(Permission::InputMonitoring));
                })))
                .into_any_element(),
            (false, _) => text::ui(crate::shell::os_text!("Needs Input Monitoring. Sayso asks when you set it.", "Needs keyboard access on some desktops."), 13., FontWeight::NORMAL, c.graphite).into_any_element(),
        };
        let ptt_card = card(&c)
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(text::title("Push to talk", 20., &c).line_height(px(24.)))
                    .child(ptt_actions),
            )
            .child(body)
            .child(im_line);

        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(22.))
            .pt(px(28.))
            .px(px(72.))
            .pb(px(24.))
            .child(heading("Choose your keys", "Press the keys to test them. Sayso checks that no other app uses the same keys.", &c))
            .child(div().flex().items_start().gap(px(14.)).child(toggle_card).child(ptt_card));
        for x in &conflicts {
            col = col.child(crate::hub::settings::kit::notice(BannerKind::Warning, recorder::conflict_text(x, true), cx));
        }
        if let Some(note) = &self.recorder.note {
            col = col.child(crate::hub::settings::kit::notice(BannerKind::Warning, note.clone(), cx));
        }
        col.when(window_only && self.recorder.slot.is_some(), |d| {
            d.child(crate::hub::settings::kit::notice(
                BannerKind::Info,
                "Without Input Monitoring, Sayso hears keys only while this window is in front. Single keys such as Right Option need Input Monitoring.", cx))
        })
        .into_any_element()
    }
}

fn dashed_box(c: &Colors) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.))
        .h(px(88.))
        .rounded(px(12.))
        .border(px(1.5))
        .border_dashed()
        .border_color(c.deboss_shade)
}

fn listening(window_only: bool, c: &Colors) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(div().size(px(8.)).rounded_full().bg(c.accent))
        .child(text::ui(
            if window_only { "Press the keys in this window…" } else { "Press the keys…" },
            14.,
            FontWeight::MEDIUM,
            c.graphite,
        ))
}
