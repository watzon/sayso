//! [`ModelResolver`] of local mode: the options of the app and the catalog of the engine.
//! The checks and the codes are those of Sayso (`crates/sayso-host/src/jobs/resolve.rs` and
//! `live/resolve.rs`), and a test program runs both. Only the choices differ
//! (`docs/sdk.md`, "What differs between modes"): the options of the app, else the first
//! suitable model on disk.

use crate::ModelChoices;
use crate::local_engine::LocalEngine;
use crate::protocol::engine::CatalogModel;
use crate::protocol::{ErrorCode, ErrorReply, ModelInfo, ModelState, Role, SessionStart, TimestampLevel, TranscribeFile};
use sayso_speech::jobs::VadRun;
use sayso_speech::{JobSpec, ModelId, ModelResolver, Refusal, SessionSpec, Timestamps};
use std::collections::HashMap;

/// The id of Basic VAD, which is no model.
const BASIC_VAD: &str = "basic";

pub(crate) struct LocalModels {
    catalog: Vec<CatalogModel>,
    states: HashMap<String, ModelState>,
    features: Vec<String>,
    choices: ModelChoices,
}

fn refuse(code: ErrorCode, message: impl Into<String>) -> Refusal {
    Refusal::new(code, message)
}

fn is_speech(model: &CatalogModel) -> bool {
    model.roles.contains(&Role::Final) || model.roles.contains(&Role::Streaming)
}

impl LocalModels {
    pub fn read(engine: &LocalEngine, choices: &ModelChoices) -> Result<LocalModels, ErrorReply> {
        if !engine.has("catalog") {
            return Err(ErrorReply { code: Some(ErrorCode::Unsupported), message: "the engine of this app has no catalog of models; ship a newer engine".into(), capability: None, reason: None });
        }
        let catalog = engine.catalog()?;
        let states = catalog.states.into_iter().map(|state| (state.model, state.state)).collect();
        Ok(LocalModels { catalog: catalog.models, states, features: engine.features(), choices: choices.clone() })
    }

    fn has(&self, feature: &str) -> bool {
        self.features.is_empty() || self.features.iter().any(|f| f == feature)
    }

    fn state(&self, id: &str) -> ModelState {
        self.states.get(id).cloned().unwrap_or(ModelState::NotDownloaded)
    }

    fn on_disk(&self, id: &str) -> bool {
        matches!(self.state(id), ModelState::Downloaded | ModelState::Optimizing | ModelState::Ready)
    }

    fn model(&self, id: &str) -> Option<&CatalogModel> {
        self.catalog.iter().find(|model| model.id == id)
    }

    fn first_on_disk(&self, wanted: impl Fn(&CatalogModel) -> bool) -> Option<&CatalogModel> {
        self.catalog.iter().find(|model| wanted(model) && self.on_disk(&model.id))
    }

    pub fn find(&self, id: &str) -> Option<ModelInfo> {
        self.model(id).map(|model| self.info(model))
    }

    fn info(&self, model: &CatalogModel) -> ModelInfo {
        let mut choice = Vec::new();
        if self.default_final().is_ok_and(|chosen| chosen.id == model.id) {
            choice.push(Role::Final);
        }
        if self.default_final().is_ok_and(|chosen| self.default_preview(chosen, &self.default_language()).is_some_and(|preview| preview.id == model.id)) {
            choice.push(Role::Streaming);
        }
        if matches!(self.default_vad(), VadRun::Model(id) if id.as_str() == model.id) {
            choice.push(Role::Vad);
        }
        if self.default_diarization().is_some_and(|chosen| chosen.id == model.id) {
            choice.push(Role::Diarization);
        }
        ModelInfo { id: model.id.clone(), name: model.name.clone(), roles: model.roles.clone(), state: self.state(&model.id), languages: model.languages.clone(), timestamps: model.timestamps.clone(), size_bytes: model.size_bytes, choice }
    }

