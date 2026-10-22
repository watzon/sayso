//! sayso-cleanup: NDJSON requests on stdin, NDJSON replies on stdout, logs on
//! stderr. Exits when stdin closes.

use sayso_cleanup::{Runtime, serve, stdio};
use std::io::BufReader;

fn main() {
    // First, before llama.cpp can print: stdout is for protocol lines only.
    let output = stdio::protocol_writer();
    let log_level = std::env::var("SAYSO_CLEANUP_LOG").unwrap_or_else(|_| "info".into());
    env_logger::Builder::new()
        .parse_filters(&log_level)
        .target(env_logger::Target::Stderr)
        .init();
    // llama.cpp and ggml write a lot at load. Show it only for debug runs.
    let llama_logs = matches!(
        log_level.trim().to_ascii_lowercase().as_str(),
        "debug" | "trace"
    );

    log::info!(
        "sayso-cleanup {} ready, pid {}",
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    );
    let mut runtime = Runtime::new(llama_logs);
    let stdin = std::io::stdin();
    match serve(&mut runtime, BufReader::new(stdin.lock()), output) {
        Ok(()) => {}
        Err(e) => {
            log::error!("the protocol stream failed: {e}");
            std::process::exit(1);
        }
    }
}
