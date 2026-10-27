//! `enhance` (`docs/extensions.md`, "Requests"): run a style of the user, or
//! a free prompt, on text from an extension, with an AI provider of the user.
//!
//! The host chooses the style and the provider from the request and the
//! settings of the user ([`plan`]). The app runs the style step with the code
//! of a dictation ([`HostServices::run_style`](crate::HostServices::run_style)),
//! and the host turns its result into the reply ([`reply`]).
//!
//! The text is never in the log: it can be anything that the user wrote.

use crate::responder::Responder;
use crate::Shared;
use sayso_core::config::{Config, Provider, ProviderKind};
use sayso_core::enhance::EnhanceError;
use sayso_core::history::EnhanceOutcome;
use sayso_core::pipeline::{CleanupNotice, PipelineOutput};
use sayso_core::style::{CleanupField, RunPlan, Style, StyleLibrary};
use sayso_protocol::{Enhance, EnhanceReply, ErrorCode, Reply};

/// The id of the style that the host makes for a `prompt`.
pub const PROMPT_STYLE_ID: &str = "extension-prompt";

/// A style step for `enhance`: what the host chose from the request and the
/// settings of the user.
pub struct StyleRequest<'a> {
    pub style: &'a Style,
    pub provider: &'a Provider,
    /// The base prompt of the style library of the user.
    pub base_prompt: &'a str,
    pub text: &'a str,
}

/// Why the host does not run a request, as the `error` reply has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnhanceRefusal {
    pub code: Option<ErrorCode>,
    pub message: String,
}

impl EnhanceRefusal {
    fn new(code: Option<ErrorCode>, message: impl Into<String>) -> EnhanceRefusal {
        EnhanceRefusal { code, message: message.into() }
    }

    fn reply(self) -> Reply {
        Reply::error(self.code, self.message)
    }
}

/// `enhance`. Runs on a worker thread: a provider can take seconds.
pub(crate) fn enhance(shared: &Shared, extension_id: &str, request: &Enhance, responder: Responder) {
    let reply = match run(shared, request) {
        Ok(reply) => reply,
        Err(refusal) => {
            log::info!("host: enhance of {extension_id} failed: {}", refusal.message);
            refusal.reply()
        }
    };
    responder.reply(reply);
}

fn run(shared: &Shared, request: &Enhance) -> Result<Reply, EnhanceRefusal> {
    let text = text_of(request)?;
    let Some(styles) = shared.services.styles() else {
        return Err(EnhanceRefusal::new(Some(ErrorCode::Unavailable), "this version of Sayso has no AI providers"));
    };
    let config = shared.services.config();
    let (style, provider) = plan(&config, &styles, request)?;
    let out = shared.services.run_style(&StyleRequest { style: &style, provider: &provider, base_prompt: styles.base_prompt(), text });
    reply(&provider, out)
}

/// The text of the request. `text` is required and not empty; `style` and
/// `prompt` do not go together.
fn text_of(request: &Enhance) -> Result<&str, EnhanceRefusal> {
    let bad = |message: &str| Err(EnhanceRefusal::new(Some(ErrorCode::BadRequest), message));
    let Some(text) = request.text.as_deref().filter(|text| !text.trim().is_empty()) else {
        return bad("enhance needs text that is not empty");
    };
    if request.style.is_some() && request.prompt.is_some() {
        return bad("send style or prompt, not both");
    }
    if request.prompt.as_deref().is_some_and(|prompt| prompt.trim().is_empty()) {
        return bad("the prompt is empty");
    }
    Ok(text)
}

/// The style and the provider of a request.
///
/// - The style: `style` (an id, or a name of the user), else a style that the
///   host makes for `prompt`, else the active style of the user.
/// - The provider: `provider` (an id or a name), else the provider of the
///   style, else the default provider of the user.
pub fn plan(config: &Config, styles: &StyleLibrary, request: &Enhance) -> Result<(Style, Provider), EnhanceRefusal> {
    let unavailable = |message: String| EnhanceRefusal::new(Some(ErrorCode::Unavailable), message);
    let bad = |message: String| EnhanceRefusal::new(Some(ErrorCode::BadRequest), message);
    let mut style = match (&request.style, &request.prompt) {
        (Some(wanted), _) => find_style(styles, wanted).cloned().ok_or_else(|| bad(format!("the user has no style {wanted}")))?,
        (None, Some(prompt)) => prompt_style(prompt),
        (None, None) => styles
            .get(&config.ai.active_style)
            .or_else(|| styles.get(sayso_core::style::DEFAULT_STYLE_ID))
            .cloned()
            .ok_or_else(|| unavailable("the user has no styles".into()))?,
    };
    if style.is_raw() {
        return Err(bad(format!("the style {} uses no AI", style.id)));
    }
    let provider = match &request.provider {
        Some(wanted) => {
            let provider = find_provider(config, wanted).ok_or_else(|| bad(format!("the user has no AI provider {wanted}")))?;
            // The model of a style is a model of its own provider.
            if style.provider.as_deref() != Some(provider.id.as_str()) {
                style.model = None;
            }
            provider
        }
        None => config.ai.provider(style.provider.as_deref()).ok_or_else(|| unavailable(format!("no AI provider is set up for the style {}", style.id)))?,
    };
    if !config.ai.enabled {
        return Err(unavailable("AI is off in Sayso. The user can turn it on in Settings".into()));
    }
    if provider.needs_model() {
        return Err(unavailable(format!("the AI provider {} has no model chosen", provider.id)));
    }
    if request.prompt.is_some() && matches!(provider.kind, ProviderKind::CleanupModel { .. }) {
        return Err(EnhanceRefusal::new(Some(ErrorCode::Unsupported), format!("the AI provider {} is a cleanup model and cannot follow a prompt", provider.id)));
    }
    if style.run_plan(provider) == RunPlan::Nothing {
        return Err(unavailable(format!("the AI provider {} cannot run the style {}", provider.id, style.id)));
    }
    Ok((style, provider.clone()))
}

