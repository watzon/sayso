//! Import from Pindrop, the dictation app that came before Sayso.
//!
//! Pindrop keeps its data in a SwiftData store: a Core Data SQLite file at
//! `~/Library/Application Support/Pindrop/default.store`. [`read`] copies the
//! store to a temporary folder and reads the copy, so it never changes
//! Pindrop's files and is safe while Pindrop runs. [`Store::import_pindrop`]
//! adds what it read. A second import adds nothing twice.
//!
//! What comes over:
//! - vocabulary words, as dictionary words;
//! - word replacements, as replacement rules;
//! - the prompt presets that the user made, for the caller to save as styles;
//! - plain dictations, as history entries without audio.
//!
//! What stays in Pindrop: notes, meetings, and media transcriptions (Sayso has
//! no place for them), and audio (Pindrop keeps `.m4a`, Sayso keeps FLAC).

use crate::Store;
use crate::error::{Result, StoreError};
use crate::history::{COLUMNS, entry_values};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, params, params_from_iter};
use sayso_core::history::{EnhanceOutcome, HistoryEntry, InsertOutcome, TargetApp};
use sayso_core::models::ModelId;
use std::collections::HashSet;
use std::path::Path;

/// The store file in the Pindrop data folder, with its two WAL side files.
const STORE_FILE: &str = "default.store";
const SIDE_FILES: [&str; 2] = ["default.store-wal", "default.store-shm"];
/// Core Data counts seconds from 2001-01-01. Unix time counts from 1970-01-01.
const CORE_DATA_EPOCH: f64 = 978_307_200.0;
/// The style id and the provider id of an imported entry that Pindrop enhanced.
pub const PINDROP_ID: &str = "pindrop";

#[derive(Debug, Clone, PartialEq)]
pub struct PindropReplacement {
    pub from: String,
    pub to: String,
    pub case_sensitive: bool,
}

/// A prompt preset that the user made in Pindrop.
#[derive(Debug, Clone, PartialEq)]
pub struct PindropPreset {
    pub name: String,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PindropDictation {
    /// The record UUID as 32 hex digits. It makes a second import skip the entry.
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub duration_ms: u64,
    /// The text before Pindrop's AI pass. Pindrop does not keep the raw model output.
    pub transcript: String,
    pub final_text: String,
    /// The AI model of Pindrop's enhancement, when one ran.
    pub enhanced_with: Option<String>,
    pub model: String,
    pub app: Option<TargetApp>,
}

/// Everything [`read`] found in a Pindrop store.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PindropData {
    pub words: Vec<String>,
    pub replacements: Vec<PindropReplacement>,
    pub presets: Vec<PindropPreset>,
    pub dictations: Vec<PindropDictation>,
}

/// How many items an import added. Items that existed already are not counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub words: usize,
    pub replacements: usize,
    pub dictations: usize,
}

/// True when `dir` holds a Pindrop store.
pub fn found(dir: &Path) -> bool {
    dir.join(STORE_FILE).is_file()
}

/// Read the Pindrop store in `dir`. Pindrop's files are only copied.
pub fn read(dir: &Path) -> Result<PindropData> {
    if !found(dir) {
        return Err(StoreError::InvalidInput(format!("no Pindrop data in {}", dir.display())));
    }
    // The WAL can hold rows that are not in the main file yet, and a reader
    // writes to the -shm file. So read a copy of all three files.
    let copy = tempfile::tempdir()?;
    std::fs::copy(dir.join(STORE_FILE), copy.path().join(STORE_FILE))?;
    for name in SIDE_FILES {
        if dir.join(name).is_file() {
            std::fs::copy(dir.join(name), copy.path().join(name))?;
        }
    }
    let conn = Connection::open(copy.path().join(STORE_FILE))?;
    Ok(PindropData {
        words: read_words(&conn)?,
        replacements: read_replacements(&conn)?,
        presets: read_presets(&conn)?,
        dictations: read_dictations(&conn)?,
    })
}

