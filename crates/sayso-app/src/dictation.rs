//! The dictation controller: runs the effects of `sayso_core::dictation`.
//!
//! The state machine decides what happens. This file does it: microphone,
//! engine, pipeline, insertion, history, sounds.

use crate::model::{AppEvent, AppModel};
use gpui_kit::*;
use parking_lot::Mutex;
use sayso_core::dictation::{Effect, Event, SessionId, Sound, State};
use sayso_core::dictionary::AppliedReplacement;
use sayso_core::history::{EnhanceOutcome, HistoryEntry, InsertOutcome, TargetApp, waveform_summary};
use sayso_core::models::ModelInfo;
use sayso_core::pipeline::TextPipeline;
use sayso_core::speech::{SpeechProvider, SpeechRequest, split_model_id};
use sayso_core::stt::{EngineEvent, ModelStatus, SAMPLE_RATE, SessionOptions, Transcript, encode_wav, to_pcm16};
use sayso_enhance::SecretStore;
use sayso_platform::SttBackend;
use sayso_platform::{
    AppInfo, CaptureHandle, HotkeyEvent, InsertError, InsertMethod, InsertResult, Permission, PermissionState, SoundKind,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Per-dictation data the state machine does not hold.
#[derive(Default)]
pub struct Session {
    pub app: Option<AppInfo>,
    pub samples: Arc<Mutex<Vec<f32>>>,
    pub capture: Option<Box<dyn CaptureHandle>>,
    pub streaming: bool,
    pub started: Option<Instant>,
    pub duration_ms: u64,
    pub transcript: Option<String>,
    pub transcribe_ms: u64,
    pub model: Option<sayso_core::models::ModelId>,
    pub replacements: Vec<AppliedReplacement>,
    pub style_id: String,
    pub enhance: Option<EnhanceOutcome>,
    pub final_text: Option<String>,
    pub insert: Option<InsertOutcome>,
}

pub type Sessions = HashMap<SessionId, Session>;

impl AppModel {
    /// Feed an event to the state machine and run the effects.
    pub fn dispatch(&mut self, event: Event, cx: &mut Context<Self>) {
        let starting = matches!(event, Event::Toggle | Event::PushToTalkDown)
            && !matches!(self.machine.state, State::Recording { .. } | State::Processing { .. });
        if starting && !self.check_can_start(cx) {
            return;
        }
        let was_recording = self.machine.is_recording();
        let now = self.now_ms();
        let effects = self.machine.handle(event, now);
        for fx in effects {
            self.run_effect(fx, cx);
        }
        let recording = self.machine.is_recording();
        if recording != was_recording {
            self.services.platform.hotkeys.set_recording(recording);
        }
        cx.notify();
    }

    /// Check what a dictation needs. Shows a notice and returns false when something is missing.
    fn check_can_start(&mut self, cx: &mut Context<Self>) -> bool {
        if let Some(holder) = self.services.platform.hotkeys.secure_input_holder() {
            self.notice(format!("A password field is active in {holder}"), cx);
            return false;
        }
        match self.permission(Permission::Microphone) {
            _ if crate::dev::fake_mic().is_some() => {}
            PermissionState::Granted => {}
            PermissionState::NotDetermined => {
                self.request_permission(Permission::Microphone);
                self.notice("Allow microphone access, then try again".into(), cx);
                return false;
            }
            PermissionState::Denied => {
                self.open_permission_settings(Permission::Microphone);
                self.notice("Sayso needs microphone access".into(), cx);
                return false;
            }
        }
        let model = self.active_model();
        if model.is_remote() {
            if !self.status_of(&model.id).is_usable() {
                self.notice("Set up the cloud provider of your model first".into(), cx);
                cx.emit(AppEvent::OpenHub(crate::hub::Route::Models));
                return false;
            }
            return true;
        }
        if self.services.engine.is_none() {
            self.notice("The speech engine is not installed".into(), cx);
            return false;
        }
        if !self.status_of(&model.id).is_on_disk() {
            self.notice("Download a speech model first".into(), cx);
            cx.emit(AppEvent::OpenHub(crate::hub::Route::Models));
            return false;
        }
        true
    }

    fn notice(&mut self, text: String, cx: &mut Context<Self>) {
        self.blocked_notice = Some((text, Instant::now()));
        cx.notify();
    }

    fn session(&mut self, id: SessionId) -> &mut Session {
        self.sessions.entry(id).or_default()
    }

    fn run_effect(&mut self, fx: Effect, cx: &mut Context<Self>) {
        match fx {
            Effect::StartCapture { session } => self.start_capture(session, cx),
            Effect::StopCapture { session } => self.stop_capture(session),
            Effect::Transcribe { session } => self.transcribe(session, cx),
            Effect::ApplyStyle { session, transcript } => self.apply_style(session, transcript, cx),
            Effect::Insert { session, text } => self.insert(session, text, cx),
            Effect::Discard { session } => {
                if let Some(mut s) = self.sessions.remove(&session) {
                    if let Some(c) = s.capture.take() {
                        c.stop();
                    }
                    if s.streaming
                        && let Some(e) = &self.services.engine {
                            e.end_stream(session);
                        }
                }
            }
            Effect::SaveHistory { session } => self.save_history(session, cx),
            Effect::PlaySound(sound) => self.play(sound),
        }
    }

    fn play(&self, sound: Sound) {
        let s = &self.config.sounds;
        let (kind, on) = match sound {
            Sound::Start => (SoundKind::Start, s.start),
            Sound::Stop => (SoundKind::Stop, s.stop),
            Sound::Cancel => (SoundKind::Cancel, s.cancel),
            Sound::Insert => (SoundKind::Insert, s.insert),
        };
        if s.enabled && on {
            self.services.platform.sounds.play(kind, s.volume);
        }
    }

    fn session_options(&self) -> SessionOptions {
        SessionOptions {
            final_model: self.active_model().id,
            preview_model: self.preview_model(),
            language: self.config.dictation.language.clone(),
            vocabulary: self.vocabulary(),
        }
    }

    fn start_capture(&mut self, session: SessionId, cx: &mut Context<Self>) {
        // Dictation into Sayso itself is recorded as Sayso. A run outside a
        // bundle has no bundle id, so match the process instead.
        let app = self.services.platform.context.frontmost_app().map(|a| {
            if a.pid == std::process::id() as i32 {
                AppInfo { bundle_id: "dev.sayso.Sayso".into(), name: "Sayso".into(), pid: a.pid }
            } else {
                a
            }
        });
        self.live.clear();
        self.preview = (String::new(), String::new());
        let options = self.session_options();
        let engine = self.services.engine.clone();
        let streaming = match (&engine, &options.preview_model) {
            (Some(e), Some(p)) if self.config.overlay.show_preview && self.status_of(p).is_usable() => {
                e.start_stream(session, &options).is_ok()
            }
            _ => false,
        };
        let samples = Arc::new(Mutex::new(Vec::<f32>::with_capacity(SAMPLE_RATE as usize * 30)));
        let live = self.live.clone();
        let buf = samples.clone();
        let stream_engine = if streaming { engine.clone() } else { None };
        let on_frame = Box::new(move |frame: sayso_platform::AudioFrame| {
            live.push(frame.level);
            if let Some(e) = &stream_engine {
                let _ = e.push_audio(session, &frame.samples);
            }
            buf.lock().extend_from_slice(&frame.samples);
        });
        let device = self.config.audio.input_device.clone();
        let started = match crate::dev::fake_mic() {
            Some(path) => crate::dev::start_fake_capture(&path, on_frame)
                .ok_or_else(|| sayso_platform::PlatformError::Failed("could not read SAYSO_FAKE_MIC".into())),
            None => self.services.platform.audio.start(device.as_deref(), on_frame),
        };
        let capture = match started {
            Ok(c) => Some(c),
            Err(e) => {
                log::error!("microphone: {e}");
                self.notice("The microphone could not start".into(), cx);
                // Leave recording; the audio buffer is empty so it ends as "No speech".
                cx.spawn(async move |this, cx| {
                    let _ = this.update(cx, |m, cx| m.dispatch(Event::Cancel, cx));
                })
                .detach();
                None
            }
        };
        let s = self.session(session);
        s.app = app;
        s.samples = samples;
        s.capture = capture;
        s.streaming = streaming;
        s.started = Some(Instant::now());
    }

    fn stop_capture(&mut self, session: SessionId) {
        let engine = self.services.engine.clone();
        let s = self.session(session);
        if let Some(c) = s.capture.take() {
            c.stop();
        }
        if let Some(t) = s.started {
            s.duration_ms = t.elapsed().as_millis() as u64;
        }
        if s.streaming {
            s.streaming = false;
            if let Some(e) = engine {
                e.end_stream(session);
            }
        }
    }

    /// How the final pass of the active model runs. None, with the reason, when it cannot run.
    fn final_pass(&self) -> Result<FinalPass, String> {
        let info = self.active_model();
        if let Some((provider, model)) = split_model_id(&info.id) {
            let provider = self.config.speech.provider(provider).cloned().ok_or("The cloud provider of this model is not set up.")?;
            // If the provider fails, a local model that is already in memory takes over.
            let fallback = self
                .services
                .engine
                .clone()
                .zip(self.preview_model().and_then(|id| sayso_core::models::find(&id)).map(Box::new))
                .filter(|(_, m)| m.final_pass && self.status_of(&m.id).is_usable());
            return Ok(FinalPass::Cloud {
                provider,
                model: model.to_string(),
                info: Box::new(info),
                secrets: self.services.secrets.clone(),
                timeout: Duration::from_millis(self.config.speech.timeout_ms),
                fallback,
            });
        }
        let engine = self.services.engine.clone().ok_or("The speech engine is not running.")?;
        Ok(FinalPass::Local { engine, model: info.id })
    }

    fn transcribe(&mut self, session: SessionId, cx: &mut Context<Self>) {
        let pass = match self.final_pass() {
            Ok(pass) => pass,
            Err(message) => {
                self.dispatch(Event::TranscribeFailed { session, message }, cx);
                return;
            }
        };
        let samples = self.session(session).samples.lock().clone();
        let options = self.session_options();
        let replacer = self.replacer.clone();
        if matches!(pass, FinalPass::Local { .. }) && !self.status_of(&options.final_model).is_usable() {
            self.load_active_models(cx);
        }
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let (t, note) = pass.run(&samples, &options)?;
                    let (replaced, applied) = replacer.apply(t.text.trim());
                    Ok::<_, String>((t, replaced, applied, note))
                })
                .await;
            let _ = this.update(cx, |m, cx| match result {
                Ok((t, replaced, applied, note)) => {
                    let s = m.session(session);
                    s.transcript = Some(t.text.clone());
                    s.transcribe_ms = t.elapsed_ms;
                    s.model = Some(t.model.clone());
                    s.replacements = applied;
                    if let Some(note) = note {
                        m.notice(note, cx);
                    }
                    m.dispatch(Event::Transcribed { session, text: replaced }, cx);
                }
                Err(message) => m.dispatch(Event::TranscribeFailed { session, message }, cx),
            });
        })
        .detach();
    }

    fn apply_style(&mut self, session: SessionId, text: String, cx: &mut Context<Self>) {
        let style = self.active_style();
        self.session(session).style_id = style.id.clone();
        let provider = self.provider_for(&style).cloned();
        let Some(provider) = provider.filter(|_| style.uses_ai()) else {
            self.session(session).enhance = Some(EnhanceOutcome::NotUsed);
            self.dispatch(Event::ReadyToInsert { session, text, style_name: None, enhance_error: None }, cx);
            return;
        };
        // An HTTP provider has no default model, so the request would fail.
        // Insert the text as it is and say what to fix.
        if provider.needs_model() {
            self.session(session).enhance =
                Some(EnhanceOutcome::Failed { provider_id: provider.id.clone(), reason: "no model is chosen for it".into() });
            let enhance_error = Some(format!("{} has no model chosen", provider.id));
            self.dispatch(Event::ReadyToInsert { session, text, style_name: None, enhance_error }, cx);
            return;
        }
        self.dispatch(Event::EnhanceStarted { session }, cx);
        let ai = self.config.ai.clone();
        let secrets = self.services.secrets.clone();
        let vocabulary = self.vocabulary();
        let replacements = self.session(session).replacements.clone();
        let timeout = Duration::from_millis(match provider.kind {
            sayso_core::config::ProviderKind::OpenAiCompatible { .. } => ai.http_timeout_ms,
            _ => ai.cli_timeout_ms,
        });
        let style_name = style.name.clone();
        cx.spawn(async move |this, cx| {
            let out = cx
                .background_spawn(async move {
                    let enhancer = sayso_enhance::build_enhancer(&provider, secrets.as_ref(), &ai);
                    let replacer = sayso_core::dictionary::Replacer::new(&[]);
                    let p = TextPipeline { replacer: &replacer, style: &style, vocabulary: &vocabulary, enhancer: Some(enhancer.as_ref()), timeout };
                    p.run_style(text, replacements)
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                let applied = matches!(out.enhance, EnhanceOutcome::Applied { .. });
                m.session(session).enhance = Some(out.enhance.clone());
                let name = provider_display(m, out.enhance_error.as_deref());
                m.dispatch(
                    Event::ReadyToInsert {
                        session,
                        text: out.text,
                        style_name: applied.then_some(style_name),
                        enhance_error: name,
                    },
                    cx,
                );
            });
        })
        .detach();
    }

    fn insert(&mut self, session: SessionId, text: String, cx: &mut Context<Self>) {
        let target = self.session(session).app.clone();
        let typed = target.as_ref().is_some_and(|a| self.config.insertion.type_in_apps.contains(&a.bundle_id));
        let method = if typed {
            InsertMethod::Type
        } else {
            InsertMethod::Paste {
                restore_clipboard: self.config.insertion.restore_clipboard,
                restore_delay: Duration::from_millis(self.config.insertion.restore_delay_ms),
            }
        };
        self.session(session).final_text = Some(text.clone());
        self.last_text = Some(text.clone());
        self.last_app = target.as_ref().map(|a| a.name.clone());
        let inserter = self.services.platform.inserter.clone();
        log::debug!(
            "insert into {:?}; frontmost now {:?}",
            target.as_ref().map(|a| &a.bundle_id),
            self.services.platform.context.frontmost_app().map(|a| a.bundle_id)
        );
        cx.spawn(async move |this, cx| {
            let t = text.clone();
            let result = cx.background_spawn(async move { inserter.insert(&t, method) }).await;
            let _ = this.update(cx, |m, cx| match result {
                Ok(r) => {
                    m.session(session).insert = Some(match r {
                        InsertResult::Pasted { clipboard_restored } => InsertOutcome::Pasted { clipboard_restored },
                        InsertResult::Typed => InsertOutcome::Typed,
                    });
                    m.dispatch(Event::Inserted { session }, cx);
                }
                Err(e) => {
                    let reason = match e {
                        InsertError::NoFocusedField => "No text field is focused".to_string(),
                        other => other.to_string(),
                    };
                    m.session(session).insert = Some(InsertOutcome::Failed { reason: reason.clone() });
                    m.dispatch(Event::InsertFailed { session, text, reason }, cx);
                }
            });
        })
        .detach();
    }

    /// Paste the last text into the focused app.
    pub fn paste_last(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.last_text.clone().or_else(|| self.recent.first().map(|e| e.final_text.clone())) else {
            return;
        };
        self.insert_text(text, cx);
    }

    /// Insert any text into the focused app (History "Insert", popover "Paste").
    pub fn insert_text(&mut self, text: String, cx: &mut Context<Self>) {
        let inserter = self.services.platform.inserter.clone();
        let method = InsertMethod::Paste {
            restore_clipboard: self.config.insertion.restore_clipboard,
            restore_delay: Duration::from_millis(self.config.insertion.restore_delay_ms),
        };
        cx.spawn(async move |this, cx| {
            // Give a click in our own window time to hand focus back.
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let result = cx.background_spawn(async move { inserter.insert(&text, method) }).await;
            if let Err(e) = result {
                let _ = this.update(cx, |m, cx| m.notice(e.to_string(), cx));
            }
        })
        .detach();
    }

    fn save_history(&mut self, session: SessionId, cx: &mut Context<Self>) {
        let Some(store) = self.services.store.clone() else { return };
        let Some(s) = self.sessions.remove(&session) else { return };
        let samples = s.samples.lock().clone();
        let save_audio = self.config.history.save_audio && !samples.is_empty();
        let model = s.model.clone().unwrap_or_else(|| self.config.dictation.model.clone());
        let transcript = s.transcript.clone().unwrap_or_default();
        let entry = HistoryEntry {
            id: 0,
            created_at: chrono::Utc::now(),
            duration_ms: s.duration_ms,
            app: s.app.map(|a| TargetApp { bundle_id: a.bundle_id, name: a.name }),
            final_text: s.final_text.clone().unwrap_or_else(|| transcript.clone()),
            transcript,
            style_id: if s.style_id.is_empty() { self.config.ai.active_style.clone() } else { s.style_id },
            model,
            transcribe_ms: s.transcribe_ms,
            replacements: s.replacements,
            enhance: s.enhance.unwrap_or(EnhanceOutcome::NotUsed),
            insert: s.insert.unwrap_or(InsertOutcome::NotInserted),
            audio_file: None,
            waveform: waveform_summary(&samples, 96),
        };
        cx.spawn(async move |this, cx| {
            cx.background_spawn(async move {
                let mut entry = entry;
                if save_audio {
                    match store.save_audio(&samples) {
                        Ok(name) => entry.audio_file = Some(name),
                        Err(e) => log::warn!("could not save audio: {e}"),
                    }
                }
                if let Err(e) = store.insert_entry(&entry) {
                    log::error!("could not save history: {e}");
                }
                let _ = store.record_replacement_uses(&entry.replacements);
            })
            .await;
            let _ = this.update(cx, |m, cx| {
                m.reload_history();
                m.reload_dictionary();
                cx.notify();
            });
        })
        .detach();
    }

    /// Transcribe a history entry again from its saved audio.
    pub fn retranscribe_entry(&mut self, entry: HistoryEntry, cx: &mut Context<Self>) {
        let (Some(store), Some(file)) = (self.services.store.clone(), entry.audio_file.clone()) else {
            return;
        };
        let pass = match self.final_pass() {
            Ok(pass) => pass,
            Err(message) => {
                self.notice(message, cx);
                return;
            }
        };
        let options = self.session_options();
        let replacer = self.replacer.clone();
        cx.spawn(async move |this, cx| {
            cx.background_spawn(async move {
                let samples = store.load_audio(&file).ok()?;
                let (t, _) = pass.run(&samples, &options).inspect_err(|e| log::warn!("transcribe again: {e}")).ok()?;
                let (text, applied) = replacer.apply(t.text.trim());
                let mut e = entry;
                e.transcript = t.text;
                e.final_text = text;
                e.replacements = applied;
                e.model = t.model;
                e.transcribe_ms = t.elapsed_ms;
                e.enhance = EnhanceOutcome::NotUsed;
                store.update_entry(&e).ok()
            })
            .await;
            let _ = this.update(cx, |m, cx| {
                m.reload_history();
                cx.notify();
            });
        })
        .detach();
    }

    /// Run the active style on a history entry again ("Enhance").
    pub fn enhance_entry(&mut self, entry: HistoryEntry, cx: &mut Context<Self>) {
        let style = self.active_style();
        let Some(provider) = self.provider_for(&style).cloned() else {
            self.notice("Set up an AI provider in Styles first".into(), cx);
            return;
        };
        if provider.needs_model() {
            self.notice(format!("Choose a model for {} in Styles first", provider.name), cx);
            return;
        }
        let Some(store) = self.services.store.clone() else { return };
        let ai = self.config.ai.clone();
        let secrets = self.services.secrets.clone();
        let vocabulary = self.vocabulary();
        let timeout = Duration::from_millis(ai.http_timeout_ms.max(ai.cli_timeout_ms));
        cx.spawn(async move |this, cx| {
            let ok = cx
                .background_spawn(async move {
                    let enhancer = sayso_enhance::build_enhancer(&provider, secrets.as_ref(), &ai);
                    let replacer = sayso_core::dictionary::Replacer::new(&[]);
                    let p = TextPipeline { replacer: &replacer, style: &style, vocabulary: &vocabulary, enhancer: Some(enhancer.as_ref()), timeout };
                    let out = p.run_style(entry.final_text.clone(), entry.replacements.clone());
                    let ok = out.enhance_error.is_none();
                    let mut e = entry;
                    e.final_text = out.text;
                    e.enhance = out.enhance;
                    e.style_id = style.id.clone();
                    let _ = store.update_entry(&e);
                    ok
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                if !ok {
                    m.notice("The style could not run. The text did not change.".into(), cx);
                }
                m.reload_history();
                cx.notify();
            });
        })
        .detach();
    }

    fn on_hotkey(&mut self, event: HotkeyEvent, cx: &mut Context<Self>) {
        match event {
            HotkeyEvent::Toggle => self.dispatch(Event::Toggle, cx),
            HotkeyEvent::PushToTalkDown => self.dispatch(Event::PushToTalkDown, cx),
            HotkeyEvent::PushToTalkUp => self.dispatch(Event::PushToTalkUp, cx),
            HotkeyEvent::Cancel => self.dispatch(Event::Cancel, cx),
            HotkeyEvent::PasteLast => self.paste_last(cx),
            HotkeyEvent::CycleStyle => self.cycle_style(cx),
        }
    }

    fn on_engine_event(&mut self, event: EngineEvent, cx: &mut Context<Self>) {
        match event {
            EngineEvent::ModelStatus { model, status } => {
                let became_downloaded = matches!(status, ModelStatus::Downloaded)
                    && !matches!(self.model_status.get(&model), Some(ModelStatus::Ready | ModelStatus::Optimizing));
                self.model_status.insert(model.clone(), status);
                if became_downloaded && (model == self.config.dictation.model || Some(&model) == self.preview_model().as_ref()) {
                    self.load_active_models(cx);
                }
            }
            EngineEvent::Partial { session, committed, tentative } => {
                if matches!(self.machine.state, State::Recording { session: s, .. } if s == session) {
                    self.preview = (committed, tentative);
                }
            }
            EngineEvent::Crashed { message } => {
                log::error!("engine crashed: {message}");
                self.engine_down = Some(message);
            }
            EngineEvent::Restarted => {
                self.engine_down = None;
                self.load_active_models(cx);
            }
            EngineEvent::Log { level, message } => match level.as_str() {
                "error" => log::error!(target: "engine", "{message}"),
                "warn" | "warning" => log::warn!(target: "engine", "{message}"),
                _ => log::debug!(target: "engine", "{message}"),
            },
        }
        cx.notify();
    }

    /// Re-read config.toml and the styles folder when they change on disk.
    fn check_files(&mut self, cx: &mut Context<Self>) {
        let path = self.paths.config_file();
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if mtime != self.config_mtime {
            self.config_mtime = mtime;
            if let Ok(loaded) = sayso_core::config::Config::load(&path)
                && loaded.config != self.config {
                    let before = std::mem::replace(&mut self.config, loaded.config);
                    self.config_issues = loaded.issues;
                    self.apply_config_change(&before, cx);
                    cx.notify();
                }
        }
        let styles = sayso_core::style::StyleLibrary::load(&self.paths.styles_dir());
        if styles.entries != self.styles.entries || styles.errors != self.styles.errors {
            self.styles = styles;
            cx.notify();
        }
    }

    fn apply_retention(&mut self) {
        let Some(store) = self.services.store.clone() else { return };
        let h = &self.config.history;
        let audio_days = if h.save_audio { h.keep_audio_days } else { Some(0) };
        match store.apply_retention(h.keep_text_days, audio_days, chrono::Utc::now()) {
            Ok(r) => log::info!("retention: {r:?}"),
            Err(e) => log::warn!("retention: {e}"),
        }
    }
}

