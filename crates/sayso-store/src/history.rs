//! History entries: insert, update, read, search, delete.

use crate::error::{Result, StoreError};
use crate::Store;
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::types::Value;
use rusqlite::{Row, params, params_from_iter};
use sayso_core::history::{HistoryEntry, TargetApp};
use sayso_core::models::ModelId;

/// Columns of `history` in the order [`entry_from_row`] reads them.
pub(crate) const COLUMNS: &str = "id, created_at, duration_ms, app, transcript, final_text, style_id, model, \
                       transcribe_ms, replacements, enhance, insert_result, audio_file, waveform";

/// Filters for [`Store::list`] and [`Store::count`]. All filters combine with AND.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryQuery {
    /// Full-text search over the transcript and the final text. Every word
    /// must match, and the last word may match a prefix. Blank text matches all.
    pub text: Option<String>,
    /// Only entries where AI enhancement was applied.
    pub enhanced_only: bool,
    /// Only entries that still have an audio file.
    pub has_audio: bool,
    /// Only entries inserted into this app.
    pub app_bundle_id: Option<String>,
    /// Page size. None returns all rows. [`Store::count`] ignores it.
    pub limit: Option<usize>,
    /// Rows to skip. [`Store::count`] ignores it.
    pub offset: usize,
}

/// One row of the App filter: an app that has history, and how many entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppCount {
    pub bundle_id: String,
    /// The most recent name seen for this bundle id.
    pub name: String,
    pub count: usize,
}

impl HistoryQuery {
    /// The SQL conditions and their parameters. Table alias: none.
    fn conditions(&self) -> (Vec<&'static str>, Vec<Value>) {
        let mut sql = Vec::new();
        let mut args = Vec::new();
        if let Some(fts) = self.text.as_deref().and_then(fts_query) {
            sql.push("id IN (SELECT rowid FROM history_fts WHERE history_fts MATCH ?)");
            args.push(Value::Text(fts));
        }
        if self.enhanced_only {
            sql.push("json_extract(enhance, '$.result') = 'applied'");
        }
        if self.has_audio {
            sql.push("audio_file IS NOT NULL");
        }
        if let Some(bundle) = &self.app_bundle_id {
            sql.push("json_extract(app, '$.bundle_id') = ?");
            args.push(Value::Text(bundle.clone()));
        }
        (sql, args)
    }

    fn where_clause(sql: &[&str]) -> String {
        if sql.is_empty() { String::new() } else { format!(" WHERE {}", sql.join(" AND ")) }
    }
}

/// Turn user text into a safe FTS5 query: each word is a quoted phrase, and the
/// last one is a prefix match. Returns None when there is no searchable word.
fn fts_query(text: &str) -> Option<String> {
    let words: Vec<String> =
        text.split_whitespace().map(|w| format!("\"{}\"", w.replace('"', "\"\""))).collect();
    let (last, rest) = words.split_last()?;
    let mut parts: Vec<String> = rest.to_vec();
    parts.push(format!("{last}*"));
    Some(parts.join(" "))
}

fn millis_to_time(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms).single().unwrap_or_default()
}

fn json_column<T: serde::de::DeserializeOwned>(row: &Row, index: usize) -> rusqlite::Result<T> {
    let text: String = row.get(index)?;
    serde_json::from_str(&text)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e)))
}

fn entry_from_row(row: &Row) -> rusqlite::Result<HistoryEntry> {
    let app: Option<String> = row.get(3)?;
    let app: Option<TargetApp> = match app {
        Some(text) => Some(serde_json::from_str(&text).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(e))
        })?),
        None => None,
    };
    Ok(HistoryEntry {
        id: row.get(0)?,
        created_at: millis_to_time(row.get(1)?),
        duration_ms: row.get::<_, i64>(2)? as u64,
        app,
        transcript: row.get(4)?,
        final_text: row.get(5)?,
        style_id: row.get(6)?,
        model: ModelId::new(row.get::<_, String>(7)?),
        transcribe_ms: row.get::<_, i64>(8)? as u64,
        replacements: json_column(row, 9)?,
        enhance: json_column(row, 10)?,
        insert: json_column(row, 11)?,
        audio_file: row.get(12)?,
        waveform: row.get(13)?,
    })
}

