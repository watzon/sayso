//! The models of a job: the checks of the request (`docs/extensions.md`,
//! "Requests", and the error codes in "Code").
//!
//! A field that names a model must name a model of this system
//! (`unknown_model`) of the right task (`wrong_role`), on disk
//! (`model_not_downloaded`), that the engine can run (`unsupported`). A field
//! that is missing gets the choice of the user. A choice of the user that
//! cannot run falls back where the docs allow it: Basic VAD for VAD, and the
//! timestamp level `none` for an engine without segments.
//!
//! [`SaysoModels`] gives these rules to `sayso-speech` as its `ModelResolver`.

use super::{JobSpec, VadRun};
use crate::live::SessionSpec;
use sayso_core::config::{Config, PreviewChoice, RoleChoice};
use sayso_core::models::{self, BASIC_VAD, ModelId, ModelTask, Timestamps, Vad};
use sayso_platform::SttBackend;
use sayso_protocol::{ErrorCode, SessionStart, TranscribeFile};
use sayso_speech::ModelResolver;

/// A request that the host refuses: the `error` reply of `transcribe_file`
/// or `session_start`.
pub use sayso_speech::Refusal;

/// The choices of the user that a job uses when it names no model.
#[derive(Debug, Clone, Copy)]
pub struct UserChoices<'a> {
    /// `dictation.model`. A cloud model cannot run a job.
    pub final_model: &'a ModelId,
    /// `dictation.language`.
    pub language: &'a str,
    /// `models.vad`.
    pub vad: &'a RoleChoice,
    /// `models.diarization`.
    pub diarization: &'a RoleChoice,
}

/// What the engine of this computer has.
#[derive(Clone, Copy)]
pub struct EngineView<'a> {
    /// True when the files of the model are on disk.
    pub on_disk: &'a dyn Fn(&ModelId) -> bool,
    /// The `features` of the engine (`SttBackend::features`). Empty when the
    /// engine did not say: then the engine answers each request itself.
    pub features: &'a [String],
}

impl EngineView<'_> {
    pub(crate) fn has(&self, feature: &str) -> bool {
        self.features.is_empty() || self.features.iter().any(|f| f == feature)
    }
}

pub(crate) fn refuse(code: ErrorCode, message: impl Into<String>) -> Refusal {
    Refusal::new(code, message)
}

/// The models of Sayso: the choices of the user and the catalog of this computer.
pub struct SaysoModels<'a> {
    pub user: UserChoices<'a>,
    /// `dictation.preview_model`, for live sessions.
    pub preview: &'a PreviewChoice,
    pub engine: EngineView<'a>,
}

impl SaysoModels<'_> {
    /// Run `f` with the settings of the user in `config` and the engine now.
    pub(crate) fn with<T>(config: &Config, engine: &dyn SttBackend, f: impl FnOnce(&SaysoModels<'_>) -> T) -> T {
        let features = engine.features();
        let on_disk = |id: &ModelId| engine.status(id).is_on_disk();
        let user = UserChoices {
            final_model: &config.dictation.model,
            language: &config.dictation.language,
            vad: &config.models.vad,
            diarization: &config.models.diarization,
        };
        f(&SaysoModels { user, preview: &config.dictation.preview_model, engine: EngineView { on_disk: &on_disk, features: &features } })
    }
}

impl ModelResolver for SaysoModels<'_> {
    fn job(&self, request: &TranscribeFile) -> Result<JobSpec, Refusal> {
        resolve(request, &self.user, &self.engine)
    }

    fn session(&self, request: &SessionStart) -> Result<SessionSpec, Refusal> {
        crate::live::resolve(request, &self.user, self.preview, &self.engine)
    }
}

