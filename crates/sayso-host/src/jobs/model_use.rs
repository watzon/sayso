//! The models that jobs use, for the memory policy of the app
//! (`crates/sayso-app/src/memory.rs`, and `docs/extensions.md`, "Scheduling").
//!
//! The app unloads each model that it does not want. A job holds its models
//! ([`ModelUse::hold`]) before it loads them, and the app adds
//! [`ModelUse::wanted`] to the models that it wants, so it does not unload a
//! model while a job uses it. After the job, the model stays wanted for the
//! idle time of the other models (`dictation.keep_model_minutes`), so the
//! next job of a queue finds it loaded.

use parking_lot::Mutex;
use sayso_core::models::ModelId;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct ModelUse {
    models: Mutex<HashMap<ModelId, Use>>,
}

#[derive(Default)]
struct Use {
    /// The jobs that hold the model now.
    holders: usize,
    /// When the last holder let go.
    released: Option<Instant>,
}

impl ModelUse {
    pub fn new() -> Arc<ModelUse> {
        Arc::new(ModelUse::default())
    }

    /// Hold `models` until the result drops.
    pub fn hold(self: &Arc<Self>, models: Vec<ModelId>) -> ModelHold {
        let mut map = self.models.lock();
        for id in &models {
            map.entry(id.clone()).or_default().holders += 1;
        }
        ModelHold { owner: self.clone(), models }
    }

    /// The models that must stay in memory: held now, or let go less than
    /// `keep` ago. `keep` None keeps them always, as `keep_model_minutes`
    /// None does. Forgets the models whose time ended.
    pub fn wanted(&self, keep: Option<Duration>) -> Vec<ModelId> {
        let mut map = self.models.lock();
        map.retain(|_, u| u.holders > 0 || keep.is_none_or(|keep| u.released.is_some_and(|at| at.elapsed() < keep)));
        let mut ids: Vec<ModelId> = map.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// True while a job holds `model`.
    pub fn held(&self, model: &ModelId) -> bool {
        self.models.lock().get(model).is_some_and(|u| u.holders > 0)
    }
}

/// Models that a job holds. Dropping it lets them go.
pub struct ModelHold {
    owner: Arc<ModelUse>,
    models: Vec<ModelId>,
}

impl Drop for ModelHold {
    fn drop(&mut self) {
        let mut map = self.owner.models.lock();
        for id in &self.models {
            if let Some(u) = map.get_mut(id) {
                u.holders = u.holders.saturating_sub(1);
                if u.holders == 0 {
                    u.released = Some(Instant::now());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> ModelId {
        ModelId::new(s)
    }

    #[test]
    fn a_held_model_is_wanted_until_the_idle_time_ends() {
        let models = ModelUse::new();
        let a = models.hold(vec![id("speech"), id("vad")]);
        let b = models.hold(vec![id("speech")]);
        assert_eq!(models.wanted(Some(Duration::ZERO)), [id("speech"), id("vad")]);
        drop(a);
        assert!(models.held(&id("speech")) && !models.held(&id("vad")));
        assert_eq!(models.wanted(Some(Duration::from_secs(60))), [id("speech"), id("vad")], "vad waits for the idle time");
        assert_eq!(models.wanted(Some(Duration::ZERO)), [id("speech")], "the idle time of vad ended");
        drop(b);
        assert_eq!(models.wanted(None), [id("speech")], "no idle time: kept");
        assert!(models.wanted(Some(Duration::ZERO)).is_empty());
    }
}
