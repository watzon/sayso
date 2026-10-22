//! The text part of the pipeline: replacements, then the style (plan §3).
//!
//! The rule is that text is never lost. Any AI failure returns the text after
//! replacements, with the reason.

use crate::cleanup::{CleanupModelInfo, CleanupRequest, Cleaner};
use crate::dictionary::{AppliedReplacement, Replacer};
use crate::enhance::{EnhanceError, EnhanceRequest, Enhancer};
use crate::history::{CleanupTimings, EnhanceAttempt, EnhanceOutcome, EnhanceTimings};
use crate::style::{RunPlan, Style, system_prompt};
use std::time::{Duration, Instant};

pub struct TextPipeline<'a> {
    pub replacer: &'a Replacer,
    pub style: &'a Style,
    /// The base prompt of the style library.
    pub base_prompt: &'a str,
    pub vocabulary: &'a [String],
    /// None when AI is off or no provider is set up.
    pub enhancer: Option<&'a dyn Enhancer>,
    /// Set when the provider of the style is a cleanup model. Then `enhancer` is not used.
    pub cleaner: Option<CleanerRun<'a>>,
    pub timeout: Duration,
}

/// A cleanup model for the style step.
#[derive(Clone, Copy)]
pub struct CleanerRun<'a> {
    pub cleaner: &'a dyn Cleaner,
    /// The id of the provider, for the history.
    pub provider_id: &'a str,
    pub model: &'static CleanupModelInfo,
    /// From [`Style::run_plan`]: `Cleanup` or `CleanupOnly`.
    pub plan: RunPlan,
    /// The dictation language setting, and the languages of the final model.
    pub language_setting: &'a str,
    pub final_model_languages: &'a [&'a str],
    pub app: Option<&'a str>,
    pub site: Option<&'a str>,
}

