//! Retention: two independent limits (plan §3, "History and retention").
//!
//! - Text limit: entries older than the limit are deleted, with their audio.
//! - Audio limit: audio older than the limit is deleted. The entry, its text,
//!   and its waveform summary stay. `audio_file` becomes `NULL`.

use crate::Store;
use crate::error::Result;
use chrono::{DateTime, Duration, Utc};
use rusqlite::params;

/// What one retention run removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RetentionReport {
    /// Entries deleted because of the text limit.
    pub entries_deleted: usize,
    /// Audio files deleted, for either limit.
    pub audio_deleted: usize,
}

fn cutoff_millis(now: DateTime<Utc>, days: u32) -> i64 {
    (now - Duration::days(i64::from(days))).timestamp_millis()
}

impl Store {
    /// Apply the retention limits. `None` keeps that kind of data forever.
    /// `now` is a parameter so tests do not depend on the clock.
    ///
    /// Entries are removed from the database first. If a file cannot be
    /// deleted afterward, it stays on disk and the run goes on.
    pub fn apply_retention(
        &self,
        keep_text_days: Option<u32>,
        keep_audio_days: Option<u32>,
        now: DateTime<Utc>,
    ) -> Result<RetentionReport> {
        let mut report = RetentionReport::default();

        if let Some(days) = keep_text_days {
            let cutoff = cutoff_millis(now, days);
            let files = {
                let mut conn = self.conn.lock();
                let tx = conn.transaction()?;
                let files = audio_files(&tx, "created_at < ?", cutoff)?;
                report.entries_deleted = tx.execute("DELETE FROM history WHERE created_at < ?", params![cutoff])?;
                tx.commit()?;
                files
            };
            report.audio_deleted += files.iter().filter(|f| self.remove_audio_file(f)).count();
        }

        if let Some(days) = keep_audio_days {
            let cutoff = cutoff_millis(now, days);
            let files = audio_files(&self.conn.lock(), "created_at < ?", cutoff)?;
            for file in files {
                // Clear the column first. A file with no row is an orphan, which is
                // harmless. A row that points at a deleted file would be a broken player.
                self.conn.lock().execute("UPDATE history SET audio_file = NULL WHERE audio_file = ?", params![file])?;
                if self.remove_audio_file(&file) {
                    report.audio_deleted += 1;
                }
            }
        }
        Ok(report)
    }
}

/// Audio file names of the entries that match `condition`.
fn audio_files(conn: &rusqlite::Connection, condition: &str, arg: i64) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("SELECT audio_file FROM history WHERE audio_file IS NOT NULL AND {condition}"))?;
    let rows = stmt.query_map(params![arg], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HistoryQuery;
    use crate::test_util::*;

    /// An entry with a real audio file, `age` old.
    fn with_audio(store: &Store, text: &str, age: Duration) -> (i64, String) {
        let name = store.save_audio(&[0.2; 800]).unwrap();
        let mut e = entry(text, age);
        e.audio_file = Some(name.clone());
        (store.insert_entry(&e).unwrap(), name)
    }

    #[test]
    fn audio_expiry_keeps_the_entry_and_its_waveform() {
        let (_dir, store) = temp_store();
        let (old_id, old_file) = with_audio(&store, "old", Duration::days(40));
        let (new_id, new_file) = with_audio(&store, "new", Duration::days(2));

        let report = store.apply_retention(None, Some(30), base_time()).unwrap();
        assert_eq!(report, RetentionReport { entries_deleted: 0, audio_deleted: 1 });

        let old = store.get(old_id).unwrap().expect("the entry stays");
        assert_eq!(old.audio_file, None);
        assert_eq!(old.final_text, "old");
        assert_eq!(old.waveform, vec![1, 2, 3], "the waveform stays");
        assert!(!store.audio_path(&old_file).exists());

        assert_eq!(store.get(new_id).unwrap().unwrap().audio_file, Some(new_file.clone()));
        assert!(store.audio_path(&new_file).exists());
    }

    #[test]
    fn text_expiry_deletes_entries_and_their_audio() {
        let (_dir, store) = temp_store();
        let (old_id, old_file) = with_audio(&store, "old", Duration::days(100));
        let (new_id, _) = with_audio(&store, "new", Duration::days(1));
        store.insert_entry(&entry("old without audio", Duration::days(200))).unwrap();

        let report = store.apply_retention(Some(90), None, base_time()).unwrap();
        assert_eq!(report, RetentionReport { entries_deleted: 2, audio_deleted: 1 });
        assert_eq!(store.get(old_id).unwrap(), None);
        assert!(!store.audio_path(&old_file).exists());
        assert!(store.get(new_id).unwrap().is_some());
        assert_eq!(store.count(&HistoryQuery::default()).unwrap(), 1);
    }

    #[test]
    fn none_keeps_everything() {
        let (_dir, store) = temp_store();
        let (id, file) = with_audio(&store, "ancient", Duration::days(5000));
        assert_eq!(store.apply_retention(None, None, base_time()).unwrap(), RetentionReport::default());
        assert!(store.get(id).unwrap().unwrap().audio_file.is_some());
        assert!(store.audio_path(&file).exists());
    }

    #[test]
    fn both_limits_together_count_each_file_once() {
        let (_dir, store) = temp_store();
        with_audio(&store, "very old", Duration::days(100));
        let (mid_id, _) = with_audio(&store, "mid", Duration::days(40));
        let report = store.apply_retention(Some(90), Some(30), base_time()).unwrap();
        assert_eq!(report, RetentionReport { entries_deleted: 1, audio_deleted: 2 });
        assert_eq!(store.get(mid_id).unwrap().unwrap().audio_file, None);
    }

    #[test]
    fn a_missing_audio_file_does_not_break_the_run() {
        let (_dir, store) = temp_store();
        let (id, file) = with_audio(&store, "x", Duration::days(60));
        std::fs::remove_file(store.audio_path(&file)).unwrap();
        let report = store.apply_retention(None, Some(30), base_time()).unwrap();
        assert_eq!(report.audio_deleted, 0);
        assert_eq!(store.get(id).unwrap().unwrap().audio_file, None, "the dangling name is cleared");
    }
}
