//! Cleanup models in the app: download, delete, add as a provider, and test.
//! See `docs/cleanup-models.md`.

use crate::model::AppModel;
use gpui_kit::*;
use parking_lot::Mutex;
use sayso_cleanup_client::download::{self, DownloadError, Progress};
use sayso_core::cleanup::{self, Cleaner, CleanupModelInfo, CleanupRequest};
use sayso_core::config::{Provider, ProviderKind};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The downloads that run, and the last error of each model.
#[derive(Default)]
pub struct CleanupModels {
    downloads: HashMap<&'static str, Running>,
    errors: HashMap<&'static str, String>,
    /// Models whose files are being deleted.
    deleting: std::collections::HashSet<&'static str>,
    /// The last "Delete download" that failed. The files can still be there.
    delete_errors: HashMap<&'static str, String>,
}

struct Running {
    progress: Arc<Mutex<Progress>>,
    cancel: Arc<AtomicBool>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CleanupModelState {
    NotDownloaded,
    /// The fraction of the bytes, from 0 to 1.
    Downloading(f32),
    Ready,
    /// The last download failed. The text is in English, for "Copy details".
    Failed(String),
    /// "Delete download" runs.
    Deleting,
    /// "Delete download" failed. The text is in English, for "Copy details".
    DeleteFailed(String),
}

/// The sentence that "Run test" cleans up.
const TEST_TEXT: &str = "um so the meeting is on tuesday no wait wednesday at three thirty";

impl AppModel {
    fn cleanup_models_dir(&self) -> PathBuf {
        self.paths.models_dir()
    }

    pub fn cleanup_model_state(&self, id: &str) -> CleanupModelState {
        let Some(info) = cleanup::find(id) else { return CleanupModelState::NotDownloaded };
        if self.cleanup.deleting.contains(info.id) {
            return CleanupModelState::Deleting;
        }
        // Before Ready: the files of a failed delete are often still there.
        if let Some(error) = self.cleanup.delete_errors.get(info.id) {
            return CleanupModelState::DeleteFailed(error.clone());
        }
        if let Some(running) = self.cleanup.downloads.get(info.id) {
            let p = *running.progress.lock();
            return CleanupModelState::Downloading(if p.bytes_total == 0 { 0.0 } else { p.bytes_done as f32 / p.bytes_total as f32 });
        }
        if download::is_ready(&self.cleanup_models_dir(), info) {
            return CleanupModelState::Ready;
        }
        match self.cleanup.errors.get(info.id) {
            Some(error) => CleanupModelState::Failed(error.clone()),
            None => CleanupModelState::NotDownloaded,
        }
    }

    /// The model file, when the model is ready.
    pub fn cleanup_model_file(&self, id: &str) -> Option<PathBuf> {
        download::model_path(&self.cleanup_models_dir(), cleanup::find(id)?)
    }

    /// The provider of a cleanup model, when the user added it.
    pub fn cleanup_provider(&self, id: &str) -> Option<&Provider> {
        self.config.ai.providers.iter().find(|p| matches!(&p.kind, ProviderKind::CleanupModel { model } if model == id))
    }

    /// Add a cleanup model as a provider and start its download. The provider
    /// id is the model id.
    pub fn add_cleanup_model(&mut self, id: &str, make_default: bool, cx: &mut Context<Self>) {
        let Some(info) = cleanup::find(id) else { return };
        if self.cleanup_provider(info.id).is_none() || make_default {
            let provider = Provider { id: info.id.into(), name: info.name.into(), kind: ProviderKind::CleanupModel { model: info.id.into() } };
            self.save_provider(provider, None, make_default, cx);
        }
        self.download_cleanup_model(info.id, cx);
    }