/// Check `request` and choose its models. `diarization_model` without
/// `diarize` asks for diarization; `diarize: false` turns it off.
pub fn resolve(request: &TranscribeFile, user: &UserChoices, engine: &EngineView) -> Result<JobSpec, Refusal> {
    if !request.path.is_absolute() {
        return Err(refuse(ErrorCode::BadRequest, format!("path must be absolute: {}", request.path.display())));
    }
    if request.speakers == Some(0) {
        return Err(refuse(ErrorCode::BadRequest, "speakers must be 1 or more"));
    }

    let model = final_model(request.model.as_deref(), user, engine)?;
    // An engine without segments still transcribes: one segment for each chunk.
    let timestamps = if engine.has("segments") { model.engine.timestamps() } else { Timestamps::None };
    let vad = vad(request.vad_model.as_deref(), user, engine)?;
    let diarization = diarization(request.diarize, request.diarization_model.as_deref(), user, engine)?;

    Ok(JobSpec {
        path: request.path.clone(),
        model: model.id.into(),
        timestamps: timestamps.into(),
        vad,
        diarization: diarization.map(Into::into),
        language: request.language.clone().unwrap_or_else(|| user.language.to_string()),
        speakers: request.speakers,
    })
}

/// The speech model of the field `model`, else the final model of the user.
/// It must do the final pass. Jobs and live sessions use the same rules.
pub(crate) fn final_model(asked: Option<&str>, user: &UserChoices, engine: &EngineView) -> Result<models::ModelInfo, Refusal> {
    let model = match asked {
        Some(id) => checked(&ModelId::new(id), ModelTask::Speech, "model", engine)?,
        None => {
            let id = user.final_model;
            match models::find_local(id) {
                Some(info) if info.task() == ModelTask::Speech => {
                    if !(engine.on_disk)(id) {
                        return Err(refuse(ErrorCode::ModelNotDownloaded, format!("the final model of Sayso, {id}, is not downloaded")));
                    }
                    info
                }
                _ => {
                    return Err(refuse(
                        ErrorCode::Unsupported,
                        format!("the final model of Sayso, {id}, is not a model of this computer; name a local model in model"),
                    ));
                }
            }
        }
    };
    if !model.final_pass {
        return Err(refuse(ErrorCode::WrongRole, format!("{} cannot do the final pass", model.id)));
    }
    Ok(model)
}

/// The VAD of the field `vad_model`, else the choice of the user, which
/// falls back to Basic VAD.
pub(crate) fn vad(asked: Option<&str>, user: &UserChoices, engine: &EngineView) -> Result<VadRun, Refusal> {
    Ok(match asked {
        Some(BASIC_VAD) => VadRun::Basic,
        Some(id) => {
            let info = checked(&ModelId::new(id), ModelTask::Vad, "vad_model", engine)?;
            need_feature(engine, "vad", &info.id)?;
            VadRun::Model(info.id.into())
        }
        None => match models::vad_for(None, user.vad, |id| (engine.on_disk)(id)) {
            Vad::Model(id) if engine.has("vad") => VadRun::Model(id.into()),
            _ => VadRun::Basic,
        },
    })
}

/// The diarization model. `diarization_model` without `diarize` asks for
/// diarization; `diarize: false` turns it off.
pub(crate) fn diarization(diarize: Option<bool>, asked: Option<&str>, user: &UserChoices, engine: &EngineView) -> Result<Option<ModelId>, Refusal> {
    let diarize = diarize.unwrap_or(asked.is_some());
    match (asked, diarize) {
        (_, false) => Ok(None),
        (Some(id), true) => {
            let info = checked(&ModelId::new(id), ModelTask::Diarization, "diarization_model", engine)?;
            need_feature(engine, "diarization", &info.id)?;
            Ok(Some(info.id))
        }
        (None, true) => {
            let id = models::diarization_for(None, user.diarization, |id| (engine.on_disk)(id)).ok_or_else(|| {
                refuse(ErrorCode::ModelNotDownloaded, "no diarization model is downloaded; send download_model")
            })?;
            need_feature(engine, "diarization", &id)?;
            Ok(Some(id))
        }
    }
}

/// The model `id` that the field `field` names, after the checks of the
/// module, for `task`.
pub(crate) fn checked(id: &ModelId, task: ModelTask, field: &str, engine: &EngineView) -> Result<models::ModelInfo, Refusal> {
    let Some(info) = models::find_local(id) else {
        return Err(refuse(ErrorCode::UnknownModel, format!("{field}: {id} is not a model of this computer")));
    };
    if info.task() != task {
        let what = match task {
            ModelTask::Speech => "a speech model",
            ModelTask::Vad => "a VAD model",
            ModelTask::Diarization => "a diarization model",
        };
        return Err(refuse(ErrorCode::WrongRole, format!("{field}: {id} is not {what}")));
    }
    if !(engine.on_disk)(id) {
        return Err(refuse(ErrorCode::ModelNotDownloaded, format!("{field}: {id} is not downloaded; send download_model")));
    }
    Ok(info)
}

