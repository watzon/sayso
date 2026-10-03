//! Step 6: try a dictation into a real text field in this window.
//!
//! Our window is the focused app, so the toggle hotkey records and the paste
//! lands in this field. The result line comes from the newest history entry.

use super::{OnboardingView, heading};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::*;
use sayso_core::history::{EnhanceOutcome, HistoryEntry, InsertOutcome};
use sayso_core::stt::ModelStatus;
use sayso_ui::assets::Icon;
use sayso_ui::components::*;
use sayso_ui::{ActivePaper, Colors, text};

pub const SENTENCE: &str = crate::shell::os_text!(
    "Sayso writes what I say, in any app on my Mac, and nothing leaves this computer.",
    "Sayso writes what I say, in any app on my PC, and nothing leaves this computer.",
    "Sayso writes what I say, in any app I use, and nothing leaves this computer.",
);

pub(super) struct Practice {
    field: Entity<TextareaState>,
    /// Entries before this time belong to earlier attempts.
    since: chrono::DateTime<chrono::Utc>,
}

impl Practice {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        let field = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 5).placeholder("Click here, then press your hotkey and read the sentence."));
        Self { field, since: chrono::Utc::now() }
    }

    pub(super) fn enter(&mut self, window: &mut Window, cx: &mut App) {
        self.since = chrono::Utc::now();
        self.field.update(cx, |s, cx| s.focus(window, cx));
    }
}

/// Words of the target sentence that the result has, in order (longest common subsequence).
fn words_correct(result: &str) -> (usize, usize) {
    let norm = |s: &str| -> Vec<String> {
        s.split_whitespace()
            .map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase())
            .filter(|w| !w.is_empty())
            .collect()
    };
    let a = norm(SENTENCE);
    let b = norm(result);
    let mut dp = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            dp[i][j] = if a[i - 1] == b[j - 1] { dp[i - 1][j - 1] + 1 } else { dp[i - 1][j].max(dp[i][j - 1]) };
        }
    }
    (dp[a.len()][b.len()], a.len())
}

/// Time from the stop to the insert: the final pass plus the style, if any.
fn delay_ms(e: &HistoryEntry) -> u64 {
    let ai = match &e.enhance {
        EnhanceOutcome::Applied { elapsed_ms, .. } => *elapsed_ms,
        _ => 0,
    };
    e.transcribe_ms + ai
}

fn readiness(label: String, color: Hsla, c: &Colors) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(div().size(px(7.)).rounded_full().bg(color))
        .child(text::ui(label, 13., FontWeight::NORMAL, c.ink))
}

