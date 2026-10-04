//! Enhancement with a language model that the engine runs on this computer.

use sayso_core::enhance::{EnhanceError, EnhanceRequest, EnhanceResponse, Enhancer, LanguageModel, parse_value, user_message};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The language model of the engine as an [`Enhancer`]. On macOS this is
/// Apple Intelligence. No text leaves this computer.
pub struct EngineModel {
    id: String,
    /// None when the engine does not run.
    model: Option<Arc<dyn LanguageModel>>,
    default_timeout: Duration,
}

impl EngineModel {
    pub fn new(id: &str, model: Option<Arc<dyn LanguageModel>>) -> Self {
        Self { id: id.to_string(), model, default_timeout: Duration::from_secs(4) }
    }

    /// The timeout for requests whose own timeout is zero.
    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }
}

impl Enhancer for EngineModel {
    fn id(&self) -> &str {
        &self.id
    }

    fn is_cloud(&self) -> bool {
        false
    }

    fn enhance(&self, request: &EnhanceRequest) -> Result<EnhanceResponse, EnhanceError> {
        let model = self.model.as_ref().ok_or_else(|| EnhanceError::NotConfigured("the engine does not run".into()))?;
        let timeout = if request.timeout.is_zero() { self.default_timeout } else { request.timeout };
        let started = Instant::now();
        let reply = model.generate(&request.system_prompt, &user_message(&request.transcript), request.temperature, timeout)?;
        // The engine sends the bound "text" field. The same checks as for the other providers apply.
        let text = parse_value(&json!({ "text": reply.text }))?;
        Ok(EnhanceResponse {
            text,
            provider_id: self.id.clone(),
            model: reply.model,
            elapsed_ms: started.elapsed().as_millis() as u64,
            mode: "guided_generation".into(),
        })
    }
}