/// The backend for one final pass, with everything a background thread needs.
enum FinalPass {
    Local {
        engine: Arc<dyn SttBackend>,
        model: sayso_core::models::ModelId,
    },
    Cloud {
        provider: SpeechProvider,
        /// The provider's model id.
        model: String,
        info: Box<ModelInfo>,
        secrets: Arc<dyn SecretStore>,
        timeout: Duration,
        /// A local model that is in memory, with the engine that runs it.
        fallback: Option<(Arc<dyn SttBackend>, Box<ModelInfo>)>,
    },
}

impl FinalPass {
    /// Run the final pass. Blocks. The second value is a notice for the user,
    /// set when a local model replaced a cloud provider that failed.
    fn run(&self, samples: &[f32], options: &SessionOptions) -> Result<(Transcript, Option<String>), String> {
        match self {
            FinalPass::Local { engine, model } => {
                // Wait for the model: the first load compiles it for the Neural Engine.
                let deadline = Instant::now() + Duration::from_secs(300);
                loop {
                    match engine.status(model) {
                        ModelStatus::Ready => break,
                        ModelStatus::Failed { message } => return Err(message),
                        _ if Instant::now() > deadline => return Err("The model did not load in time.".into()),
                        _ => std::thread::sleep(Duration::from_millis(150)),
                    }
                }
                engine.transcribe(samples, options).map(|t| (t, None)).map_err(|e| e.to_string())
            }
            FinalPass::Cloud { provider, model, info, secrets, timeout, fallback } => {
                let started = Instant::now();
                let key = provider.api_key_account.as_deref().and_then(|account| secrets.get(account));
                let language = info.language_for(&options.language);
                let vocabulary: &[String] = if info.vocabulary { &options.vocabulary } else { &[] };
                let request = SpeechRequest {
                    model,
                    wav: &encode_wav(&to_pcm16(samples)),
                    language: (language != sayso_core::languages::AUTO).then_some(language.as_str()),
                    vocabulary,
                    timeout: *timeout,
                };
                match sayso_transcribe::transcribe(provider, key.as_deref(), &request) {
                    Ok(text) => {
                        let elapsed_ms = started.elapsed().as_millis() as u64;
                        Ok((Transcript { text, model: info.id.clone(), elapsed_ms }, None))
                    }
                    Err(e) => {
                        log::warn!("cloud final pass with {} failed: {e}", provider.id);
                        let message = e.user_message(&provider.name);
                        let Some((engine, local)) = fallback else { return Err(message) };
                        let options = SessionOptions { final_model: local.id.clone(), ..options.clone() };
                        let t = engine.transcribe(samples, &options).map_err(|_| message)?;
                        Ok((t, Some(format!("{} did not answer. Sayso used {}.", provider.name, local.name))))
                    }
                }
            }
        }
    }
}

