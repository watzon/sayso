//! Cleanup models: small local models that clean up text with fixed settings
//! and read no prompt. See `docs/cleanup-models.md`.
//!
//! This crate holds all knowledge about the models: the catalog, the input
//! format of each model, how long text splits, and the checks of an answer. It
//! does no I/O. The cleanup runtime (`native/cleanup`, GPL) knows no model.
//! `sayso-cleanup-client` starts the runtime and downloads the model files.

pub mod cpu;
pub mod protocol;

use protocol::Stop;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};

/// How a model wants its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptFormat {
    /// S1-mini: ChatML with a fixed system prompt and a control line.
    S1Chat,
    /// Cleanup 1: `key: value` lines, then `out:`.
    KeyValue,
}

/// One cleanup model that Sayso can download and run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupModelInfo {
    /// The catalog id, also the `model` of a provider (`s1-mini`).
    pub id: &'static str,
    /// The name to show. The license of S1-mini asks for this exact name.
    pub name: &'static str,
    /// The publisher, shown as "by Superwhisper".
    pub publisher: &'static str,
    /// The Hugging Face repository and the pinned revision of the files.
    pub repo: &'static str,
    pub revision: &'static str,
    pub file: &'static str,
    pub size_bytes: u64,
    pub sha256: &'static str,
    /// Files of the repository that must stay beside the model file.
    pub license_files: &'static [&'static str],
    /// The languages that the model supports, as language codes.
    pub languages: &'static [&'static str],
    /// True when the model has tones (Cleanup 1).
    pub tones: bool,
    pub layouts: &'static [Layout],
    pub formats: &'static [Format],
    /// True when the model reads the app and the site of the dictation.
    pub context: bool,
    pub prompt: PromptFormat,
}

/// The context size that the runtime loads a model with. With
/// [`MAX_PART_TOKENS`] of text, the prompt and the longest answer fit.
pub const N_CTX: u32 = 2048;

/// The most text tokens in one part. S1-mini wants under about 1,000.
/// 700 tokens of text, the prompt around it, and an answer of 1.3 × 700 + 32
/// tokens fit in [`N_CTX`].
pub const MAX_PART_TOKENS: u32 = 700;

const S1_SYSTEM: &str = "You are a text normalizer for speech-to-text transcripts. The input begins with a control line \
specifying the styling, structure, and context settings; clean the transcript to match those settings and output only \
the cleaned text.";

const CATALOG: &[CleanupModelInfo] = &[
    CleanupModelInfo {
        id: "s1-mini",
        name: "S1-mini",
        publisher: "Superwhisper",
        repo: "superwhisper/s1-mini-GGUF",
        revision: "34add00a48a2e5d24e5a4ee5405a99620a3a240c",
        file: "s1-mini-q4_k_m.gguf",
        size_bytes: 484_219_808,
        sha256: "3b41ebe2502cbd03e811d5d16b022f5ab551eda58d62597d152f89535003c634",
        license_files: &["LICENSE", "NOTICE"],
        languages: &["en"],
        tones: false,
        layouts: &[Layout::Paragraphs, Layout::List],
        formats: &[Format::Plain, Format::Email],
        context: false,
        prompt: PromptFormat::S1Chat,
    },
    CleanupModelInfo {
        id: "spokenly-cleanup-1",
        name: "Cleanup 1",
        publisher: "Spokenly",
        repo: "spokenly/cleanup-1-gguf",
        revision: "79936c95e5c0ea9162ae7a435e64cb3127020e36",
        file: "cleanup-1-q8_0.gguf",
        size_bytes: 639_442_784,
        sha256: "66509e0fe1578d2ac4b0773c79a6c83e8d6f218618dfe11e4532beacd999b6e8",
        license_files: &["LICENSE", "NOTICE"],
        languages: &["en", "de", "es", "fr", "zh", "ru", "ja"],
        tones: true,
        layouts: &[Layout::Paragraphs],
        formats: &[Format::Plain],
        context: true,
        prompt: PromptFormat::KeyValue,
    },
];

pub fn catalog() -> &'static [CleanupModelInfo] {
    CATALOG
}

pub fn find(id: &str) -> Option<&'static CleanupModelInfo> {
    CATALOG.iter().find(|m| m.id == id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    VeryCasual,
    Casual,
    #[default]
    Formal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    #[default]
    Paragraphs,
    List,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Plain,
    Email,
}