impl OnboardingView {
    pub(super) fn practice_step(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let c = cx.paper().colors;
        let m = self.model.read(cx);
        let keys = m.config.hotkeys.toggle.map(|h| h.keycaps().join(" "));
        let lead = match &keys {
            Some(k) => format!("Click the field, press {k}, read the sentence, then press {k} again."),
            None => "Click the field, hold your push-to-talk key, and read the sentence.".to_string(),
        };
        let latest = m.recent.first().filter(|e| e.created_at >= self.practice.since).cloned();
        let info = m.active_model();
        let status = m.status_of(&info.id);
        let preview = m.preview_model();
        let preview_status = preview.as_ref().map(|p| m.status_of(p));
        let ready = m.ready_to_dictate();
        let model_on_disk = m.status_of(&m.config.dictation.model).is_on_disk();
        let mic_ok = m.permission(sayso_platform::Permission::Microphone) == sayso_platform::PermissionState::Granted;
        let state_text = match m.state() {
            sayso_core::dictation::State::Recording { .. } => Some("Listening… press the key again when you are done."),
            sayso_core::dictation::State::Processing { .. } => Some("Writing…"),
            _ => None,
        };

        // Readiness dots.
        let (final_label, final_color) = match &status {
            s if s.is_on_disk() => ("Final pass ready".to_string(), c.success),
            ModelStatus::Downloading { fraction, .. } => (format!("Final pass downloading · {:.0}%", fraction * 100.0), c.accent),
            ModelStatus::Failed { .. } => ("Final pass download stopped".to_string(), c.danger),
            _ => ("Final pass not downloaded".to_string(), c.pencil),
        };
        let (preview_label, preview_color) = match (&preview, &preview_status) {
            (None, _) if !m.config.dictation.live_preview => ("Live preview off".to_string(), c.pencil),
            (None, _) => ("No live preview for this model".to_string(), c.pencil),
            (Some(_), Some(ModelStatus::Ready)) => ("Live preview ready".to_string(), c.success),
            (Some(_), Some(ModelStatus::Downloading { fraction, .. })) => (format!("Live preview downloading · {:.0}%", fraction * 100.0), c.accent),
            (Some(_), _) => ("Live preview loading".to_string(), c.accent),
        };
        let (opt_label, opt_color) = match &status {
            ModelStatus::Ready => (crate::shell::os_text!("Optimized for this Mac", "Ready", "Ready on this computer").to_string(), c.success),
            ModelStatus::Optimizing => (crate::shell::os_text!("Optimizing for this Mac…", "Loading the model…", "Preparing the model…").to_string(), c.accent),
            ModelStatus::Downloaded => ("Loading the model…".to_string(), c.accent),
            _ => ("Optimizes after the download".to_string(), c.pencil),
        };

        let result: AnyElement = match &latest {
            Some(e) if !matches!(e.insert, InsertOutcome::Failed { .. }) => {
                let (ok, total) = words_correct(&e.final_text);
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .size(px(22.))
                            .rounded_full()
                            .bg(c.accent)
                            .shadow(vec![BoxShadow::new(px(0.), px(-1.5), gpui_kit::black().opacity(0.3)).blur_radius(px(2.)).inset()])
                            .child(icon(Icon::Check, 11., c.on_ink)),
                    )
                    .child(text::ui(
                        format!("Inserted {:.1} s after you stopped", delay_ms(e) as f64 / 1000.0),
                        13.,
                        FontWeight::SEMIBOLD,
                        c.ink,
                    ))
                    .child(text::ui(format!("· {ok} of {total} words correct"), 13., FontWeight::NORMAL, c.graphite))
                    .into_any_element()
            }
            Some(e) => {
                let reason = match &e.insert {
                    InsertOutcome::Failed { reason } => reason.clone(),
                    _ => String::new(),
                };
                text::ui(
                    if crate::shell::HAS_INPUT_PERMISSIONS {
                        format!("Sayso heard you, but could not paste: {reason}. Check Accessibility in the previous steps.")
                    } else {
                        format!("Sayso heard you, but could not paste: {reason}.")
                    },
                    13.,
                    FontWeight::NORMAL,
                    c.danger,
                )
                .into_any_element()
            }
            None => text::ui(state_text.unwrap_or("Your words appear here."), 13., FontWeight::NORMAL, c.graphite).into_any_element(),
        };

        let field = div()
            .id("practice-field")
            .flex()
            .flex_col()
            .gap(px(12.))
            .min_h(px(120.))
            .py(px(18.))
            .px(px(20.))
            .rounded(px(12.))
            .bg(c.sheet_raised)
            .shadow(vec![
                BoxShadow::new(px(0.), px(1.), c.shadow(0.12)).blur_radius(px(3.)).inset(),
                BoxShadow::new(px(0.), px(0.), c.accent).spread_radius(px(2.)),
                BoxShadow::new(px(0.), px(1.), c.highlight(1.0)),
            ])
            .cursor_text()
            .on_click(cx.listener(|this, _, window, cx| {
                this.practice.field.update(cx, |s, cx| s.focus(window, cx));
            }))
            .child(
                div()
                    .font_family(sayso_ui::fonts::DISPLAY)
                    .text_size(px(20.))
                    .line_height(px(28.))
                    .text_color(c.ink)
                    .child(Textarea::new(&self.practice.field).appearance(false).font_family(sayso_ui::fonts::DISPLAY).text_size(px(20.)).line_height(px(28.)).p_0()),
            )
            .child(result);

        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(22.))
            .pt(px(28.))
            .px(px(72.))
            .pb(px(24.))
            .child(heading("Try it", &lead, &c))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .px(px(4.))
                    .child(text::caps("Read this", &c))
                    .child(
                        div()
                            .font_family(sayso_ui::fonts::DISPLAY)
                            .italic()
                            .text_size(px(22.))
                            .line_height(px(30.))
                            .text_color(c.graphite)
                            .child(format!("“{SENTENCE}”")),
                    ),
            )
            .child(field)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(20.))
                    .px(px(4.))
                    .child(readiness(final_label, final_color, &c))
                    .child(readiness(preview_label, preview_color, &c))
                    .child(readiness(opt_label, opt_color, &c)),
            );
        if !ready {
            let why = if !model_on_disk {
                "Sayso can dictate when the model download ends. You can wait here, or finish setup now and try later."
            } else if !mic_ok {
                "Sayso needs the microphone to dictate. Go back to Permissions and allow it, or finish now and allow it later in Settings › Permissions."
            } else {
                "The speech engine is not running, so Sayso cannot dictate now. Finish setup and open Sayso again."
            };
            col = col.child(crate::hub::settings::kit::notice(BannerKind::Info, why, cx));
        }
        col.into_any_element()
    }
}
