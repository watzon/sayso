//! Step 2: choose a speech model. Choosing one starts its download, and
//! the step does not continue until a model is on disk. The step offers
//! three models, and a list of all local models (`all_models`).

use super::{OnboardingView, arrow_target, heading, link, plain, plain_mono, plain_title, size_text};
use crate::hub::pages::models::size_label;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sayso_i18n::t;
use sayso_core::models::{ModelId, ModelInfo};
use sayso_core::stt::ModelStatus;
use sayso_ui::a11y::{A11yExt as _, LiveStatus, Pressable, Role};
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::paper::{self, PaperStyled};
use sayso_ui::{ActivePaper, Colors, text};
use std::time::{Duration, Instant};

/// The model to suggest for the dictation language.
pub(super) fn suggested(language: &str) -> ModelId {
    sayso_core::models::suggested_for(language, crate::shell::model_runs_here)
}

/// The three models onboarding offers. The first is the suggested model for
/// the dictation language. Next comes a GPU model that understands the
/// language, when this computer can run one. The others come from the best
/// models for English, for European languages, and for every other language:
/// first the ones that understand the dictation language.
fn choices(language: &str) -> Vec<ModelId> {
    choices_with(language, crate::shell::model_runs_here)
}

/// [`choices`] for a computer whose `model_runs_here` is `runs_here`.
fn choices_with(language: &str, runs_here: impl Fn(&ModelInfo) -> bool) -> Vec<ModelId> {
    let (english, european) = crate::shell::os_text!(("parakeet-unified-en", "parakeet-ultra"), ("parakeet-tdt-v2", "parakeet-tdt-v3"));
    let mut rest = [english, european, "whisper-large-v3-turbo"].map(ModelId::new);
    rest.sort_by_key(|id| !sayso_core::models::find(id).is_some_and(|m| m.understands(language)));
    let gpu = sayso_core::models::catalog()
        .into_iter()
        .find(|m| m.engine.needs_gpu() && m.understands(language) && runs_here(m))
        .map(|m| m.id);
    let mut list = vec![sayso_core::models::suggested_for(language, &runs_here)];
    for id in gpu.into_iter().chain(rest) {
        if !list.contains(&id) {
            list.push(id);
        }
    }
    list.truncate(3);
    list
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

    pub(super) fn choose_model(&mut self, id: ModelId, cx: &mut Context<Self>) {
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
        Some(if left < 90.0 {
            t!("onboarding.model.seconds_left", seconds = left.ceil().max(1.0) as u64)
        } else {
            t!("onboarding.model.minutes_left", minutes = (left / 60.0).ceil() as u64)
        })
    }

    pub(super) fn model_step(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let active = m.config.dictation.model.clone();
        let choices = choices(&m.config.dictation.language);
        let engine = m.services.engine.is_some();
        let infos: Vec<ModelInfo> = choices.iter().filter_map(sayso_core::models::find).collect();
        let status = m.status_of(&active);
        let active_info = m.active_model();

        // One choice of a set: the cards are radio buttons, and the arrow keys
        // change the choice, as in a `Segmented`.
        let count = infos.len();
        let ids: Vec<ModelId> = infos.iter().map(|i| i.id.clone()).collect();
        let selected_index = ids.iter().position(|id| *id == active);
        let mut cards = self
            .columns()
            .items_stretch()
            .id("model-choices")
            .role(Role::RadioGroup)
            .aria_label(t!("onboarding.model.title"))
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, window, cx| {
                let Some((next, forward)) = arrow_target(&e.keystroke.key, selected_index, count) else { return };
                cx.stop_propagation();
                this.choose_model(ids[next].clone(), cx);
                if forward { window.focus_next(cx) } else { window.focus_prev(cx) }
            }));
        for (i, info) in infos.into_iter().enumerate() {
            let selected = info.id == active;
            let id = info.id.clone();
            let lang = info.language_label();
            let details = format!("{} {} · {lang}", info.description, size_text(info.size_bytes));
            cards = cards.child(
                Pressable::new(SharedString::from(format!("model-card-{}", info.id)))
                    .a11y_radio(info.name.clone(), selected, i, count)
                    .aria_description(details.clone())
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(10.))
                    .p(px(18.))
                    .rounded(px(14.))
                    .bg(c.sheet_raised)
                    .cursor_pointer()
                    .map(|d| if selected { d.shadows(super::ring(&c)) } else { d.shadows(paper::raised(&c)).hover(|s| s.bg(c.sheet)) })
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_model(id.clone(), cx)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .h(px(22.))
                            .child(radio_dot(selected, cx))
                            .when(i == 0, |d| d.child(Badge::new(t!("onboarding.model.recommended"), BadgeTone::Accent))),
                    )
                    .child(plain_title(info.name.clone(), 20., &c).line_height(px(24.)))
                    .child(plain(info.description.clone(), 13., FontWeight::NORMAL, c.graphite).line_height(px(19.)))
                    .child(plain_mono(format!("{} · {lang}", size_text(info.size_bytes)), 12., c.ink).pt(px(4.))),
            );
        }

        // The way to the list of all models. A model from that list shows here as the choice.
        let open = cx.listener(|this, _, window, cx| this.open_all_models(window, cx));
        let more = if choices.contains(&active) {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(text::ui(t!("onboarding.model.different_model"), 13., FontWeight::NORMAL, c.graphite))
                .child(link("all-models-link", t!("onboarding.model.show_all", count = self.local_models(cx).len()), 13., &c).on_click(open))
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .py(px(12.))
                .px(px(18.))
                .rounded(px(14.))
                .bg(c.sheet_raised)
                .shadow(super::ring(&c))
                .child(radio_dot(true, cx))
                .child(text::title(active_info.name.clone(), 16., &c).line_height(px(20.)))
                .child(
                    text::mono(format!("{} · {}", size_label(&active_info), active_info.language_label()), 12., c.ink)
                        .font_weight(FontWeight::NORMAL)
                        .flex_1(),
                )
                .child(link("all-models-link", t!("onboarding.model.change"), 13., &c).label(t!("onboarding.model.change_label")).on_click(open))
        };

        let strip = self.download_strip(&active_info, status, engine, &c, cx);
        div()
            .flex()
            .flex_col()
            .gap(px(22.))
            .pt(px(28.))
            .px(px(self.side()))
            .pb(px(24.))
            .child(heading(&t!("onboarding.model.title"), &t!("onboarding.model.lead"), &c))
            .child(div().flex().flex_col().gap(px(14.)).child(cards).child(more))
            .child(strip)
            .into_any_element()
    }

    fn download_strip(&mut self, info: &ModelInfo, status: ModelStatus, engine: bool, c: &Colors, cx: &mut Context<Self>) -> AnyElement {
        // One live node for the state of the download: a screen reader reads
        // its text when the state changes, not at each step of the progress.
        let well = |state: String| {
            LiveStatus::new("model-status", state).flex().flex_col().gap(px(10.)).py(px(16.)).px(px(18.)).rounded(px(12.)).debossed(c)
        };
        if !engine {
            return crate::hub::settings::kit::notice(
                BannerKind::Warning,
                t!("onboarding.model.no_engine"), cx)
            .into_any_element();
        }
        let parts_note = matches!(info.engine, sayso_core::models::EngineKind::ParakeetUnified { .. })
            .then(|| t!("onboarding.model.parts_note"));
        match status {
            ModelStatus::Downloading { fraction, bytes_done, bytes_total } => {
                let total = if bytes_total > 0 { bytes_total } else { info.size_bytes };
                // macOS downloads Apple Speech and gives no sizes.
                let mut right = if total > 0 { t!("onboarding.model.progress", done = size_text(bytes_done), total = size_text(total)) } else { String::new() };
                if let Some(left) = self.time_left(&info.id, bytes_done, total) {
                    right = t!("onboarding.model.progress_left", progress = right, left = left);
                }
                let title = t!("onboarding.model.downloading", name = info.name);
                well(title.clone())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(text::ui(title.clone(), 14., FontWeight::SEMIBOLD, c.ink))
                            .child(text::mono(right, 12., c.graphite).font_weight(FontWeight::NORMAL)),
                    )
                    .child(track(fraction, 8., title, c))
                    .when_some(parts_note, |d, n| d.child(text::ui(n, 12., FontWeight::NORMAL, c.graphite)))
                    .into_any_element()
            }
            ModelStatus::Failed { message, reason } => div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(div().flex_1().min_w_0().child(crate::hub::settings::kit::notice(
                    BannerKind::Warning,
                    match reason {
                        Some(reason) => t!("onboarding.model.download_failed_reason", name = info.name, reason = crate::engine_text::describe(Some(reason), &message)),
                        None => t!("onboarding.model.download_failed", name = info.name, error = message),
                    }, cx)))
                .child(Button::new("retry-download", t!("onboarding.model.try_again")).on_click(cx.listener(|this, _, _, cx| this.ensure_download(cx))))
                .into_any_element(),
            ModelStatus::NotDownloaded => well(t!("onboarding.model.not_downloaded", name = info.name))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(text::ui(t!("onboarding.model.not_downloaded", name = info.name), 14., FontWeight::SEMIBOLD, c.ink))
                        .child(
                            Button::new("start-download", if info.size_bytes > 0 { t!("onboarding.model.download_size", size = size_text(info.size_bytes)) } else { t!("onboarding.model.download") })
                                .small()
                                .icon(Icon::Download)
                                .on_click(cx.listener(|this, _, _, cx| this.ensure_download(cx))),
                        ),
                )
                .child(text::ui(t!("onboarding.model.choose_then_download"), 12., FontWeight::NORMAL, c.graphite))
                .into_any_element(),
            ModelStatus::Optimizing | ModelStatus::Downloaded | ModelStatus::Ready => {
                let detail = match status {
                    ModelStatus::Ready => t!(crate::shell::os_text!("onboarding.model.ready_mac", "onboarding.model.ready_windows", "onboarding.model.ready_other")),
                    ModelStatus::Optimizing => t!(crate::shell::os_text!("onboarding.model.optimizing_mac", "onboarding.model.optimizing_other")),
                    _ => t!("onboarding.model.downloaded"),
                };
                well(format!("{}. {detail}", info.name))
                    .child(super::check_line_w(format!("{} · {}", info.name, size_label(info)), FontWeight::SEMIBOLD, c))
                    .child(text::ui(detail, 12., FontWeight::NORMAL, c.graphite))
                    .into_any_element()
            }
        }
    }
}