/// A style by id, else by its name or the name that Sayso shows, without case.
fn find_style<'a>(styles: &'a StyleLibrary, wanted: &str) -> Option<&'a Style> {
    let wanted = wanted.trim();
    styles.get(wanted).or_else(|| styles.styles().find(|style| style.name.eq_ignore_ascii_case(wanted) || style.display_name().eq_ignore_ascii_case(wanted)))
}

/// A provider by id, else by name, without case.
fn find_provider<'a>(config: &'a Config, wanted: &str) -> Option<&'a Provider> {
    let wanted = wanted.trim();
    let providers = &config.ai.providers;
    providers.iter().find(|p| p.id == wanted).or_else(|| providers.iter().find(|p| p.name.eq_ignore_ascii_case(wanted)))
}

/// The style for a free prompt: the prompt is its instruction. It is
/// standalone, so the cleanup rules of the base prompt do not apply.
fn prompt_style(prompt: &str) -> Style {
    Style {
        id: PROMPT_STYLE_ID.into(),
        name: "Prompt".into(),
        ink: String::new(),
        description: String::new(),
        prompt: prompt.trim().to_string(),
        provider: None,
        model: None,
        temperature: None,
        timeout_ms: None,
        standalone: true,
        cleanup: CleanupField::Off,
        inherited_cleanup: None,
    }
}

/// The reply to the result of the style step. The text is never lost in a
/// dictation, but an extension asked for the style: a step that did not
/// apply is an error.
pub fn reply(provider: &Provider, out: PipelineOutput) -> Result<Reply, EnhanceRefusal> {
    if let Some((provider_id, error)) = &out.enhance_error {
        return Err(refusal(provider_id, error));
    }
    match out.enhance {
        EnhanceOutcome::Applied { provider_id, model, .. } => Ok(Reply::ok_with(&EnhanceReply { text: out.text, provider: provider_id, model })),
        EnhanceOutcome::Failed { provider_id, .. } => Err(EnhanceRefusal::new(Some(ErrorCode::Unavailable), format!("the AI provider {provider_id} failed"))),
        EnhanceOutcome::NotUsed => {
            let message = match out.notice {
                Some(CleanupNotice::Language { model, language }) => format!("the cleanup model {model} does not support the language {language}"),
                Some(CleanupNotice::UnknownLanguage { model }) => format!("the cleanup model {model} cannot tell the language of the text"),
                _ => format!("the AI provider {} did not run", provider.id),
            };
            Err(EnhanceRefusal::new(Some(ErrorCode::Unavailable), message))
        }
    }
}

