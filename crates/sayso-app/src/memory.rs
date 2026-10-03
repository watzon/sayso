//! Which local models stay in engine memory.
//!
//! The engine keeps a model until it gets `unload`. The app wants two models
//! in memory: the active model (for the final pass) and the live-preview
//! model. Every other model leaves memory. The active model also leaves when
//! the idle time of `dictation.keep_model_minutes` ended, and the next
//! dictation loads it again.

use sayso_core::models::ModelId;
use sayso_core::stt::ModelStatus;
use std::collections::HashMap;
use std::time::Duration;

/// The models that must be in memory. `active` is None for a cloud model.
/// With `idle`, the idle time unloaded the active model: it stays out, also
/// when it gives the live preview.
pub fn wanted(active: Option<&ModelId>, preview: Option<&ModelId>, idle: bool) -> Vec<ModelId> {
    let mut ids: Vec<ModelId> = Vec::new();
    for id in [active, preview].into_iter().flatten() {
        if !(idle && Some(id) == active) && !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    ids
}

/// The models to unload: in memory, and not wanted. A model that loads now
/// is not in the list. It gets its turn when the load is complete.
pub fn unwanted(statuses: &HashMap<ModelId, ModelStatus>, wanted: &[ModelId]) -> Vec<ModelId> {
    let mut ids: Vec<ModelId> =
        statuses.iter().filter(|(id, status)| status.is_usable() && !wanted.contains(id)).map(|(id, _)| id.clone()).collect();
    ids.sort();
    ids
}

/// True when the unload of `id` must keep the live preview in memory: `id`
/// gives the live preview. Such a model is unwanted only as the idle active
/// model. An engine that cannot split the model unloads all of it.
pub fn keeps_preview(id: &ModelId, preview: Option<&ModelId>) -> bool {
    preview == Some(id)
}

/// True when the idle time of the active model ended. `keep_minutes` is the
/// setting: None keeps the model in memory always.
pub fn idle_time_ended(keep_minutes: Option<u32>, idle: Duration) -> bool {
    keep_minutes.is_some_and(|minutes| idle >= Duration::from_secs(u64::from(minutes) * 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> ModelId {
        ModelId::new(s)
    }

    #[test]
    fn the_active_model_and_the_preview_model_are_wanted() {
        let (final_pass, preview) = (id("whisper"), id("kroko"));
        assert_eq!(wanted(Some(&final_pass), Some(&preview), false), [id("whisper"), id("kroko")]);
        assert_eq!(wanted(Some(&final_pass), None, false), [id("whisper")]);
        // One model for the final pass and the live preview.
        assert_eq!(wanted(Some(&final_pass), Some(&final_pass), false), [id("whisper")]);
        // A cloud model: only the live-preview model is in the engine.
        assert_eq!(wanted(None, Some(&preview), false), [id("kroko")]);
        assert!(wanted(None, None, false).is_empty());
    }

    #[test]
    fn the_idle_time_takes_out_the_active_model_only() {
        let (final_pass, preview) = (id("whisper"), id("kroko"));
        assert_eq!(wanted(Some(&final_pass), Some(&preview), true), [id("kroko")], "the live preview stays");
        assert!(wanted(Some(&final_pass), Some(&final_pass), true).is_empty(), "a shared model leaves too");
        assert_eq!(wanted(None, Some(&preview), true), [id("kroko")]);
    }

    #[test]
    fn a_model_that_is_no_longer_in_use_is_unloaded() {
        let statuses: HashMap<ModelId, ModelStatus> = [
            (id("old"), ModelStatus::Ready),
            (id("older"), ModelStatus::Ready),
            (id("new"), ModelStatus::Ready),
            (id("kroko"), ModelStatus::Ready),
            (id("loading"), ModelStatus::Optimizing),
            (id("on-disk"), ModelStatus::Downloaded),
            (id("missing"), ModelStatus::NotDownloaded),
        ]
        .into();
        // The user changed the active model from "old" to "new".
        let keep = wanted(Some(&id("new")), Some(&id("kroko")), false);
        assert_eq!(unwanted(&statuses, &keep), [id("old"), id("older")]);
        // After the idle time, the active model goes too. The live preview stays.
        let keep = wanted(Some(&id("new")), Some(&id("kroko")), true);
        assert_eq!(unwanted(&statuses, &keep), [id("new"), id("old"), id("older")]);
        // Nothing to do when only the wanted models are in memory.
        let tidy: HashMap<ModelId, ModelStatus> = [(id("new"), ModelStatus::Ready), (id("kroko"), ModelStatus::Ready)].into();
        assert!(unwanted(&tidy, &wanted(Some(&id("new")), Some(&id("kroko")), false)).is_empty());
    }

    #[test]
    fn only_the_idle_shared_model_keeps_its_preview() {
        let statuses: HashMap<ModelId, ModelStatus> =
            [(id("old"), ModelStatus::Ready), (id("shared"), ModelStatus::Ready), (id("kroko"), ModelStatus::Ready)].into();
        // One model for both passes, after the idle time.
        let preview = id("shared");
        let keep = wanted(Some(&id("shared")), Some(&preview), true);
        let plan: Vec<(ModelId, bool)> =
            unwanted(&statuses, &keep).into_iter().map(|m| (m.clone(), keeps_preview(&m, Some(&preview)))).collect();
        assert_eq!(plan, [(id("kroko"), false), (id("old"), false), (id("shared"), true)]);
        // A separate preview model stays in memory, and the active model leaves whole.
        let preview = id("kroko");
        let keep = wanted(Some(&id("shared")), Some(&preview), true);
        let plan: Vec<(ModelId, bool)> =
            unwanted(&statuses, &keep).into_iter().map(|m| (m.clone(), keeps_preview(&m, Some(&preview)))).collect();
        assert_eq!(plan, [(id("old"), false), (id("shared"), false)]);
        assert!(!keeps_preview(&id("old"), None));
    }

    #[test]
    fn the_idle_time_ends_after_the_minutes_of_the_setting() {
        let minutes = |m: u64| Duration::from_secs(m * 60);
        // "Always": the model never leaves.
        assert!(!idle_time_ended(None, minutes(100_000)));
        assert!(!idle_time_ended(Some(5), Duration::ZERO));
        assert!(!idle_time_ended(Some(5), minutes(5) - Duration::from_secs(1)));
        assert!(idle_time_ended(Some(5), minutes(5)));
        assert!(!idle_time_ended(Some(60), minutes(59)));
        assert!(idle_time_ended(Some(60), minutes(61)));
    }
}
