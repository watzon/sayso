//! Step 2: choose a speech model. Choosing one starts its download, and
//! the step does not continue until a model is on disk.

use super::{OnboardingView, heading, size_text};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_core::models::{ModelId, ModelInfo, default_model};
use sayso_core::stt::ModelStatus;
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper::{self, PaperStyled};
use sayso_ui::{ActivePaper, Colors, text};
use std::time::{Duration, Instant};

/// The three models onboarding offers: the best for English, for European
/// languages, and for every other language.
fn choices() -> [ModelId; 3] {
    [default_model(), ModelId::new("parakeet-ultra"), ModelId::new("whisper-large-v3-turbo")]
}

impl OnboardingView {
    /// Start the active model's download if it is not on disk or downloading.
    pub(super) fn ensure_download(&mut self, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| {
            let id = m.config.dictation.model.clone();
            if matches!(m.status_of(&id), ModelStatus::NotDownloaded | ModelStatus::Failed { .. }) {
                m.download_model(id, cx);
            }
        });
    }

    fn choose_model(&mut self, id: ModelId, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| {
            m.set_active_model(id.clone(), cx);
            if matches!(m.status_of(&id), ModelStatus::NotDownloaded | ModelStatus::Failed { .. }) {
                m.download_model(id, cx);
            }
        });
    }

    /// "about 40 s left", from the bytes seen in the last seconds.
    fn time_left(&mut self, id: &ModelId, done: u64, total: u64) -> Option<String> {
        let now = Instant::now();
        let samples = match &mut self.download {
            Some((m, s)) if m == id => s,
            _ => {
                self.download = Some((id.clone(), Default::default()));
                &mut self.download.as_mut().unwrap().1
            }
        };
        if samples.back().is_none_or(|(_, b)| *b != done) {
            samples.push_back((now, done));
        }
        while samples.len() > 2 && samples.front().is_some_and(|(t, _)| now.duration_since(*t) > Duration::from_secs(8)) {
            samples.pop_front();
        }
        let (t0, b0) = *samples.front()?;
        let secs = now.duration_since(t0).as_secs_f64();
        if secs < 1.0 || done <= b0 || total <= done {
            return None;
        }
        let rate = (done - b0) as f64 / secs;
        let left = (total - done) as f64 / rate;
        Some(if left < 90.0 { format!("about {} s left", left.ceil().max(1.0) as u64) } else { format!("about {} min left", (left / 60.0).ceil() as u64) })
    }

    pub(super) fn model_step(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let active = m.config.dictation.model.clone();
        let engine = m.services.engine.is_some();
        let infos: Vec<ModelInfo> = choices().iter().filter_map(sayso_core::models::find).collect();
        let status = m.status_of(&active);
        let active_info = m.active_model();

        let mut cards = div().flex().gap(px(14.));
        for (i, info) in infos.into_iter().enumerate() {
            let selected = info.id == active;
            let id = info.id.clone();
            let lang = info.language_label();
            cards = cards.child(
                div()
                    .id(("model-card", i))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(10.))
                    .p(px(18.))
                    .rounded(px(14.))
                    .bg(c.sheet_raised)
                    .cursor_pointer()
                    .map(|d| if selected { d.shadow(super::ring(&c)) } else { d.shadow(paper::raised(&c)).hover(|s| s.bg(c.sheet)) })
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_model(id.clone(), cx)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .h(px(22.))
                            .child(radio_dot(selected, cx))
                            .when(info.recommended, |d| d.child(Badge::new("Recommended", BadgeTone::Accent))),
                    )
                    .child(text::title(info.name.clone(), 20., &c).line_height(px(24.)))
                    .child(text::ui(info.description.clone(), 13., FontWeight::NORMAL, c.graphite).line_height(px(19.)))
                    .child(
                        text::mono(format!("{} · {lang}", size_text(info.size_bytes)), 12., c.ink)
                            .font_weight(FontWeight::NORMAL)
                            .pt(px(4.)),
                    ),
            );
        }

        let strip = self.download_strip(&active_info, status, engine, &c, cx);
        div()
            .flex()
            .flex_col()
            .gap(px(22.))
            .pt(px(28.))
            .px(px(72.))
            .pb(px(24.))
            .child(heading(
                "Choose a speech model",
                "Sayso needs one downloaded model to work. You can add or switch models later.",
                &c,
            ))
            .child(cards)
            .child(strip)
            .into_any_element()
    }

    fn download_strip(&mut self, info: &ModelInfo, status: ModelStatus, engine: bool, c: &Colors, cx: &mut Context<Self>) -> AnyElement {
        let well = || div().flex().flex_col().gap(px(10.)).py(px(16.)).px(px(18.)).rounded(px(12.)).debossed(c);
        if !engine {
            return crate::hub::settings::kit::notice(
                BannerKind::Warning,
                "The speech engine is not installed, so Sayso cannot download a model. Reinstall Sayso, then open it again.", cx)
            .into_any_element();
        }
        let parts_note = matches!(info.engine, sayso_core::models::EngineKind::ParakeetUnified { .. })
            .then_some("Includes the final pass, the live preview, and the dictionary helper.");
        match status {
            ModelStatus::Downloading { fraction, bytes_done, bytes_total } => {
                let total = if bytes_total > 0 { bytes_total } else { info.size_bytes };
                let mut right = format!("{} of {}", size_text(bytes_done), size_text(total));
                if let Some(left) = self.time_left(&info.id, bytes_done, total) {
                    right = format!("{right} · {left}");
                }
                well()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(text::ui(format!("Downloading {}", info.name), 14., FontWeight::SEMIBOLD, c.ink))
                            .child(text::mono(right, 12., c.graphite).font_weight(FontWeight::NORMAL)),
                    )
                    .child(track(fraction, 8., c))
                    .when_some(parts_note, |d, n| d.child(text::ui(n, 12., FontWeight::NORMAL, c.graphite)))
                    .into_any_element()
            }
            ModelStatus::Failed { message } => div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(div().flex_1().min_w_0().child(crate::hub::settings::kit::notice(
                    BannerKind::Warning,
                    format!("The download of {} stopped: {message}. Check your internet connection, then try again.", info.name), cx)))
                .child(Button::new("retry-download", "Try again").on_click(cx.listener(|this, _, _, cx| this.ensure_download(cx))))
                .into_any_element(),
            ModelStatus::NotDownloaded => well()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(text::ui(format!("{} is not downloaded yet", info.name), 14., FontWeight::SEMIBOLD, c.ink))
                        .child(
                            Button::new("start-download", format!("Download {}", size_text(info.size_bytes)))
                                .small()
                                .icon(Icon::Download)
                                .on_click(cx.listener(|this, _, _, cx| this.ensure_download(cx))),
                        ),
                )
                .child(text::ui("Choose a model above, then press Download. You can continue when the download is done.", 12., FontWeight::NORMAL, c.graphite))
                .into_any_element(),
            ModelStatus::Optimizing | ModelStatus::Downloaded | ModelStatus::Ready => {
                let detail = match status {
                    ModelStatus::Ready if cfg!(target_os = "macos") => "Downloaded and ready on this Mac.",
                    ModelStatus::Ready => "Downloaded and ready on this PC.",
                    ModelStatus::Optimizing if cfg!(target_os = "macos") => "Downloaded. Sayso is optimizing it for your Mac. This takes a minute the first time.",
                    ModelStatus::Optimizing => "Downloaded. Sayso is loading it.",
                    _ => "Downloaded.",
                };
                well()
                    .child(super::check_line_w(format!("{} · {}", info.name, size_text(info.size_bytes)), FontWeight::SEMIBOLD, c))
                    .child(text::ui(detail, 12., FontWeight::NORMAL, c.graphite))
                    .into_any_element()
            }
        }
    }
}

/// A progress track. The design uses the darker deboss shade inside a well.
pub(super) fn track(fraction: f32, height: f32, c: &Colors) -> Div {
    div()
        .flex()
        .w_full()
        .h(px(height))
        .rounded_full()
        .bg(c.deboss_shade)
        .shadow(vec![BoxShadow::new(px(0.), px(1.), c.shadow(0.3)).blur_radius(px(2.)).inset()])
        .child(div().h_full().w(relative(fraction.clamp(0.0, 1.0))).rounded_full().bg(c.ink_fill))
}
