//! The request loop: one JSON object per line in, one per line out.

use crate::runtime::Runtime;
use sayso_core::cleanup::protocol::{ErrorCode, Reply, ReplyLine, Request, RequestLine};
use std::io::{BufRead, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Read requests until the end of the input. Each request is handled, and its
/// reply is written and flushed, before the next line is read.
pub fn serve(
    runtime: &mut Runtime,
    mut input: impl BufRead,
    mut output: impl Write,
) -> std::io::Result<()> {
    let mut line = Vec::new();
    loop {
        line.clear();
        if input.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        let reply = match std::str::from_utf8(&line) {
            Ok(text) if text.trim().is_empty() => continue,
            Ok(text) => handle_line(runtime, text.trim()),
            Err(_) => error_line("", ErrorCode::BadRequest, "the line is not UTF-8"),
        };
        serde_json::to_writer(&mut output, &reply)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
}

/// Handle one line. This never panics: a panic while handling the request
/// becomes a `decode_failed` error and the runtime stays usable.
pub fn handle_line(runtime: &mut Runtime, line: &str) -> ReplyLine {
    let request: RequestLine = match serde_json::from_str(line) {
        Ok(request) => request,
        Err(e) => {
            return error_line(
                &id_of(line),
                ErrorCode::BadRequest,
                &format!("bad request: {e}"),
            );
        }
    };
    let id = request.id.clone();
    // A newer client can still ask for the version with `hello`.
    if request.v != sayso_core::cleanup::protocol::PROTOCOL_VERSION
        && request.request != Request::Hello
    {
        return error_line(
            &id,
            ErrorCode::BadRequest,
            &format!("unsupported protocol version {}", request.v),
        );
    }
    guarded(&id, || runtime.handle(request.request))
}

/// Run `work`. A panic becomes a `decode_failed` error.
fn guarded(id: &str, work: impl FnOnce() -> Reply) -> ReplyLine {
    match catch_unwind(AssertUnwindSafe(work)) {
        Ok(reply) => ReplyLine::new(id, reply),
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            log::error!("panic while handling a request: {message}");
            error_line(id, ErrorCode::DecodeFailed, &format!("panic: {message}"))
        }
    }
}

/// The `id` of a line that is not a valid request, if it has one.
fn id_of(line: &str) -> String {
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|v| v.get("id")?.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn error_line(id: &str, code: ErrorCode, message: &str) -> ReplyLine {
    ReplyLine::new(
        id,
        Reply::Error {
            code,
            message: message.into(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_lines_give_bad_request_with_the_id_when_readable() {
        let mut runtime = Runtime::new(false);
        for (line, id) in [
            ("nope", ""),
            (r#"{"v":1,"id":"7","type":"fly"}"#, "7"),
            ("{}", ""),
            ("[1]", ""),
        ] {
            let reply = handle_line(&mut runtime, line);
            assert_eq!(reply.id, id, "{line}");
            assert!(
                matches!(
                    reply.reply,
                    Reply::Error {
                        code: ErrorCode::BadRequest,
                        ..
                    }
                ),
                "{line}"
            );
        }
    }

    #[test]
    fn a_panic_becomes_decode_failed() {
        let reply = guarded("9", || panic!("boom"));
        assert_eq!(reply.id, "9");
        match reply.reply {
            Reply::Error {
                code: ErrorCode::DecodeFailed,
                message,
            } => assert!(message.contains("boom"), "{message}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_foreign_version_is_refused_except_for_hello() {
        let mut runtime = Runtime::new(false);
        let reply = handle_line(&mut runtime, r#"{"v":2,"id":"1","type":"unload"}"#);
        assert!(matches!(
            reply.reply,
            Reply::Error {
                code: ErrorCode::BadRequest,
                ..
            }
        ));
        let reply = handle_line(&mut runtime, r#"{"v":2,"id":"2","type":"hello"}"#);
        assert!(matches!(reply.reply, Reply::Hello { version: 1, .. }));
    }
}
