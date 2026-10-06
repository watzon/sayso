//! History entry types. Storage lives in `sayso-store`.

use crate::dictionary::AppliedReplacement;
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
}

impl HistoryEntry {
    pub fn word_count(&self) -> usize {
        self.final_text.split_whitespace().count()
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
}