/// Something the overlay says about a cleanup model that did less than the style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupNotice {
    /// The model could not run the style and did a cleanup only.
    CleanupOnly { model: &'static str },
    /// The model does not support the language, so it did not run.
    Language { model: &'static str, language: String },
    /// Sayso could not tell the language, so the model did not run.
    UnknownLanguage { model: &'static str },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PipelineOutput {
    pub text: String,
    pub replacements: Vec<AppliedReplacement>,
    pub enhance: EnhanceOutcome,
    /// Set when the style failed: the id of the provider and the error. The
    /// overlay shows it.
    pub enhance_error: Option<(String, EnhanceError)>,
    /// The attempts, when a provider ran. The caller knows the time of the
    /// whole step, the setup time, and the kind of the provider.
    pub timings: Option<EnhanceTimings>,
    pub notice: Option<CleanupNotice>,
}

impl TextPipeline<'_> {
    /// True when [`run_style`](Self::run_style) will call a provider.
    pub fn will_enhance(&self) -> bool {
        match &self.cleaner {
            Some(run) => matches!(run.plan, RunPlan::Cleanup(_) | RunPlan::CleanupOnly),
            None => self.style.uses_ai() && self.enhancer.is_some(),
        }
    }

    pub fn replace(&self, transcript: &str) -> (String, Vec<AppliedReplacement>) {
        self.replacer.apply(transcript.trim())
    }

    /// Run the style on text that already has replacements applied.
    pub fn run_style(&self, replaced: String, replacements: Vec<AppliedReplacement>) -> PipelineOutput {
        if let Some(run) = self.cleaner {
            return self.run_cleanup(run, replaced, replacements);
        }
        // A model answers an empty transcript with a remark about it.
        let Some(enhancer) = self.enhancer.filter(|_| self.style.uses_ai() && !replaced.trim().is_empty()) else {
            return PipelineOutput { text: replaced, replacements, enhance: EnhanceOutcome::NotUsed, enhance_error: None, timings: None, notice: None };
        };
        // The model gets spelled words as words. A failure returns the text without this step.
        let spoken = crate::spelled::apply(&replaced);
        let timeout = self.style.timeout_ms.map(Duration::from_millis).unwrap_or(self.timeout);
        let mut request = EnhanceRequest {
            system_prompt: system_prompt(self.style, self.base_prompt, self.vocabulary, &spoken),
            transcript: spoken,
            model: self.style.model.clone(),
            temperature: self.style.temperature,
            timeout,
        };
        // One time limit for all attempts. A zero timeout gives the default of
        // the provider, so then there is no deadline here.
        let deadline = (!timeout.is_zero()).then(|| Instant::now() + timeout);
        let mut attempts = Vec::new();
        let mut attempt = |request: &EnhanceRequest| {
            let started = Instant::now();
            let result = enhancer.enhance(request).and_then(|response| {
                if is_runaway(&replaced, &response.text) {
                    return Err(EnhanceError::InvalidOutput("the reply is much longer than the text".into()));
                }
                Ok(response)
            });
            attempts.push(EnhanceAttempt {
                elapsed_ms: started.elapsed().as_millis() as u64,
                error: result.as_ref().err().map(|e| e.to_string()),
                mode: result.as_ref().ok().map(|r| r.mode.clone()),
                report: result.as_ref().ok().and_then(|r| r.report.clone()),
            });
            result
        };
        // One retry when the reply has the wrong shape, with the time that is
        // left. Timeouts are not retried.
        let mut result = attempt(&request);
        let left = deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
        if matches!(result, Err(EnhanceError::InvalidOutput(_))) && left != Some(Duration::ZERO) {
            if let Some(left) = left {
                request.timeout = left;
            }
            result = attempt(&request);
            // The retry ran out of the time limit of the whole step.
            if deadline.is_some() && matches!(result, Err(EnhanceError::Timeout(_))) {
                result = Err(EnhanceError::Timeout(timeout.as_millis() as u64));
            }
        }
        // The time the user waited: a retry counts too.
        let elapsed_ms = attempts.iter().map(|a| a.elapsed_ms).sum();
        let timings = Some(EnhanceTimings {
            timeout_ms: timeout.as_millis() as u64,
            prompt_chars: request.system_prompt.chars().count() + request.transcript.chars().count(),
            attempts,
            ..Default::default()
        });
        match result {
            Ok(resp) => PipelineOutput {
                text: resp.text.trim().to_string(),
                replacements,
                enhance: EnhanceOutcome::Applied {
                    provider_id: resp.provider_id,
                    model: resp.model,
                    elapsed_ms,
                },
                enhance_error: None,
                timings,
                notice: None,
            },
            Err(err) => {
                log::warn!("style {} failed with {}: {err}", self.style.id, enhancer.id());
                PipelineOutput {
                    text: replaced,
                    replacements,
                    enhance: EnhanceOutcome::Failed { provider_id: enhancer.id().to_string(), reason: err.to_string() },
                    enhance_error: Some((enhancer.id().to_string(), err)),
                    timings,
                    notice: None,
                }
            }
        }
    }

    /// The style step with a cleanup model. See `docs/cleanup-models.md`, "Pipeline".
    fn run_cleanup(&self, run: CleanerRun<'_>, replaced: String, replacements: Vec<AppliedReplacement>) -> PipelineOutput {
        let not_used = |text: String, replacements, notice| PipelineOutput { text, replacements, enhance: EnhanceOutcome::NotUsed, enhance_error: None, timings: None, notice };
        let settings = match run.plan {
            RunPlan::Cleanup(settings) => settings,
            RunPlan::CleanupOnly => Default::default(),
            RunPlan::Prompt | RunPlan::Nothing => return not_used(replaced, replacements, None),
        };
        if replaced.trim().is_empty() {
            return not_used(replaced, replacements, None);
        }
        let model = run.model.id;
        let language = crate::cleanup::dictation_language(run.language_setting, run.final_model_languages, &replaced, crate::cleanup::detect_language);
        match &language {
            None => return not_used(replaced, replacements, Some(CleanupNotice::UnknownLanguage { model })),
            Some(code) if !run.model.supports_language(code) => {
                return not_used(replaced, replacements, Some(CleanupNotice::Language { model, language: code.clone() }));
            }
            Some(_) => {}
        }
        // The model gets spelled words as words, as a language model does.
        let spoken = crate::spelled::apply(&replaced);
        let timeout = self.style.timeout_ms.map(Duration::from_millis).unwrap_or(self.timeout);
        let request = CleanupRequest {
            model: model.to_string(),
            text: spoken.clone(),
            settings,
            app: run.app.map(str::to_string),
            site: run.site.map(str::to_string),
            timeout,
        };
        let started = Instant::now();
        let result = run.cleaner.clean(&request);
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let cleanup_only = run.plan == RunPlan::CleanupOnly;
        let mut timings = EnhanceTimings {
            timeout_ms: timeout.as_millis() as u64,
            prompt_chars: spoken.chars().count(),
            attempts: vec![EnhanceAttempt { elapsed_ms, error: result.as_ref().err().map(|e| e.to_string()), mode: Some("cleanup".into()), report: None }],
            cleanup: Some(CleanupTimings { language: language.clone(), cleanup_only, ..Default::default() }),
            ..Default::default()
        };
        match result {
            Ok(response) => {
                if let Some(c) = timings.cleanup.as_mut() {
                    (c.load_ms, c.parts, c.tokens_in, c.tokens_out) = (response.load_ms, response.parts, response.tokens_in, response.tokens_out);
                }
                // The models take no word list: put back the spelling of
                // dictionary words and of the replacements that ran.
                let terms: Vec<String> = self.vocabulary.iter().cloned().chain(replacements.iter().map(|r| r.to.clone())).collect();
                let text = crate::cleanup::restore_spelling(&spoken, &response.text, &terms);
                PipelineOutput {
                    text,
                    replacements,
                    enhance: EnhanceOutcome::Applied { provider_id: run.provider_id.to_string(), model: response.model, elapsed_ms },
                    enhance_error: None,
                    timings: Some(timings),
                    notice: cleanup_only.then_some(CleanupNotice::CleanupOnly { model }),
                }
            }
            Err(err) => {
                log::warn!("style {} failed with cleanup model {model}: {err}", self.style.id);
                PipelineOutput {
                    text: replaced,
                    replacements,
                    enhance: EnhanceOutcome::Failed { provider_id: run.provider_id.to_string(), reason: err.to_string() },
                    enhance_error: Some((run.provider_id.to_string(), err)),
                    timings: Some(timings),
                    notice: None,
                }
            }
        }
    }

    pub fn run(&self, transcript: &str) -> PipelineOutput {
        let (replaced, applied) = self.replace(transcript);
        self.run_style(replaced, applied)
    }
}

/// True when `reply` is too long to be an edit of `text`: the model answered
/// the text or did not stop. A greeting and a sign-off fit in the limit.
fn is_runaway(text: &str, reply: &str) -> bool {
    reply.chars().count() > text.chars().count() * 3 + 200
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
        Ok(EnhanceResponse { text: text.into(), provider_id: "fake".into(), model: "m".into(), elapsed_ms: 5, mode: "json_schema".into(), report: None })
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
        let p = TextPipeline { replacer: &replacer, style: &clean, base_prompt: "", vocabulary: &[], enhancer: Some(&fake), cleaner: None, timeout: Duration::from_secs(4) };
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
        let p = TextPipeline { replacer: &replacer, style: &raw, base_prompt: "", vocabulary: &[], enhancer: Some(&fake), cleaner: None, timeout: Duration::from_secs(4) };
        assert!(!p.will_enhance());
        assert_eq!(p.run("hi there").enhance, EnhanceOutcome::NotUsed);
    }

    #[test]
    fn failure_keeps_the_replaced_text() {
        let fake = Fake::new(vec![Err(EnhanceError::Timeout(4000))]);
        let replacer = Replacer::new(&rules());
        let clean = style("clean");
        let p = TextPipeline { replacer: &replacer, style: &clean, base_prompt: "", vocabulary: &[], enhancer: Some(&fake), cleaner: None, timeout: Duration::from_secs(4) };
        let out = p.run("on git hub");
        assert_eq!(out.text, "on GitHub");
        assert_eq!(out.enhance_error, Some(("fake".to_string(), EnhanceError::Timeout(4000))));
        assert!(matches!(out.enhance, EnhanceOutcome::Failed { .. }));
    }

    #[test]
    fn invalid_output_is_retried_once() {
        let fake = Fake::new(vec![Err(EnhanceError::InvalidOutput("x".into())), ok("Fixed.")]);
        let replacer = Replacer::new(&[]);
        let clean = style("clean");
        let p = TextPipeline { replacer: &replacer, style: &clean, base_prompt: "", vocabulary: &[], enhancer: Some(&fake), cleaner: None, timeout: Duration::from_secs(4) };
        let out = p.run("fixed");
        assert_eq!(out.text, "Fixed.");
        assert_eq!(fake.seen.lock().unwrap().len(), 2);
        // The diagnostics keep both attempts, with the reason for the retry.
        let timings = out.timings.unwrap();
        assert_eq!(timings.timeout_ms, 4000);
        assert_eq!(timings.attempts.len(), 2);
        assert!(timings.attempts[0].error.as_deref().is_some_and(|e| e.contains("schema")));
        assert_eq!((timings.attempts[1].error.as_deref(), timings.attempts[1].mode.as_deref()), (None, Some("json_schema")));
    }

    /// A provider that answers with the wrong shape after `delay`, and keeps
    /// to the timeout of the request like a real one.
    struct SlowInvalid {
        delay: Duration,
        timeouts: Mutex<Vec<Duration>>,
    }

    impl Enhancer for SlowInvalid {
        fn id(&self) -> &str {
            "slow"
        }
        fn is_cloud(&self) -> bool {
            false
        }
        fn enhance(&self, request: &EnhanceRequest) -> Result<EnhanceResponse, EnhanceError> {
            self.timeouts.lock().unwrap().push(request.timeout);
            if self.delay >= request.timeout {
                std::thread::sleep(request.timeout);
                return Err(EnhanceError::Timeout(request.timeout.as_millis() as u64));
            }
            std::thread::sleep(self.delay);
            Err(EnhanceError::InvalidOutput("x".into()))
        }
    }

    #[test]
    fn the_retry_gets_only_the_time_that_is_left() {
        let slow = SlowInvalid { delay: Duration::from_millis(300), timeouts: Mutex::new(vec![]) };
        let replacer = Replacer::new(&[]);
        let clean = style("clean");
        let timeout = Duration::from_millis(400);
        let p = TextPipeline { replacer: &replacer, style: &clean, base_prompt: "", vocabulary: &[], enhancer: Some(&slow), cleaner: None, timeout };
        let started = Instant::now();
        let out = p.run("keep this text");
        // Two full attempts would take 600 ms. A slow CI runner adds more than
        // 100 ms to one attempt (517 ms on a macOS runner).
        assert!(started.elapsed() < timeout + Duration::from_millis(180), "took {:?}", started.elapsed());
        assert_eq!(out.text, "keep this text");
        assert_eq!(out.enhance_error, Some(("slow".to_string(), EnhanceError::Timeout(400))), "the limit of the whole step");
        let timeouts = slow.timeouts.lock().unwrap();
        assert_eq!(timeouts.len(), 2);
        assert_eq!(timeouts[0], timeout);
        assert!(timeouts[1] <= Duration::from_millis(100), "the retry got {:?}", timeouts[1]);
        assert_eq!(out.timings.unwrap().timeout_ms, 400);
    }

    #[test]
    fn no_retry_when_the_time_is_gone() {
        let fake = Fake::new(vec![Err(EnhanceError::InvalidOutput("x".into())), ok("Too late.")]);
        let replacer = Replacer::new(&[]);
        let clean = style("clean");
        let p = TextPipeline { replacer: &replacer, style: &clean, base_prompt: "", vocabulary: &[], enhancer: Some(&fake), cleaner: None, timeout: Duration::from_nanos(1) };
        let out = p.run("keep this text");
        assert_eq!(out.text, "keep this text");
        assert_eq!(fake.seen.lock().unwrap().len(), 1);
        assert!(matches!(out.enhance_error, Some((_, EnhanceError::InvalidOutput(_)))));
    }

    #[test]
    fn style_overrides_reach_the_request() {
        let fake = Fake::new(vec![ok("x")]);
        let replacer = Replacer::new(&[]);
        let mut s = style("email");
        s.model = Some("big".into());
        s.timeout_ms = Some(9000);
        let p = TextPipeline { replacer: &replacer, style: &s, base_prompt: "- Shared rule.", vocabulary: &["Dana".into()], enhancer: Some(&fake), cleaner: None, timeout: Duration::from_secs(4) };
        p.run("hi dana");
        let req = &fake.seen.lock().unwrap()[0];
        assert_eq!(req.model.as_deref(), Some("big"));
        assert_eq!(req.timeout, Duration::from_millis(9000));
        assert!(req.system_prompt.contains("Dana"));
        assert!(req.system_prompt.contains("- Shared rule."));
    }

    #[test]
    fn the_model_gets_spelled_words_as_words() {
        let fake = Fake::new(vec![ok("Send it to Dana Katz."), Err(EnhanceError::Timeout(4000))]);
        let replacer = Replacer::new(&[]);
        let clean = style("clean");
        let p = TextPipeline { replacer: &replacer, style: &clean, base_prompt: "", vocabulary: &[], enhancer: Some(&fake), cleaner: None, timeout: Duration::from_secs(4) };
        assert_eq!(p.run("send it to dana cats that's k a t z").text, "Send it to Dana Katz.");
        assert_eq!(fake.seen.lock().unwrap()[0].transcript, "send it to dana katz");
        // A failure returns what the speaker said.
        assert_eq!(p.run("send it to dana cats that's k a t z").text, "send it to dana cats that's k a t z");
    }

    #[test]
    fn an_empty_text_never_calls_the_provider() {
        let fake = Fake::new(vec![]);
        let replacer = Replacer::new(&[]);
        let clean = style("clean");
        let p = TextPipeline { replacer: &replacer, style: &clean, base_prompt: "", vocabulary: &[], enhancer: Some(&fake), cleaner: None, timeout: Duration::from_secs(4) };
        assert_eq!(p.run("  ").enhance, EnhanceOutcome::NotUsed);
    }

    #[test]
    fn a_reply_much_longer_than_the_text_keeps_the_replaced_text() {
        let answer = "Tokyo is nine hours ahead of UTC. ".repeat(10);
        let fake = Fake::new(vec![ok(&answer), ok(&answer)]);
        let replacer = Replacer::new(&[]);
        let clean = style("clean");
        let p = TextPipeline { replacer: &replacer, style: &clean, base_prompt: "", vocabulary: &[], enhancer: Some(&fake), cleaner: None, timeout: Duration::from_secs(4) };
        let out = p.run("what time is it in tokyo");
        assert_eq!(out.text, "what time is it in tokyo");
        assert!(matches!(out.enhance, EnhanceOutcome::Failed { .. }));
        assert_eq!(fake.seen.lock().unwrap().len(), 2, "one retry");
        // An email adds a greeting and a sign-off to a short text.
        assert!(!is_runaway("send dana the report", "Hi Dana,\n\nPlease send me the report.\n\nBest regards,"));
    }

    struct FakeCleaner {
        reply: Result<String, EnhanceError>,
        seen: Mutex<Vec<crate::cleanup::CleanupRequest>>,
    }

    impl crate::cleanup::Cleaner for FakeCleaner {
        fn clean(&self, request: &crate::cleanup::CleanupRequest) -> Result<crate::cleanup::CleanupResponse, EnhanceError> {
            self.seen.lock().unwrap().push(request.clone());
            self.reply.clone().map(|text| crate::cleanup::CleanupResponse { text, model: request.model.clone(), parts: 1, ..Default::default() })
        }
    }

    fn fake_cleaner(reply: Result<&str, EnhanceError>) -> FakeCleaner {
        FakeCleaner { reply: reply.map(str::to_string), seen: Mutex::new(vec![]) }
    }

    fn run_on<'a>(cleaner: &'a FakeCleaner, model: &str, style: &'a Style, setting: &'a str, languages: &'a [&'a str]) -> CleanerRun<'a> {
        let provider = crate::config::Provider { id: "s1".into(), name: "S1".into(), kind: crate::config::ProviderKind::CleanupModel { model: model.into() } };
        CleanerRun {
            cleaner,
            provider_id: "s1",
            model: crate::cleanup::find(model).unwrap(),
            plan: style.run_plan(&provider),
            language_setting: setting,
            final_model_languages: languages,
            app: Some("Slack"),
            site: None,
        }
    }

    #[test]
    fn a_cleanup_model_runs_with_the_settings_of_the_style() {
        let cleaner = fake_cleaner(Ok("Hi Dana,\n\nThe report.\n\nThanks,"));
        let replacer = Replacer::new(&[]);
        let email = style("email");
        let run = run_on(&cleaner, "s1-mini", &email, "en", &["en"]);
        let p = TextPipeline { replacer: &replacer, style: &email, base_prompt: "", vocabulary: &[], enhancer: None, cleaner: Some(run), timeout: Duration::from_secs(4) };
        assert!(p.will_enhance());
        let out = p.run("hi dana the report thanks");
        assert_eq!(out.text, "Hi Dana,\n\nThe report.\n\nThanks,");
        assert!(matches!(out.enhance, EnhanceOutcome::Applied { ref model, .. } if model == "s1-mini"));
        assert_eq!(out.notice, None);
        let seen = cleaner.seen.lock().unwrap();
        assert_eq!(seen[0].settings.format, crate::cleanup::Format::Email);
        assert_eq!(seen[0].app.as_deref(), Some("Slack"));
        let cleanup = out.timings.unwrap().cleanup.unwrap();
        assert_eq!((cleanup.language.as_deref(), cleanup.cleanup_only), (Some("en"), false));
    }

    #[test]
    fn a_style_that_the_model_cannot_run_gets_a_cleanup_only_with_a_notice() {
        let cleaner = fake_cleaner(Ok("Is the server up?"));
        let replacer = Replacer::new(&[]);
        let rambling = style("rambling");
        let run = run_on(&cleaner, "spokenly-cleanup-1", &rambling, "en", &["en"]);
        let p = TextPipeline { replacer: &replacer, style: &rambling, base_prompt: "", vocabulary: &[], enhancer: None, cleaner: Some(run), timeout: Duration::from_secs(4) };
        let out = p.run("is the server up");
        assert_eq!(out.text, "Is the server up?");
        assert_eq!(out.notice, Some(CleanupNotice::CleanupOnly { model: "spokenly-cleanup-1" }));
        assert_eq!(cleaner.seen.lock().unwrap()[0].settings, crate::cleanup::Settings::default());
        assert!(out.timings.unwrap().cleanup.unwrap().cleanup_only);
    }

    #[test]
    fn an_unsupported_language_does_not_run_the_model() {
        let cleaner = fake_cleaner(Ok("never"));
        let replacer = Replacer::new(&[]);
        let clean = style("clean");
        let run = run_on(&cleaner, "s1-mini", &clean, "de", &["en", "de"]);
        let p = TextPipeline { replacer: &replacer, style: &clean, base_prompt: "", vocabulary: &[], enhancer: None, cleaner: Some(run), timeout: Duration::from_secs(4) };
        let out = p.run("äh das meeting ist am mittwoch");
        assert_eq!(out.text, "äh das meeting ist am mittwoch");
        assert_eq!(out.enhance, EnhanceOutcome::NotUsed);
        assert_eq!(out.notice, Some(CleanupNotice::Language { model: "s1-mini", language: "de".into() }));
        assert!(cleaner.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn an_unknown_language_does_not_run_the_model() {
        let cleaner = fake_cleaner(Ok("never"));
        let replacer = Replacer::new(&[]);
        let clean = style("clean");
        // "auto", a final model with many languages, and a text too short to detect.
        let run = run_on(&cleaner, "spokenly-cleanup-1", &clean, "auto", &["en", "de", "fr"]);
        let p = TextPipeline { replacer: &replacer, style: &clean, base_prompt: "", vocabulary: &[], enhancer: None, cleaner: Some(run), timeout: Duration::from_secs(4) };
        let out = p.run("ok");
        assert_eq!(out.notice, Some(CleanupNotice::UnknownLanguage { model: "spokenly-cleanup-1" }));
        assert!(cleaner.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn a_cleanup_failure_keeps_the_replaced_text() {
        let cleaner = fake_cleaner(Err(EnhanceError::InvalidOutput("the answer stopped early".into())));
        let replacer = Replacer::new(&rules());
        let clean = style("clean");
        let run = run_on(&cleaner, "s1-mini", &clean, "en", &["en"]);
        let p = TextPipeline { replacer: &replacer, style: &clean, base_prompt: "", vocabulary: &[], enhancer: None, cleaner: Some(run), timeout: Duration::from_secs(4) };
        let out = p.run("um on git hub");
        assert_eq!(out.text, "um on GitHub");
        assert!(matches!(out.enhance, EnhanceOutcome::Failed { .. }));
        assert!(out.enhance_error.is_some());
    }

    #[test]
    fn the_spelling_of_replacements_and_words_comes_back_after_the_model() {
        let cleaner = fake_cleaner(Ok("Open a Github issue about gpui."));
        let replacer = Replacer::new(&rules());
        let clean = style("clean");
        let run = run_on(&cleaner, "s1-mini", &clean, "en", &["en"]);
        let vocabulary = ["GPUI".to_string()];
        let p = TextPipeline { replacer: &replacer, style: &clean, base_prompt: "", vocabulary: &vocabulary, enhancer: None, cleaner: Some(run), timeout: Duration::from_secs(4) };
        let out = p.run("open a git hub issue about GPUI");
        assert_eq!(out.text, "Open a GitHub issue about GPUI.");
    }

    #[test]
    fn raw_never_runs_a_cleanup_model() {
        let cleaner = fake_cleaner(Ok("never"));
        let replacer = Replacer::new(&[]);
        let raw = style("raw");
        let run = run_on(&cleaner, "s1-mini", &raw, "en", &["en"]);
        let p = TextPipeline { replacer: &replacer, style: &raw, base_prompt: "", vocabulary: &[], enhancer: None, cleaner: Some(run), timeout: Duration::from_secs(4) };
        assert!(!p.will_enhance());
        assert_eq!(p.run("um hi").enhance, EnhanceOutcome::NotUsed);
        assert!(cleaner.seen.lock().unwrap().is_empty());
    }
}
