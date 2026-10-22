//! Tests with real model files. They are ignored by default. Run them with
//!
//! ```text
//! SAYSO_CLEANUP_S1=/path/s1-mini-q4_k_m.gguf \
//! SAYSO_CLEANUP_SPOKENLY=/path/cleanup-1-q8_0.gguf \
//! cargo test -- --ignored --test-threads=1
//! ```

mod common;

use common::Session;
use sayso_core::cleanup::protocol::{ErrorCode, Reply, Request, Stop};

const S1_PROMPT: &str = "<|im_start|>system\nYou are a text normalizer for speech-to-text transcripts. The input begins with a control line specifying the styling, structure, and context settings; clean the transcript to match those settings and output only the cleaned text.<|im_end|>\n<|im_start|>user\n[Styling: semi-formal] [Structure: prose] [Context: general]\num so the meeting is on tuesday no wait wednesday at three thirty<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n";
const SPOKENLY_PROMPT: &str =
    "tone: formal\ntext: um so the meeting is on tuesday no wait wednesday at three thirty\nout:";

fn model_path(var: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| panic!("set {var} to a model file"))
}

fn load(s: &mut Session, model_id: &str, path: String) {
    match s.call(Request::Load {
        model_id: model_id.into(),
        path,
        n_ctx: 2048,
    }) {
        Reply::Loaded {
            model_id: id,
            load_ms,
        } => {
            assert_eq!(id, model_id);
            eprintln!("load {model_id}: {load_ms} ms");
        }
        other => panic!("{other:?}"),
    }
}

struct Done {
    text: String,
    stop: Stop,
    tokens_in: u32,
    tokens_out: u32,
}

fn complete(s: &mut Session, model_id: &str, prompt: &str, end: &[&str], max_tokens: u32) -> Done {
    let request = Request::Complete {
        model_id: model_id.into(),
        prompt: prompt.into(),
        end: end.iter().map(|e| e.to_string()).collect(),
        max_tokens,
    };
    match s.call(request) {
        Reply::Completion {
            text,
            stop,
            tokens_in,
            tokens_out,
            ms,
        } => {
            eprintln!(
                "complete {model_id}: {ms} ms, {tokens_in} in, {tokens_out} out, {stop:?}, {text:?}"
            );
            Done {
                text,
                stop,
                tokens_in,
                tokens_out,
            }
        }
        other => panic!("{other:?}"),
    }
}

#[test]
#[ignore = "needs SAYSO_CLEANUP_S1, the path of s1-mini-q4_k_m.gguf"]
fn s1_mini_cleans_the_meeting_sentence() {
    let mut s = Session::start();
    load(&mut s, "s1-mini", model_path("SAYSO_CLEANUP_S1"));
    let done = complete(&mut s, "s1-mini", S1_PROMPT, &["<|im_end|>"], 64);
    assert_eq!(done.stop, Stop::End);
    assert_eq!(done.text.trim(), "So the meeting is on Wednesday at 3:30.");
    assert!(!done.text.contains("<|im_end|>"));
    assert!(done.tokens_in > 50 && done.tokens_out > 3);
    assert!(s.finish().success());
}

#[test]
#[ignore = "needs SAYSO_CLEANUP_SPOKENLY, the path of cleanup-1-q8_0.gguf"]
fn spokenly_cleans_the_meeting_sentence() {
    let mut s = Session::start();
    load(&mut s, "cleanup-1", model_path("SAYSO_CLEANUP_SPOKENLY"));
    let done = complete(&mut s, "cleanup-1", SPOKENLY_PROMPT, &[], 64);
    assert_eq!(done.stop, Stop::End);
    assert_eq!(done.text.trim(), "The meeting is on Wednesday at 3:30.");
    assert!(s.finish().success());
}

#[test]
#[ignore = "needs SAYSO_CLEANUP_S1"]
fn max_tokens_gives_a_limit_stop() {
    let mut s = Session::start();
    load(&mut s, "s1-mini", model_path("SAYSO_CLEANUP_S1"));
    let done = complete(&mut s, "s1-mini", S1_PROMPT, &["<|im_end|>"], 2);
    assert_eq!(done.stop, Stop::Limit);
    assert_eq!(done.tokens_out, 2);
    assert!(s.finish().success());
}