impl Tone {
    pub fn code(self) -> &'static str {
        match self {
            Tone::VeryCasual => "very-casual",
            Tone::Casual => "casual",
            Tone::Formal => "formal",
        }
    }
}

impl Layout {
    pub fn code(self) -> &'static str {
        match self {
            Layout::Paragraphs => "paragraphs",
            Layout::List => "list",
        }
    }
}

impl Format {
    pub fn code(self) -> &'static str {
        match self {
            Format::Plain => "plain",
            Format::Email => "email",
        }
    }
}

/// The `[cleanup]` table of a style file. The values are strings, so a value
/// from a newer Sayso does not break the file. It makes [`CleanupModelInfo::runs`]
/// false instead.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

/// Cleanup settings with known values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Settings {
    pub tone: Tone,
    pub layout: Layout,
    pub format: Format,
}

impl CleanupSettings {
    pub fn new(tone: Tone, layout: Layout, format: Format) -> Self {
        let set =
            |code: &'static str, default: &'static str| (code != default).then(|| code.to_string());
        CleanupSettings {
            tone: set(tone.code(), Tone::default().code()),
            layout: set(layout.code(), Layout::default().code()),
            format: set(format.code(), Format::default().code()),
        }
    }

    /// The settings, or None when a value is not known to this version.
    pub fn known(&self) -> Option<Settings> {
        let tone = match self.tone.as_deref() {
            None | Some("formal") => Tone::Formal,
            Some("casual") => Tone::Casual,
            Some("very-casual") => Tone::VeryCasual,
            Some(_) => return None,
        };
        let layout = match self.layout.as_deref() {
            None | Some("paragraphs") => Layout::Paragraphs,
            Some("list") => Layout::List,
            Some(_) => return None,
        };
        let format = match self.format.as_deref() {
            None | Some("plain") => Format::Plain,
            Some("email") => Format::Email,
            Some(_) => return None,
        };
        Some(Settings {
            tone,
            layout,
            format,
        })
    }
}

impl CleanupModelInfo {
    /// True when the model can run a style with these settings: every value is
    /// known, and the model has the layout and the format.
    pub fn runs(&self, settings: &CleanupSettings) -> bool {
        settings
            .known()
            .is_some_and(|s| self.layouts.contains(&s.layout) && self.formats.contains(&s.format))
    }

    pub fn supports_language(&self, code: &str) -> bool {
        self.languages.contains(&code)
    }

    /// The URL of a file of the pinned revision.
    pub fn file_url(&self, file: &str) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{file}",
            self.repo, self.revision
        )
    }
}

/// One request to a cleanup model, for one part of the text.
#[derive(Debug, Clone, PartialEq)]
pub struct RawPrompt {
    pub text: String,
    /// Strings that end the answer, beside the end-of-generation token.
    pub end: Vec<String>,
    pub max_tokens: u32,
}

/// The longest answer for a part of `text_tokens` tokens.
pub fn max_answer_tokens(text_tokens: u32) -> u32 {
    text_tokens * 13 / 10 + 32
}

/// Write the input of `model` for one part of the text. `app` and `site` are
/// used only by a model that reads context.
pub fn render(
    model: &CleanupModelInfo,
    text: &str,
    settings: Settings,
    app: Option<&str>,
    site: Option<&str>,
    text_tokens: u32,
) -> RawPrompt {
    let max_tokens = max_answer_tokens(text_tokens);
    match model.prompt {
        PromptFormat::S1Chat => {
            // The evaluation: casual styling keeps filler words, and
            // semi-formal and formal give the same text. So always semi-formal.
            let structure = if settings.layout == Layout::List {
                "lists"
            } else {
                "prose"
            };
            let context = if settings.format == Format::Email {
                "email"
            } else {
                "general"
            };
            RawPrompt {
                text: format!(
                    "<|im_start|>system\n{S1_SYSTEM}<|im_end|>\n<|im_start|>user\n[Styling: semi-formal] [Structure: {structure}] [Context: {context}]\n{text}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
                ),
                end: vec!["<|im_end|>".into()],
                max_tokens,
            }
        }
        PromptFormat::KeyValue => {
            let one_line = |value: &str| value.split_whitespace().collect::<Vec<_>>().join(" ");
            let mut lines = Vec::new();
            if model.context {
                if let Some(app) = app.map(one_line).filter(|a| !a.is_empty()) {
                    lines.push(format!("app: {app}"));
                }
                if let Some(site) = site.map(one_line).filter(|s| !s.is_empty()) {
                    lines.push(format!("site: {site}"));
                }
            }
            if model.tones {
                let tone = match settings.tone {
                    Tone::VeryCasual => "very casual",
                    Tone::Casual => "casual",
                    Tone::Formal => "formal",
                };
                lines.push(format!("tone: {tone}"));
            }
            lines.push(format!("text: {text}"));
            lines.push("out:".into());
            // The model ends with its end-of-text token. The strings guard
            // against an answer that goes on with a new key.
            RawPrompt {
                text: lines.join("\n"),
                end: vec!["\ntext:".into(), "\nout:".into()],
                max_tokens,
            }
        }
    }
}

