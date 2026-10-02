//! Developer flags for screenshots and design review.
//!
//! - `--hub` opens the Hub at startup.
//! - `--route=home|history|dictionary|styles|models|settings-<page>` picks the page.
//! - `--onboarding[=N]` opens onboarding at step N (0 to 6).
//! - `--dark` / `--light` force the appearance.
//! - `--seed-demo` fills an empty database with the sample content from the design.
//!
//! Use temporary `XDG_CONFIG_HOME` and `XDG_DATA_HOME` with `--seed-demo` so
//! your real history stays untouched.

use crate::hub::{Route, SettingsPage};
use sayso_core::dictionary::Replacement;
use sayso_core::history::{EnhanceOutcome, HistoryEntry, InsertOutcome, TargetApp};
use sayso_core::models::default_model;

pub fn arg(name: &str) -> Option<String> {
    let prefix = format!("--{name}=");
    std::env::args().find_map(|a| a.strip_prefix(&prefix).map(str::to_string))
}

pub fn flag(name: &str) -> bool {
    let f = format!("--{name}");
    std::env::args().any(|a| a == f)
}

pub fn route() -> Option<Route> {
    let r = arg("route")?;
    Some(match r.as_str() {
        "home" => Route::Home,
        "history" => Route::History,
        "dictionary" => Route::Dictionary,
        "styles" => Route::Styles,
        "models" => Route::Models,
        s => {
            let page = match s.strip_prefix("settings-").unwrap_or("dictation") {
                "general" => SettingsPage::General,
                "overlay" => SettingsPage::Overlay,
                "audio" => SettingsPage::Audio,
                "appearance" => SettingsPage::Appearance,
                "history" => SettingsPage::HistoryPrivacy,
                "permissions" => SettingsPage::Permissions,
                "advanced" => SettingsPage::Advanced,
                _ => SettingsPage::Dictation,
            };
            Route::Settings(page)
        }
    })
}