    pub fn download_cleanup_model(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(info) = cleanup::find(id) else { return };
        if self.cleanup.downloads.contains_key(info.id) || self.cleanup.deleting.contains(info.id) || download::is_ready(&self.cleanup_models_dir(), info) {
            return;
        }
        self.cleanup.errors.remove(info.id);
        let progress = Arc::new(Mutex::new(Progress { bytes_done: 0, bytes_total: info.size_bytes }));
        let cancel = Arc::new(AtomicBool::new(false));
        self.cleanup.downloads.insert(info.id, Running { progress: progress.clone(), cancel: cancel.clone() });
        cx.notify();
        let dir = self.cleanup_models_dir();
        let endpoint = std::env::var("SAYSO_HF_ENDPOINT").ok().filter(|e| !e.is_empty()).unwrap_or_else(|| download::DEFAULT_ENDPOINT.into());
        let done = Arc::new(AtomicBool::new(false));
        let task = {
            let (progress, done) = (progress.clone(), done.clone());
            cx.background_spawn(async move {
                let result = download::download(&dir, info, &endpoint, &cancel, &mut |p| *progress.lock() = p);
                done.store(true, Ordering::Relaxed);
                result
            })
        };
        // Redraw the progress a few times a second while the download runs.
        cx.spawn(async move |this, cx| {
            while !done.load(Ordering::Relaxed) {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |m, cx| {
                m.cleanup.downloads.remove(info.id);
                match result {
                    Ok(_) => log::info!("downloaded the cleanup model {}", info.id),
                    Err(DownloadError::Cancelled) => {}
                    Err(e) => {
                        log::warn!("could not download the cleanup model {}: {e}", info.id);
                        m.cleanup.errors.insert(info.id, e.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn cancel_cleanup_download(&mut self, id: &str) {
        if let Some(running) = self.cleanup.downloads.get(id) {
            running.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// "Delete download": stop the runtime, delete the files, and remove the
    /// provider. The model is then a suggestion again. It runs in the
    /// background: a cleanup that runs now ends first (within its deadline),
    /// and no new cleanup or load ahead starts for the model meanwhile.
    pub fn delete_cleanup_model(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(info) = cleanup::find(id) else { return };
        if !self.cleanup.deleting.insert(info.id) {
            return;
        }
        self.cleanup.delete_errors.remove(info.id);
        self.cancel_cleanup_download(info.id);
        cx.notify();
        let (runtime, dir) = (self.services.cleanup_runtime.clone(), self.cleanup_models_dir());
        let task = cx.background_spawn(async move {
            // No load of the model from here on: the runtime checks the block
            // under the lock of every load.
            runtime.block(info.id);
            // A cancelled download ends at its next read; a load ahead that
            // was already waiting can open the file again, so the delete tries
            // a few times, with the runtime stopped each time. Windows cannot
            // delete an open file.
            let mut result = Ok(());
            for _ in 0..20 {
                runtime.stop();
                result = download::delete(&dir, info);
                if result.is_ok() || !download::model_dir(&dir, info).exists() {
                    result = Ok(());
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            runtime.unblock(info.id);
            result.map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |m, cx| {
                m.cleanup.deleting.remove(info.id);
                match result {
                    Ok(()) => {
                        m.cleanup.errors.remove(info.id);
                        if let Some(provider) = m.cleanup_provider(info.id).map(|p| p.id.clone()) {
                            m.remove_provider(&provider, cx);
                        }
                    }
                    Err(e) => {
                        log::warn!("could not delete the cleanup model {}: {e}", info.id);
                        m.cleanup.delete_errors.insert(info.id, e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// "Run test": clean up a fixed English sentence. Gives the time in ms.
    pub fn test_cleanup_model(&mut self, id: &str, cx: &mut Context<Self>, done: impl FnOnce(&mut Self, Result<u64, String>, &mut Context<Self>) + 'static) {
        let Some(info) = cleanup::find(id) else { return };
        let cleaner = self.services.cleaner.clone();
        let timeout = Duration::from_millis(self.config.ai.http_timeout_ms);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let request = CleanupRequest { model: info.id.into(), text: TEST_TEXT.into(), settings: Default::default(), app: None, site: None, timeout };
                    cleaner.clean(&request).map(|r| r.elapsed_ms).map_err(|e| e.to_string())
                })
                .await;
            let _ = this.update(cx, |m, cx| done(m, result, cx));
        })
        .detach();
    }

    /// The catalog entries, for the provider list and onboarding.
    pub fn cleanup_catalog(&self) -> &'static [CleanupModelInfo] {
        cleanup::catalog()
    }
}
