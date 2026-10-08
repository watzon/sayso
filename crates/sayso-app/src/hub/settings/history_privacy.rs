//! Settings › History and privacy: the history switch, retention, the data on
//! this computer, and what leaves it. See the Paper page "History off".

use super::kit::{self, group, row};
use crate::model::AppModel;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::models::format_size;
use sayso_core::stats::format_count;
use sayso_ui::ActivePaper;
use sayso_ui::components::*;
use sayso_ui::text;

const TEXT_DAYS: [Option<u32>; 4] = [None, Some(90), Some(30), Some(7)];
const AUDIO_DAYS: [u32; 3] = [30, 7, 1];

/// Data that the user can delete from this page.
#[derive(Clone, Copy, PartialEq)]
enum Wipe {
    History,
    Audio,
}

pub struct HistorySettings {
    model: Entity<AppModel>,
    /// The delete that waits for a second click.
    confirm: Option<Wipe>,
}

impl HistorySettings {
    pub fn new(model: Entity<AppModel>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { model, confirm: None }
    }
}

impl Render for HistorySettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let h = m.config.history.clone();
        let total = m.history_total;
        let storage = m.storage;
        let models_bytes = m.models_disk_bytes();
        let models_count = m.models_on_disk();
        let data_dir = m.paths.data_dir.clone();
        let ai_on = m.config.ai.enabled;
        // A server on this Mac keeps the audio here, so only a cloud provider is named.
        let speech_provider = m.active_speech_provider().filter(|p| p.is_cloud()).map(|p| p.name.clone());
        let providers: Vec<(String, bool)> = m.config.ai.providers.iter().map(|p| (p.name.clone(), p.is_cloud())).collect();

        let text_index = TEXT_DAYS.iter().position(|d| *d == h.keep_text_days).unwrap_or(0);
        let audio_index = AUDIO_DAYS.iter().position(|d| Some(*d) == h.keep_audio_days).unwrap_or(0);
        let (m0, m1, m2, m3, m4) = (self.model.clone(), self.model.clone(), self.model.clone(), self.model.clone(), self.model.clone());
        let save_history = Switch::new("save-history", h.enabled).on_toggle(move |on, _, cx| {
            m0.update(cx, |m, cx| m.edit_config(cx, |c| c.history.enabled = on));
        });
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

        let site_icons = Switch::new("site-icons", h.site_icons).on_toggle(move |on, _, cx| {
            m4.update(cx, |m, cx| m.edit_config(cx, |c| c.history.site_icons = on));
        });

        // The limits have no effect on new dictations while history is off.
        let dim = |r: Div| r.when(!h.enabled, |d| d.opacity(0.45));
        let keep = group("History", cx)
            .child(row(
                "Save history",
                if h.enabled {
                    "Sayso keeps each dictation so you can find it again."
                } else {
                    "History is off. Sayso saves no text and no audio from new dictations."
                },
                save_history,
                cx,
            ))
            .child(dim(row("Keep text", "When text expires, Sayso deletes the whole entry.", keep_text, cx)))
            .child(dim(row("Save audio", "Lets you play a dictation again or transcribe it again.", save_audio, cx)))
            .child(dim(row(
                "Keep audio",
                "When audio expires, only the audio file is deleted. The text stays.",
                div().when(h.enabled && !h.save_audio, |d| d.opacity(0.45)).child(keep_audio),
                cx,
            )))
            .child(row(
                "Show site icons",
                "For a dictation into a web page. Sayso gets the icon from the site. If the site gives none, Sayso asks DuckDuckGo, which sees the name of the site.",
                site_icons,
                cx,
            ));

        // A site icon is a request to the site, so the text names it.
        let rest = if h.site_icons {
            "For a dictation into a web page, Sayso asks the site for its icon, and DuckDuckGo if the site gives none. Nothing else is sent."
        } else {
            "Nothing else is sent."
        };
        let mut leaves = div().flex().flex_col().gap(px(10.)).py(px(14.)).child(
            text::body(
                match &speech_provider {
                    Some(name) => format!(
                        "Your model is a cloud model. Sayso sends the audio of each dictation and your dictionary words to {name}. When a style uses AI, Sayso sends the transcript, the style prompt, and your dictionary words to the provider of that style. {rest}"
                    ),
                    None => format!("Speech recognition runs on this {}. Your audio never leaves it. When a style uses AI, Sayso sends the transcript, the style prompt, and your dictionary words to the provider of that style. {rest}", crate::shell::COMPUTER),
                },
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
                    .child(text::ui(format!("AI is off. No text leaves this {}.", crate::shell::COMPUTER), 13., FontWeight::MEDIUM, c.ink)),
            );
        } else {
            for (name, cloud) in providers {
                leaves = leaves.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(text::ui(name, 13., FontWeight::SEMIBOLD, c.ink))
                        .child(if cloud { Badge::new("Cloud", BadgeTone::Accent) } else { Badge::new(format!("This {}", crate::shell::COMPUTER), BadgeTone::Muted) }),
                );
            }
        }
        let privacy = group(&format!("What leaves this {}", crate::shell::COMPUTER), cx).child(leaves.border_b_1().border_color(c.rule));

        // One row of the data group: a dot in the color of its bar segment, the size, and an action.
        let data_row = |dot: Hsla, title: &str, description: String, bytes: u64, action: AnyElement| {
            div()
                .flex()
                .items_center()
                .w_full()
                .gap(px(16.))
                .py(px(14.))
                .border_b_1()
                .border_color(c.rule)
                .child(div().flex_none().size(px(8.)).rounded_full().bg(dot))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .flex_1()
                        .min_w_0()
                        .child(text::ui(title.to_string(), 15., FontWeight::SEMIBOLD, c.ink).line_height(px(18.)))
                        .child(text::ui(description, 13., FontWeight::NORMAL, c.graphite).line_height(px(16.))),
                )
                .child(text::mono(format_size(bytes), 13., c.ink).flex_none().w(px(72.)).text_right())
                .child(div().flex().flex_none().justify_end().w(px(172.)).child(action))
        };
        // A delete button that asks one time more before it deletes.
        let wipe_control = |wipe: Wipe, id: &'static str, label: &'static str, confirm_label: &'static str, empty: bool| {
            if self.confirm == Some(wipe) {
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(Button::new((id, 1usize), "Cancel").ghost().small().on_click(cx.listener(|this, _, _, cx| {
                        this.confirm = None;
                        cx.notify();
                    })))
                    .child(Button::new((id, 2usize), confirm_label).danger().small().on_click(cx.listener(move |this, _, _, cx| {
                        this.confirm = None;
                        this.model.update(cx, |m, cx| match wipe {
                            Wipe::History => m.clear_history(cx),
                            Wipe::Audio => m.clear_audio(cx),
                        });
                    })))
                    .into_any_element()
            } else {
                Button::new((id, 0usize), label)
                    .danger()
                    .small()
                    .disabled(empty)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.confirm = Some(wipe);
                        cx.notify();
                    }))
                    .into_any_element()
            }
        };
        let history_control = wipe_control(Wipe::History, "clear-history", "Clear history", "Delete all", total == 0);
        let audio_control = wipe_control(Wipe::Audio, "delete-audio", "Delete audio", "Delete audio", storage.audio_files == 0);

        let entries = format!("{} {}", format_count(total), if total == 1 { "entry" } else { "entries" });
        let history_desc = if self.confirm == Some(Wipe::History) {
            format!("Delete all {entries} and their audio? You cannot undo this.")
        } else if total == 0 {
            "History is empty.".to_string()
        } else if h.enabled {
            format!("{entries}.")
        } else {
            format!("{entries}. They stay until you clear them.")
        };
        let recordings =
            format!("{} {}", format_count(storage.audio_files), if storage.audio_files == 1 { "recording" } else { "recordings" });
        let audio_desc = if self.confirm == Some(Wipe::Audio) {
            format!("Delete {recordings}? The text of each entry stays. You cannot undo this.")
        } else if storage.audio_files == 0 {
            "No audio is saved.".to_string()
        } else {
            format!("{recordings}. Delete audio keeps the text of each entry.")
        };
        let models_desc = match models_count {
            0 => "No local model is downloaded.".to_string(),
            1 => "1 local model. Delete a model on the Models page.".to_string(),
            n => format!("{n} local models. Delete a model on the Models page."),
        };
        let open_models = {
            let model = self.model.clone();
            crate::hub::pages::kit::link("open-models", "Open Models", 13., cx)
                .on_click(move |_, _, cx| model.update(cx, |m, cx| m.navigate(crate::hub::Route::Models, cx)))
        };

        // The bar. A part that holds data is never thinner than 1% of the bar.
        let parts = [(models_bytes, c.ink), (storage.audio_bytes, c.accent), (storage.database_bytes, c.pencil)];
        let all_bytes: u64 = parts.iter().map(|(bytes, _)| bytes).sum();
        let mut bar = div()
            .flex()
            .flex_1()
            .gap(px(2.))
            .h(px(10.))
            .p(px(2.))
            .rounded_full()
            .bg(c.deboss)
            .shadow(vec![BoxShadow::new(px(0.), px(1.), c.shadow(0.28)).blur_radius(px(2.)).inset()]);
        for (bytes, color) in parts.into_iter().filter(|(bytes, _)| *bytes > 0) {
            let mut part = div().h(px(6.)).rounded_full().bg(color);
            part.style().flex_grow = Some((bytes as f32 / all_bytes as f32).max(0.01));
            part.style().flex_shrink = Some(1.);
            part.style().flex_basis = Some(relative(0.).into());
            bar = bar.child(part);
        }
        let show = crate::hub::pages::kit::link(
            "show-data",
            crate::shell::os_text!("Show in Finder", "Show in Explorer", "Open folder"),
            13.,
            cx,
        )
        .on_click(move |_, _, _| {
            let _ = std::fs::create_dir_all(&data_dir);
            AppModel::open_path(&data_dir);
        });
        let summary = div()
            .flex()
            .items_center()
            .w_full()
            .gap(px(28.))
            .pt(px(12.))
            .pb(px(16.))
            .border_b_1()
            .border_color(c.rule)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_baseline()
                    .gap(px(8.))
                    .child(text::display(format_size(all_bytes), 32., &c))
                    .child(text::ui("in total", 13., FontWeight::NORMAL, c.graphite)),
            )
            .child(bar)
            .child(show);
        let data = group(&format!("Data on this {}", crate::shell::COMPUTER), cx)
            .child(summary)
            .child(data_row(c.pencil, "History", history_desc, storage.database_bytes, history_control))
            .child(data_row(c.accent, "Audio", audio_desc, storage.audio_bytes, audio_control))
            .child(data_row(c.ink, "Models", models_desc, models_bytes, open_models.into_any_element()));
        // Shown only on a Mac that has Pindrop data.
        let import = self.model.read(cx).pindrop_found.then(|| {
            let state = self.model.read(cx).pindrop_import.clone();
            let running = state == crate::pindrop_import::PindropImport::Running;
            let button = Button::new("import-pindrop", "Import").small().disabled(running).on_click(cx.listener(|this, _, _, cx| {
                this.model.update(cx, |m, cx| m.import_pindrop(cx));
            }));
            group("Import", cx).child(kit::row_s("Import from Pindrop".into(), state.message(), button, cx))
        });
        kit::page(
            "history-page",
            "History and privacy",
            crate::shell::os_text!("What Sayso keeps, for how long, and what leaves this Mac.", "What Sayso keeps, for how long, and what leaves this PC.", "What Sayso keeps, for how long, and what leaves this computer."),
            kit::body().child(keep).child(data).child(privacy).children(import),
            cx,
        )
    }
}
