//! Settings › Overlay: size, the idle pill, full screen, live preview, pill position.

use super::kit::{self, group, row};
use crate::model::AppModel;
use gpui_kit::*;
use sayso_core::config::OverlaySize;
use sayso_ui::components::*;

pub struct OverlaySettings {
    model: Entity<AppModel>,
    reset_note: Option<Result<(), String>>,
}

impl OverlaySettings {
    pub fn new(model: Entity<AppModel>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { model, reset_note: None }
    }
}

impl Render for OverlaySettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let o = self.model.read(cx).config.overlay.clone();
        let (m1, m2, m3, m4) = (self.model.clone(), self.model.clone(), self.model.clone(), self.model.clone());
        let size_index = OverlaySize::ALL.iter().position(|s| *s == o.size).unwrap_or(2);
        let size = Segmented::new("overlay-size", ["Small", "Medium", "Large"], size_index).on_select(move |i, _, cx| {
            let size = OverlaySize::ALL[i];
            m4.update(cx, |m, cx| m.edit_config(cx, |c| c.overlay.size = size));
        });
        let pill = Switch::new("idle-pill", o.idle_pill).on_toggle(move |on, _, cx| {
            m1.update(cx, |m, cx| m.edit_config(cx, |c| c.overlay.idle_pill = on));
        });
        let fullscreen = Switch::new("hide-fullscreen", o.hide_in_fullscreen).on_toggle(move |on, _, cx| {
            m2.update(cx, |m, cx| m.edit_config(cx, |c| c.overlay.hide_in_fullscreen = on));
        });
        let preview = Switch::new("show-preview", o.show_preview).on_toggle(move |on, _, cx| {
            m3.update(cx, |m, cx| m.edit_config(cx, |c| c.overlay.show_preview = on));
        });
        let reset = Button::new("reset-pill", "Reset position").small().on_click(cx.listener(|this, _, _, cx| {
            this.reset_note = Some(this.model.read(cx).reset_pill_position());
            cx.notify();
        }));

        let mut g = group("Pill", cx)
            .child(row("Size", "The size of the pill and of everything it shows while you dictate.", size, cx))
            .child(row("Show the idle pill", "A small pill at the bottom of the screen. Click it to start a dictation.", pill, cx))
            .child(row("Hide over full-screen apps", "The pill stays out of the way of videos and presentations.", fullscreen, cx))
            .child(row(
                "Pill position",
                "Puts the pill back at the bottom center. To do this now, right-click the pill and choose Reset position.",
                reset,
                cx,
            ));
        match &self.reset_note {
            Some(Ok(())) => {
                g = g.child(kit::banner(BannerKind::Info, "Sayso forgot the saved position. The pill moves to the bottom center the next time Sayso starts.", cx))
            }
            Some(Err(e)) => {
                g = g.child(kit::banner(BannerKind::Warning, format!("Sayso could not reset the position: {e}. Check that the data folder can be written."), cx))
            }
            None => {}
        }

        let rec = group("While you dictate", cx).child(row(
            "Show the live preview",
            "Shows your words above the pill while you speak. Only the final text goes into the app.",
            preview,
            cx,
        ));

        kit::page("overlay-page", "Overlay", "The pill and what it shows while you dictate.", kit::body().child(g).child(rec), cx)
    }
}