    pub fn list(&self, role: Option<&Role>) -> Vec<ModelInfo> {
        self.catalog.iter().filter(|model| role.is_none_or(|role| model.roles.contains(role))).map(|model| self.info(model)).collect()
    }

    fn default_language(&self) -> String {
        self.choices.language.clone().unwrap_or_else(|| "auto".to_string())
    }

    /// The final model of the app, else the first model on disk that does the final pass.
    fn default_final(&self) -> Result<&CatalogModel, Refusal> {
        match &self.choices.final_model {
            Some(id) => {
                let Some(model) = self.model(id).filter(|model| is_speech(model)) else {
                    return Err(refuse(ErrorCode::Unsupported, format!("the final model of the app, {id}, is not a speech model of this engine")));
                };
                if !self.on_disk(id) {
                    return Err(refuse(ErrorCode::ModelNotDownloaded, format!("the final model of the app, {id}, is not downloaded")));
                }
                Ok(model)
            }
            None => self.first_on_disk(|model| model.roles.contains(&Role::Final)).ok_or_else(|| refuse(ErrorCode::ModelNotDownloaded, "no speech model is downloaded; send download_model")),
        }
    }

    /// The model that the field `field` names, with the checks of Sayso.
    fn checked(&self, id: &str, wanted: impl Fn(&CatalogModel) -> bool, what: &str, field: &str) -> Result<&CatalogModel, Refusal> {
        let Some(model) = self.model(id) else {
            return Err(refuse(ErrorCode::UnknownModel, format!("{field}: {id} is not a model of this computer")));
        };
        if !wanted(model) {
            return Err(refuse(ErrorCode::WrongRole, format!("{field}: {id} is not {what}")));
        }
        if !self.on_disk(id) {
            return Err(refuse(ErrorCode::ModelNotDownloaded, format!("{field}: {id} is not downloaded; send download_model")));
        }
        Ok(model)
    }

    fn need_feature(&self, feature: &str, id: &str) -> Result<(), Refusal> {
        if self.has(feature) { Ok(()) } else { Err(refuse(ErrorCode::Unsupported, format!("the engine of this app cannot run {id}: it has no {feature}"))) }
    }

    fn final_model(&self, asked: Option<&str>) -> Result<&CatalogModel, Refusal> {
        let model = match asked {
            Some(id) => self.checked(id, is_speech, "a speech model", "model")?,
            None => self.default_final()?,
        };
        if !model.roles.contains(&Role::Final) {
            return Err(refuse(ErrorCode::WrongRole, format!("{} cannot do the final pass", model.id)));
        }
        Ok(model)
    }

    /// The VAD of the app, else the first VAD model on disk, else Basic VAD. A chosen model
    /// that is not on disk gives Basic VAD, as the choice of the user does in Sayso.
    fn default_vad(&self) -> VadRun {
        let model = match self.choices.vad_model.as_deref() {
            Some(BASIC_VAD) => None,
            Some(id) => self.model(id).filter(|model| model.roles.contains(&Role::Vad) && self.on_disk(id)),
            None => self.first_on_disk(|model| model.roles.contains(&Role::Vad)),
        };
        match model {
            Some(model) if self.has("vad") => VadRun::Model(ModelId::new(&model.id)),
            _ => VadRun::Basic,
        }
    }

    fn vad(&self, asked: Option<&str>) -> Result<VadRun, Refusal> {
        match asked {
            Some(BASIC_VAD) => Ok(VadRun::Basic),
            Some(id) => {
                let model = self.checked(id, |model| model.roles.contains(&Role::Vad), "a VAD model", "vad_model")?;
                self.need_feature("vad", &model.id)?;
                Ok(VadRun::Model(ModelId::new(&model.id)))
            }
            None => Ok(self.default_vad()),
        }
    }

    fn default_diarization(&self) -> Option<&CatalogModel> {
        let wanted = |model: &CatalogModel| model.roles.contains(&Role::Diarization);
        match self.choices.diarization_model.as_deref() {
            Some(id) => self.model(id).filter(|model| wanted(model) && self.on_disk(id)),
            None => self.first_on_disk(wanted),
        }
    }