#[test]
#[ignore = "needs SAYSO_CLEANUP_S1"]
fn a_prompt_that_does_not_fit_gives_a_context_stop() {
    let mut s = Session::start();
    load(&mut s, "s1-mini", model_path("SAYSO_CLEANUP_S1"));
    // n_ctx is 2048. A prompt of about 3000 tokens, and a short prompt with a
    // long answer, both stop with `context` and decode nothing.
    let long = "word ".repeat(3000);
    let done = complete(&mut s, "s1-mini", &long, &[], 16);
    assert_eq!(
        (done.stop, done.text.as_str(), done.tokens_out),
        (Stop::Context, "", 0)
    );
    let done = complete(&mut s, "s1-mini", S1_PROMPT, &["<|im_end|>"], 2048);
    assert_eq!(
        (done.stop, done.text.as_str(), done.tokens_out),
        (Stop::Context, "", 0)
    );
    // A prompt longer than one decode batch of the default size still works.
    let long = "word ".repeat(1200);
    let done = complete(&mut s, "s1-mini", &long, &[], 4);
    assert!(done.tokens_in > 1000 && done.stop == Stop::Limit);
    assert!(s.finish().success());
}

#[test]
#[ignore = "needs SAYSO_CLEANUP_S1"]
fn count_matches_the_tokenizer_and_the_model_id_is_checked() {
    let mut s = Session::start();
    load(&mut s, "s1-mini", model_path("SAYSO_CLEANUP_S1"));
    match s.call(Request::Count {
        model_id: "s1-mini".into(),
        text: "hello world".into(),
    }) {
        Reply::Count { tokens } => assert_eq!(tokens, 2),
        other => panic!("{other:?}"),
    }
    let wrong = s.call(Request::Count {
        model_id: "cleanup-1".into(),
        text: "x".into(),
    });
    assert!(
        matches!(
            wrong,
            Reply::Error {
                code: ErrorCode::WrongModel,
                ..
            }
        ),
        "{wrong:?}"
    );
    let wrong = s.call(Request::Complete {
        model_id: "cleanup-1".into(),
        prompt: "x".into(),
        end: vec![],
        max_tokens: 2,
    });
    assert!(
        matches!(
            wrong,
            Reply::Error {
                code: ErrorCode::WrongModel,
                ..
            }
        ),
        "{wrong:?}"
    );
    // Unload gives NotLoaded afterwards.
    assert_eq!(s.call(Request::Unload), Reply::Unloaded);
    let none = s.call(Request::Count {
        model_id: "s1-mini".into(),
        text: "x".into(),
    });
    assert!(
        matches!(
            none,
            Reply::Error {
                code: ErrorCode::NotLoaded,
                ..
            }
        ),
        "{none:?}"
    );
    assert!(s.finish().success());
}

#[test]
#[ignore = "needs SAYSO_CLEANUP_S1"]
fn two_completions_in_a_row_give_the_same_text() {
    let mut s = Session::start();
    load(&mut s, "s1-mini", model_path("SAYSO_CLEANUP_S1"));
    let first = complete(&mut s, "s1-mini", S1_PROMPT, &["<|im_end|>"], 64);
    let second = complete(&mut s, "s1-mini", S1_PROMPT, &["<|im_end|>"], 64);
    assert_eq!(first.text, second.text);
    assert_eq!(
        (first.tokens_in, first.tokens_out),
        (second.tokens_in, second.tokens_out)
    );
    assert!(s.finish().success());
}

#[test]
#[ignore = "needs SAYSO_CLEANUP_S1 and SAYSO_CLEANUP_SPOKENLY"]
fn load_is_a_no_op_for_the_same_model_and_swaps_for_another() {
    let mut s = Session::start();
    let s1 = model_path("SAYSO_CLEANUP_S1");
    load(&mut s, "s1-mini", s1.clone());
    match s.call(Request::Load {
        model_id: "s1-mini".into(),
        path: s1,
        n_ctx: 2048,
    }) {
        Reply::Loaded { load_ms, .. } => assert_eq!(load_ms, 0),
        other => panic!("{other:?}"),
    }
    load(&mut s, "cleanup-1", model_path("SAYSO_CLEANUP_SPOKENLY"));
    let wrong = s.call(Request::Count {
        model_id: "s1-mini".into(),
        text: "x".into(),
    });
    assert!(
        matches!(
            wrong,
            Reply::Error {
                code: ErrorCode::WrongModel,
                ..
            }
        ),
        "{wrong:?}"
    );
    let done = complete(&mut s, "cleanup-1", SPOKENLY_PROMPT, &[], 64);
    assert_eq!(done.text.trim(), "The meeting is on Wednesday at 3:30.");
    assert!(s.finish().success());
}
