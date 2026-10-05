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

/// The longest run of spoken words that one dictionary word can be: "may
/// firm o s" for "MayFirmOS".
const MAX_SPOKEN_WORDS: usize = 4;

/// The words of the dictionary that `transcript` can contain: with the correct
/// spelling, in separate words ("pin drop" for "Pindrop"), or with a small
/// error ("fennec o" for "Fenneko"). The style prompt names only these words.
///
/// The comparison ignores case and all characters that are not letters or
/// digits. A word fits when a run of spoken words differs from it in no more
/// than one third of its characters.
pub fn words_heard<'a>(words: &'a [String], transcript: &str) -> Vec<&'a str> {
    let spoken: Vec<Vec<char>> = transcript
        .split(|c: char| !c.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| part.to_lowercase().chars().collect())
        .collect();
    words
        .iter()
        .filter(|word| {
            let key: Vec<char> = word.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
            if key.is_empty() {
                return false;
            }
            let allowed = key.len() / 3;
            (0..spoken.len()).any(|start| {
                let mut run: Vec<char> = Vec::new();
                spoken[start..].iter().take(MAX_SPOKEN_WORDS).any(|part| {
                    run.extend(part);
                    run.len().abs_diff(key.len()) <= allowed && edit_distance(&run, &key) <= allowed
                })
            })
        })
        .map(String::as_str)
        .collect()
}

/// The number of characters to add, remove, or change to make `a` into `b`.
pub(crate) fn edit_distance(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let change = diagonal + usize::from(x != y);
            diagonal = row[j + 1];
            row[j + 1] = change.min(row[j] + 1).min(diagonal + 1);
        }
    }
    row[b.len()]
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

    fn heard(transcript: &str) -> Vec<&'static str> {
        const WORDS: [&str; 7] = ["Claude Code", "Devhouse", "Fenneko", "ForgeCAD", "MayFirmOS", "Pindrop", "Sayso"];
        let words: Vec<String> = WORDS.map(String::from).to_vec();
        // The result borrows from `words`, so map it back to the constants.
        words_heard(&words, transcript).into_iter().map(|w| *WORDS.iter().find(|c| **c == w).unwrap()).collect()
    }

    #[test]
    fn a_transcript_without_a_dictionary_word_has_no_words_heard() {
        assert!(heard("Commit everything you have locally, get it pushed up, and then let's create a new release.").is_empty());
        assert!(heard("").is_empty());
    }

    #[test]
    fn words_heard_finds_other_case_and_separate_words() {
        assert_eq!(heard("open pin drop, then say so and forge cad"), ["ForgeCAD", "Pindrop", "Sayso"]);
        assert_eq!(heard("I asked claude code about it"), ["Claude Code"]);
        assert_eq!(heard("the may firm o s build"), ["MayFirmOS"]);
        assert_eq!(heard("Sayso."), ["Sayso"]);
    }

    #[test]
    fn words_heard_finds_a_word_with_a_small_error() {
        assert_eq!(heard("ask fennec o for the file"), ["Fenneko"]);
        assert_eq!(heard("the dev houses repo"), ["Devhouse"]);
    }

    #[test]
    fn the_edit_distance_counts_changes() {
        let d = |a: &str, b: &str| edit_distance(&a.chars().collect::<Vec<_>>(), &b.chars().collect::<Vec<_>>());
        assert_eq!(d("sayso", "sayso"), 0);
        assert_eq!(d("fennec", "fenneko"), 2);
        assert_eq!(d("", "abc"), 3);
    }
}