/// The columns of a table. Empty when the table does not exist. Pindrop added
/// columns over time, so an old store lacks some of them.
fn columns(conn: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare("SELECT name FROM pragma_table_info(?)")?;
    let rows = stmt.query_map(params![table], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// `column` when the table has it, or a NULL in its place.
fn optional(columns: &HashSet<String>, column: &str) -> String {
    if columns.contains(column) { column.to_string() } else { format!("NULL AS {column}") }
}

fn read_words(conn: &Connection) -> Result<Vec<String>> {
    if !columns(conn, "ZVOCABULARYWORD")?.contains("ZWORD") {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare("SELECT ZWORD FROM ZVOCABULARYWORD WHERE length(trim(ZWORD)) > 0 ORDER BY Z_PK")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn read_replacements(conn: &Connection) -> Result<Vec<PindropReplacement>> {
    let cols = columns(conn, "ZWORDREPLACEMENT")?;
    if !cols.contains("ZORIGINALS") || !cols.contains("ZREPLACEMENT") {
        return Ok(Vec::new());
    }
    let mode = optional(&cols, "ZMATCHMODERAWVALUE");
    let mut stmt = conn.prepare(&format!("SELECT ZORIGINALS, ZREPLACEMENT, {mode} FROM ZWORDREPLACEMENT ORDER BY Z_PK"))?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, Option<Vec<u8>>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?)))?;
    let mut out = Vec::new();
    for row in rows {
        let (Some(originals), Some(to), mode) = row? else { continue };
        // "command" rules run an action in Pindrop. Sayso has no such rule.
        if mode.as_deref() == Some("command") {
            continue;
        }
        let case_sensitive = mode.as_deref() == Some("exact");
        for from in archived_strings(&originals) {
            if !from.trim().is_empty() {
                out.push(PindropReplacement { from, to: to.clone(), case_sensitive });
            }
        }
    }
    Ok(out)
}

/// The strings of an `NSKeyedArchiver` archive of an `NSArray` of `NSString`,
/// which is how SwiftData stores a `[String]`. A blob in another format gives
/// no strings.
fn archived_strings(blob: &[u8]) -> Vec<String> {
    let Ok(archive) = plist::Value::from_reader(std::io::Cursor::new(blob)) else { return Vec::new() };
    let strings = || -> Option<Vec<String>> {
        let archive = archive.as_dictionary()?;
        let objects = archive.get("$objects")?.as_array()?;
        let object = |value: &plist::Value| objects.get(value.as_uid()?.get() as usize);
        let root = object(archive.get("$top")?.as_dictionary()?.get("root")?)?.as_dictionary()?;
        let items = root.get("NS.objects")?.as_array()?;
        Some(items.iter().filter_map(|item| object(item)?.as_string().map(str::to_string)).collect())
    };
    strings().unwrap_or_default()
}

fn read_presets(conn: &Connection) -> Result<Vec<PindropPreset>> {
    let cols = columns(conn, "ZPROMPTPRESET")?;
    if !cols.contains("ZNAME") || !cols.contains("ZPROMPT") {
        return Ok(Vec::new());
    }
    // Pindrop writes its built-in presets again at each start. They are not the user's.
    let user_made = if cols.contains("ZISBUILTIN") { "COALESCE(ZISBUILTIN, 0) = 0" } else { "1" };
    let mut stmt = conn.prepare(&format!(
        "SELECT ZNAME, ZPROMPT FROM ZPROMPTPRESET
         WHERE {user_made} AND length(trim(ZNAME)) > 0 AND length(trim(ZPROMPT)) > 0 ORDER BY Z_PK"
    ))?;
    let rows = stmt.query_map([], |r| Ok(PindropPreset { name: r.get(0)?, prompt: r.get(1)? }))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn read_dictations(conn: &Connection) -> Result<Vec<PindropDictation>> {
    let cols = columns(conn, "ZTRANSCRIPTIONRECORD")?;
    if !["ZID", "ZTEXT", "ZTIMESTAMP"].iter().all(|c| cols.contains(*c)) {
        return Ok(Vec::new());
    }
    // A record is a plain dictation when it came from the microphone and no
    // note or meeting session owns it.
    let from_microphone = if cols.contains("ZSOURCEKINDRAWVALUE") {
        "(r.ZSOURCEKINDRAWVALUE IS NULL OR r.ZSOURCEKINDRAWVALUE = 'voiceRecording')"
    } else {
        "1"
    };
    let no_session = if columns(conn, "ZCAPTURESESSIONMODEL")?.contains("ZTRANSCRIPTIONRECORDID") {
        "NOT EXISTS (SELECT 1 FROM ZCAPTURESESSIONMODEL s WHERE s.ZTRANSCRIPTIONRECORDID = r.ZID)"
    } else {
        "1"
    };
    let sql = format!(
        "SELECT hex(r.ZID), r.ZTIMESTAMP, {duration}, {original}, r.ZTEXT, {enhanced}, {model}, {app_name}, {bundle}
         FROM ZTRANSCRIPTIONRECORD r
         WHERE {from_microphone} AND {no_session}
           AND r.ZID IS NOT NULL AND r.ZTIMESTAMP IS NOT NULL AND length(trim(r.ZTEXT)) > 0
         ORDER BY r.ZTIMESTAMP",
        duration = optional(&cols, "ZDURATION"),
        original = optional(&cols, "ZORIGINALTEXT"),
        enhanced = optional(&cols, "ZENHANCEDWITH"),
        model = optional(&cols, "ZMODELUSED"),
        app_name = optional(&cols, "ZDESTINATIONAPPNAME"),
        bundle = optional(&cols, "ZDESTINATIONAPPBUNDLEID"),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| {
        let saved_at: f64 = r.get(1)?;
        let duration_s: f64 = r.get::<_, Option<f64>>(2)?.unwrap_or(0.0).max(0.0);
        let final_text: String = r.get(4)?;
        let app = match (r.get::<_, Option<String>>(7)?, r.get::<_, Option<String>>(8)?) {
            (Some(name), Some(bundle_id)) if !bundle_id.is_empty() => Some(TargetApp { bundle_id, name }),
            _ => None,
        };
        Ok(PindropDictation {
            id: r.get::<_, String>(0)?.to_lowercase(),
            // Pindrop stores the time it saved the record, at the end of the dictation.
            created_at: Utc.timestamp_millis_opt(((saved_at + CORE_DATA_EPOCH - duration_s) * 1000.0) as i64).single().unwrap_or_default(),
            duration_ms: (duration_s * 1000.0) as u64,
            transcript: r.get::<_, Option<String>>(3)?.unwrap_or_else(|| final_text.clone()),
            final_text,
            enhanced_with: r.get::<_, Option<String>>(5)?.filter(|m| !m.is_empty()),
            model: r.get::<_, Option<String>>(6)?.filter(|m| !m.is_empty()).unwrap_or_else(|| PINDROP_ID.into()),
            app,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The history entry for a Pindrop dictation. It has no audio and no waveform.
fn history_entry(d: &PindropDictation) -> HistoryEntry {
    let (style_id, enhance) = match &d.enhanced_with {
        Some(model) => (PINDROP_ID, EnhanceOutcome::Applied { provider_id: PINDROP_ID.into(), model: model.clone(), elapsed_ms: 0 }),
        None => (sayso_core::style::RAW_STYLE_ID, EnhanceOutcome::NotUsed),
    };
    HistoryEntry {
        id: 0,
        created_at: d.created_at,
        duration_ms: d.duration_ms,
        app: d.app.clone(),
        transcript: d.transcript.clone(),
        final_text: d.final_text.clone(),
        style_id: style_id.into(),
        model: ModelId::new(d.model.clone()),
        transcribe_ms: 0,
        replacements: Vec::new(),
        enhance,
        // Pindrop saves a record only after it delivered the text.
        insert: InsertOutcome::Pasted { clipboard_restored: true },
        audio_file: None,
        waveform: Vec::new(),
    }
}

impl Store {
    /// Add the words, the replacement rules, and the dictations of `data`.
    /// Items that exist already are skipped, so a second import adds nothing.
    /// All changes commit together.
    pub fn import_pindrop(&self, data: &PindropData) -> Result<ImportReport> {
        let mut report = ImportReport::default();
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        for word in &data.words {
            report.words += tx.execute("INSERT OR IGNORE INTO words (text) VALUES (?)", params![word.trim()])?;
        }
        for rule in &data.replacements {
            report.replacements += tx.execute(
                "INSERT INTO replacements (from_text, to_text, case_sensitive)
                 SELECT ?1, ?2, ?3 WHERE NOT EXISTS
                   (SELECT 1 FROM replacements WHERE from_text = ?1 COLLATE NOCASE AND to_text = ?2)",
                params![rule.from.trim(), rule.to, rule.case_sensitive],
            )?;
        }
        let columns = &COLUMNS["id, ".len()..];
        let marks = vec!["?"; columns.split(", ").count() + 1].join(", ");
        let sql = format!("INSERT OR IGNORE INTO history ({columns}, import_id) VALUES ({marks})");
        for dictation in &data.dictations {
            let mut values = entry_values(&history_entry(dictation))?;
            values.push(rusqlite::types::Value::Text(format!("{PINDROP_ID}:{}", dictation.id)));
            report.dictations += tx.execute(&sql, params_from_iter(values))?;
        }
        tx.commit()?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HistoryQuery;
    use crate::test_util::temp_store;
    use plist::{Dictionary, Uid, Value};

    /// The archive SwiftData writes for a `[String]`.
    fn archive(strings: &[&str]) -> Vec<u8> {
        let mut objects = vec![Value::String("$null".into())];
        let mut root = Dictionary::new();
        root.insert("NS.objects".into(), Value::Array((0..strings.len()).map(|i| Value::Uid(Uid::new(i as u64 + 2))).collect()));
        objects.push(Value::Dictionary(root));
        objects.extend(strings.iter().map(|s| Value::String((*s).into())));
        let mut top = Dictionary::new();
        top.insert("root".into(), Value::Uid(Uid::new(1)));
        let mut archive = Dictionary::new();
        archive.insert("$archiver".into(), Value::String("NSKeyedArchiver".into()));
        archive.insert("$objects".into(), Value::Array(objects));
        archive.insert("$top".into(), Value::Dictionary(top));
        let mut bytes = Vec::new();
        Value::Dictionary(archive).to_writer_binary(&mut bytes).unwrap();
        bytes
    }

    /// A Pindrop data folder with the tables and columns of schema 1.0.14.
    fn pindrop_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join(STORE_FILE)).unwrap();
        conn.execute_batch(
            "CREATE TABLE ZVOCABULARYWORD (Z_PK INTEGER PRIMARY KEY, ZWORD VARCHAR, ZID BLOB, ZUSAGECOUNT INTEGER);
             CREATE TABLE ZWORDREPLACEMENT (Z_PK INTEGER PRIMARY KEY, ZREPLACEMENT VARCHAR, ZORIGINALS BLOB, ZMATCHMODERAWVALUE VARCHAR);
             CREATE TABLE ZPROMPTPRESET (Z_PK INTEGER PRIMARY KEY, ZISBUILTIN INTEGER, ZNAME VARCHAR, ZPROMPT VARCHAR);
             CREATE TABLE ZCAPTURESESSIONMODEL (Z_PK INTEGER PRIMARY KEY, ZTRANSCRIPTIONRECORDID BLOB);
             CREATE TABLE ZTRANSCRIPTIONRECORD (Z_PK INTEGER PRIMARY KEY, ZDURATION FLOAT, ZTIMESTAMP TIMESTAMP,
                 ZENHANCEDWITH VARCHAR, ZMODELUSED VARCHAR, ZORIGINALTEXT VARCHAR, ZSOURCEKINDRAWVALUE VARCHAR, ZTEXT VARCHAR,
                 ZID BLOB, ZDESTINATIONAPPNAME VARCHAR, ZDESTINATIONAPPBUNDLEID VARCHAR);
             INSERT INTO ZVOCABULARYWORD (ZWORD) VALUES ('Sayso'), ('  '), ('GPUI');
             INSERT INTO ZPROMPTPRESET (ZISBUILTIN, ZNAME, ZPROMPT) VALUES (1, 'Clean', 'Built in.'), (0, 'Changelog', 'Write a changelog line.');",
        )
        .unwrap();
        let rule = "INSERT INTO ZWORDREPLACEMENT (ZREPLACEMENT, ZORIGINALS, ZMATCHMODERAWVALUE) VALUES (?, ?, ?)";
        conn.execute(rule, params!["GitHub", archive(&["git hub", "get hub"]), None::<String>]).unwrap();
        conn.execute(rule, params!["SQL", archive(&["sequel"]), "exact"]).unwrap();
        conn.execute(rule, params!["new line", archive(&["press enter"]), "command"]).unwrap();
        let record = "INSERT INTO ZTRANSCRIPTIONRECORD
            (ZID, ZTIMESTAMP, ZDURATION, ZTEXT, ZORIGINALTEXT, ZENHANCEDWITH, ZMODELUSED, ZSOURCEKINDRAWVALUE, ZDESTINATIONAPPNAME, ZDESTINATIONAPPBUNDLEID)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
        // 2026-04-07 17:47:25 UTC in Core Data time.
        let t = 797_276_845.0;
        let n = None::<String>;
        conn.execute(record, params![vec![0xA1u8; 16], t, 4.0, "Hello there.", "hello there", "openai/gpt-4o-mini", "parakeet-tdt-0.6b-v3", "voiceRecording", "Slack", "com.tinyspeck.slackmacgap"]).unwrap();
        conn.execute(record, params![vec![0xB2u8; 16], t + 60.0, 2.5, "plain text", n, n, "openai_whisper-tiny", n, n, n]).unwrap();
        // Not dictations: a media transcription, and a recording that a note owns.
        conn.execute(record, params![vec![0xC3u8; 16], t + 120.0, 9.0, "a web video", n, n, "m", "webLink", n, n]).unwrap();
        conn.execute(record, params![vec![0xD4u8; 16], t + 180.0, 9.0, "a voice note", n, n, "m", "voiceRecording", n, n]).unwrap();
        conn.execute("INSERT INTO ZCAPTURESESSIONMODEL (ZTRANSCRIPTIONRECORDID) VALUES (?)", params![vec![0xD4u8; 16]]).unwrap();
        dir
    }

    #[test]
    fn reads_words_rules_presets_and_only_plain_dictations() {
        let dir = pindrop_dir();
        let data = read(dir.path()).unwrap();
        assert_eq!(data.words, ["Sayso", "GPUI"]);
        assert_eq!(
            data.replacements,
            [
                PindropReplacement { from: "git hub".into(), to: "GitHub".into(), case_sensitive: false },
                PindropReplacement { from: "get hub".into(), to: "GitHub".into(), case_sensitive: false },
                PindropReplacement { from: "sequel".into(), to: "SQL".into(), case_sensitive: true },
            ],
            "one rule for each original, and no command rule"
        );
        assert_eq!(data.presets, [PindropPreset { name: "Changelog".into(), prompt: "Write a changelog line.".into() }]);
        let texts: Vec<&str> = data.dictations.iter().map(|d| d.final_text.as_str()).collect();
        assert_eq!(texts, ["Hello there.", "plain text"]);

        let first = &data.dictations[0];
        assert_eq!(first.id, "a1".repeat(16));
        assert_eq!(first.transcript, "hello there");
        assert_eq!(first.enhanced_with.as_deref(), Some("openai/gpt-4o-mini"));
        assert_eq!(first.duration_ms, 4_000);
        // The saved time minus the duration.
        assert_eq!(first.created_at, Utc.with_ymd_and_hms(2026, 4, 7, 17, 47, 21).unwrap());
        assert_eq!(first.app, Some(TargetApp { bundle_id: "com.tinyspeck.slackmacgap".into(), name: "Slack".into() }));
        let second = &data.dictations[1];
        assert_eq!(second.transcript, "plain text", "without an AI pass the final text is the transcript");
        assert_eq!((second.enhanced_with.clone(), second.app.clone()), (None, None));
    }

    #[test]
    fn leaves_the_pindrop_files_unchanged() {
        let dir = pindrop_dir();
        let path = dir.path().join(STORE_FILE);
        let before = std::fs::read(&path).unwrap();
        read(dir.path()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let files: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|f| f.unwrap().file_name()).collect();
        assert_eq!(files, [STORE_FILE], "the read makes no side files in the Pindrop folder");
    }

    #[test]
    fn imports_into_the_store_and_a_second_import_adds_nothing() {
        let dir = pindrop_dir();
        let data = read(dir.path()).unwrap();
        let (_guard, store) = temp_store();
        store.add_word("sayso").unwrap();

        let report = store.import_pindrop(&data).unwrap();
        assert_eq!(report, ImportReport { words: 1, replacements: 3, dictations: 2 }, "the word Sayso existed already");

        let entries = store.list(&HistoryQuery::default()).unwrap();
        assert_eq!(entries.len(), 2);
        let enhanced = entries.iter().find(|e| e.final_text == "Hello there.").unwrap();
        assert_eq!(enhanced.style_id, PINDROP_ID);
        assert_eq!(enhanced.enhance, EnhanceOutcome::Applied { provider_id: PINDROP_ID.into(), model: "openai/gpt-4o-mini".into(), elapsed_ms: 0 });
        assert_eq!(enhanced.model, ModelId::new("parakeet-tdt-0.6b-v3"));
        let plain = entries.iter().find(|e| e.final_text == "plain text").unwrap();
        assert_eq!((plain.style_id.as_str(), &plain.enhance, &plain.audio_file), ("raw", &EnhanceOutcome::NotUsed, &None));
        assert_eq!(store.count(&HistoryQuery { text: Some("hello".into()), ..Default::default() }).unwrap(), 1, "imported text is in the search index");

        assert_eq!(store.import_pindrop(&data).unwrap(), ImportReport::default());
        assert_eq!(store.list(&HistoryQuery::default()).unwrap().len(), 2);
        assert_eq!(store.list_replacements().unwrap().len(), 3);
    }

    #[test]
    fn reads_an_old_store_that_lacks_the_newer_columns_and_tables() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join(STORE_FILE)).unwrap();
        conn.execute_batch(
            "CREATE TABLE ZTRANSCRIPTIONRECORD (Z_PK INTEGER PRIMARY KEY, ZDURATION FLOAT, ZTIMESTAMP TIMESTAMP, ZMODELUSED VARCHAR, ZTEXT VARCHAR, ZID BLOB);
             INSERT INTO ZTRANSCRIPTIONRECORD (ZID, ZTIMESTAMP, ZDURATION, ZMODELUSED, ZTEXT) VALUES (x'0102', 797276845.0, 1.0, 'tiny', 'old entry');",
        )
        .unwrap();
        drop(conn);
        let data = read(dir.path()).unwrap();
        assert_eq!(data.dictations.len(), 1);
        assert_eq!(data.dictations[0].final_text, "old entry");
        assert!(data.words.is_empty() && data.replacements.is_empty() && data.presets.is_empty());
    }

    /// Reads a real Pindrop folder and imports it into a temporary store. It
    /// prints counts only, never text.
    /// `SAYSO_PINDROP_DIR=~/Library/Application\ Support/Pindrop cargo test -p sayso-store real_pindrop -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_pindrop_store() {
        let dir = std::env::var("SAYSO_PINDROP_DIR").expect("set SAYSO_PINDROP_DIR");
        let data = read(Path::new(&dir)).unwrap();
        println!("read: {} words, {} rules, {} presets, {} dictations", data.words.len(), data.replacements.len(), data.presets.len(), data.dictations.len());
        let enhanced = data.dictations.iter().filter(|d| d.enhanced_with.is_some()).count();
        let with_app = data.dictations.iter().filter(|d| d.app.is_some()).count();
        println!("dictations: {enhanced} enhanced, {with_app} with an app, first {:?}, last {:?}", data.dictations.first().map(|d| d.created_at), data.dictations.last().map(|d| d.created_at));
        let (_guard, store) = temp_store();
        let started = std::time::Instant::now();
        println!("import: {:?} in {:?}", store.import_pindrop(&data).unwrap(), started.elapsed());
        assert_eq!(store.import_pindrop(&data).unwrap(), ImportReport::default());
        assert_eq!(store.count(&HistoryQuery::default()).unwrap(), data.dictations.len());
    }

    #[test]
    fn a_folder_without_a_store_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!found(dir.path()));
        assert!(matches!(read(dir.path()), Err(StoreError::InvalidInput(_))));
    }
}