fn provider_display(m: &AppModel, error: Option<&str>) -> Option<String> {
    let error = error?;
    // "openrouter timed out after 4 s" → "OpenRouter timed out after 4 s"
    let mut parts = error.splitn(2, ' ');
    let id = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");
    let name = m.config.ai.providers.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_else(|| id.to_string());
    Some(format!("{name} {rest}"))
}

/// Forward a blocking crossbeam receiver into an async channel, on its own thread.
fn forward<T: Send + 'static>(rx: crossbeam_channel::Receiver<T>) -> async_channel::Receiver<T> {
    let (tx, arx) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("sayso-forward".into())
        .spawn(move || {
            while let Ok(v) = rx.recv() {
                if tx.send_blocking(v).is_err() {
                    break;
                }
            }
        })
        .ok();
    arx
}

/// The model's event sources and timers.
///
/// Hotkey and engine events wake the UI only when they arrive. The state
/// machine clock runs every 100 ms only while a dictation is active.
/// Permissions, config files, and the system appearance are checked every
/// 1.5 s, and retention runs hourly.
pub fn start_background_loops(
    hotkeys: crossbeam_channel::Receiver<HotkeyEvent>,
    engine: Option<crossbeam_channel::Receiver<EngineEvent>>,
    cx: &mut Context<AppModel>,
) {
    let hotkeys = forward(hotkeys);
    cx.spawn(async move |this, cx| {
        while let Ok(ev) = hotkeys.recv().await {
            if this.update(cx, |m, cx| m.on_hotkey(ev, cx)).is_err() {
                break;
            }
        }
    })
    .detach();
    if let Some(engine) = engine {
        let engine = forward(engine);
        cx.spawn(async move |this, cx| {
            while let Ok(ev) = engine.recv().await {
                if this.update(cx, |m, cx| m.on_engine_event(ev, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }
    // The dictation clock.
    cx.spawn(async move |this, cx| {
        loop {
            let active = this
                .update(cx, |m, cx| {
                    if m.blocked_notice.as_ref().is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(3)) {
                        m.blocked_notice = None;
                        cx.notify();
                    }
                    let active = !matches!(m.machine.state, State::Idle);
                    if active {
                        m.dispatch(Event::Tick, cx);
                    }
                    active || m.blocked_notice.is_some()
                })
                .ok();
            let Some(active) = active else { break };
            cx.background_executor().timer(Duration::from_millis(if active { 100 } else { 400 })).await;
        }
    })
    .detach();
    // Slow checks.
    cx.spawn(async move |this, cx| {
        let _ = this.update(cx, |m, _| {
            m.register_hotkeys();
            m.apply_retention();
        });
        let mut n = 0u64;
        loop {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            n += 1;
            let ok = this
                .update(cx, |m, cx| {
                    if m.refresh_permissions() {
                        cx.notify();
                    }
                    m.check_files(cx);
                    let dark = m.services.platform.prefs.dark_mode();
                    if dark != m.system_dark {
                        m.system_dark = dark;
                        if m.config.appearance.theme == sayso_core::config::ThemeMode::System {
                            crate::app::apply_theme(&m.config, &m.services, cx);
                            cx.notify();
                        }
                    }
                    if n.is_multiple_of(2400) {
                        m.apply_retention();
                    }
                })
                .is_ok();
            if !ok {
                break;
            }
        }
    })
    .detach();
}
