//! Codex CLI provider: `codex exec` with an output schema.
//!
//! Slow (about 5 s) and heavy (about 21k tokens of agent overhead per call),
//! so the Styles page says so. The instructions and the transcript go in
//! through stdin (`-`), which keeps the text out of the process list.

use crate::cli::{self, Invocation, tail};
use crate::detect::detect_codex_cli;
use sayso_core::enhance::{EnhanceError, EnhanceRequest, EnhanceResponse, Enhancer, output_schema, parse_output, user_message};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct CodexCli {
    id: String,
    /// None when no binary was found. `enhance` then fails with `NotConfigured`.
    program: Option<PathBuf>,
    model: Option<String>,
    default_timeout: Duration,
}

impl CodexCli {
    /// `path` is the binary from the provider config. None searches for it.
    /// `model` None lets Codex pick its default.
    pub fn new(id: impl Into<String>, path: Option<PathBuf>, model: Option<String>) -> Self {
        let program = path.or_else(detect_codex_cli);
        Self { id: id.into(), program, model, default_timeout: Duration::from_secs(12) }
    }

    /// The timeout used when a request has a zero timeout.
    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }
}

/// Codex has no separate system prompt, so everything goes in one prompt.
fn build_prompt(request: &EnhanceRequest) -> String {
    format!(
        "{}\n\nDo not run commands or read files. Process only the transcript between the tags \
         and reply with a JSON object that has one field, \"text\", holding the final text.\n\n{}\n",
        request.system_prompt,
        user_message(&request.transcript)
    )
}

impl Enhancer for CodexCli {
    fn id(&self) -> &str {
        &self.id
    }

    /// The CLI sends text to OpenAI.
    fn is_cloud(&self) -> bool {
        true
    }

    fn enhance(&self, request: &EnhanceRequest) -> Result<EnhanceResponse, EnhanceError> {
        let program = self
            .program
            .as_deref()
            .ok_or_else(|| EnhanceError::NotConfigured("the codex command was not found".into()))?;
        let timeout = if request.timeout.is_zero() { self.default_timeout } else { request.timeout };
        let model = request.model.clone().or_else(|| self.model.clone());
        let workdir = cli::workdir()?;
        let schema_file = workdir.path().join("schema.json");
        let output_file = workdir.path().join("answer.json");
        std::fs::write(&schema_file, output_schema().to_string())
            .map_err(|e| EnhanceError::Cli(format!("could not write the schema file: {e}")))?;

        let mut args: Vec<std::ffi::OsString> = [
            "exec",
            "--skip-git-repo-check",
            "--ephemeral",
            "--sandbox",
            "read-only",
            "--ignore-user-config",
            "--ignore-rules",
            "--output-schema",
        ]
        .iter()
        .map(Into::into)
        .collect();
        args.push(schema_file.into());
        args.push("-o".into());
        args.push(output_file.clone().into());
        if let Some(m) = &model {
            args.push("-m".into());
            args.push(m.into());
        }
        args.push("-".into()); // read the prompt from stdin

        let started = Instant::now();
        let output = cli::run(&Invocation {
            program,
            args,
            envs: Vec::new(),
            cwd: workdir.path(),
            stdin: Some(build_prompt(request)),
            timeout,
        })?;
        if !output.status.success() {
            return Err(EnhanceError::Cli(format!(
                "codex exited with {}: {}",
                output.status,
                tail(&output.stderr)
            )));
        }
        let answer = std::fs::read_to_string(&output_file)
            .map_err(|_| EnhanceError::Cli(format!("codex wrote no answer: {}", tail(&output.stderr))))?;
        let text = parse_output(&answer)?;
        Ok(EnhanceResponse {
            text,
            provider_id: self.id.clone(),
            model: model.unwrap_or_else(|| "codex default".into()),
            elapsed_ms: started.elapsed().as_millis() as u64,
            mode: "codex_cli".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_contains_instructions_and_transcript() {
        let req = EnhanceRequest {
            system_prompt: "Fix grammar.".into(),
            transcript: "hello wrold".into(),
            model: None,
            temperature: None,
            timeout: Duration::from_secs(1),
        };
        let prompt = build_prompt(&req);
        assert!(prompt.starts_with("Fix grammar."));
        assert!(prompt.contains("<transcript>\nhello wrold\n</transcript>"));
    }
}
