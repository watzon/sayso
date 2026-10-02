//! The personal dictionary: words (recognition bias) and replacements
//! (rules applied after transcription).

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Word {
    pub id: i64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replacement {
    pub id: i64,
    /// What the speaker says, for example "git hub".
    pub from: String,
    /// What to write. `\n` writes a line break.
    pub to: String,
    #[serde(default)]
    pub case_sensitive: bool,
    /// How many times the rule was applied. Shown in the ledger.
    #[serde(default)]
    pub uses: u64,
}

/// One rule that changed the text, for the History trail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedReplacement {
    pub replacement_id: i64,
    pub from: String,
    pub to: String,
    pub count: usize,
}

/// Compiled replacement rules. Build once per dictionary change.
pub struct Replacer {
    rules: Vec<(Replacement, Regex)>,
}

impl Replacer {
    pub fn new(replacements: &[Replacement]) -> Self {
        // Longer phrases first, so "new paragraph" wins over "new".
        let mut sorted: Vec<&Replacement> = replacements.iter().filter(|r| !r.from.trim().is_empty()).collect();
        sorted.sort_by(|a, b| b.from.len().cmp(&a.from.len()).then(a.id.cmp(&b.id)));
        let rules = sorted
            .into_iter()
            .filter_map(|r| {
                // Words in the phrase may be split by any whitespace in the transcript.
                let words: Vec<String> = r.from.split_whitespace().map(regex::escape).collect();
                let body = words.join(r"\s+");
                // Word boundaries only where the phrase starts or ends with a word character.
                let start = if r.from.trim_start().starts_with(|c: char| c.is_alphanumeric()) { r"\b" } else { "" };
                let end = if r.from.trim_end().ends_with(|c: char| c.is_alphanumeric()) { r"\b" } else { "" };
                let pattern = format!("{start}{body}{end}");
                RegexBuilder::new(&pattern).case_insensitive(!r.case_sensitive).build().ok().map(|re| (r.clone(), re))
            })
            .collect();
        Replacer { rules }
    }

    pub fn apply(&self, text: &str) -> (String, Vec<AppliedReplacement>) {
        let mut out = text.to_string();
        let mut applied = Vec::new();
        for (rule, re) in &self.rules {
            let count = re.find_iter(&out).count();
            if count == 0 {
                continue;
            }
            let to = rule.to.replace("\\n", "\n");
            out = re.replace_all(&out, regex::NoExpand(&to)).into_owned();
            applied.push(AppliedReplacement { replacement_id: rule.id, from: rule.from.clone(), to: rule.to.clone(), count });
        }
        (tidy_spacing(&out), applied)
    }
}

/// Remove spaces that replacements leave around line breaks and punctuation.
fn tidy_spacing(s: &str) -> String {
    s.split('\n').map(|line| line.trim_matches(' ')).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: i64, from: &str, to: &str) -> Replacement {
        Replacement { id, from: from.into(), to: to.into(), case_sensitive: false, uses: 0 }
    }

    #[test]
    fn replaces_phrases_case_insensitive_with_whitespace_variance() {
        let r = Replacer::new(&[rule(1, "git hub", "GitHub")]);
        let (out, applied) = r.apply("open a Git  Hub issue on git hub");
        assert_eq!(out, "open a GitHub issue on GitHub");
        assert_eq!(applied[0].count, 2);
    }

    #[test]
    fn respects_word_boundaries() {
        let r = Replacer::new(&[rule(1, "cat", "dog")]);
        assert_eq!(r.apply("concatenate the cat").0, "concatenate the dog");
    }

    #[test]
    fn longer_rules_win_over_overlapping_shorter_ones() {
        let r = Replacer::new(&[rule(1, "new", "NEW"), rule(2, "new paragraph", "\\n\\n")]);
        let (out, _) = r.apply("first part new paragraph second part is new");
        assert_eq!(out, "first part\n\nsecond part is NEW");
    }

    #[test]
    fn case_sensitive_rules_only_match_exact_case() {
        let mut r = rule(1, "Go", "Golang");
        r.case_sensitive = true;
        let rep = Replacer::new(&[r]);
        assert_eq!(rep.apply("go learn Go").0, "go learn Golang");
    }

    #[test]
    fn replacement_text_is_literal() {
        let r = Replacer::new(&[rule(1, "my email", "chris+$1@example.com")]);
        assert_eq!(r.apply("send it to my email").0, "send it to chris+$1@example.com");
    }

    #[test]
    fn empty_rules_are_ignored() {
        let r = Replacer::new(&[rule(1, "  ", "x")]);
        assert_eq!(r.apply("hello").0, "hello");
    }
}
