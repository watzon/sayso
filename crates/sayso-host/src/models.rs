//! `list_models` (`docs/apps.md`, "Requests"): the local models of this
//! system with their state and the choice of the user.
//!
//! The list has speech, VAD, and diarization models from
//! `sayso_core::models::local_models`. Cloud models are not in it, and Basic
//! VAD is not a model. The state comes from the engine.

use crate::Shared;
use crate::responder::Responder;
use sayso_core::config::Config;
use sayso_core::engine_catalog;
use sayso_core::models::{self as core, ModelId, Timestamps, Vad};
use sayso_core::stt::ModelStatus;
use sayso_protocol::{ErrorCode, ListModels, ListModelsReply, ModelInfo, ModelState, Reply, Role, TimestampLevel};

/// The protocol state for the status of the engine.
pub(crate) fn state(status: &ModelStatus) -> ModelState {
    match status {
        ModelStatus::NotDownloaded => ModelState::NotDownloaded,
        ModelStatus::Downloading { .. } => ModelState::Downloading,
        ModelStatus::Optimizing => ModelState::Optimizing,
        ModelStatus::Downloaded => ModelState::Downloaded,
        ModelStatus::Ready => ModelState::Ready,
        ModelStatus::Failed { .. } => ModelState::Failed,
    }
}

fn timestamps(level: Timestamps) -> TimestampLevel {
    match level {
        Timestamps::Word => TimestampLevel::Word,
        Timestamps::Segment => TimestampLevel::Segment,
        Timestamps::None => TimestampLevel::None,
    }
}

/// The model that the host uses for each role when a request names none:
/// the final model of the user, the model that gives the live preview, the
/// VAD model (Basic VAD is no model), and the diarization model. A model for
/// the last three roles counts only when it is on disk (`on_disk`).
fn chosen(config: &Config, on_disk: &dyn Fn(&ModelId) -> bool) -> Vec<(ModelId, Role)> {
    let dictation = &config.dictation;
    let mut chosen = vec![(dictation.model.clone(), Role::Final)];
    if let Some(active) = core::find_with(&config.speech, &dictation.model)
        && let Some(preview) = core::preview_for(&active, &dictation.preview_model, &dictation.language, on_disk)
    {
        chosen.push((preview, Role::Streaming));
    }
    if let Vad::Model(id) = core::vad_for(None, &config.models.vad, on_disk) {
        chosen.push((id, Role::Vad));
    }
    if let Some(id) = core::diarization_for(None, &config.models.diarization, on_disk) {
        chosen.push((id, Role::Diarization));
    }
    chosen
}

/// The models of `catalog` for the reply to `list_models`, in catalog order.
/// With `role`, only the models that have it. `status` tells the state of a
/// model.
pub(crate) fn list(catalog: &[core::ModelInfo], status: &dyn Fn(&ModelId) -> ModelStatus, config: &Config, role: Option<&Role>) -> Vec<ModelInfo> {
    let chosen = chosen(config, &|id| status(id).is_on_disk());
    catalog
        .iter()
        .filter_map(|model| {
            let roles: Vec<Role> = engine_catalog::roles(model).into_iter().map(Role::parse).collect();
            if role.is_some_and(|wanted| !roles.contains(wanted)) {
                return None;
            }
            Some(ModelInfo {
                id: model.id.to_string(),
                name: model.name.clone(),
                roles,
                state: state(&status(&model.id)),
                languages: model.languages.iter().map(|language| language.to_string()).collect(),
                timestamps: timestamps(model.engine.timestamps()),
                size_bytes: model.size_bytes,
                choice: chosen.iter().filter(|(id, _)| *id == model.id).map(|(_, role)| role.clone()).collect(),
            })
        })
        .collect()
}

/// `list_models`. Runs on a worker thread, because the engine can still start.
pub(crate) fn list_models(shared: &Shared, request: &ListModels, responder: Responder) {
    let reply = match crate::download::ready_engine(shared, &responder) {
        Ok(engine) => {
            let config = shared.services.config();
            let models = list(core::local_models(), &|id| engine.status(id), &config, request.role.as_ref());
            Reply::ok_with(&ListModelsReply { models })
        }
        Err(reply) => reply,
    };
    responder.reply(reply);
}