/// The error code of a failed style step. The message names the provider and
/// the kind of failure, and never has the text or the body of a reply.
pub fn refusal(provider_id: &str, error: &EnhanceError) -> EnhanceRefusal {
    let (code, what) = match error {
        EnhanceError::Timeout(ms) => (Some(ErrorCode::Timeout), format!("gave no reply in {} s", ms.div_ceil(1000))),
        EnhanceError::Refused(_) => (Some(ErrorCode::Refused), "refused the text".to_string()),
        EnhanceError::NotConfigured(_) => (Some(ErrorCode::Unavailable), "is not set up".to_string()),
        EnhanceError::Http { status, .. } => (Some(ErrorCode::Unavailable), format!("replied with HTTP {status}")),
        EnhanceError::Network(_) => (Some(ErrorCode::Unavailable), "cannot be reached".to_string()),
        EnhanceError::Cli(_) => (Some(ErrorCode::Unavailable), "failed to run".to_string()),
        EnhanceError::InvalidOutput(_) => (None, "gave a reply that Sayso cannot use".to_string()),
    };
    EnhanceRefusal::new(code, format!("the AI provider {provider_id} {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sayso_core::config::ProviderKind;

    fn provider(id: &str) -> Provider {
        Provider { id: id.into(), name: format!("Provider {id}"), kind: ProviderKind::ClaudeCli { path: None, model: "haiku".into(), thinking: None, config_dir: None } }
    }

    fn config() -> Config {
        let mut config = Config::default();
        config.ai.enabled = true;
        config.ai.providers = vec![provider("claude"), provider("other")];
        config.ai.default_provider = Some("claude".into());
        config
    }

    fn styles() -> StyleLibrary {
        // A folder without files: the built-in styles.
        let dir = tempfile::tempdir().unwrap();
        StyleLibrary::load(dir.path())
    }

    fn request(text: &str) -> Enhance {
        Enhance { text: Some(text.into()), ..Default::default() }
    }

    #[test]
    fn the_request_needs_text_and_not_both_style_and_prompt() {
        assert_eq!(text_of(&request("hi")).unwrap(), "hi");
        for bad in [Enhance::default(), request("  "), Enhance { style: Some("email".into()), prompt: Some("p".into()), ..request("hi") }, Enhance { prompt: Some(" ".into()), ..request("hi") }] {
            assert_eq!(text_of(&bad).unwrap_err().code, Some(ErrorCode::BadRequest), "{bad:?}");
        }
    }

    #[test]
    fn the_style_is_found_by_id_or_name_else_the_active_style() {
        let (config, styles) = (config(), styles());
        let (style, provider) = plan(&config, &styles, &request("hi")).unwrap();
        assert_eq!((style.id.as_str(), provider.id.as_str()), ("clean", "claude"), "the active style and the default provider");
        let (style, _) = plan(&config, &styles, &Enhance { style: Some("email".into()), ..request("hi") }).unwrap();
        assert_eq!(style.id, "email");
        let (style, _) = plan(&config, &styles, &Enhance { style: Some("Notes".into()), ..request("hi") }).unwrap();
        assert_eq!(style.id, "notes", "by name");
        let error = plan(&config, &styles, &Enhance { style: Some("pirate".into()), ..request("hi") }).unwrap_err();
        assert_eq!(error.code, Some(ErrorCode::BadRequest));
        let error = plan(&config, &styles, &Enhance { style: Some("raw".into()), ..request("hi") }).unwrap_err();
        assert_eq!(error.code, Some(ErrorCode::BadRequest), "Raw uses no AI");
    }

    #[test]
    fn a_prompt_is_a_standalone_style() {
        let (style, _) = plan(&config(), &styles(), &Enhance { prompt: Some(" Translate to French. ".into()), ..request("hi") }).unwrap();
        assert_eq!(style.id, PROMPT_STYLE_ID);
        assert_eq!(style.prompt, "Translate to French.");
        assert!(style.standalone);
    }

    #[test]
    fn the_provider_is_named_or_of_the_style_or_the_default() {
        let (config, styles) = (config(), styles());
        let (_, provider) = plan(&config, &styles, &Enhance { provider: Some("other".into()), ..request("hi") }).unwrap();
        assert_eq!(provider.id, "other");
        let (_, provider) = plan(&config, &styles, &Enhance { provider: Some("provider OTHER".into()), ..request("hi") }).unwrap();
        assert_eq!(provider.id, "other", "by name");
        let error = plan(&config, &styles, &Enhance { provider: Some("openai".into()), ..request("hi") }).unwrap_err();
        assert_eq!(error.code, Some(ErrorCode::BadRequest));

        let mut off = config.clone();
        off.ai.enabled = false;
        assert_eq!(plan(&off, &styles, &request("hi")).unwrap_err().code, Some(ErrorCode::Unavailable), "AI is off");
        let mut none = config.clone();
        none.ai.default_provider = None;
        assert_eq!(plan(&none, &styles, &request("hi")).unwrap_err().code, Some(ErrorCode::Unavailable), "no provider");
    }

    #[test]
    fn failures_map_to_codes_without_the_text() {
        let cases = [
            (EnhanceError::Timeout(4000), Some(ErrorCode::Timeout)),
            (EnhanceError::Refused("secret text".into()), Some(ErrorCode::Refused)),
            (EnhanceError::NotConfigured("no key".into()), Some(ErrorCode::Unavailable)),
            (EnhanceError::Http { status: 401, body: "secret text".into() }, Some(ErrorCode::Unavailable)),
            (EnhanceError::Network("secret text".into()), Some(ErrorCode::Unavailable)),
            (EnhanceError::Cli("secret text".into()), Some(ErrorCode::Unavailable)),
            (EnhanceError::InvalidOutput("secret text".into()), None),
        ];
        for (error, code) in cases {
            let refusal = refusal("claude", &error);
            assert_eq!(refusal.code, code, "{error:?}");
            assert!(refusal.message.contains("claude") && !refusal.message.contains("secret"), "{}", refusal.message);
        }
    }
}