    /// `diarization_model` without `diarize` asks for diarization; `diarize: false` turns it off.
    fn diarization(&self, diarize: Option<bool>, asked: Option<&str>) -> Result<Option<ModelId>, Refusal> {
        if !diarize.unwrap_or(asked.is_some()) {
            return Ok(None);
        }
        let model = match asked {
            Some(id) => self.checked(id, |model| model.roles.contains(&Role::Diarization), "a diarization model", "diarization_model")?,
            None => self.default_diarization().ok_or_else(|| refuse(ErrorCode::ModelNotDownloaded, "no diarization model is downloaded; send download_model"))?,
        };
        self.need_feature("diarization", &model.id)?;
        Ok(Some(ModelId::new(&model.id)))
    }

    /// The preview model of the app, else the final model when it streams, else the first
    /// streaming model on disk that understands the language.
    fn default_preview<'a>(&'a self, final_model: &'a CatalogModel, language: &str) -> Option<&'a CatalogModel> {
        let understands = |model: &CatalogModel| language == "auto" || model.languages.is_empty() || model.languages.iter().any(|known| known == language);
        let streams = |model: &CatalogModel| model.roles.contains(&Role::Streaming) && understands(model);
        match self.choices.preview_model.as_deref() {
            Some(id) => self.model(id).filter(|model| streams(model) && self.on_disk(id)),
            None if streams(final_model) => Some(final_model),
            None => self.first_on_disk(streams),
        }
    }
}

impl ModelResolver for LocalModels {
    fn job(&self, request: &TranscribeFile) -> Result<JobSpec, Refusal> {
        if !request.path.is_absolute() {
            return Err(refuse(ErrorCode::BadRequest, format!("path must be absolute: {}", request.path.display())));
        }
        if request.speakers == Some(0) {
            return Err(refuse(ErrorCode::BadRequest, "speakers must be 1 or more"));
        }
        let model = self.final_model(request.model.as_deref())?;
        // An engine without segments still transcribes: one segment for each chunk.
        let timestamps = if self.has("segments") {
            match model.timestamps {
                TimestampLevel::Word => Timestamps::Word,
                TimestampLevel::Segment => Timestamps::Segment,
                _ => Timestamps::None,
            }
        } else {
            Timestamps::None
        };
        Ok(JobSpec {
            path: request.path.clone(),
            model: ModelId::new(&model.id),
            timestamps,
            vad: self.vad(request.vad_model.as_deref())?,
            diarization: self.diarization(request.diarize, request.diarization_model.as_deref())?,
            language: request.language.clone().unwrap_or_else(|| self.default_language()),
            speakers: request.speakers,
        })
    }

    fn session(&self, request: &SessionStart) -> Result<SessionSpec, Refusal> {
        let final_model = self.final_model(request.model.as_deref())?;
        let language = request.language.clone().unwrap_or_else(|| self.default_language());
        let preview_model = match request.preview_model.as_deref() {
            Some(id) => {
                let model = self.checked(id, is_speech, "a speech model", "preview_model")?;
                if !model.roles.contains(&Role::Streaming) {
                    return Err(refuse(ErrorCode::WrongRole, format!("preview_model: {id} cannot stream")));
                }
                self.need_feature("concurrent_streams", &model.id)?;
                Some(ModelId::new(&model.id))
            }
            None if self.has("concurrent_streams") => self.default_preview(final_model, &language).map(|model| ModelId::new(&model.id)),
            None => None,
        };
        Ok(SessionSpec {
            sources: request.sources.clone(),
            final_model: ModelId::new(&final_model.id),
            preview_model,
            vad: self.vad(request.vad_model.as_deref())?,
            diarization: self.diarization(request.diarize, request.diarization_model.as_deref())?,
            language,
            record_to: request.record_to.clone(),
        })
    }
}