/// The reply when no engine runs.
pub(crate) fn no_engine() -> Reply {
    Reply::error(Some(ErrorCode::Unsupported), "the speech engine does not run, so Sayso cannot list or download models")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sayso_core::config::RoleChoice;
    use sayso_core::models::ModelTask;
    use std::collections::HashMap;

    fn ids(task: ModelTask) -> Vec<ModelId> {
        core::models_for(task).map(|m| m.id.clone()).collect()
    }

    fn statuses(known: &[(&ModelId, ModelStatus)]) -> impl Fn(&ModelId) -> ModelStatus {
        let map: HashMap<ModelId, ModelStatus> = known.iter().map(|(id, status)| ((*id).clone(), status.clone())).collect();
        move |id| map.get(id).cloned().unwrap_or(ModelStatus::NotDownloaded)
    }

    fn find<'a>(models: &'a [ModelInfo], id: &ModelId) -> &'a ModelInfo {
        models.iter().find(|m| m.id == id.as_str()).unwrap_or_else(|| panic!("{id} is not in the list"))
    }

    #[test]
    fn the_list_has_every_local_model_and_no_cloud_model() {
        let models = list(core::local_models(), &|_| ModelStatus::NotDownloaded, &Config::default(), None);
        assert_eq!(models.len(), core::local_models().len());
        assert!(models.iter().any(|m| m.roles.contains(&Role::Vad)));
        assert!(models.iter().any(|m| m.roles.contains(&Role::Diarization)));
        assert!(models.iter().all(|m| m.id != "basic"), "Basic VAD is no model");
        assert!(models.iter().all(|m| !m.id.contains(':')), "a cloud model id is '<provider>:<model>'");
        assert!(models.iter().all(|m| m.state == ModelState::NotDownloaded && !m.name.is_empty()));
    }

    #[test]
    fn a_model_has_the_roles_of_its_catalog_entry() {
        let models = list(core::local_models(), &|_| ModelStatus::NotDownloaded, &Config::default(), None);
        for model in core::local_models() {
            let want: Vec<Role> = engine_catalog::roles(model).into_iter().map(Role::parse).collect();
            assert_eq!(find(&models, &model.id).roles, want, "{}", model.id);
        }
        let vad = &ids(ModelTask::Vad)[0];
        assert_eq!(find(&models, vad).roles, vec![Role::Vad]);
        let diarization = &ids(ModelTask::Diarization)[0];
        assert_eq!(find(&models, diarization).roles, vec![Role::Diarization]);
        let streaming = core::local_models().iter().find(|m| m.live_preview && m.final_pass).unwrap();
        assert_eq!(find(&models, &streaming.id).roles, vec![Role::Final, Role::Streaming]);
    }

    #[test]
    fn a_role_filters_the_list() {
        let all = list(core::local_models(), &|_| ModelStatus::NotDownloaded, &Config::default(), None);
        for role in [Role::Final, Role::Streaming, Role::Vad, Role::Diarization] {
            let some = list(core::local_models(), &|_| ModelStatus::NotDownloaded, &Config::default(), Some(&role));
            assert!(!some.is_empty(), "{role}");
            assert!(some.iter().all(|m| m.roles.contains(&role)), "{role}");
            assert_eq!(some.len(), all.iter().filter(|m| m.roles.contains(&role)).count(), "{role}");
        }
        let vad: Vec<String> = ids(ModelTask::Vad).iter().map(|id| id.to_string()).collect();
        let listed: Vec<String> = list(core::local_models(), &|_| ModelStatus::NotDownloaded, &Config::default(), Some(&Role::Vad)).into_iter().map(|m| m.id).collect();
        assert_eq!(listed, vad);
        assert!(list(core::local_models(), &|_| ModelStatus::NotDownloaded, &Config::default(), Some(&Role::Other("translate".into()))).is_empty());
    }

    #[test]
    fn the_engine_status_gives_the_state() {
        let ids = [ModelStatus::NotDownloaded, ModelStatus::Downloading { fraction: 0.5, bytes_done: 1, bytes_total: 2 }, ModelStatus::Optimizing, ModelStatus::Downloaded, ModelStatus::Ready, ModelStatus::Failed { message: "x".into(), reason: None }];
        let states = [ModelState::NotDownloaded, ModelState::Downloading, ModelState::Optimizing, ModelState::Downloaded, ModelState::Ready, ModelState::Failed];
        for (status, want) in ids.iter().zip(states) {
            assert_eq!(state(status), want);
        }
        let catalog = core::local_models();
        let known: Vec<(&ModelId, ModelStatus)> = catalog.iter().zip(ids.iter().cycle()).map(|(m, s)| (&m.id, s.clone())).collect();
        let models = list(catalog, &statuses(&known), &Config::default(), None);
        for ((_, status), model) in known.iter().zip(&models) {
            assert_eq!(model.state, state(status), "{}", model.id);
        }
    }

    #[test]
    fn the_choice_is_the_model_that_the_host_uses_for_each_role() {
        let final_model = core::local_models().iter().find(|m| m.final_pass && !m.live_preview).unwrap().id.clone();
        let streaming = core::local_models().iter().find(|m| m.live_preview && m.id != final_model && m.understands("en")).unwrap().id.clone();
        let vad = ids(ModelTask::Vad)[0].clone();
        let diarization = ids(ModelTask::Diarization)[0].clone();
        let mut config = Config::default();
        config.dictation.model = final_model.clone();
        config.dictation.language = "en".into();
        config.dictation.preview_model = RoleChoice::Model(streaming.clone());
        let on_disk = [(&final_model, ModelStatus::Ready), (&streaming, ModelStatus::Downloaded), (&vad, ModelStatus::Downloaded), (&diarization, ModelStatus::Downloaded)];
        let models = list(core::local_models(), &statuses(&on_disk), &config, None);
        assert_eq!(find(&models, &final_model).choice, vec![Role::Final]);
        assert_eq!(find(&models, &streaming).choice, vec![Role::Streaming]);
        assert_eq!(find(&models, &vad).choice, vec![Role::Vad], "automatic VAD: the VAD model on disk");
        assert_eq!(find(&models, &diarization).choice, vec![Role::Diarization]);
        assert_eq!(models.iter().filter(|m| !m.choice.is_empty()).count(), 4);

        // A model that does two roles has both.
        let both = core::local_models().iter().find(|m| m.final_pass && m.live_preview).unwrap().id.clone();
        config.dictation.model = both.clone();
        config.dictation.preview_model = RoleChoice::Auto;
        let models = list(core::local_models(), &statuses(&[(&both, ModelStatus::Ready)]), &config, None);
        assert_eq!(find(&models, &both).choice, vec![Role::Final, Role::Streaming]);
    }

    #[test]
    fn vad_and_diarization_models_that_are_not_on_disk_are_not_chosen() {
        let vad = ids(ModelTask::Vad)[0].clone();
        let diarization = ids(ModelTask::Diarization)[0].clone();
        let mut config = Config::default();
        config.models.vad = RoleChoice::Model(vad.clone());
        config.models.diarization = RoleChoice::Model(diarization.clone());
        let models = list(core::local_models(), &|_| ModelStatus::NotDownloaded, &config, None);
        assert!(find(&models, &vad).choice.is_empty());
        assert!(find(&models, &diarization).choice.is_empty());
        let models = list(core::local_models(), &statuses(&[(&vad, ModelStatus::Downloaded), (&diarization, ModelStatus::Ready)]), &config, None);
        assert_eq!(find(&models, &vad).choice, vec![Role::Vad]);
        assert_eq!(find(&models, &diarization).choice, vec![Role::Diarization]);
    }

    #[test]
    fn basic_vad_gives_no_choice() {
        let vad = ids(ModelTask::Vad)[0].clone();
        let mut config = Config::default();
        config.models.vad = RoleChoice::Model(ModelId::new(core::BASIC_VAD));
        let models = list(core::local_models(), &statuses(&[(&vad, ModelStatus::Downloaded)]), &config, None);
        assert!(models.iter().all(|m| !m.choice.contains(&Role::Vad)));
    }
}