/// Split `text` into parts of at most `max_tokens` tokens. `count` gives the
/// tokens of a text; its first error ends the split. Every character of `text`
/// is in exactly one part, in order. A part ends after a sentence end if it
/// can, else after a comma, else after a space, else after any character.
pub fn plan_parts<E>(
    text: &str,
    max_tokens: u32,
    count: &mut dyn FnMut(&str) -> Result<u32, E>,
) -> Result<Vec<String>, E> {
    let mut parts = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        if count(rest)? <= max_tokens {
            parts.push(rest.to_string());
            break;
        }
        let cut = best_cut(rest, max_tokens, count)?;
        parts.push(rest[..cut].to_string());
        rest = &rest[cut..];
    }
    Ok(parts)
}

/// The byte index of the end of the longest prefix of `text` that fits.
fn best_cut<E>(
    text: &str,
    max_tokens: u32,
    count: &mut dyn FnMut(&str) -> Result<u32, E>,
) -> Result<usize, E> {
    let sentence = |c: char| matches!(c, '.' | '?' | '!' | '。' | '？' | '！' | '\n');
    let levels: [&dyn Fn(char) -> bool; 3] = [
        &sentence,
        &|c| matches!(c, ',' | '，' | '、'),
        &char::is_whitespace,
    ];
    for is_cut in levels {
        let cuts = cut_points(text, is_cut);
        if let Some(cut) = longest_fit(text, &cuts, max_tokens, count)? {
            return Ok(cut);
        }
    }
    let every: Vec<usize> = text.char_indices().map(|(i, c)| i + c.len_utf8()).collect();
    // One character always goes, so the loop ends also when one character is over the limit.
    Ok(longest_fit(text, &every, max_tokens, count)?.unwrap_or(every[0]))
}

/// The byte index after each character for which `is_cut` is true, with the
/// white space that follows it.
fn cut_points(text: &str, is_cut: &dyn Fn(char) -> bool) -> Vec<usize> {
    let mut cuts = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if is_cut(c) {
            let mut end = i + c.len_utf8();
            while let Some(&(j, w)) = chars.peek() {
                if !w.is_whitespace() {
                    break;
                }
                end = j + w.len_utf8();
                chars.next();
            }
            if end < text.len() {
                cuts.push(end);
            }
        }
    }
    cuts.dedup();
    cuts
}

/// The largest cut whose prefix fits, by binary search (the count grows with the prefix).
fn longest_fit<E>(
    text: &str,
    cuts: &[usize],
    max_tokens: u32,
    count: &mut dyn FnMut(&str) -> Result<u32, E>,
) -> Result<Option<usize>, E> {
    let (mut lo, mut hi, mut best) = (0usize, cuts.len(), None);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if count(&text[..cuts[mid]])? <= max_tokens {
            best = Some(cuts[mid]);
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    Ok(best)
}

/// Join the answers of the parts. A part that ended with a new line keeps it
/// between the answers; other parts get one space.
pub fn join_answers(parts: &[String], answers: &[String]) -> String {
    let mut out = String::new();
    for (i, answer) in answers.iter().enumerate() {
        if answer.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(
                if parts[i - 1].trim_end_matches([' ', '\t']).ends_with('\n') {
                    '\n'
                } else {
                    ' '
                },
            );
        }
        out.push_str(answer);
    }
    out
}

/// Why an answer is not used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CheckFailure {
    #[error("the answer stopped early ({0:?})")]
    Cut(Stop),
    #[error("the answer is empty")]
    Empty,
    #[error("the answer is much shorter than the text")]
    TooShort,
    #[error("the answer is much longer than the text")]
    TooLong,
}

const CONTROL_TEXT: &[&str] = &[
    "<|im_start|>",
    "<|im_end|>",
    "<|endoftext|>",
    "<think>",
    "</think>",
];

