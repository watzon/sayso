//! The text part of the pipeline: replacements, then the style (plan §3).
//!
//! The rule is that text is never lost. Any AI failure returns the text after
//! replacements, with the reason.

use crate::dictionary::{AppliedReplacement, Replacer};
use crate::enhance::{EnhanceError, EnhanceRequest, Enhancer};
use crate::history::EnhanceOutcome;
use crate::style::{Style, system_prompt};
use std::time::Duration;

pub struct TextPipeline<'a> {
    pub replacer: &'a Replacer,
    pub style: &'a Style,
    pub vocabulary: &'a [String],
    /// None when AI is off or no provider is set up.
    pub enhancer: Option<&'a dyn Enhancer>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PipelineOutput {
    pub text: String,
    pub replacements: Vec<AppliedReplacement>,
    pub enhance: EnhanceOutcome,
    /// Set when the style failed. Short form for the overlay.
    pub enhance_error: Option<String>,
}

impl TextPipeline<'_> {
    /// True when [`run_style`](Self::run_style) will call a provider.
    pub fn will_enhance(&self) -> bool {
        self.style.uses_ai() && self.enhancer.is_some()
    }

    pub fn replace(&self, transcript: &str) -> (String, Vec<AppliedReplacement>) {
        self.replacer.apply(transcript.trim())
    }

    /// Run the style on text that already has replacements applied.
    pub fn run_style(&self, replaced: String, replacements: Vec<AppliedReplacement>) -> PipelineOutput {
        let Some(enhancer) = self.enhancer.filter(|_| self.style.uses_ai()) else {
            return PipelineOutput { text: replaced, replacements, enhance: EnhanceOutcome::NotUsed, enhance_error: None };
        };
        let request = EnhanceRequest {
            system_prompt: system_prompt(self.style, self.vocabulary, &replaced),
            transcript: replaced.clone(),
            model: self.style.model.clone(),
            temperature: self.style.temperature,
            timeout: self.style.timeout_ms.map(Duration::from_millis).unwrap_or(self.timeout),
        };
        // One retry when the reply has the wrong shape. Timeouts are not retried.
        let mut result = enhancer.enhance(&request);
        if matches!(result, Err(EnhanceError::InvalidOutput(_))) {
            result = enhancer.enhance(&request);
        }
        match result {
            Ok(resp) => PipelineOutput {
                text: resp.text.trim().to_string(),
                replacements,
                enhance: EnhanceOutcome::Applied {
                    provider_id: resp.provider_id,
                    model: resp.model,
                    elapsed_ms: resp.elapsed_ms,
                },
                enhance_error: None,
            },
            Err(err) => {
                log::warn!("style {} failed with {}: {err}", self.style.id, enhancer.id());
                PipelineOutput {
                    text: replaced,
                    replacements,
                    enhance: EnhanceOutcome::Failed { provider_id: enhancer.id().to_string(), reason: err.to_string() },
                    enhance_error: Some(format!("{} {}", enhancer.id(), err.short())),
                }
            }
        }
    }

    pub fn run(&self, transcript: &str) -> PipelineOutput {
        let (replaced, applied) = self.replace(transcript);
        self.run_style(replaced, applied)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::Replacement;
    use crate::enhance::EnhanceResponse;
    use crate::style::builtin_styles;
    use std::sync::Mutex;

    struct Fake {
        replies: Mutex<Vec<Result<EnhanceResponse, EnhanceError>>>,
        seen: Mutex<Vec<EnhanceRequest>>,
    }

    impl Fake {
        fn new(replies: Vec<Result<EnhanceResponse, EnhanceError>>) -> Self {
            Self { replies: Mutex::new(replies), seen: Mutex::new(vec![]) }
        }
    }

    impl Enhancer for Fake {
        fn id(&self) -> &str {
            "fake"
        }
        fn is_cloud(&self) -> bool {
            false
        }
        fn enhance(&self, request: &EnhanceRequest) -> Result<EnhanceResponse, EnhanceError> {
            self.seen.lock().unwrap().push(request.clone());
            self.replies.lock().unwrap().remove(0)
        }
    }

    fn ok(text: &str) -> Result<EnhanceResponse, EnhanceError> {
        Ok(EnhanceResponse { text: text.into(), provider_id: "fake".into(), model: "m".into(), elapsed_ms: 5, mode: "json_schema".into() })
    }

    fn style(id: &str) -> Style {
        builtin_styles().into_iter().find(|s| s.id == id).unwrap()
    }

    fn rules() -> Vec<Replacement> {
        vec![Replacement { id: 1, from: "git hub".into(), to: "GitHub".into(), case_sensitive: false, uses: 0 }]
    }

    #[test]
    fn replacements_run_before_the_style() {
        let fake = Fake::new(vec![ok("Open a GitHub issue.")]);
        let replacer = Replacer::new(&rules());
        let clean = style("clean");
        let p = TextPipeline { replacer: &replacer, style: &clean, vocabulary: &[], enhancer: Some(&fake), timeout: Duration::from_secs(4) };
        let out = p.run("open a git hub issue");
        assert_eq!(out.text, "Open a GitHub issue.");
        assert_eq!(fake.seen.lock().unwrap()[0].transcript, "open a GitHub issue");
        assert_eq!(out.replacements.len(), 1);
    }

    #[test]
    fn raw_style_never_calls_the_provider() {
        let fake = Fake::new(vec![]);
        let replacer = Replacer::new(&[]);
        let raw = style("raw");
        let p = TextPipeline { replacer: &replacer, style: &raw, vocabulary: &[], enhancer: Some(&fake), timeout: Duration::from_secs(4) };
        assert!(!p.will_enhance());
        assert_eq!(p.run("hi there").enhance, EnhanceOutcome::NotUsed);
    }

    #[test]
    fn failure_keeps_the_replaced_text() {
        let fake = Fake::new(vec![Err(EnhanceError::Timeout(4000))]);
        let replacer = Replacer::new(&rules());
        let clean = style("clean");
        let p = TextPipeline { replacer: &replacer, style: &clean, vocabulary: &[], enhancer: Some(&fake), timeout: Duration::from_secs(4) };
        let out = p.run("on git hub");
        assert_eq!(out.text, "on GitHub");
        assert_eq!(out.enhance_error.as_deref(), Some("fake timed out after 4 s"));
        assert!(matches!(out.enhance, EnhanceOutcome::Failed { .. }));
    }

    #[test]
    fn invalid_output_is_retried_once() {
        let fake = Fake::new(vec![Err(EnhanceError::InvalidOutput("x".into())), ok("Fixed.")]);
        let replacer = Replacer::new(&[]);
        let clean = style("clean");
        let p = TextPipeline { replacer: &replacer, style: &clean, vocabulary: &[], enhancer: Some(&fake), timeout: Duration::from_secs(4) };
        assert_eq!(p.run("fixed").text, "Fixed.");
        assert_eq!(fake.seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn style_overrides_reach_the_request() {
        let fake = Fake::new(vec![ok("x")]);
        let replacer = Replacer::new(&[]);
        let mut s = style("email");
        s.model = Some("big".into());
        s.timeout_ms = Some(9000);
        let p = TextPipeline { replacer: &replacer, style: &s, vocabulary: &["Dana".into()], enhancer: Some(&fake), timeout: Duration::from_secs(4) };
        p.run("hi dana");
        let req = &fake.seen.lock().unwrap()[0];
        assert_eq!(req.model.as_deref(), Some("big"));
        assert_eq!(req.timeout, Duration::from_millis(9000));
        assert!(req.system_prompt.contains("Dana"));
    }
}
