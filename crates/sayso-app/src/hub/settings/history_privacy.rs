//! Settings › History and privacy: the history switch, retention, the data on
//! this computer, and what leaves it. See the Paper page "History off".

use super::kit::{self, group, row};
use crate::model::AppModel;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::models::format_size;
use sayso_core::stats::format_count;
use sayso_i18n::t;
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
    /// The history revision of the last storage measure.
    storage_revision: Option<u64>,
}

impl HistorySettings {
    pub fn new(model: Entity<AppModel>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { model, confirm: None, storage_revision: None }
    }
}

impl Render for HistorySettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = cx.paper().colors;
        // Measure the storage when the page shows, and again after each change of history.
        let revision = self.model.read(cx).history_revision;
        if self.storage_revision != Some(revision) {
            self.storage_revision = Some(revision);
            let model = self.model.clone();
            cx.defer(move |cx| model.update(cx, |m, cx| m.refresh_storage(cx)));
        }
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
        let save_history = Switch::new("save-history", h.enabled).label(t!("settings.history.save")).on_toggle(move |on, _, cx| {
            m0.update(cx, |m, cx| m.edit_config(cx, |c| c.history.enabled = on));
        });
        let keep_text = Segmented::new(
            "keep-text",
            [
                t!("settings.history.forever"),
                t!("settings.history.days", count = 90),
                t!("settings.history.days", count = 30),
                t!("settings.history.days", count = 7),
            ],
            text_index,
        )
        .label(t!("settings.history.keep_text"))
        .on_select(move |i, _, cx| {
            m1.update(cx, |m, cx| m.edit_config(cx, |c| c.history.keep_text_days = TEXT_DAYS[i]));
        });
        let save_audio = Switch::new("save-audio", h.save_audio).label(t!("settings.history.save_audio")).on_toggle(move |on, _, cx| {
            m2.update(cx, |m, cx| m.edit_config(cx, |c| c.history.save_audio = on));
        });
        let keep_audio = Segmented::new(
            "keep-audio",
            [t!("settings.history.days", count = 30), t!("settings.history.days", count = 7), t!("settings.history.days", count = 1)],
            audio_index,
        )
            .label(t!("settings.history.keep_audio"))
            .on_select(move |i, _, cx| {
                m3.update(cx, |m, cx| m.edit_config(cx, |c| c.history.keep_audio_days = Some(AUDIO_DAYS[i])));
            });

        let site_icons = Switch::new("site-icons", h.site_icons).label(t!("settings.history.site_icons")).on_toggle(move |on, _, cx| {
            m4.update(cx, |m, cx| m.edit_config(cx, |c| c.history.site_icons = on));
        });

        // The limits have no effect on new dictations while history is off.
        let dim = |r: Div| r.when(!h.enabled, |d| d.opacity(0.45));
        let keep = group(&t!("settings.history.history"), cx)
            .child(row(
                &t!("settings.history.save"),
                &if h.enabled { t!("settings.history.save_detail") } else { t!("settings.history.save_detail_off") },
                save_history,
                cx,
            ))
            .child(dim(row(&t!("settings.history.keep_text"), &t!("settings.history.keep_text_detail"), keep_text, cx)))
            .child(dim(row(&t!("settings.history.save_audio"), &t!("settings.history.save_audio_detail"), save_audio, cx)))
            .child(dim(row(
                &t!("settings.history.keep_audio"),
                &t!("settings.history.keep_audio_detail"),
                div().when(h.enabled && !h.save_audio, |d| d.opacity(0.45)).child(keep_audio),
                cx,
            )))
            .child(row(&t!("settings.history.site_icons"), &t!("settings.history.site_icons_detail"), site_icons, cx));

        // A site icon is a request to the site, so the text names it.
        let mut leaves = div().flex().flex_col().gap(px(10.)).py(px(14.)).child(
            text::body(
                match (&speech_provider, h.site_icons) {
                    (Some(name), true) => t!("settings.history.leaves_cloud_icons", name = name),
                    (Some(name), false) => t!("settings.history.leaves_cloud", name = name),
                    (None, true) => t!("settings.history.leaves_local_icons", computer = crate::shell::computer()),
                    (None, false) => t!("settings.history.leaves_local", computer = crate::shell::computer()),
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
                    .child(text::ui(t!("settings.history.ai_off", computer = crate::shell::computer()), 13., FontWeight::MEDIUM, c.ink)),
            );
        } else {
            for (i, (name, cloud)) in providers.into_iter().enumerate() {
                leaves = leaves.child(
                    div()
                        .id(("leaves-provider", i))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(text::ui(name, 13., FontWeight::SEMIBOLD, c.ink))
                        .child(if cloud { Badge::new(t!("settings.history.cloud"), BadgeTone::Accent) } else { Badge::new(t!("settings.history.this_computer", computer = crate::shell::computer()), BadgeTone::Muted) }),
                );
            }
        }
        let privacy = group(&t!("settings.history.leaves", computer = crate::shell::computer()), cx).child(leaves.border_b_1().border_color(c.rule));

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
        let wipe_control = |wipe: Wipe, id: &'static str, label: String, confirm_label: String, what: String, empty: bool| {
            if self.confirm == Some(wipe) {
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(Button::new((id, 1usize), t!("settings.history.cancel")).ghost().small().on_click(cx.listener(|this, _, _, cx| {
                        this.confirm = None;
                        cx.notify();
                    })))
                    .child(Button::new((id, 2usize), confirm_label.clone()).a11y_label(format!("{confirm_label}, {what}")).danger().small().on_click(cx.listener(move |this, _, _, cx| {
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
        let history_control = wipe_control(Wipe::History, "clear-history", t!("settings.history.clear_history"), t!("settings.history.delete_all"), t!("settings.history.data_history"), total == 0);
        let audio_control =
            wipe_control(Wipe::Audio, "delete-audio", t!("settings.history.delete_audio"), t!("settings.history.delete_audio"), t!("settings.history.data_audio"), storage.audio_files == 0);

        let entries = format_count(total);
        let history_desc = if self.confirm == Some(Wipe::History) {
            t!("settings.history.entries_confirm", count = total, number = entries)
        } else if total == 0 {
            t!("settings.history.entries_none")
        } else if h.enabled {
            t!("settings.history.entries", count = total, number = entries)
        } else {
            t!("settings.history.entries_kept", count = total, number = entries)
        };
        let recordings = format_count(storage.audio_files);
        let audio_desc = if self.confirm == Some(Wipe::Audio) {
            t!("settings.history.recordings_confirm", count = storage.audio_files, number = recordings)
        } else if storage.audio_files == 0 {
            t!("settings.history.recordings_none")
        } else {
            t!("settings.history.recordings", count = storage.audio_files, number = recordings)
        };
        let models_desc = match models_count {
            0 => t!("settings.history.models_none"),
            n => t!("settings.history.models", count = n),
        };
        let open_models = {
            let model = self.model.clone();
            crate::hub::pages::kit::link("open-models", t!("settings.history.open_models"), 13., cx)
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
            t!(crate::shell::os_text!("settings.history.show_mac", "settings.history.show_windows", "settings.history.show_other")),
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
                    .child(text::display_figure(format_size(all_bytes), 32., &c))
                    .child(text::ui(t!("settings.history.in_total"), 13., FontWeight::NORMAL, c.graphite)),
            )
            .child(bar)
            .child(show);
        let data = group(&t!("settings.history.data", computer = crate::shell::computer()), cx)
            .child(summary)
            .child(data_row(c.pencil, &t!("settings.history.data_history"), history_desc, storage.database_bytes, history_control))
            .child(data_row(c.accent, &t!("settings.history.data_audio"), audio_desc, storage.audio_bytes, audio_control))
            .child(data_row(c.ink, &t!("settings.history.data_models"), models_desc, models_bytes, open_models.into_any_element()));
        // Shown only on a Mac that has Pindrop data.
        let import = self.model.read(cx).pindrop_found.then(|| {
            let state = self.model.read(cx).pindrop_import.clone();
            let running = state == crate::pindrop_import::PindropImport::Running;
            let button = Button::new("import-pindrop", t!("settings.history.import")).small().disabled(running).on_click(cx.listener(|this, _, _, cx| {
                this.model.update(cx, |m, cx| m.import_pindrop(cx));
            }));
            group(&t!("settings.history.import_group"), cx).child(kit::row_s(t!("settings.history.import_pindrop"), state.message(), button, cx))
        });
        kit::page(
            "history-page",
            &t!("settings.history.title"),
            &t!(crate::shell::os_text!("settings.history.subtitle_mac", "settings.history.subtitle_windows", "settings.history.subtitle_other")),
            kit::body().child(keep).child(data).child(privacy).children(import),
            cx,
        )
    }
}