/// The column values of an entry, without the id. Order matches `COLUMNS[1..]`.
pub(crate) fn entry_values(e: &HistoryEntry) -> Result<Vec<Value>> {
    let app = match &e.app {
        Some(app) => Value::Text(serde_json::to_string(app)?),
        None => Value::Null,
    };
    Ok(vec![
        Value::Integer(e.created_at.timestamp_millis()),
        Value::Integer(e.duration_ms as i64),
        app,
        Value::Text(e.transcript.clone()),
        Value::Text(e.final_text.clone()),
        Value::Text(e.style_id.clone()),
        Value::Text(e.model.to_string()),
        Value::Integer(e.transcribe_ms as i64),
        Value::Text(serde_json::to_string(&e.replacements)?),
        Value::Text(serde_json::to_string(&e.enhance)?),
        Value::Text(serde_json::to_string(&e.insert)?),
        e.audio_file.clone().map_or(Value::Null, Value::Text),
        Value::Blob(e.waveform.clone()),
    ])
}

impl Store {
    /// Add an entry and return its new id. The `id` field of `entry` is ignored.
    pub fn insert_entry(&self, entry: &HistoryEntry) -> Result<i64> {
        let values = entry_values(entry)?;
        let columns = &COLUMNS["id, ".len()..];
        let marks = vec!["?"; values.len()].join(", ");
        let conn = self.conn.lock();
        conn.execute(&format!("INSERT INTO history ({columns}) VALUES ({marks})"), params_from_iter(values))?;
        Ok(conn.last_insert_rowid())
    }

    /// Overwrite every field of the entry with `entry.id`.
    /// Fails with [`StoreError::NotFound`] when the id does not exist.
    pub fn update_entry(&self, entry: &HistoryEntry) -> Result<()> {
        let mut values = entry_values(entry)?;
        values.push(Value::Integer(entry.id));
        let assignments: Vec<String> = COLUMNS["id, ".len()..].split(", ").map(|c| format!("{c} = ?")).collect();
        let sql = format!("UPDATE history SET {} WHERE id = ?", assignments.join(", "));
        let changed = self.conn.lock().execute(&sql, params_from_iter(values))?;
        if changed == 0 { Err(StoreError::NotFound(entry.id)) } else { Ok(()) }
    }

