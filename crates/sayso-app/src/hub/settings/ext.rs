//! `AppModel` methods that Settings and onboarding need.

use crate::live::LiveAudio;
use crate::model::AppModel;
use gpui_kit::*;
use sayso_core::hotkey::Hotkey;
use sayso_platform::{AppInfo, CaptureHandle, LoginItemState, SoundKind};
use std::sync::Arc;
use std::time::Duration;

/// The result of one key recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    Key(Hotkey),
    /// The system key listener could not start (no Input Monitoring).
    Unavailable,
    TimedOut,
}

impl AppModel {
    /// Record the next key combination through the system key listener.
    /// Unlike `capture_hotkey`, this also reports a failure and a timeout.
    pub fn record_hotkey(&mut self, cx: &mut Context<Self>, done: impl FnOnce(&mut Self, Recorded, &mut Context<Self>) + 'static) {
        let rx = self.services.platform.hotkeys.capture_next();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    match rx.recv_timeout(Duration::from_secs(20)) {
                        Ok(hk) => Recorded::Key(hk),
                        Err(e) if e.is_timeout() => Recorded::TimedOut,
                        Err(_) => Recorded::Unavailable,
                    }
                })
                .await;
            let _ = this.update(cx, |m, cx| done(m, result, cx));
        })
        .detach();
    }

    pub fn login_item_state(&self) -> LoginItemState {
        self.services.platform.login_item.state()
    }

    /// Apps that run now, for the "Type instead of paste" picker.
    pub fn running_apps(&self) -> Vec<AppInfo> {
        let mut apps: Vec<AppInfo> = self
            .services
            .platform
            .context
            .running_apps()
            .into_iter()
            .filter(|a| a.bundle_id != "dev.sayso.Sayso" && !a.name.is_empty())
            .collect();
        apps.sort_by_key(|a| a.name.to_lowercase());
        apps.dedup_by(|a, b| a.bundle_id == b.bundle_id);
        apps
    }

    /// A display name for a bundle id: the running app's name, else the last part of the id.
    /// A Windows id is the name of an exe file, and its name is the part before `.exe`.
    pub fn app_name_for(&self, bundle_id: &str, running: &[AppInfo]) -> String {
        if let Some(a) = running.iter().find(|a| a.bundle_id == bundle_id) {
            return a.name.clone();
        }
        let last = match bundle_id.len().checked_sub(4).and_then(|at| bundle_id.split_at_checked(at)) {
            Some((stem, ext)) if ext.eq_ignore_ascii_case(".exe") => stem,
            _ => bundle_id.rsplit('.').next().unwrap_or(bundle_id),
        };
        let mut chars = last.chars();
        match chars.next() {
            Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
            None => bundle_id.to_string(),
        }
    }

    /// Forget the saved pill positions. The overlay reads them when Sayso starts.
    pub fn reset_pill_position(&self) -> Result<(), String> {
        let path = self.paths.state_file();
        let mut value: serde_json::Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(map) = value.as_object_mut() {
            map.remove("pill");
        } else {
            value = serde_json::json!({});
        }
        let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| e.to_string())
    }

    /// The device id for a config value. Config may hold an id or a name.
    pub fn resolve_input_device(&self, value: Option<&str>) -> Option<String> {
        let value = value?;
        let devices = self.input_devices();
        devices
            .iter()
            .find(|d| d.id == value)
            .or_else(|| devices.iter().find(|d| d.name == value))
            .map(|d| d.id.clone())
    }

    /// Open the microphone for a level meter (mic test). Levels go to `live`,
    /// not to the dictation levels. Drop the handle to stop.
    pub fn start_level_meter(&self, device: Option<&str>, live: Arc<LiveAudio>) -> Result<Box<dyn CaptureHandle>, String> {
        let id = self.resolve_input_device(device);
        self.services
            .platform
            .audio
            .start(id.as_deref(), Box::new(move |frame| live.push(frame.level)))
            .map_err(|e| e.to_string())
    }

    /// Play the start sound at the configured volume, for the volume slider.
    pub fn preview_sound(&self) {
        self.services.platform.sounds.play(SoundKind::Start, self.config.sounds.volume);
    }
}