pub(crate) fn need_feature(engine: &EngineView, feature: &str, id: &ModelId) -> Result<(), Refusal> {
    if engine.has(feature) {
        Ok(())
    } else {
        Err(refuse(ErrorCode::Unsupported, format!("the speech engine of this computer cannot run {id}: it has no {feature}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The first local model of `task` in this system's catalog.
    fn first(task: ModelTask) -> ModelId {
        models::models_for(task).next().unwrap_or_else(|| panic!("no {task:?} model")).id.clone()
    }

    fn final_model() -> ModelId {
        models::catalog().iter().find(|m| m.final_pass).unwrap().id.clone()
    }

    fn request() -> TranscribeFile {
        TranscribeFile { path: PathBuf::from(if cfg!(windows) { r"C:\a.wav" } else { "/tmp/a.wav" }), ..Default::default() }
    }

    struct Setup {
        final_model: ModelId,
        vad: RoleChoice,
        diarization: RoleChoice,
        missing: Vec<ModelId>,
        features: Vec<String>,
    }

    impl Setup {
        fn new() -> Setup {
            Setup {
                final_model: final_model(),
                vad: RoleChoice::Auto,
                diarization: RoleChoice::Auto,
                missing: Vec::new(),
                features: ["segments", "vad", "diarization"].map(String::from).to_vec(),
            }
        }

        fn resolve(&self, request: &TranscribeFile) -> Result<JobSpec, Refusal> {
            let user = UserChoices { final_model: &self.final_model, language: "en", vad: &self.vad, diarization: &self.diarization };
            let on_disk = |id: &ModelId| !self.missing.contains(id);
            resolve(request, &user, &EngineView { on_disk: &on_disk, features: &self.features })
        }

        fn code(&self, request: &TranscribeFile) -> ErrorCode {
            self.resolve(request).unwrap_err().code
        }
    }

    #[test]
    fn missing_fields_take_the_choices_of_the_user() {
        let setup = Setup::new();
        let job = setup.resolve(&request()).unwrap();
        assert_eq!(job.model, setup.final_model.clone().into());
        assert_eq!(job.vad, VadRun::Model(first(ModelTask::Vad).into()), "auto: the VAD model on disk");
        assert_eq!(job.diarization, None);
        assert_eq!(job.language, "en");
        let job = setup.resolve(&TranscribeFile { diarize: Some(true), ..request() }).unwrap();
        assert_eq!(job.diarization, Some(first(ModelTask::Diarization).into()));
        let basic = Setup { vad: RoleChoice::Model(ModelId::new(BASIC_VAD)), ..Setup::new() };
        assert_eq!(basic.resolve(&request()).unwrap().vad, VadRun::Basic);
    }

    #[test]
    fn named_models_are_checked() {
        let setup = Setup::new();
        let named = TranscribeFile {
            model: Some(final_model().0),
            vad_model: Some(first(ModelTask::Vad).0),
            diarization_model: Some(first(ModelTask::Diarization).0),
            language: Some("de".into()),
            speakers: Some(3),
            ..request()
        };
        let job = setup.resolve(&named).unwrap();
        assert_eq!(job.diarization, Some(first(ModelTask::Diarization).into()), "diarization_model asks for diarization");
        assert_eq!((job.language.as_str(), job.speakers), ("de", Some(3)));
        let off = TranscribeFile { diarize: Some(false), ..named.clone() };
        assert_eq!(setup.resolve(&off).unwrap().diarization, None);
        let basic = TranscribeFile { vad_model: Some(BASIC_VAD.into()), ..named };
        assert_eq!(setup.resolve(&basic).unwrap().vad, VadRun::Basic);
    }

    #[test]
    fn an_unknown_model_is_refused() {
        let setup = Setup::new();
        for request in [
            TranscribeFile { model: Some("no-such-model".into()), ..request() },
            TranscribeFile { vad_model: Some("no-such-vad".into()), ..request() },
            TranscribeFile { diarization_model: Some("no-such-diarizer".into()), ..request() },
        ] {
            assert_eq!(setup.code(&request), ErrorCode::UnknownModel, "{request:?}");
        }
    }

    #[test]
    fn a_model_of_another_role_is_refused() {
        let setup = Setup::new();
        let (speech, vad, diarization) = (final_model().0, first(ModelTask::Vad).0, first(ModelTask::Diarization).0);
        for request in [
            TranscribeFile { model: Some(vad.clone()), ..request() },
            TranscribeFile { vad_model: Some(speech.clone()), ..request() },
            TranscribeFile { vad_model: Some(diarization.clone()), ..request() },
            TranscribeFile { diarization_model: Some(speech), ..request() },
            TranscribeFile { diarization_model: Some(vad), ..request() },
        ] {
            assert_eq!(setup.code(&request), ErrorCode::WrongRole, "{request:?}");
        }
        // A model that only streams cannot do the final pass of a job.
        if let Some(streaming) = models::catalog().iter().find(|m| !m.final_pass) {
            assert_eq!(setup.code(&TranscribeFile { model: Some(streaming.id.0.clone()), ..request() }), ErrorCode::WrongRole);
        }
    }

    #[test]
    fn a_model_that_is_not_on_disk_is_refused() {
        let missing = vec![final_model(), first(ModelTask::Vad), first(ModelTask::Diarization)];
        let setup = Setup { missing, ..Setup::new() };
        for request in [
            TranscribeFile { model: Some(final_model().0), ..request() },
            TranscribeFile { model: Some(final_model().0), vad_model: Some(first(ModelTask::Vad).0), ..request() },
            TranscribeFile { diarization_model: Some(first(ModelTask::Diarization).0), ..request() },
            // The choice of the user is not on disk either.
            request(),
        ] {
            assert_eq!(setup.code(&request), ErrorCode::ModelNotDownloaded, "{request:?}");
        }
        // Without a VAD model on disk, auto is Basic VAD. Without a
        // diarization model, diarization cannot run.
        let speech_on_disk = Setup { missing: vec![first(ModelTask::Vad), first(ModelTask::Diarization)], ..Setup::new() };
        assert_eq!(speech_on_disk.resolve(&request()).unwrap().vad, VadRun::Basic);
        assert_eq!(speech_on_disk.code(&TranscribeFile { diarize: Some(true), ..request() }), ErrorCode::ModelNotDownloaded);
    }

    #[test]
    fn an_engine_without_the_feature_is_unsupported() {
        let setup = Setup { features: vec!["segments".into()], ..Setup::new() };
        assert_eq!(setup.code(&TranscribeFile { vad_model: Some(first(ModelTask::Vad).0), ..request() }), ErrorCode::Unsupported);
        assert_eq!(setup.code(&TranscribeFile { diarize: Some(true), ..request() }), ErrorCode::Unsupported);
        // The choice of the user falls back to Basic VAD.
        assert_eq!(setup.resolve(&request()).unwrap().vad, VadRun::Basic);
        // Without segments, every model gives one segment for each chunk.
        let old = Setup { features: vec!["vad".into()], ..Setup::new() };
        assert_eq!(old.resolve(&request()).unwrap().timestamps, Timestamps::None.into());
        // An engine that did not list its features answers each request itself.
        let unknown = Setup { features: Vec::new(), ..Setup::new() };
        assert!(unknown.resolve(&TranscribeFile { diarize: Some(true), ..request() }).is_ok());
    }

    #[test]
    fn a_cloud_final_model_or_a_bad_field_is_refused() {
        let cloud = Setup { final_model: ModelId::new("openai/whisper-1"), ..Setup::new() };
        assert_eq!(cloud.code(&request()), ErrorCode::Unsupported);
        let setup = Setup::new();
        assert_eq!(setup.code(&TranscribeFile { path: PathBuf::from("a.wav"), ..request() }), ErrorCode::BadRequest);
        assert_eq!(setup.code(&TranscribeFile { speakers: Some(0), ..request() }), ErrorCode::BadRequest);
    }
}
