//! Settings › History and privacy: retention, audio, what leaves the Mac, clear.

use super::kit::{self, group, row};
use crate::model::AppModel;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_ui::ActivePaper;
use sayso_ui::components::*;
use sayso_ui::text;

const TEXT_DAYS: [Option<u32>; 4] = [None, Some(90), Some(30), Some(7)];
const AUDIO_DAYS: [u32; 3] = [30, 7, 1];

pub struct HistorySettings {
    model: Entity<AppModel>,
    confirm_clear: bool,
    cleared: bool,
}

impl HistorySettings {
    pub fn new(model: Entity<AppModel>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { model, confirm_clear: false, cleared: false }
    }
}

impl Render for HistorySettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let h = m.config.history.clone();
        let total = m.history_total;
        let ai_on = m.config.ai.enabled;
        let providers: Vec<(String, bool)> = m.config.ai.providers.iter().map(|p| (p.name.clone(), p.is_cloud())).collect();

        let text_index = TEXT_DAYS.iter().position(|d| *d == h.keep_text_days).unwrap_or(0);
        let audio_index = AUDIO_DAYS.iter().position(|d| Some(*d) == h.keep_audio_days).unwrap_or(0);
        let (m1, m2, m3) = (self.model.clone(), self.model.clone(), self.model.clone());
        let keep_text = Segmented::new("keep-text", ["Forever", "90 days", "30 days", "7 days"], text_index).on_select(move |i, _, cx| {
            m1.update(cx, |m, cx| m.edit_config(cx, |c| c.history.keep_text_days = TEXT_DAYS[i]));
        });
        let save_audio = Switch::new("save-audio", h.save_audio).on_toggle(move |on, _, cx| {
            m2.update(cx, |m, cx| m.edit_config(cx, |c| c.history.save_audio = on));
        });
        let keep_audio = Segmented::new("keep-audio", ["30 days", "7 days", "1 day"], audio_index)
            .on_select(move |i, _, cx| {
                m3.update(cx, |m, cx| m.edit_config(cx, |c| c.history.keep_audio_days = Some(AUDIO_DAYS[i])));
            });

        let keep = group("History", cx)
            .child(row(
                "Keep text",
                "When text expires, Sayso deletes the whole entry.",
                keep_text,
                cx,
            ))
            .child(row("Save audio", "Lets you play a dictation again or transcribe it again.", save_audio, cx))
            .child(row(
                "Keep audio",
                "When audio expires, only the audio file is deleted. The text stays.",
                div().when(!h.save_audio, |d| d.opacity(0.45)).child(keep_audio),
                cx,
            ));

        let mut leaves = div().flex().flex_col().gap(px(10.)).py(px(14.)).child(
            text::body(
                "Speech recognition runs on this Mac. Your audio never leaves it. When a style uses AI, Sayso sends the transcript, the style prompt, and your dictionary words to the provider of that style. Nothing else is sent.",
                &c,
            )
            .max_w(px(640.)),
        );
        if !ai_on || providers.is_empty() {
            leaves = leaves.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(kit::halo_dot(7., c.success, 3., 0.16))
                    .child(text::ui("AI is off. No text leaves this Mac.", 13., FontWeight::MEDIUM, c.ink)),
            );
        } else {
            for (name, cloud) in providers {
                leaves = leaves.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(text::ui(name, 13., FontWeight::SEMIBOLD, c.ink))
                        .child(if cloud { Badge::new("Cloud", BadgeTone::Accent) } else { Badge::new("This Mac", BadgeTone::Muted) }),
                );
            }
        }
        let privacy = group("What leaves this Mac", cx).child(leaves.border_b_1().border_color(c.rule));

        let clear_control = if self.confirm_clear {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(Button::new("clear-cancel", "Cancel").ghost().small().on_click(cx.listener(|this, _, _, cx| {
                    this.confirm_clear = false;
                    cx.notify();
                })))
                .child(Button::new("clear-confirm", "Delete all").danger().small().on_click(cx.listener(|this, _, _, cx| {
                    this.confirm_clear = false;
                    this.cleared = true;
                    this.model.update(cx, |m, cx| m.clear_history(cx));
                })))
        } else {
            div().child(Button::new("clear-history", "Clear history").danger().small().disabled(total == 0).on_click(cx.listener(
                |this, _, _, cx| {
                    this.confirm_clear = true;
                    this.cleared = false;
                    cx.notify();
                },
            )))
        };
        let clear_desc = if self.confirm_clear {
            format!("Delete all {total} entries and their audio? You cannot undo this.")
        } else if self.cleared && total == 0 {
            "History is empty.".to_string()
        } else {
            format!("Deletes all {total} entries and their audio.")
        };
        let danger = group("Clear", cx).child(kit::row_s("Clear history".into(), clear_desc, clear_control, cx));

        kit::page(
            "history-page",
            "History and privacy",
            "What Sayso keeps, for how long, and what leaves this Mac.",
            kit::body().child(keep).child(privacy).child(danger),
            cx,
        )
    }
}
