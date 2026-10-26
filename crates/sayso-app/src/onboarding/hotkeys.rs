//! Step 5: the toggle key and an optional push-to-talk key, with conflicts.

use super::{OnboardingView, card, heading, live_ui, plain, rec, status_ui};
use crate::hub::settings::recorder::{self, Slot};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_i18n::t;
use sayso_core::hotkey::{Hotkey, SoloModifier};
use sayso_platform::{Permission, PermissionState};
use sayso_ui::a11y::{LiveStatus, Pressable};
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
            (true, _) => well.child(listening(None, window_only, &c)),
            (false, Some(hk)) => well.child(crate::widgets::hotkey_keycaps(&hk).size(KeySize::Large).with_plus()),
            (false, None) => well.child(text::ui(t!("onboarding.hotkeys.no_key"), 14., FontWeight::NORMAL, c.graphite)),
        };
        // One live node for the state of the key: a screen reader says it when
        // it changes, for example "Press the new keys".
        let toggle_status: AnyElement = if rec_toggle {
            status_ui("hotkey-toggle-status", t!("onboarding.hotkeys.press_new_keys"), 13., FontWeight::NORMAL, c.graphite).into_any_element()
        } else if toggle.is_some() && conflicts.iter().all(|x| Some(x.hotkey) != toggle) {
            let text = t!("onboarding.hotkeys.no_conflict");
            LiveStatus::new("hotkey-toggle-status", text.clone())
                .child(super::check_line(text, &c))
                .into_any_element()
        } else if toggle.is_some() {
            status_ui("hotkey-toggle-status", t!("onboarding.hotkeys.conflict"), 13., FontWeight::NORMAL, c.danger).into_any_element()
        } else {
            status_ui("hotkey-toggle-status", t!("onboarding.hotkeys.set_a_key"), 13., FontWeight::NORMAL, c.graphite).into_any_element()
        };
        let change_label = if rec_toggle { t!("onboarding.hotkeys.cancel") } else { t!("onboarding.hotkeys.change") };
        let toggle_card = card(&c)
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(text::title(t!("onboarding.hotkeys.start_stop"), 20., &c).line_height(px(24.)))
                    .child(
                        Pressable::new("change-toggle")
                            .label(if rec_toggle { t!("onboarding.hotkeys.cancel_label") } else { t!("onboarding.hotkeys.change_toggle") })
                            .cursor_pointer()
                            .child(plain(change_label, 13., FontWeight::MEDIUM, c.accent))
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
            (true, _) => dashed_box(&c).child(listening(Some("hotkey-ptt-listening"), window_only, &c)).into_any_element(),
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
                        .child(crate::widgets::hotkey_keycaps(&hk).size(KeySize::Large).with_plus()),
                )
                .into_any_element(),
            (false, None) => dashed_box(&c)
                .child(text::ui(t!("onboarding.hotkeys.hold_hint"), 14., FontWeight::NORMAL, c.graphite))
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(Button::new("ptt-right-option", t!("onboarding.hotkeys.use_right_option")).small().on_click(cx.listener(|this, _, _, cx| {
                            this.recorder.note = None;
                            this.model.update(cx, |m, cx| Slot::PushToTalk.apply(m, Some(Hotkey::Solo(SoloModifier::RightOption)), cx));
                        })))
                        .child(
                            Button::new("ptt-record", t!("onboarding.hotkeys.record_key"))
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
                    Pressable::new("ptt-change")
                        .label(t!("onboarding.hotkeys.change_ptt"))
                        .cursor_pointer()
                        .child(plain(t!("onboarding.hotkeys.change"), 13., FontWeight::MEDIUM, c.accent))
                        .on_click(cx.listener(|this, _, window, cx| this.record(Slot::PushToTalk, window, cx))),
                )
                .child(
                    Pressable::new("ptt-clear")
                        .label(t!("onboarding.hotkeys.clear_ptt"))
                        .cursor_pointer()
                        .child(plain(t!("onboarding.hotkeys.clear"), 13., FontWeight::MEDIUM, c.graphite))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.update(cx, |m, cx| Slot::PushToTalk.apply(m, None, cx));
                        })),
                )
        } else if rec_ptt {
            div().child(
                Pressable::new("ptt-cancel")
                    .label(t!("onboarding.hotkeys.cancel_label"))
                    .cursor_pointer()
                    .child(plain(t!("onboarding.hotkeys.cancel"), 13., FontWeight::MEDIUM, c.accent))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.recorder.cancel();
                        cx.notify();
                    })),
            )
        } else {
            div().child(text::ui(t!("onboarding.hotkeys.optional"), 13., FontWeight::NORMAL, c.graphite))
        };
        let im_line: AnyElement = match (m.config.hotkeys.needs_input_monitoring() || window_only, im) {
            // Windows needs no permission to hear keys in other apps.
            _ if !crate::shell::HAS_INPUT_PERMISSIONS => {
                text::ui(t!("onboarding.hotkeys.works_everywhere"), 13., FontWeight::NORMAL, c.graphite).into_any_element()
            }
            (true, PermissionState::Granted) => super::check_line(t!(crate::shell::os_text!("onboarding.hotkeys.im_on_mac", "onboarding.hotkeys.im_on_other")), &c).into_any_element(),
            (true, _) => div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(text::ui(t!(crate::shell::os_text!("onboarding.hotkeys.im_off_mac", "onboarding.hotkeys.im_off_other")), 13., FontWeight::NORMAL, c.danger).flex_1().min_w_0())
                .child(Button::new("open-im", crate::shell::open_permission_settings()).small().on_click(cx.listener(|this, _, _, cx| {
                    this.model.update(cx, |m, _| m.open_permission_settings(Permission::InputMonitoring));
                })))
                .into_any_element(),
            (false, _) => text::ui(t!(crate::shell::os_text!("onboarding.hotkeys.im_needed_mac", "onboarding.hotkeys.im_needed_other")), 13., FontWeight::NORMAL, c.graphite).into_any_element(),
        };
        let ptt_card = card(&c)
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(text::title(t!("onboarding.hotkeys.push_to_talk"), 20., &c).line_height(px(24.)))
                    .child(ptt_actions),
            )
            .child(body)
            .child(im_line);

        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(22.))
            .pt(px(28.))
            .px(px(self.side()))
            .pb(px(24.))
            .child(heading(&t!("onboarding.hotkeys.title"), &t!("onboarding.hotkeys.lead"), &c))
            .child(self.columns().child(toggle_card).child(ptt_card));
        for (i, x) in conflicts.iter().enumerate() {
            // The notice takes its id from its place in the code, so each one in the loop needs a parent id.
            col = col.child(div().id(("conflict", i)).child(crate::hub::settings::kit::notice(BannerKind::Warning, recorder::conflict_text(x, true), cx)));
        }
        if let Some(note) = &self.recorder.note {
            col = col.child(crate::hub::settings::kit::notice(BannerKind::Warning, note.clone(), cx));
        }
        col.when(window_only && self.recorder.slot.is_some(), |d| {
            d.child(crate::hub::settings::kit::notice(
                BannerKind::Info,
                t!("onboarding.hotkeys.window_only"), cx))
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

/// The "Press the keys" line. With an `id` it is a live node.
fn listening(id: Option<&'static str>, window_only: bool, c: &Colors) -> Div {
    let label = if window_only { t!("onboarding.hotkeys.listening_window") } else { t!("onboarding.hotkeys.listening") };
    let text: AnyElement = match id {
        Some(id) => live_ui(id, label, 14., FontWeight::MEDIUM, c.graphite, false).into_any_element(),
        None => plain(label, 14., FontWeight::MEDIUM, c.graphite).into_any_element(),
    };
    div().flex().items_center().gap(px(10.)).child(div().size(px(8.)).rounded_full().bg(c.accent)).child(text)
}
