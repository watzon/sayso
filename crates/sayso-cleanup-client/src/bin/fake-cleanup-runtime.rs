//! A stand-in for the cleanup runtime in tests. It answers with the text of the
//! prompt in capitals. A prompt with `SLEEP` waits 10 s, `CRASH` ends the
//! process, `ILLEGAL` runs an illegal instruction (like a build for another
//! processor), and `LIMIT` stops at the token limit. A load of a path with
//! `missing` fails.

use sayso_cleanup_models::protocol::{ErrorCode, Reply, ReplyLine, Request, RequestLine, Stop};
use std::io::{BufRead, Write};

fn text_of(prompt: &str) -> String {
    if let Some(rest) = prompt.split("text: ").nth(1) {
        return rest.trim_end_matches("\nout:").to_string();
    }
    let body = prompt.split("]\n").nth(1).unwrap_or(prompt);
    body.split("<|im_end|>").next().unwrap_or(body).to_string()
}

fn main() {
    let mut loaded: Option<String> = None;
    let stdout = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(RequestLine { id, request, .. }) = serde_json::from_str::<RequestLine>(&line) else {
            continue;
        };
        let error = |code, message: &str| Reply::Error {
            code,
            message: message.into(),
        };
        let reply = match request {
            Request::Hello => Reply::Hello {
                version: 1,
                llama_cpp: "fake".into(),
                backend: "cpu".into(),
            },
            // Like the real runtime: the old model is unloaded before the load.
            Request::Load { path, .. } if path.contains("missing") => {
                loaded = None;
                error(ErrorCode::LoadFailed, "no such file")
            }
            Request::Load { model_id, .. } => {
                loaded = Some(model_id.clone());
                Reply::Loaded {
                    model_id,
                    load_ms: 1,
                }
            }
            Request::Count { model_id, .. } | Request::Complete { model_id, .. }
                if loaded.as_deref() != Some(model_id.as_str()) =>
            {
                error(
                    if loaded.is_none() {
                        ErrorCode::NotLoaded
                    } else {
                        ErrorCode::WrongModel
                    },
                    "load it first",
                )
            }
            Request::Count { text, .. } => Reply::Count {
                tokens: text.len().div_ceil(4) as u32,
            },
            Request::Complete { prompt, .. } => {
                if prompt.contains("CRASH") {
                    std::process::exit(3);
                }
                if prompt.contains("ILLEGAL") {
                    illegal_instruction();
                }
                if prompt.contains("SLEEP") {
                    std::thread::sleep(std::time::Duration::from_secs(10));
                }
                let stop = if prompt.contains("LIMIT") {
                    Stop::Limit
                } else {
                    Stop::End
                };
                Reply::Completion {
                    text: format!(" {}", text_of(&prompt).to_uppercase()),
                    stop,
                    tokens_in: 10,
                    tokens_out: 5,
                    ms: 1,
                }
            }
            Request::Unload => {
                loaded = None;
                Reply::Unloaded
            }
        };
        let mut out = stdout.lock();
        let _ = writeln!(
            out,
            "{}",
            serde_json::to_string(&ReplyLine::new(id, reply)).unwrap()
        );
        let _ = out.flush();
    }
}

fn illegal_instruction() {
    // SAFETY: the instruction stops the process; nothing runs after it.
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    unsafe {
        std::arch::asm!("ud2")
    };
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!("udf #0")
    };
    std::process::exit(4);
}