    /// One entry by id.
    pub fn get(&self, id: i64) -> Result<Option<HistoryEntry>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM history WHERE id = ?"))?;
        let mut rows = stmt.query_map(params![id], entry_from_row)?;
        Ok(rows.next().transpose()?)
    }

    /// Entries that match the query, newest first.
    pub fn list(&self, query: &HistoryQuery) -> Result<Vec<HistoryEntry>> {
        let (conditions, mut args) = query.conditions();
        let sql = format!(
            "SELECT {COLUMNS} FROM history{} ORDER BY created_at DESC, id DESC LIMIT ? OFFSET ?",
            HistoryQuery::where_clause(&conditions)
        );
        // SQLite treats a negative LIMIT as "no limit".
        args.push(Value::Integer(query.limit.map_or(-1, |l| l as i64)));
        args.push(Value::Integer(query.offset as i64));
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), entry_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The number of entries that match the query, ignoring its page settings.
    pub fn count(&self, query: &HistoryQuery) -> Result<usize> {
        let (conditions, args) = query.conditions();
        let sql = format!("SELECT COUNT(*) FROM history{}", HistoryQuery::where_clause(&conditions));
        let n: i64 = self.conn.lock().query_row(&sql, params_from_iter(args), |r| r.get(0))?;
        Ok(n as usize)
    }

    /// Delete one entry and its audio file. Returns false when the id does not exist.
    pub fn delete(&self, id: i64) -> Result<bool> {
        let audio_file: Option<Option<String>> = {
            let conn = self.conn.lock();
            let found = conn
                .query_row("SELECT audio_file FROM history WHERE id = ?", params![id], |r| r.get(0))
                .map(Some)
                .or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(None) } else { Err(e) })?;
            if found.is_some() {
                conn.execute("DELETE FROM history WHERE id = ?", params![id])?;
            }
            found
        };
        match audio_file {
            None => Ok(false),
            Some(file) => {
                if let Some(name) = file {
                    self.remove_audio_file(&name);
                }
                Ok(true)
            }
        }
    }

    /// Delete all history and all audio files. Returns the number of entries deleted.
    /// The dictionary stays.
    pub fn clear_all(&self) -> Result<usize> {
        let deleted = {
            let mut conn = self.conn.lock();
            let tx = conn.transaction()?;
            let deleted = tx.execute("DELETE FROM history", [])?;
            tx.commit()?;
            deleted
        };
        self.remove_all_audio_files();
        Ok(deleted)
    }

    /// All entries created at or after `since`, newest first. Input for the Home stats.
    pub fn entries_since(&self, since: DateTime<Utc>) -> Result<Vec<HistoryEntry>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(&format!("SELECT {COLUMNS} FROM history WHERE created_at >= ? ORDER BY created_at DESC, id DESC"))?;
        let rows = stmt.query_map(params![since.timestamp_millis()], entry_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Apps that have history, most used first. Feeds the App filter.
    pub fn app_list(&self) -> Result<Vec<AppCount>> {
        let conn = self.conn.lock();
        // `max(created_at)` makes SQLite pick `name` from the newest row of each group.
        let mut stmt = conn.prepare(
            "SELECT json_extract(app, '$.bundle_id') AS bundle, json_extract(app, '$.name'), COUNT(*), max(created_at)
             FROM history WHERE app IS NOT NULL GROUP BY bundle
             ORDER BY COUNT(*) DESC, bundle",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(AppCount { bundle_id: r.get(0)?, name: r.get(1)?, count: r.get::<_, i64>(2)? as usize })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::*;
    use chrono::Duration;
    use sayso_core::dictionary::AppliedReplacement;
    use sayso_core::history::{EnhanceOutcome, InsertOutcome};

    #[test]
    fn insert_get_round_trips_every_field() {
        let (_dir, store) = temp_store();
        let mut e = entry("hello world", Duration::hours(1));
        e.app = app("com.apple.Notes", "Notes");
        e.replacements = vec![AppliedReplacement { replacement_id: 4, from: "git hub".into(), to: "GitHub".into(), count: 2 }];
        e.enhance = EnhanceOutcome::Applied { provider_id: "or".into(), model: "m".into(), elapsed_ms: 410 };
        e.insert = InsertOutcome::Pasted { clipboard_restored: true };
        e.audio_file = Some("a.flac".into());
        e.waveform = (0..=255).collect();
        let id = store.insert_entry(&e).unwrap();
        e.id = id;
        assert_eq!(store.get(id).unwrap(), Some(e));
        assert_eq!(store.get(id + 1).unwrap(), None);
    }

    #[test]
    fn insert_ignores_the_id_field() {
        let (_dir, store) = temp_store();
        let mut e = entry("one", Duration::zero());
        e.id = 999;
        let first = store.insert_entry(&e).unwrap();
        let second = store.insert_entry(&e).unwrap();
        assert_eq!((first, second), (1, 2));
    }

    #[test]
    fn update_changes_fields_and_fails_for_unknown_ids() {
        let (_dir, store) = temp_store();
        let id = store.insert_entry(&entry("before", Duration::zero())).unwrap();
        let mut e = store.get(id).unwrap().unwrap();
        e.final_text = "after".into();
        e.enhance = EnhanceOutcome::Failed { provider_id: "x".into(), reason: "timed out".into() };
        store.update_entry(&e).unwrap();
        assert_eq!(store.get(id).unwrap().unwrap(), e);
        e.id = 77;
        assert!(matches!(store.update_entry(&e), Err(StoreError::NotFound(77))));
    }

    #[test]
    fn list_is_newest_first_and_pages() {
        let (_dir, store) = temp_store();
        for (text, hours) in [("old", 30), ("new", 1), ("mid", 10)] {
            store.insert_entry(&entry(text, Duration::hours(hours))).unwrap();
        }
        let all = store.list(&HistoryQuery::default()).unwrap();
        let texts: Vec<_> = all.iter().map(|e| e.final_text.as_str()).collect();
        assert_eq!(texts, ["new", "mid", "old"]);
        let page = store.list(&HistoryQuery { limit: Some(1), offset: 1, ..Default::default() }).unwrap();
        assert_eq!(page[0].final_text, "mid");
        let q = HistoryQuery { limit: Some(1), ..Default::default() };
        assert_eq!(store.count(&q).unwrap(), 3);
    }

    #[test]
    fn full_text_search_covers_transcript_and_final_text() {
        let (_dir, store) = temp_store();
        let mut a = entry("meeting notes for monday", Duration::hours(3));
        a.final_text = "Meeting notes for Monday.".into();
        store.insert_entry(&a).unwrap();
        let mut b = entry("buy milk", Duration::hours(2));
        b.final_text = "Buy oat milk, please.".into();
        store.insert_entry(&b).unwrap();
        let search = |t: &str| {
            store.list(&HistoryQuery { text: Some(t.into()), ..Default::default() }).unwrap().len()
        };
        assert_eq!(search("monday"), 1);
        assert_eq!(search("oat"), 1, "matches the final text");
        assert_eq!(search("milk please"), 1, "all words must match");
        assert_eq!(search("meet"), 1, "the last word matches a prefix");
        assert_eq!(search("mon notes"), 0, "only the last word is a prefix");
        assert_eq!(search("   "), 2, "blank text matches everything");
        assert_eq!(search("\"quote"), 0, "quotes are escaped, not syntax errors");
        assert_eq!(search("NEAR(a b) OR"), 0, "FTS operators are plain words");
    }

    #[test]
    fn search_follows_updates_and_deletes() {
        let (_dir, store) = temp_store();
        let id = store.insert_entry(&entry("alpha", Duration::zero())).unwrap();
        let q = |t: &str| HistoryQuery { text: Some(t.into()), ..Default::default() };
        let mut e = store.get(id).unwrap().unwrap();
        e.transcript = "beta".into();
        e.final_text = "beta".into();
        store.update_entry(&e).unwrap();
        assert_eq!(store.count(&q("alpha")).unwrap(), 0);
        assert_eq!(store.count(&q("beta")).unwrap(), 1);
        store.delete(id).unwrap();
        assert_eq!(store.count(&q("beta")).unwrap(), 0);
    }

    #[test]
    fn filters_combine() {
        let (_dir, store) = temp_store();
        let mut notes = entry("note one", Duration::hours(3));
        notes.app = app("com.apple.Notes", "Notes");
        notes.audio_file = Some("n.flac".into());
        notes.enhance = EnhanceOutcome::Applied { provider_id: "p".into(), model: "m".into(), elapsed_ms: 1 };
        store.insert_entry(&notes).unwrap();
        let mut mail = entry("mail one", Duration::hours(2));
        mail.app = app("com.apple.mail", "Mail");
        store.insert_entry(&mail).unwrap();
        let mut failed = entry("note two", Duration::hours(1));
        failed.app = app("com.apple.Notes", "Notes");
        failed.enhance = EnhanceOutcome::Failed { provider_id: "p".into(), reason: "r".into() };
        store.insert_entry(&failed).unwrap();

        let n = |q: HistoryQuery| store.count(&q).unwrap();
        assert_eq!(n(HistoryQuery { enhanced_only: true, ..Default::default() }), 1);
        assert_eq!(n(HistoryQuery { has_audio: true, ..Default::default() }), 1);
        assert_eq!(n(HistoryQuery { app_bundle_id: Some("com.apple.Notes".into()), ..Default::default() }), 2);
        let both = HistoryQuery {
            text: Some("note".into()),
            app_bundle_id: Some("com.apple.Notes".into()),
            enhanced_only: true,
            ..Default::default()
        };
        assert_eq!(n(both.clone()), 1);
        assert_eq!(store.list(&both).unwrap()[0].final_text, "note one");
    }

    #[test]
    fn app_list_counts_entries_per_app() {
        let (_dir, store) = temp_store();
        for (hours, bundle, name) in [(3, "a.notes", "Notes"), (2, "a.notes", "Notes 2"), (1, "a.mail", "Mail")] {
            let mut e = entry("x", Duration::hours(hours));
            e.app = app(bundle, name);
            store.insert_entry(&e).unwrap();
        }
        store.insert_entry(&entry("no app", Duration::zero())).unwrap();
        let apps = store.app_list().unwrap();
        assert_eq!(apps.len(), 2, "entries without an app are not listed");
        assert_eq!((apps[0].bundle_id.as_str(), apps[0].count), ("a.notes", 2));
        assert_eq!(apps[0].name, "Notes 2", "uses the newest name");
        assert_eq!((apps[1].bundle_id.as_str(), apps[1].count), ("a.mail", 1));
    }

    #[test]
    fn entries_since_returns_only_recent_entries() {
        let (_dir, store) = temp_store();
        store.insert_entry(&entry("old", Duration::days(10))).unwrap();
        store.insert_entry(&entry("new", Duration::days(1))).unwrap();
        let recent = store.entries_since(base_time() - Duration::days(7)).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].final_text, "new");
    }

    #[test]
    fn delete_removes_the_entry_and_its_audio() {
        let (_dir, store) = temp_store();
        let name = store.save_audio(&[0.1; 1600]).unwrap();
        let mut e = entry("with audio", Duration::zero());
        e.audio_file = Some(name.clone());
        let id = store.insert_entry(&e).unwrap();
        assert!(store.audio_path(&name).exists());
        assert!(store.delete(id).unwrap());
        assert!(!store.audio_path(&name).exists());
        assert_eq!(store.get(id).unwrap(), None);
        assert!(!store.delete(id).unwrap(), "a second delete finds nothing");
    }

    #[test]
    fn clear_all_removes_history_and_audio_but_keeps_the_dictionary() {
        let (_dir, store) = temp_store();
        let name = store.save_audio(&[0.1; 1600]).unwrap();
        let mut e = entry("one", Duration::zero());
        e.audio_file = Some(name.clone());
        store.insert_entry(&e).unwrap();
        store.insert_entry(&entry("two", Duration::zero())).unwrap();
        store.add_word("Sayso").unwrap();
        assert_eq!(store.clear_all().unwrap(), 2);
        assert_eq!(store.count(&HistoryQuery::default()).unwrap(), 0);
        assert!(!store.audio_path(&name).exists());
        assert_eq!(store.list_words().unwrap().len(), 1);
    }
}
