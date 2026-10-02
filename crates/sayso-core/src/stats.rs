//! Home screen numbers.

use crate::history::HistoryEntry;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate};

/// Typing speed used to estimate time saved. A common average for adults.
pub const TYPING_WPM: f64 = 40.0;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Stats {
    pub words_today: usize,
    pub minutes_saved_today: u64,
    /// Average speaking pace over the last 7 days. None without data.
    pub pace_wpm: Option<u32>,
    pub words_this_week: usize,
    /// Monday first. Words per day of the current week.
    pub week_by_day: [usize; 7],
    /// Index into `week_by_day` for today.
    pub today_index: usize,
}

pub fn compute(entries: &[HistoryEntry], now: DateTime<Local>) -> Stats {
    let today: NaiveDate = now.date_naive();
    let monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    let week_ago = now - Duration::days(7);
    let mut s = Stats { today_index: today.weekday().num_days_from_monday() as usize, ..Default::default() };
    let mut pace_words = 0usize;
    let mut pace_ms = 0u64;
    let mut saved_minutes_today = 0.0;
    for e in entries {
        let local = e.created_at.with_timezone(&Local);
        let words = e.word_count();
        let day = local.date_naive();
        if day == today {
            s.words_today += words;
            let typing_min = words as f64 / TYPING_WPM;
            let speaking_min = e.duration_ms as f64 / 60_000.0;
            saved_minutes_today += (typing_min - speaking_min).max(0.0);
        }
        if day >= monday && day <= today {
            s.words_this_week += words;
            s.week_by_day[(day - monday).num_days() as usize] += words;
        }
        if local >= week_ago && e.duration_ms > 0 {
            pace_words += words;
            pace_ms += e.duration_ms;
        }
    }
    s.minutes_saved_today = saved_minutes_today.round() as u64;
    if pace_ms >= 10_000 {
        s.pace_wpm = Some((pace_words as f64 / (pace_ms as f64 / 60_000.0)).round() as u32);
    }
    s
}

/// "2,184", "11.9k".
pub fn format_count(n: usize) -> String {
    if n >= 10_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else if n >= 1000 {
        format!("{},{:03}", n / 1000, n % 1000)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{EnhanceOutcome, InsertOutcome};
    use crate::models::default_model;
    use chrono::{TimeZone, Utc};

    fn entry(at: DateTime<Utc>, words: usize, duration_ms: u64) -> HistoryEntry {
        HistoryEntry {
            id: 0,
            created_at: at,
            duration_ms,
            app: None,
            transcript: String::new(),
            final_text: vec!["word"; words].join(" "),
            style_id: "raw".into(),
            model: default_model(),
            transcribe_ms: 0,
            replacements: vec![],
            enhance: EnhanceOutcome::NotUsed,
            insert: InsertOutcome::Typed,
            audio_file: None,
            waveform: vec![],
        }
    }

    #[test]
    fn counts_today_week_and_pace() {
        let now = Local.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap(); // a Thursday
        let today = now.with_timezone(&Utc);
        let yesterday = today - Duration::days(1);
        let last_week = today - Duration::days(9);
        let entries = vec![entry(today, 120, 60_000), entry(yesterday, 30, 15_000), entry(last_week, 500, 60_000)];
        let s = compute(&entries, now);
        assert_eq!(s.words_today, 120);
        assert_eq!(s.words_this_week, 150);
        assert_eq!(s.today_index, 3);
        assert_eq!(s.week_by_day[3], 120);
        assert_eq!(s.week_by_day[2], 30);
        assert_eq!(s.pace_wpm, Some(120));
        // 120 words typed at 40 wpm = 3 min, spoken in 1 min: 2 min saved.
        assert_eq!(s.minutes_saved_today, 2);
    }

    #[test]
    fn formats_counts() {
        assert_eq!(format_count(42), "42");
        assert_eq!(format_count(2184), "2,184");
        assert_eq!(format_count(11_920), "11.9k");
    }
}
