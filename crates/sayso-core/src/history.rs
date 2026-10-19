//! History entry types. Storage lives in `sayso-store`.

use crate::dictionary::AppliedReplacement;
use crate::enhance::ProviderReport;
use crate::models::ModelId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum EnhanceOutcome {
    /// The style had no prompt, or AI is off.
    NotUsed,
    Applied { provider_id: String, model: String, elapsed_ms: u64 },
    /// The transcript was inserted without the style.
    Failed { provider_id: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum InsertOutcome {
    Pasted { clipboard_restored: bool },
    Typed,
    Failed { reason: String },
    /// The user cancelled. The entry keeps the text, but nothing was inserted.
    NotInserted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetApp {
    pub bundle_id: String,
    pub name: String,
    /// The host of the page, when the app showed a web page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: i64,
    pub created_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub app: Option<TargetApp>,
    /// Raw model output.
    pub transcript: String,
    /// What was inserted (after replacements and the style).
    pub final_text: String,
    pub style_id: String,
    pub model: ModelId,
    pub transcribe_ms: u64,
    pub replacements: Vec<AppliedReplacement>,
    pub enhance: EnhanceOutcome,
    pub insert: InsertOutcome,
    /// File name in the audio directory. None when not saved or expired.
    pub audio_file: Option<String>,
    /// Peak summary for the waveform, kept after the audio expires.
    pub waveform: Vec<u8>,
    /// None for an entry from before Sayso kept diagnostics, and for an import.
    pub diagnostics: Option<Diagnostics>,
}

/// How a dictation ran, for a bug report: the version, the system, and the
/// time of each step. It holds no dictated text.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Diagnostics {
    pub app_version: String,
    pub system: SystemInfo,
    /// From the stop of the recording to the end of the insertion. None when
    /// nothing was inserted. The steps below are parts of this time.
    pub stop_to_insert_ms: Option<u64>,
    pub transcribe: TranscribeTimings,
    /// None when no provider ran.
    pub enhance: Option<EnhanceTimings>,
    /// None when the dictation did not get to the insertion.
    pub insert: Option<InsertTimings>,
    /// The user ran "Enhance" or "Transcribe again" on the entry later. The
    /// text, the model, and the style outcome of the entry are from that run.
    pub changed_later: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SystemInfo {
    /// `std::env::consts::OS`.
    pub os: String,
    pub os_version: Option<String>,
    pub arch: String,
    /// Linux: the display server of the session, "wayland" or "x11".
    pub session: Option<String>,
    /// Linux: the display server that the windows of Sayso use.
    pub ui: Option<String>,
    /// Linux: the desktop, for example "gnome".
    pub desktop: Option<String>,
    /// Linux: the package of the install, for example "flatpak". None for
    /// the tarball, the `.deb`, and the `.rpm`.
    pub package: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TranscribeTimings {
    /// The whole step: the wait for the model, the final pass, and the replacements.
    pub step_ms: u64,
    /// The wait until the local model was in memory.
    pub model_wait_ms: u64,
    pub cloud: bool,
    /// A local model did the final pass, because the cloud provider failed.
    pub fallback: bool,
    pub language: String,
    pub live_preview: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EnhanceTimings {
    /// The whole step: the setup and all attempts.
    pub step_ms: u64,
    /// The time to make the provider ready, for example to find its tool.
    pub setup_ms: u64,
    /// [`crate::config::ProviderKind::name`].
    pub provider_kind: String,
    /// The thinking level of the provider, when it has one.
    pub thinking: Option<String>,
    pub timeout_ms: u64,
    pub prompt_chars: usize,
    pub attempts: Vec<EnhanceAttempt>,
}

/// One call of a provider. A reply with the wrong shape gets a second attempt.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EnhanceAttempt {
    pub elapsed_ms: u64,
    /// None when the attempt gave the text.
    pub error: Option<String>,
    pub mode: Option<String>,
    pub report: Option<ProviderReport>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct InsertTimings {
    /// The whole call. A paste returns at the first read, and the old
    /// clipboard comes back after the call.
    pub elapsed_ms: u64,
    /// "paste" or "type": what Sayso asked for.
    pub method: String,
    /// Set when the paste gives the old clipboard back.
    pub restore_delay_ms: Option<u64>,
    /// The parts of the system that did the work, where it has more than one way.
    pub backend: Option<String>,
}

/// What [`HistoryEntry::metadata_json`] writes.
#[derive(Serialize)]
struct Metadata<'a> {
    id: i64,
    created_at: DateTime<Utc>,
    recording_ms: u64,
    app_id: Option<&'a str>,
    app_name: Option<&'a str>,
    model: &'a ModelId,
    transcribe_ms: u64,
    transcript_chars: usize,
    final_text_chars: usize,
    replacements: usize,
    style: &'a str,
    enhance: &'a EnhanceOutcome,
    insert: &'a InsertOutcome,
    audio_saved: bool,
    diagnostics: &'a Option<Diagnostics>,
}

impl HistoryEntry {
    pub fn word_count(&self) -> usize {
        self.final_text.split_whitespace().count()
    }

    /// The entry as JSON for a bug report. The dictated text and the site are
    /// not in it, only the lengths of the text.
    pub fn metadata_json(&self) -> String {
        let metadata = Metadata {
            id: self.id,
            created_at: self.created_at,
            recording_ms: self.duration_ms,
            app_id: self.app.as_ref().map(|a| a.bundle_id.as_str()),
            app_name: self.app.as_ref().map(|a| a.name.as_str()),
            model: &self.model,
            transcribe_ms: self.transcribe_ms,
            transcript_chars: self.transcript.chars().count(),
            final_text_chars: self.final_text.chars().count(),
            replacements: self.replacements.iter().map(|r| r.count.max(1)).sum(),
            style: &self.style_id,
            enhance: &self.enhance,
            insert: &self.insert,
            audio_saved: self.audio_file.is_some(),
            diagnostics: &self.diagnostics,
        };
        serde_json::to_string_pretty(&metadata).unwrap_or_default()
    }
}

/// A new entry before the store assigns an id.
pub type NewHistoryEntry = HistoryEntry;

/// Peaks for a waveform drawing: `buckets` values from 0 to 255.
pub fn waveform_summary(samples: &[f32], buckets: usize) -> Vec<u8> {
    if samples.is_empty() || buckets == 0 {
        return vec![0; buckets];
    }
    let per = (samples.len() as f64 / buckets as f64).max(1.0);
    let mut peaks: Vec<f32> = (0..buckets)
        .map(|i| {
            let start = (i as f64 * per) as usize;
            let end = (((i + 1) as f64 * per) as usize).min(samples.len());
            if start >= end {
                return 0.0;
            }
            // RMS reads closer to perceived loudness than the max sample.
            let sum: f32 = samples[start..end].iter().map(|s| s * s).sum();
            (sum / (end - start) as f32).sqrt()
        })
        .collect();
    let max = peaks.iter().cloned().fold(0.0f32, f32::max);
    if max > 0.0 {
        for p in &mut peaks {
            *p /= max;
        }
    }
    peaks.into_iter().map(|p| (p.clamp(0.0, 1.0) * 255.0).round() as u8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waveform_has_requested_length_and_normalizes() {
        let samples: Vec<f32> = (0..16_000).map(|i| (i as f32 / 16_000.0).sin() * 0.5).collect();
        let w = waveform_summary(&samples, 96);
        assert_eq!(w.len(), 96);
        assert_eq!(*w.iter().max().unwrap(), 255);
        assert_eq!(waveform_summary(&[], 10), vec![0; 10]);
        assert_eq!(waveform_summary(&[0.1, 0.2], 8).len(), 8);
    }

    #[test]
    fn the_metadata_has_the_timings_and_no_dictated_text() {
        let entry = HistoryEntry {
            id: 7,
            created_at: DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
            duration_ms: 4200,
            app: Some(TargetApp { bundle_id: "org.mozilla.firefox".into(), name: "Firefox".into(), site: Some("bank.example".into()) }),
            transcript: "my secret words".into(),
            final_text: "My secret words.".into(),
            style_id: "clean".into(),
            model: ModelId::new("parakeet"),
            transcribe_ms: 180,
            replacements: vec![],
            enhance: EnhanceOutcome::Applied { provider_id: "claude".into(), model: "haiku".into(), elapsed_ms: 9100 },
            insert: InsertOutcome::Typed,
            audio_file: None,
            waveform: vec![],
            diagnostics: Some(Diagnostics {
                stop_to_insert_ms: Some(9900),
                enhance: Some(EnhanceTimings {
                    attempts: vec![EnhanceAttempt { elapsed_ms: 9100, ..Default::default() }],
                    ..Default::default()
                }),
                ..Default::default()
            }),
        };
        let json = entry.metadata_json();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["diagnostics"]["stop_to_insert_ms"], 9900);
        assert_eq!(value["diagnostics"]["enhance"]["attempts"][0]["elapsed_ms"], 9100);
        assert_eq!((value["transcript_chars"].as_u64(), value["final_text_chars"].as_u64()), (Some(15), Some(16)));
        assert_eq!(value["app_id"], "org.mozilla.firefox");
        assert!(!json.to_lowercase().contains("secret"), "the dictated text is in the metadata");
        assert!(!json.contains("bank.example"), "the site is in the metadata");
    }

    #[test]
    fn diagnostics_from_another_version_still_load() {
        // An older version wrote fewer fields, and a newer one can write more.
        let d: Diagnostics = serde_json::from_str(r#"{"app_version":"0.8.0","new_field":1,"insert":{"elapsed_ms":40}}"#).unwrap();
        assert_eq!(d.app_version, "0.8.0");
        assert_eq!(d.insert.unwrap().elapsed_ms, 40);
        assert_eq!(d.enhance, None);
    }
}