/// True when this process runs from an app bundle. macOS gives a process
/// started from a terminal the terminal's permissions, and it does not show
/// the microphone prompt for a binary without a usage description.
pub fn running_from_bundle() -> bool {
    std::env::current_exe().is_ok_and(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
}

/// Shown on the permission screens when Sayso does not run from a bundle.
pub const UNBUNDLED_NOTE: &str = "Sayso is running from a terminal, so macOS uses the terminal's permissions and does not ask for the microphone. Quit Sayso and start it with scripts/dev.sh, or open build/Sayso.app.";

pub fn seed_demo(store: &sayso_store::Store) {
    if store.count(&Default::default()).unwrap_or(0) > 0 {
        return;
    }
    let now = chrono::Utc::now();
    let rows: [(&str, &str, &str, i64, u64, &str); 8] = [
        ("com.tinyspeck.slackmacgap", "Slack", "Let’s move the standup to Thursday and invite the design team.", 3, 7_000, "message"),
        ("com.apple.mail", "Mail", "Hi Dana, thanks for sending the contract. I’ll review it today and send comments by Friday.", 16, 14_000, "email"),
        ("com.todesktop.230313mzl4w4u92", "Cursor", "Refactor the engine client so the restart logic lives in one place and add a test for it.", 32, 9_000, "clean"),
        ("com.linear", "Linear", "Ship the overlay spike first, then the tray popover. Both block the design review.", 70, 11_000, "polished"),
        ("com.apple.Notes", "Notes", "Groceries: oat milk, two lemons, coffee beans, and something for dinner Friday.", 49, 6_000, "notes"),
        ("com.apple.Terminal", "Terminal", "git commit dash m fix the clipboard restore race", 57, 4_000, "raw"),
        ("com.apple.Notes", "Notes", "Call the bank about the card and book the dentist for next week.", 60 * 20, 8_000, "clean"),
        ("com.tinyspeck.slackmacgap", "Slack", "Sounds good, I can take the review after lunch.", 60 * 26, 3_000, "message"),
    ];
    for (bundle, name, text, minutes_ago, ms, style) in rows {
        let peaks: Vec<u8> = (0..96).map(|i| ((((i as f32) * 0.37).sin().abs() * 0.7 + 0.3) * 255.0) as u8).collect();
        let entry = HistoryEntry {
            id: 0,
            created_at: now - chrono::Duration::minutes(minutes_ago),
            duration_ms: ms,
            app: Some(TargetApp { bundle_id: bundle.into(), name: name.into() }),
            transcript: text.to_lowercase().replace(['.', ',', '’'], "").replace("standup", "stand up"),
            final_text: text.into(),
            style_id: style.into(),
            model: default_model(),
            transcribe_ms: 58,
            replacements: vec![],
            enhance: if style == "raw" {
                EnhanceOutcome::NotUsed
            } else {
                EnhanceOutcome::Applied { provider_id: "openrouter".into(), model: "llama-3.3-8b".into(), elapsed_ms: 620 }
            },
            insert: InsertOutcome::Pasted { clipboard_restored: true },
            audio_file: None,
            waveform: peaks,
        };
        let _ = store.insert_entry(&entry);
    }
    for w in ["Sayso", "Parakeet", "GPUI", "Kubernetes", "Watzon Ventures", "WhisperKit", "Dana Okafor", "OpenRouter", "Tauri", "Neovim", "Paper"] {
        let _ = store.add_word(w);
    }
    for (from, to, uses) in [("git hub", "GitHub", 214), ("my email", "chris@bitstormtech.com", 38), ("stand up", "standup", 27), ("new paragraph", "\\n\\n", 61), ("k eight s", "k8s", 9)] {
        if let Ok(r) = store.add_replacement(&Replacement { id: 0, from: from.into(), to: to.into(), case_sensitive: false, uses: 0 }) {
            let _ = store.record_replacement_uses(&[sayso_core::dictionary::AppliedReplacement {
                replacement_id: r.id,
                from: r.from.clone(),
                to: r.to.clone(),
                count: uses,
            }]);
        }
    }
}

/// `SAYSO_FAKE_MIC=/path/to/16k-mono-16bit.wav`: play a WAV file in real time
/// instead of the microphone. For end-to-end tests without a mic grant.
pub fn fake_mic() -> Option<std::path::PathBuf> {
    std::env::var_os("SAYSO_FAKE_MIC").map(std::path::PathBuf::from)
}

struct FakeCapture {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl sayso_platform::CaptureHandle for FakeCapture {
    fn stop(self: Box<Self>) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Read a 16-bit PCM WAV (mono, 16 kHz) into samples.
fn read_wav(path: &std::path::Path) -> Option<Vec<f32>> {
    let bytes = std::fs::read(path).ok()?;
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let len = u32::from_le_bytes(bytes[i + 4..i + 8].try_into().ok()?) as usize;
        if id == b"data" {
            let data = &bytes[i + 8..(i + 8 + len).min(bytes.len())];
            return Some(data.as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect());
        }
        i += 8 + len + (len & 1);
    }
    None
}

pub fn start_fake_capture(
    path: &std::path::Path,
    mut on_frame: Box<dyn FnMut(sayso_platform::AudioFrame) + Send>,
) -> Option<Box<dyn sayso_platform::CaptureHandle>> {
    let samples = read_wav(path)?;
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::spawn(move || {
        // 20 ms frames, then silence until stopped.
        let frame = 320;
        let mut pos = 0;
        while !flag.load(std::sync::atomic::Ordering::SeqCst) {
            let chunk: Vec<f32> = if pos < samples.len() {
                let end = (pos + frame).min(samples.len());
                let c = samples[pos..end].to_vec();
                pos = end;
                c
            } else {
                vec![0.0; frame]
            };
            let rms = (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len().max(1) as f32).sqrt();
            on_frame(sayso_platform::AudioFrame { level: (rms * 6.0).min(1.0), samples: chunk });
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    });
    Some(Box::new(FakeCapture { stop }))
}