/// Check the answer of a model for one part of the text, and clean it.
///
/// The end check is the guarantee: only an answer that stopped at its end
/// marker is whole. The length checks are a loose safety net.
pub fn check(input: &str, stop: Stop, answer: &str) -> Result<String, CheckFailure> {
    if stop != Stop::End {
        return Err(CheckFailure::Cut(stop));
    }
    let mut text = answer.to_string();
    for control in CONTROL_TEXT {
        text = text.replace(control, "");
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        return if is_filler_only(input) {
            Ok(text)
        } else {
            Err(CheckFailure::Empty)
        };
    }
    let (before, after) = (content_units(input), content_units(&text));
    if before >= 10 && after * 4 < before {
        return Err(CheckFailure::TooShort);
    }
    if text.chars().count() > input.chars().count() * 3 + 200 {
        return Err(CheckFailure::TooLong);
    }
    Ok(text)
}

/// English number words. A run of them is one number, like its digits.
const NUMBER_WORDS: &[&str] = &[
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
    "twenty",
    "thirty",
    "forty",
    "fifty",
    "sixty",
    "seventy",
    "eighty",
    "ninety",
    "hundred",
    "thousand",
    "million",
    "billion",
    "point",
];

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF)
}

/// How much a text says, for the length check: one unit for a word, for each
/// Chinese, Japanese, or Korean character, and for a run of number words or
/// digits. So "nine hundred and ninety nine" and "999" are one unit each.
pub(crate) fn content_units(text: &str) -> usize {
    let mut units = 0;
    let mut in_number = false;
    let mut word = String::new();
    let flush = |word: &mut String, units: &mut usize, in_number: &mut bool| {
        if word.is_empty() {
            return;
        }
        let lower = word.to_lowercase();
        let number =
            lower.chars().any(|c| c.is_ascii_digit()) || NUMBER_WORDS.contains(&lower.as_str());
        let joins = *in_number && lower == "and";
        if number || joins {
            if !*in_number {
                *units += 1;
            }
            *in_number = true;
        } else {
            *units += 1;
            *in_number = false;
        }
        word.clear();
    };
    for c in text.chars() {
        if is_cjk(c) {
            flush(&mut word, &mut units, &mut in_number);
            units += 1;
            in_number = false;
        } else if c.is_alphanumeric() {
            word.push(c);
        } else {
            // Punctuation inside a number ("999,999", "5:30") does not end the number.
            flush(&mut word, &mut units, &mut in_number);
        }
    }
    flush(&mut word, &mut units, &mut in_number);
    units
}

/// Filler words of the languages of the catalog. An empty answer is right only
/// for a text of these words.
const FILLERS: &[&str] = &[
    // English
    "um",
    "umm",
    "uh",
    "uhm",
    "uhh",
    "er",
    "erm",
    "ah",
    "hmm",
    "hm",
    "mm",
    "mhm",
    "like",
    "so",
    "well",
    "you",
    "know",
    // German
    "äh",
    "ähm",
    "öh",
    "halt",
    "also",
    // French
    "euh",
    "bah",
    "ben",
    "hein",
    // Spanish
    "eh",
    "em",
    "este",
    "pues",
    "bueno",
    // Russian
    "э",
    "эм",
    "ну",
    "вот",
    // Chinese
    "嗯",
    "呃",
    "啊",
    "那个",
    "就是",
    // Japanese
    "えー",
    "えっと",
    "あの",
    "あのー",
    "まあ",
];

/// True when every word of `text` is a filler word ("um uh you know"). A text
/// with no words is not filler only.
pub fn is_filler_only(text: &str) -> bool {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric() && c != 'ー')
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    // "you" and "know" count only together.
    let joined = format!(" {} ", words.join(" ")).replace(" you know ", " ");
    !words.is_empty()
        && joined
            .split_whitespace()
            .all(|w| FILLERS.contains(&w) && w != "you" && w != "know")
}

