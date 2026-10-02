//! The dictionary: words that bias recognition, and replacement rules.

use crate::Store;
use crate::error::{Result, StoreError};
use rusqlite::{OptionalExtension, params};
use sayso_core::dictionary::{AppliedReplacement, Replacement, Word};

fn replacement_from_row(r: &rusqlite::Row) -> rusqlite::Result<Replacement> {
    Ok(Replacement {
        id: r.get(0)?,
        from: r.get(1)?,
        to: r.get(2)?,
        case_sensitive: r.get(3)?,
        uses: r.get::<_, i64>(4)? as u64,
    })
}

fn require_from(from: &str) -> Result<()> {
    if from.trim().is_empty() {
        Err(StoreError::InvalidInput("a replacement needs text to find".into()))
    } else {
        Ok(())
    }
}

impl Store {
    /// All words, sorted without regard to case.
    pub fn list_words(&self) -> Result<Vec<Word>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, text FROM words ORDER BY text COLLATE NOCASE")?;
        let rows = stmt.query_map([], |r| Ok(Word { id: r.get(0)?, text: r.get(1)? }))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Add a word. Surrounding space is trimmed. When the word exists already,
    /// ignoring case, the existing word is returned and nothing changes.
    pub fn add_word(&self, text: &str) -> Result<Word> {
        let text = text.trim();
        if text.is_empty() {
            return Err(StoreError::InvalidInput("a word cannot be empty".into()));
        }
        let conn = self.conn.lock();
        conn.execute("INSERT OR IGNORE INTO words (text) VALUES (?)", params![text])?;
        Ok(conn.query_row("SELECT id, text FROM words WHERE text = ?", params![text], |r| {
            Ok(Word { id: r.get(0)?, text: r.get(1)? })
        })?)
    }

    /// Remove a word. Returns false when the id does not exist.
    pub fn remove_word(&self, id: i64) -> Result<bool> {
        Ok(self.conn.lock().execute("DELETE FROM words WHERE id = ?", params![id])? > 0)
    }

    /// All replacement rules in creation order.
    pub fn list_replacements(&self) -> Result<Vec<Replacement>> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT id, from_text, to_text, case_sensitive, uses FROM replacements ORDER BY id")?;
        let rows = stmt.query_map([], replacement_from_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Add a rule and return it with its new id. The `id` and `uses` fields of
    /// `rule` are ignored: a new rule has 0 uses.
    pub fn add_replacement(&self, rule: &Replacement) -> Result<Replacement> {
        require_from(&rule.from)?;
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO replacements (from_text, to_text, case_sensitive) VALUES (?, ?, ?)",
            params![rule.from.trim(), rule.to, rule.case_sensitive],
        )?;
        Ok(Replacement { id: conn.last_insert_rowid(), from: rule.from.trim().to_string(), uses: 0, ..rule.clone() })
    }

    /// Change the text, the target, and the case flag of a rule. The use count stays.
    pub fn update_replacement(&self, rule: &Replacement) -> Result<()> {
        require_from(&rule.from)?;
        let changed = self.conn.lock().execute(
            "UPDATE replacements SET from_text = ?, to_text = ?, case_sensitive = ? WHERE id = ?",
            params![rule.from.trim(), rule.to, rule.case_sensitive, rule.id],
        )?;
        if changed == 0 { Err(StoreError::NotFound(rule.id)) } else { Ok(()) }
    }

    /// Remove a rule. Returns false when the id does not exist.
    pub fn remove_replacement(&self, id: i64) -> Result<bool> {
        Ok(self.conn.lock().execute("DELETE FROM replacements WHERE id = ?", params![id])? > 0)
    }

    /// One rule by id.
    pub fn get_replacement(&self, id: i64) -> Result<Option<Replacement>> {
        Ok(self
            .conn
            .lock()
            .query_row(
                "SELECT id, from_text, to_text, case_sensitive, uses FROM replacements WHERE id = ?",
                params![id],
                replacement_from_row,
            )
            .optional()?)
    }

    /// Add the `count` of each applied rule to its `uses`. Rules that were
    /// deleted in the meantime are skipped. All updates commit together.
    pub fn record_replacement_uses(&self, applied: &[AppliedReplacement]) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        for a in applied {
            tx.execute("UPDATE replacements SET uses = uses + ? WHERE id = ?", params![a.count as i64, a.replacement_id])?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::temp_store;

    fn rule(from: &str, to: &str) -> Replacement {
        Replacement { id: 0, from: from.into(), to: to.into(), case_sensitive: false, uses: 9 }
    }

    #[test]
    fn words_dedupe_without_regard_to_case() {
        let (_dir, store) = temp_store();
        let first = store.add_word("  Sayso ").unwrap();
        let again = store.add_word("SAYSO").unwrap();
        assert_eq!(first, again);
        assert_eq!(first.text, "Sayso", "the first spelling wins");
        store.add_word("kubectl").unwrap();
        let words: Vec<_> = store.list_words().unwrap().into_iter().map(|w| w.text).collect();
        assert_eq!(words, ["kubectl", "Sayso"]);
        assert!(matches!(store.add_word("  "), Err(StoreError::InvalidInput(_))));
    }

    #[test]
    fn remove_word_reports_whether_it_existed() {
        let (_dir, store) = temp_store();
        let w = store.add_word("x").unwrap();
        assert!(store.remove_word(w.id).unwrap());
        assert!(!store.remove_word(w.id).unwrap());
        assert!(store.list_words().unwrap().is_empty());
    }

    #[test]
    fn replacement_crud() {
        let (_dir, store) = temp_store();
        let added = store.add_replacement(&rule("git hub", "GitHub")).unwrap();
        assert_eq!(added.uses, 0, "a new rule starts at zero uses");
        assert_eq!(store.list_replacements().unwrap(), vec![added.clone()]);

        let mut edited = added.clone();
        edited.to = "Github".into();
        edited.case_sensitive = true;
        store.update_replacement(&edited).unwrap();
        assert_eq!(store.get_replacement(added.id).unwrap(), Some(edited.clone()));

        edited.id = 99;
        assert!(matches!(store.update_replacement(&edited), Err(StoreError::NotFound(99))));
        assert!(store.add_replacement(&rule("  ", "x")).is_err());
        assert!(store.remove_replacement(added.id).unwrap());
        assert!(store.list_replacements().unwrap().is_empty());
    }

    #[test]
    fn use_counts_add_up_and_survive_edits() {
        let (_dir, store) = temp_store();
        let a = store.add_replacement(&rule("git hub", "GitHub")).unwrap();
        let b = store.add_replacement(&rule("new paragraph", "\\n\\n")).unwrap();
        let applied = |r: &Replacement, count| AppliedReplacement {
            replacement_id: r.id,
            from: r.from.clone(),
            to: r.to.clone(),
            count,
        };
        store.record_replacement_uses(&[applied(&a, 2), applied(&b, 1)]).unwrap();
        store.record_replacement_uses(&[applied(&a, 3)]).unwrap();
        store.record_replacement_uses(&[AppliedReplacement { replacement_id: 404, from: "x".into(), to: "y".into(), count: 1 }]).unwrap();
        let mut a_now = store.get_replacement(a.id).unwrap().unwrap();
        assert_eq!(a_now.uses, 5);
        assert_eq!(store.get_replacement(b.id).unwrap().unwrap().uses, 1);
        a_now.to = "GH".into();
        store.update_replacement(&a_now).unwrap();
        assert_eq!(store.get_replacement(a.id).unwrap().unwrap().uses, 5);
    }
}