/// A progress track. The design uses the darker deboss shade inside a well.
/// `label` says what it measures.
pub(super) fn track(fraction: f32, height: f32, label: String, c: &Colors) -> Stateful<Div> {
    div()
        .id("download-progress")
        .role(Role::ProgressIndicator)
        .aria_label(label)
        .aria_min_numeric_value(0.)
        .aria_max_numeric_value(100.)
        .aria_numeric_value((fraction.clamp(0.0, 1.0) * 100.).round() as f64)
        .flex()
        .w_full()
        .h(px(height))
        .rounded_full()
        .bg(c.deboss_shade)
        .shadow(vec![BoxShadow::new(px(0.), px(1.), c.shadow(0.3)).blur_radius(px(2.)).inset()])
        .child(div().h_full().w(relative(fraction.clamp(0.0, 1.0))).rounded_full().bg(c.ink_fill))
}

#[cfg(test)]
mod tests {
    use super::{choices, choices_with, suggested};
    use sayso_core::models::{ModelInfo, default_model, find};

    #[test]
    fn choices_are_three_different_models_with_the_suggested_one_first() {
        for language in ["en", "de", "zh", "ja", "ko", "ar", "auto"] {
            let ids = choices(language);
            assert_eq!(ids.len(), 3, "{language}: {ids:?}");
            assert!(ids.iter().all(|id| find(id).is_some()), "{language}: {ids:?}");
            assert!(ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2], "{language}: {ids:?}");
            assert_eq!(ids[0], suggested(language));
        }
        assert_eq!(choices("en")[0], default_model());
    }

    #[test]
    fn a_gpu_model_comes_second_for_its_language_only_when_it_runs_here() {
        let cpu_only = |m: &ModelInfo| !m.engine.needs_gpu();
        let with_gpu = |_: &ModelInfo| true;
        for language in ["en", "de", "auto"] {
            assert!(choices_with(language, cpu_only).iter().all(|id| !find(id).unwrap().engine.needs_gpu()), "{language}");
        }
        let ids = choices_with("en", with_gpu);
        assert_eq!(ids[0], default_model());
        let gpu_models: Vec<_> = sayso_core::models::catalog().into_iter().filter(|m| m.engine.needs_gpu()).collect();
        if let Some(gpu) = gpu_models.first() {
            assert_eq!(ids[1], gpu.id, "{ids:?}");
            assert_eq!(ids.len(), 3);
            // English only: no other language is offered it.
            for language in ["de", "auto"] {
                assert!(!choices_with(language, with_gpu).contains(&gpu.id), "{language}");
            }
        }
    }

    #[test]
    fn the_second_choice_understands_the_dictation_language_when_one_can() {
        for language in ["de", "zh", "ja"] {
            let second = find(&choices(language)[1]).unwrap();
            assert!(second.understands(language), "{language}: {}", second.id.as_str());
        }
    }
}