/// Put back the spelling of `terms` that the model changed: a term that was in
/// `sent` with its exact spelling (and in no other case), and is in `answer`
/// with the same letters in another case, as a whole word. Longer terms first, and a term inside a term
/// that was already put back is not changed.
pub fn restore_spelling(sent: &str, answer: &str, terms: &[String]) -> String {
    let mut terms: Vec<&str> = terms
        .iter()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect();
    terms.sort_by_key(|t| std::cmp::Reverse(t.chars().count()));
    terms.dedup();
    let mut out = answer.to_string();
    let mut fixed: Vec<std::ops::Range<usize>> = Vec::new();
    for term in terms {
        let Some(exact) = whole_word(term, false) else {
            continue;
        };
        if !exact.is_match(sent) {
            continue;
        }
        let Some(any_case) = whole_word(term, true) else {
            continue;
        };
        // The text had the same word in another case too ("we may ship in
        // May"): Sayso cannot tell which one the model changed.
        if any_case.find_iter(sent).any(|m| m.as_str() != term) {
            continue;
        }
        let found: Vec<std::ops::Range<usize>> = any_case
            .find_iter(&out)
            .filter(|m| m.as_str() != term && m.as_str().len() == term.len())
            .map(|m| m.range())
            .filter(|r| !fixed.iter().any(|f| f.start < r.end && r.start < f.end))
            .collect();
        for range in found {
            out.replace_range(range.clone(), term);
            fixed.push(range);
        }
        fixed.extend(exact.find_iter(&out).map(|m| m.range()));
    }
    out
}

/// A regex for `term` as a whole word, like the replacements of the dictionary.
fn whole_word(term: &str, any_case: bool) -> Option<regex::Regex> {
    let start = if term.starts_with(|c: char| c.is_alphanumeric()) {
        r"\b"
    } else {
        ""
    };
    let end = if term.ends_with(|c: char| c.is_alphanumeric()) {
        r"\b"
    } else {
        ""
    };
    RegexBuilder::new(&format!("{start}{}{end}", regex::escape(term)))
        .case_insensitive(any_case)
        .build()
        .ok()
}

/// The language setting that asks the model to detect the language.
pub const AUTO_LANGUAGE: &str = "auto";

/// The language of a dictation, when Sayso knows it.
///
/// 1. The setting is a language code: that language.
/// 2. The setting is "auto" and the final model knows one language: that language.
/// 3. Else `detect` on the text, which gives a language only when it is sure.
pub fn dictation_language(
    setting: &str,
    final_model_languages: &[&str],
    text: &str,
    detect: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    if setting != AUTO_LANGUAGE && !setting.is_empty() {
        return Some(setting.to_string());
    }
    if let [one] = final_model_languages {
        return Some(one.to_string());
    }
    detect(text)
}

/// One cleanup of a whole text with a cleanup model.
#[derive(Debug, Clone, PartialEq)]
pub struct CleanupRequest {
    /// The catalog id of the model.
    pub model: String,
    pub text: String,
    pub settings: Settings,
    /// The name of the app of the dictation, for a model that reads context.
    pub app: Option<String>,
    /// The host of the web page of the dictation.
    pub site: Option<String>,
    /// One deadline for the whole text, all parts together. Zero means no deadline.
    pub timeout: std::time::Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CleanupResponse {
    pub text: String,
    /// The catalog id of the model.
    pub model: String,
    pub elapsed_ms: u64,
    /// The time to load the model in this request. Zero when it was loaded.
    pub load_ms: u64,
    pub parts: u32,
    pub tokens_in: u32,
    pub tokens_out: u32,
}

/// Why a cleanup gave no text. The caller keeps the text that went in.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CleanError {
    #[error("timed out after {0} ms")]
    Timeout(u64),
    /// The model file or the runtime is not there.
    #[error("the cleanup model is not set up: {0}")]
    NotConfigured(String),
    /// The runtime crashed or sent an error.
    #[error("{0}")]
    Runtime(String),
    /// The answer did not pass [`check`].
    #[error("the answer is not usable: {0}")]
    InvalidOutput(String),
}

/// Runs a cleanup model. Calls block. Run them on a background thread.
pub trait Cleaner: Send + Sync {
    fn clean(&self, request: &CleanupRequest) -> Result<CleanupResponse, CleanError>;
}

/// The language of `text` as a Sayso language code, when the detector is sure.
/// A short or mixed text is often not sure, and then this gives None.
pub fn detect_language(text: &str) -> Option<String> {
    let info = whatlang::detect(text).filter(whatlang::Info::is_reliable)?;
    // Sayso uses ISO 639-1 where one exists (see `languages`).
    let code = match info.lang().code() {
        "eng" => "en",
        "deu" => "de",
        "spa" => "es",
        "fra" => "fr",
        "cmn" => "zh",
        "rus" => "ru",
        "jpn" => "ja",
        "ita" => "it",
        "por" => "pt",
        "nld" => "nl",
        "kor" => "ko",
        other => other,
    };
    Some(code.to_string())
}

#[cfg(test)]
mod tests;
