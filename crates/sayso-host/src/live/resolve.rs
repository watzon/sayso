//! The models of a live session: the checks of `session_start`, with the
//! rules and error codes of jobs (`crate::jobs::resolve`), and the preview model.

use super::SessionSpec;
use crate::jobs::resolve::{self as jobs, EngineView, Refusal, UserChoices, checked, need_feature, refuse};
use crate::jobs::VadRun;
use sayso_core::config::PreviewChoice;
use sayso_core::models::{self, ModelId, ModelTask, Vad};
use sayso_protocol::{ErrorCode, SessionStart};

/// Check `request` and choose its models. `preview` is the choice of the user
/// for the live preview (`dictation.preview_model`). The sources and
/// `record_to` are checked when the session starts.
pub fn resolve(request: &SessionStart, user: &UserChoices, preview: &PreviewChoice, engine: &EngineView) -> Result<SessionSpec, Refusal> {
    let final_model = jobs::final_model(request.model.as_deref(), user, engine)?;
    let language = request.language.clone().unwrap_or_else(|| user.language.to_string());

    // An engine without concurrent streams ends the older stream of a model
    // when a new one starts, so a preview could end the preview of a dictation.
    let streams = engine.has("concurrent_streams");
    let preview_model = match request.preview_model.as_deref() {
        Some(id) => {
            let info = checked(&ModelId::new(id), ModelTask::Speech, "preview_model", engine)?;
            if !info.live_preview {
                return Err(refuse(ErrorCode::WrongRole, format!("preview_model: {id} cannot stream")));
            }
            need_feature(engine, "concurrent_streams", &info.id)?;
            Some(info.id)
        }
        None if streams => models::preview_for(&final_model, preview, &language, |id| (engine.on_disk)(id)),
        None => None,
    };

    let vad = match jobs::vad(request.vad_model.as_deref(), user, engine)? {
        VadRun::Basic => Vad::Basic,
        VadRun::Model(id) => Vad::Model(id),
    };
    let diarization = jobs::diarization(request.diarize, request.diarization_model.as_deref(), user, engine)?;
    Ok(SessionSpec { sources: request.sources.clone(), final_model: final_model.id, preview_model, vad, diarization, language, record_to: request.record_to.clone() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sayso_core::config::RoleChoice;
    use sayso_protocol::Source;

    fn first(task: ModelTask) -> ModelId {
        models::models_for(task).next().unwrap().id.clone()
    }

    /// A model that does the final pass and streams.
    fn streaming() -> ModelId {
        models::catalog().iter().find(|m| m.final_pass && m.live_preview && m.understands("en")).unwrap().id.clone()
    }

    /// A model that does the final pass and does not stream.
    fn not_streaming() -> ModelId {
        models::catalog().iter().find(|m| m.final_pass && !m.live_preview).unwrap().id.clone()
    }

    fn request() -> SessionStart {
        SessionStart { sources: vec![Source::Client], ..Default::default() }
    }

    struct Setup {
        final_model: ModelId,
        preview: PreviewChoice,
        missing: Vec<ModelId>,
        features: Vec<String>,
    }

    impl Setup {
        fn new() -> Setup {
            Setup {
                final_model: streaming(),
                preview: PreviewChoice::Auto,
                missing: Vec::new(),
                features: ["concurrent_streams", "segments", "vad", "diarization"].map(String::from).to_vec(),
            }
        }

        fn resolve(&self, request: &SessionStart) -> Result<SessionSpec, Refusal> {
            let (vad, diarization) = (RoleChoice::Auto, RoleChoice::Auto);
            let user = UserChoices { final_model: &self.final_model, language: "en", vad: &vad, diarization: &diarization };
            let on_disk = |id: &ModelId| !self.missing.contains(id);
            resolve(request, &user, &self.preview, &EngineView { on_disk: &on_disk, features: &self.features })
        }
    }

    #[test]
    fn missing_fields_take_the_choices_of_the_user() {
        let setup = Setup::new();
        let spec = setup.resolve(&request()).unwrap();
        assert_eq!(spec.final_model, streaming());
        assert_eq!(spec.preview_model, Some(streaming()), "auto: the final model streams itself");
        assert_eq!(spec.vad, Vad::Model(first(ModelTask::Vad)));
        assert_eq!(spec.diarization, None);
        assert_eq!(spec.language, "en");
        assert_eq!(spec.sources, vec![Source::Client]);
        let spec = setup.resolve(&SessionStart { diarize: Some(true), vad_model: Some("basic".into()), language: Some("de".into()), ..request() }).unwrap();
        assert_eq!((spec.diarization, spec.vad, spec.language.as_str()), (Some(first(ModelTask::Diarization)), Vad::Basic, "de"));
    }

    #[test]
    fn the_preview_model_is_checked() {
        let setup = Setup::new();
        let named = |id: &ModelId| SessionStart { preview_model: Some(id.0.clone()), ..request() };
        assert_eq!(setup.resolve(&named(&streaming())).unwrap().preview_model, Some(streaming()));
        assert_eq!(setup.resolve(&named(&not_streaming())).unwrap_err().code, ErrorCode::WrongRole);
        assert_eq!(setup.resolve(&named(&first(ModelTask::Vad))).unwrap_err().code, ErrorCode::WrongRole);
        assert_eq!(setup.resolve(&named(&ModelId::new("no-such-model"))).unwrap_err().code, ErrorCode::UnknownModel);
        let missing = Setup { missing: vec![streaming()], final_model: not_streaming(), ..Setup::new() };
        assert_eq!(missing.resolve(&named(&streaming())).unwrap_err().code, ErrorCode::ModelNotDownloaded);
        // A final model that does not stream, with no streaming model on disk: no preview.
        let others: Vec<ModelId> = models::catalog().iter().filter(|m| m.live_preview).map(|m| m.id.clone()).collect();
        let none = Setup { final_model: not_streaming(), missing: others, ..Setup::new() };
        assert_eq!(none.resolve(&request()).unwrap().preview_model, None);
    }

    #[test]
    fn an_engine_without_concurrent_streams_gives_no_preview() {
        let old = Setup { features: ["segments", "vad", "diarization"].map(String::from).to_vec(), ..Setup::new() };
        assert_eq!(old.resolve(&request()).unwrap().preview_model, None);
        let named = SessionStart { preview_model: Some(streaming().0), ..request() };
        assert_eq!(old.resolve(&named).unwrap_err().code, ErrorCode::Unsupported);
    }

    #[test]
    fn the_rules_of_jobs_apply_to_the_other_models() {
        let setup = Setup::new();
        assert_eq!(setup.resolve(&SessionStart { model: Some(first(ModelTask::Vad).0), ..request() }).unwrap_err().code, ErrorCode::WrongRole);
        assert_eq!(setup.resolve(&SessionStart { vad_model: Some(streaming().0), ..request() }).unwrap_err().code, ErrorCode::WrongRole);
        assert_eq!(setup.resolve(&SessionStart { diarization_model: Some("nobody".into()), ..request() }).unwrap_err().code, ErrorCode::UnknownModel);
        let missing = Setup { missing: vec![first(ModelTask::Diarization)], ..Setup::new() };
        assert_eq!(missing.resolve(&SessionStart { diarize: Some(true), ..request() }).unwrap_err().code, ErrorCode::ModelNotDownloaded);
    }
}
