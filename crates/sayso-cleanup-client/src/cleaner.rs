//! The [`Cleaner`] that runs a cleanup model in the cleanup runtime.

use crate::download;
use crate::runtime::{LOAD_LIMIT, Runtime, RuntimeError};
use sayso_cleanup_models::{
    self as cleanup, CheckFailure, CleanError, Cleaner, CleanupRequest, CleanupResponse,
    MAX_PART_TOKENS, N_CTX,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The time that a cleanup gets for each token of the text, on top of the
/// style timeout. On a CPU the models write about 120 tokens a second (the
/// model card of Cleanup 1), and the answer is about as long as the text.
pub const TIME_PER_TOKEN: Duration = Duration::from_millis(15);

pub struct RuntimeCleaner {
    runtime: Arc<Runtime>,
    models_dir: PathBuf,
}

impl RuntimeCleaner {
    pub fn new(runtime: Arc<Runtime>, models_dir: PathBuf) -> Self {
        RuntimeCleaner {
            runtime,
            models_dir,
        }
    }

    /// Load `model` before the text is ready, while the microphone records.
    /// This is a hint: errors are logged, and the cleanup loads the model again
    /// if this did not work. Runs on its own thread.
    pub fn preload(&self, model: &str) {
        let Some(info) = cleanup::find(model) else {
            return;
        };
        let Some(path) = download::model_path(&self.models_dir, info) else {
            return;
        };
        let runtime = self.runtime.clone();
        let id = info.id;
        let _ = std::thread::Builder::new()
            .name("cleanup-preload".into())
            .spawn(move || {
                // The runtime refuses the load when the model is being deleted or
                // AI is off: it checks that under its lock.
                if let Err(e) = runtime.ensure_loaded(id, &path, N_CTX, None) {
                    log::warn!("could not load the cleanup model {id} ahead: {e}");
                }
            });
    }
}

impl Cleaner for RuntimeCleaner {
    fn clean(&self, request: &CleanupRequest) -> Result<CleanupResponse, CleanError> {
        let started = Instant::now();
        let info = cleanup::find(&request.model).ok_or_else(|| {
            CleanError::NotConfigured(format!("no cleanup model {}", request.model))
        })?;
        let path = download::model_path(&self.models_dir, info)
            .ok_or_else(|| CleanError::NotConfigured(format!("{} is not downloaded", info.name)))?;
        // A load that runs now (the hint at the start of the dictation) may end
        // first, within the load limit. The deadline of the text starts after the load.
        let (answer, load_ms) = self
            .runtime
            .with_model(
                info.id,
                &path,
                N_CTX,
                Some(Instant::now() + LOAD_LIMIT),
                |session| {
                    // One budget for the whole cleanup, from here: the split, the
                    // counts, and the answers. A small model takes time for each
                    // token, so a long text gets more time than the style timeout.
                    let started = Instant::now();
                    // The split and the counts get the style timeout; the answers
                    // get the rest of the budget.
                    let plan_until =
                        (!request.timeout.is_zero()).then(|| started + request.timeout);
                    let parts = cleanup::plan_parts(&request.text, MAX_PART_TOKENS, &mut |text| {
                        session.count_until(text, plan_until)
                    })?;
                    let counts = parts
                        .iter()
                        .map(|part| session.count_until(part, plan_until))
                        .collect::<Result<Vec<u32>, _>>()?;
                    let total: u32 = counts.iter().sum();
                    let deadline = (!request.timeout.is_zero())
                        .then(|| started + request.timeout + TIME_PER_TOKEN * total);
                    let (mut answers, mut tokens_in, mut tokens_out) = (Vec::new(), 0, 0);
                    for (part, &tokens) in parts.iter().zip(&counts) {
                        let prompt = cleanup::render(
                            info,
                            part,
                            request.settings,
                            request.app.as_deref(),
                            request.site.as_deref(),
                            tokens,
                        );
                        let completion = session.complete(
                            &prompt.text,
                            &prompt.end,
                            prompt.max_tokens,
                            deadline,
                        )?;
                        tokens_in += completion.tokens_in;
                        tokens_out += completion.tokens_out;
                        // A part that fails fails the whole text: the text after
                        // replacements goes in, not a mix of cleaned and not cleaned parts.
                        match cleanup::check(part, completion.stop, &completion.text) {
                            Ok(answer) => answers.push(answer),
                            Err(failure) => return Ok(Err(failure)),
                        }
                    }
                    Ok(Ok((
                        cleanup::join_answers(&parts, &answers),
                        parts.len() as u32,
                        tokens_in,
                        tokens_out,
                    )))
                },
            )
            .map_err(|e| to_clean_error(Failure::Runtime(e), request))?;
        let (text, parts, tokens_in, tokens_out) =
            answer.map_err(|e| to_clean_error(Failure::Check(e), request))?;
        Ok(CleanupResponse {
            text,
            model: info.id.to_string(),
            elapsed_ms: started.elapsed().as_millis() as u64,
            load_ms,
            parts,
            tokens_in,
            tokens_out,
        })
    }
}

enum Failure {
    Runtime(RuntimeError),
    Check(CheckFailure),
}

fn to_clean_error(failure: Failure, request: &CleanupRequest) -> CleanError {
    match failure {
        Failure::Runtime(RuntimeError::Timeout) => {
            CleanError::Timeout(request.timeout.as_millis() as u64)
        }
        Failure::Runtime(RuntimeError::NotInstalled(message)) => CleanError::NotConfigured(message),
        Failure::Runtime(e @ RuntimeError::Closed) => CleanError::NotConfigured(e.to_string()),
        Failure::Runtime(e) => CleanError::Runtime(e.to_string()),
        Failure::Check(e) => CleanError::InvalidOutput(e.to_string()),
    }
}
