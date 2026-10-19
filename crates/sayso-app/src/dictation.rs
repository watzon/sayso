//! The dictation controller: runs the effects of `sayso_core::dictation`.
//!
//! The state machine decides what happens. This file does it: microphone,
//! engine, pipeline, insertion, history, sounds.

use crate::model::{AppEvent, AppModel, NoticeIcon, OverlayNotice};
use gpui_kit::*;
use sayso_i18n::t;
use parking_lot::Mutex;
use sayso_core::dictation::{Effect, Event, SessionId, Sound, State, Trigger};
use sayso_core::dictionary::AppliedReplacement;
use sayso_core::enhance::EnhanceError;
use sayso_core::history::{
    Diagnostics, EnhanceOutcome, HistoryEntry, InsertOutcome, InsertTimings, TargetApp, TranscribeTimings, waveform_summary,
};
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Per-dictation data the state machine does not hold.
#[derive(Default)]
pub struct Session {
    pub app: Option<AppInfo>,
    /// The host of the page in the app, when the app showed a web page.
    pub site: Option<String>,
    /// The style that a style rule chose for the app or the site.
    pub rule_style: Option<String>,
    /// The style from the next-style hotkey during the recording. It wins
    /// over the rule and applies to this dictation only.
    pub picked_style: Option<String>,
    pub samples: Arc<Mutex<Vec<f32>>>,
    pub capture: Option<Box<dyn CaptureHandle>>,
    /// True while the live preview gets the audio. It can start after the
    /// capture, when the model of the preview loads during the dictation.
    pub streaming: Arc<AtomicBool>,
    pub started: Option<Instant>,
    /// When the recording stopped. The diagnostics count from here.
    pub stopped: Option<Instant>,
    pub duration_ms: u64,
    pub transcript: Option<String>,
    pub transcribe_ms: u64,
    pub model: Option<sayso_core::models::ModelId>,
    pub replacements: Vec<AppliedReplacement>,
    pub style_id: String,
    pub enhance: Option<EnhanceOutcome>,
    pub final_text: Option<String>,
    pub insert: Option<InsertOutcome>,
    /// Press Enter after the insert (`--push-to-talk-up-submit`).
    pub submit: bool,
    /// Incognito was on when the dictation started.
    pub incognito: bool,
    /// The time of each step so far.
    pub diagnostics: Diagnostics,
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
            // The resume runs also when the setting went off during the
            // recording: it plays only what the pause stopped.
            if !recording {
                self.media.resume();
            } else if self.config.audio.pause_media {
                self.media.pause();
            }
        }
        cx.notify();
    }

    /// Check what a dictation needs. Shows a notice and returns false when something is missing.
    fn check_can_start(&mut self, cx: &mut Context<Self>) -> bool {
        match self.permission(Permission::Microphone) {
            _ if crate::dev::fake_mic().is_some() => {}
            PermissionState::Granted => {}
            PermissionState::NotDetermined => {
                self.request_permission(Permission::Microphone);
                self.notice(NoticeIcon::Microphone, t!("overlay.notice.allow_microphone"), cx);
                return false;
            }
            PermissionState::Denied => {
                self.open_permission_settings(Permission::Microphone);
                self.notice(NoticeIcon::Microphone, t!("overlay.notice.needs_microphone"), cx);
                return false;
            }
        }
        let model = self.active_model();
        if model.is_remote() {
            if !self.status_of(&model.id).is_usable() {
                self.notice(NoticeIcon::Info, t!("overlay.notice.set_up_cloud_provider"), cx);
                cx.emit(AppEvent::OpenHub(crate::hub::Route::Models));
                return false;
            }
            return true;
        }
        if self.services.engine.is_none() {
            self.notice(NoticeIcon::Info, t!("overlay.notice.no_engine"), cx);
            return false;
        }
        if !self.status_of(&model.id).is_on_disk() {
            self.notice(NoticeIcon::Info, t!("overlay.notice.download_model"), cx);
            cx.emit(AppEvent::OpenHub(crate::hub::Route::Models));
            return false;
        }
        true
    }

    fn notice(&mut self, icon: NoticeIcon, title: String, cx: &mut Context<Self>) {
        self.blocked_notice = Some(OverlayNotice::new(icon, title));
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
                    if s.streaming.load(Ordering::Relaxed)
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
        // The model loads while the microphone records, if the idle time unloaded it.
        self.wake_model(cx);
        let streaming = Arc::new(AtomicBool::new(self.start_preview(session)));
        let samples = Arc::new(Mutex::new(Vec::<f32>::with_capacity(SAMPLE_RATE as usize * 30)));
        let live = self.live.clone();
        let buf = samples.clone();
        let stream_engine = self.services.engine.clone();
        let stream_on = streaming.clone();
        let on_frame = Box::new(move |frame: sayso_platform::AudioFrame| {
            live.push(frame.level);
            if stream_on.load(Ordering::Relaxed)
                && let Some(e) = &stream_engine
            {
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
                // Show why in the overlay. A cancel would show "Cancelled"
                // and offer an Undo with no audio.
                let message = t!("overlay.notice.microphone_failed");
                cx.spawn(async move |this, cx| {
                    let _ = this.update(cx, |m, cx| m.dispatch(Event::CaptureFailed { session, message }, cx));
                })
                .detach();
                None
            }
        };
        let incognito = self.incognito;
        let rule_style = self.config.ai.style_rules.resolve(app.as_ref().map(|a| a.bundle_id.as_str()), None).map(str::to_string);
        self.read_page(session, app.as_ref(), cx);
        let s = self.session(session);
        s.incognito = incognito;
        s.rule_style = rule_style;
        s.app = app;
        s.samples = samples;
        s.capture = capture;
        s.streaming = streaming;
        s.started = Some(Instant::now());
    }

    /// Ask the app for its web page: a site rule can choose the style, and
    /// the history entry keeps the host. The answer comes while the
    /// microphone records. Sayso asks only when a style rule or a history
    /// entry can use the answer.
    fn read_page(&mut self, session: SessionId, app: Option<&AppInfo>, cx: &mut Context<Self>) {
        let context = self.services.platform.context.clone();
        let Some(app) = app.filter(|a| a.pid != std::process::id() as i32).cloned() else { return };
        let saves = saves_history(self.config.history.enabled, self.incognito, self.incognito);
        if !(self.config.ai.style_rules.enabled || saves) || !context.reads_page_url() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let page = app.clone();
            let Some(url) = cx.background_spawn(async move { context.page_url(&page) }).await else { return };
            let _ = this.update(cx, |m, cx| {
                let style = m.config.ai.style_rules.resolve(Some(&app.bundle_id), Some(&url)).map(str::to_string);
                if let Some(s) = m.sessions.get_mut(&session) {
                    s.site = sayso_core::style_rules::host_of(&url);
                    s.rule_style = style;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Open the live preview stream of a dictation. False when the preview is
    /// off or its model is not in memory. After an idle unload, the engine can
    /// have the live preview of the active model in memory without its final pass.
    fn start_preview(&self, session: SessionId) -> bool {
        let options = self.session_options();
        match (&self.services.engine, &options.preview_model) {
            (Some(e), Some(p)) if self.config.overlay.show_preview && (self.status_of(p).is_usable() || e.preview_ready(p)) => {
                e.start_stream(session, &options).is_ok()
            }
            _ => false,
        }
    }

    /// `model` is in memory now. If the dictation that records now waits for
    /// it, its live preview starts here, with the audio from this moment on.
    pub(crate) fn start_late_preview(&mut self, model: &sayso_core::models::ModelId) {
        let State::Recording { session, .. } = self.machine.state else { return };
        if self.preview_model().as_ref() != Some(model) {
            return;
        }
        let Some(streaming) = self.sessions.get(&session).map(|s| s.streaming.clone()) else { return };
        if !streaming.load(Ordering::Relaxed) && self.start_preview(session) {
            streaming.store(true, Ordering::Relaxed);
        }
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
        s.stopped = Some(Instant::now());
        let streamed = s.streaming.swap(false, Ordering::Relaxed);
        s.diagnostics.transcribe.live_preview = streamed;
        if streamed
            && let Some(e) = engine
        {
            e.end_stream(session);
        }
    }

    /// How the final pass of the active model runs. None, with the reason, when it cannot run.
    fn final_pass(&self) -> Result<FinalPass, String> {
        let info = self.active_model();
        if let Some((provider, model)) = split_model_id(&info.id) {
            let provider = self.config.speech.provider(provider).cloned().ok_or_else(|| t!("overlay.error.cloud_provider_not_set_up"))?;
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
        let engine = self.services.engine.clone().ok_or_else(|| t!("overlay.error.engine_not_running"))?;
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
        let spoken_punctuation = self.config.dictation.spoken_punctuation;
        if matches!(pass, FinalPass::Local { .. }) && !self.status_of(&options.final_model).is_usable() {
            self.load_active_models(cx);
        }
        let cloud = matches!(pass, FinalPass::Cloud { .. });
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let started = Instant::now();
                    let Passed { transcript: t, note, model_wait_ms } = pass.run(&samples, &options)?;
                    let (replaced, applied) = replacer.apply(t.text.trim());
                    let replaced = if spoken_punctuation { sayso_core::spoken_punctuation::apply(&replaced) } else { replaced };
                    let timings = TranscribeTimings {
                        step_ms: started.elapsed().as_millis() as u64,
                        model_wait_ms,
                        cloud,
                        // The note says that a local model took over.
                        fallback: note.is_some(),
                        language: options.language,
                        live_preview: false,
                    };
                    Ok::<_, String>((t, replaced, applied, note, timings))
                })
                .await;
            let _ = this.update(cx, |m, cx| match result {
                Ok((t, replaced, applied, note, timings)) => {
                    let s = m.session(session);
                    s.transcript = Some(t.text.clone());
                    s.transcribe_ms = t.elapsed_ms;
                    s.model = Some(t.model.clone());
                    s.replacements = applied;
                    s.diagnostics.transcribe = TranscribeTimings { live_preview: s.diagnostics.transcribe.live_preview, ..timings };
                    if let Some(note) = note {
                        m.notice(NoticeIcon::Info, note, cx);
                    }
                    m.dispatch(Event::Transcribed { session, text: replaced }, cx);
                }
                Err(message) => m.dispatch(Event::TranscribeFailed { session, message }, cx),
            });
        })
        .detach();
    }

    fn apply_style(&mut self, session: SessionId, text: String, cx: &mut Context<Self>) {
        let style = self.session_style(session);
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
            let enhance_error = Some(t!("overlay.error.no_model_chosen", provider = provider.id));
            self.dispatch(Event::ReadyToInsert { session, text, style_name: None, enhance_error }, cx);
            return;
        }
        self.dispatch(Event::EnhanceStarted { session }, cx);
        let ai = self.config.ai.clone();
        let secrets = self.services.secrets.clone();
        let engine = self.services.language_model.clone();
        let vocabulary = self.vocabulary();
        let base_prompt = self.styles.base_prompt().to_string();
        let replacements = self.session(session).replacements.clone();
        let timeout = Duration::from_millis(match provider.kind {
            // The language model of the engine answers as fast as a server.
            sayso_core::config::ProviderKind::OpenAiCompatible { .. } | sayso_core::config::ProviderKind::AppleIntelligence => {
                ai.http_timeout_ms
            }
            _ => ai.cli_timeout_ms,
        });
        let style_name = style.display_name();
        cx.spawn(async move |this, cx| {
            let out = cx
                .background_spawn(async move {
                    let started = Instant::now();
                    let enhancer = sayso_enhance::build_enhancer(&provider, secrets.as_ref(), &ai, engine);
                    let setup_ms = started.elapsed().as_millis() as u64;
                    let replacer = sayso_core::dictionary::Replacer::new(&[]);
                    let p = TextPipeline { replacer: &replacer, style: &style, base_prompt: &base_prompt, vocabulary: &vocabulary, enhancer: Some(enhancer.as_ref()), timeout };
                    let mut out = p.run_style(text, replacements);
                    if let Some(timings) = &mut out.timings {
                        timings.step_ms = started.elapsed().as_millis() as u64;
                        timings.setup_ms = setup_ms;
                        timings.provider_kind = provider.kind.name().into();
                        timings.thinking = provider.thinking_sent().map(str::to_string);
                    }
                    out
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                let applied = matches!(out.enhance, EnhanceOutcome::Applied { .. });
                m.session(session).enhance = Some(out.enhance.clone());
                m.session(session).diagnostics.enhance = out.timings.clone();
                let name = out.enhance_error.as_ref().map(|(id, error)| style_error_text(&m.provider_name(id), error));
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
        // History and the failure notice keep the text without the trailing space.
        let to_insert = self.config.insertion.text_to_insert(&text);
        let inserter = self.services.platform.inserter.clone();
        let submit = self.session(session).submit;
        log::debug!(
            "insert into {:?}; frontmost now {:?}",
            target.as_ref().map(|a| &a.bundle_id),
            self.services.platform.context.frontmost_app().map(|a| a.bundle_id)
        );
        cx.spawn(async move |this, cx| {
            let t = to_insert;
            let (result, timings) = cx
                .background_spawn(async move {
                    let started = Instant::now();
                    let result = if submit { inserter.insert_and_submit(&t, method) } else { inserter.insert(&t, method) };
                    let timings = InsertTimings {
                        elapsed_ms: started.elapsed().as_millis() as u64,
                        method: if typed { "type" } else { "paste" }.into(),
                        restore_delay_ms: match method {
                            InsertMethod::Paste { restore_clipboard: true, restore_delay } => Some(restore_delay.as_millis() as u64),
                            _ => None,
                        },
                        backend: inserter.backend(),
                    };
                    (result, timings)
                })
                .await;
            let _ = this.update(cx, |m, cx| match result {
                Ok(r) => {
                    let s = m.session(session);
                    s.insert = Some(match r {
                        InsertResult::Pasted { clipboard_restored } => InsertOutcome::Pasted { clipboard_restored },
                        InsertResult::Typed => InsertOutcome::Typed,
                    });
                    s.diagnostics.insert = Some(timings);
                    s.diagnostics.stop_to_insert_ms = s.stopped.map(|t| t.elapsed().as_millis() as u64);
                    m.dispatch(Event::Inserted { session }, cx);
                }
                Err(e) => {
                    m.session(session).diagnostics.insert = Some(timings);
                    // The diagnostics keep the English reason, so a bug report reads the same in each language.
                    let stored = match &e {
                        InsertError::NoFocusedField => "No text field is focused".to_string(),
                        other => other.to_string(),
                    };
                    let reason = insert_error_text(&e);
                    m.session(session).insert = Some(InsertOutcome::Failed { reason: stored });
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
        let text = self.config.insertion.text_to_insert(&text);
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
                let _ = this.update(cx, |m, cx| m.notice(NoticeIcon::Info, insert_error_text(&e), cx));
            }
        })
        .detach();
    }

    fn save_history(&mut self, session: SessionId, cx: &mut Context<Self>) {
        let Some(store) = self.services.store.clone() else { return };
        let Some(s) = self.sessions.remove(&session) else { return };
        if !saves_history(self.config.history.enabled, s.incognito, self.incognito) {
            // No entry, no audio, and no replacement counts.
            return;
        }
        // The session is gone from the map, so nothing reads its recording again.
        let samples = std::mem::take(&mut *s.samples.lock());
        let save_audio = self.config.history.save_audio && !samples.is_empty();
        let model = s.model.clone().unwrap_or_else(|| self.config.dictation.model.clone());
        let transcript = s.transcript.clone().unwrap_or_default();
        let entry = HistoryEntry {
            id: 0,
            created_at: chrono::Utc::now(),
            duration_ms: s.duration_ms,
            app: s.app.map(|a| TargetApp { bundle_id: a.bundle_id, name: a.name, site: s.site }),
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
            diagnostics: Some(Diagnostics { app_version: env!("CARGO_PKG_VERSION").into(), ..s.diagnostics }),
        };
        cx.spawn(async move |this, cx| {
            cx.background_spawn(async move {
                let mut entry = entry;
                if let Some(diagnostics) = &mut entry.diagnostics {
                    diagnostics.system = crate::diagnostics::system();
                }
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
                self.notice(NoticeIcon::Info, message, cx);
                return;
            }
        };
        let options = self.session_options();
        let replacer = self.replacer.clone();
        let spoken_punctuation = self.config.dictation.spoken_punctuation;
        // The final pass waits for the model, if the idle time unloaded it.
        self.wake_model(cx);
        cx.spawn(async move |this, cx| {
            cx.background_spawn(async move {
                let samples = store.load_audio(&file).ok()?;
                let t = pass.run(&samples, &options).inspect_err(|e| log::warn!("transcribe again: {e}")).ok()?.transcript;
                let (text, applied) = replacer.apply(t.text.trim());
                let text = if spoken_punctuation { sayso_core::spoken_punctuation::apply(&text) } else { text };
                let mut e = entry;
                e.transcript = t.text;
                e.final_text = text;
                e.replacements = applied;
                e.model = t.model;
                e.transcribe_ms = t.elapsed_ms;
                e.enhance = EnhanceOutcome::NotUsed;
                if let Some(diagnostics) = &mut e.diagnostics {
                    diagnostics.changed_later = true;
                }
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
            self.notice(NoticeIcon::Info, t!("overlay.notice.set_up_ai_provider"), cx);
            return;
        };
        if provider.needs_model() {
            self.notice(NoticeIcon::Info, t!("overlay.notice.choose_ai_model", provider = provider.name), cx);
            return;
        }
        let Some(store) = self.services.store.clone() else { return };
        let ai = self.config.ai.clone();
        let secrets = self.services.secrets.clone();
        let engine = self.services.language_model.clone();
        let vocabulary = self.vocabulary();
        let base_prompt = self.styles.base_prompt().to_string();
        let timeout = Duration::from_millis(ai.http_timeout_ms.max(ai.cli_timeout_ms));
        cx.spawn(async move |this, cx| {
            let ok = cx
                .background_spawn(async move {
                    let enhancer = sayso_enhance::build_enhancer(&provider, secrets.as_ref(), &ai, engine);
                    let replacer = sayso_core::dictionary::Replacer::new(&[]);
                    let p = TextPipeline { replacer: &replacer, style: &style, base_prompt: &base_prompt, vocabulary: &vocabulary, enhancer: Some(enhancer.as_ref()), timeout };
                    let out = p.run_style(entry.final_text.clone(), entry.replacements.clone());
                    let ok = out.enhance_error.is_none();
                    let mut e = entry;
                    e.final_text = out.text;
                    e.enhance = out.enhance;
                    e.style_id = style.id.clone();
                    if let Some(diagnostics) = &mut e.diagnostics {
                        diagnostics.changed_later = true;
                    }
                    let _ = store.update_entry(&e);
                    ok
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                if !ok {
                    m.notice(NoticeIcon::Style, t!("overlay.notice.style_failed"), cx);
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
            HotkeyEvent::PushToTalkUpSubmit => {
                if let State::Recording { session, .. } = self.machine.state {
                    self.session(session).submit = true;
                }
                self.dispatch(Event::PushToTalkUp, cx)
            }
            HotkeyEvent::Submit => {
                if let State::Recording { session, trigger, .. } = self.machine.state {
                    self.session(session).submit = true;
                    // Stop the way the trigger of the dictation stops it.
                    let stop = match trigger {
                        Trigger::Toggle => Event::Toggle,
                        Trigger::PushToTalk => Event::PushToTalkUp,
                    };
                    self.dispatch(stop, cx)
                }
            }
            HotkeyEvent::Cancel => self.dispatch(Event::Cancel, cx),
            HotkeyEvent::PasteLast => self.paste_last(cx),
            HotkeyEvent::CycleStyle => self.cycle_style(cx),
            HotkeyEvent::ToggleIncognito => self.toggle_incognito(cx),
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
        if styles.entries != self.styles.entries
            || styles.errors != self.styles.errors
            || styles.user_base_prompt != self.styles.user_base_prompt
        {
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

/// True when a dictation becomes a history entry: history is on, and
/// incognito was off both when the dictation started and when it ended.
fn saves_history(history_enabled: bool, incognito_at_start: bool, incognito_at_end: bool) -> bool {
    history_enabled && !incognito_at_start && !incognito_at_end
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

/// The result of a final pass.
struct Passed {
    transcript: Transcript,
    /// A notice for the user, set when a local model replaced a cloud provider that failed.
    note: Option<String>,
    /// The wait until the local model was in memory.
    model_wait_ms: u64,
}

impl FinalPass {
    /// Run the final pass. Blocks.
    fn run(&self, samples: &[f32], options: &SessionOptions) -> Result<Passed, String> {
        match self {
            FinalPass::Local { engine, model } => {
                // Wait for the model: the first load compiles it for the Neural Engine.
                let wait_started = Instant::now();
                match engine.wait_ready(model, wait_started + Duration::from_secs(300)) {
                    ModelStatus::Ready => {}
                    ModelStatus::Failed { message, reason } => return Err(crate::engine_text::describe(reason, &message)),
                    _ => return Err(t!("overlay.error.model_load_timeout")),
                }
                let model_wait_ms = wait_started.elapsed().as_millis() as u64;
                let transcript = engine.transcribe(samples, options).map_err(|e| e.to_string())?;
                Ok(Passed { transcript, note: None, model_wait_ms })
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
                        Ok(Passed { transcript: Transcript { text, model: info.id.clone(), elapsed_ms }, note: None, model_wait_ms: 0 })
                    }
                    Err(e) => {
                        log::warn!("cloud final pass with {} failed: {e}", provider.id);
                        let message = e.user_message(&provider.name);
                        let Some((engine, local)) = fallback else { return Err(message) };
                        let options = SessionOptions { final_model: local.id.clone(), ..options.clone() };
                        let transcript = engine.transcribe(samples, &options).map_err(|_| message)?;
                        let note = Some(t!("overlay.notice.provider_fallback", provider = provider.name, local = local.name));
                        Ok(Passed { transcript, note, model_wait_ms: 0 })
                    }
                }
            }
        }
    }
}

/// Why a style failed, for the overlay: "OpenRouter timed out after 4 s".
fn style_error_text(provider: &str, error: &EnhanceError) -> String {
    match error {
        EnhanceError::Timeout(ms) => t!("overlay.style_error.timeout", provider = provider, seconds = (*ms as f64 / 1000.0).round() as u64),
        EnhanceError::NotConfigured(_) => t!("overlay.style_error.not_configured", provider = provider),
        EnhanceError::Http { status, .. } => t!("overlay.style_error.http", provider = provider, status = status),
        EnhanceError::Network(_) => t!("overlay.style_error.network", provider = provider),
        EnhanceError::InvalidOutput(_) => t!("overlay.style_error.invalid_output", provider = provider),
        EnhanceError::Refused(_) => t!("overlay.style_error.refused", provider = provider),
        EnhanceError::Cli(_) => t!("overlay.style_error.failed", provider = provider),
    }
}

/// Why the text could not go in, in the interface language. The text of
/// the system stays as it is.
fn insert_error_text(error: &InsertError) -> String {
    match error {
        InsertError::NoFocusedField => t!("overlay.error.no_focused_field"),
        InsertError::NotTaken => t!("overlay.error.not_taken"),
        InsertError::NotTrusted => t!("overlay.error.not_trusted"),
        InsertError::Elevated { app: Some(app) } => t!("overlay.error.elevated", app = app),
        InsertError::Elevated { app: None } => t!("overlay.error.elevated_unknown"),
        InsertError::Clipboard(error) => t!("overlay.error.clipboard", error = error),
        InsertError::Failed(detail) => detail.clone(),
    }
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
                    if m.blocked_notice.as_ref().is_some_and(|n| n.shown.elapsed() > Duration::from_secs(3)) {
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
                    m.unload_unused_models(cx);
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

#[cfg(test)]
mod tests {
    // No glob import: `gpui_kit` has a `test` attribute of its own.
    use super::{FinalPass, Mutex, Passed, saves_history};
    use sayso_core::models::{ModelId, ModelInfo};
    use sayso_core::stt::{EngineEvent, ModelStatus, SessionOptions, Transcript};
    use sayso_platform::{Result, SttBackend};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// An engine whose final-pass model is still loading. `transcribe` notes
    /// the model it ran and the status of that model at that moment.
    struct LoadingEngine {
        status: Mutex<ModelStatus>,
        calls: Mutex<Vec<(ModelId, ModelStatus)>>,
    }

    impl LoadingEngine {
        fn new(status: ModelStatus) -> Arc<Self> {
            Arc::new(LoadingEngine { status: Mutex::new(status), calls: Mutex::new(Vec::new()) })
        }
    }

    impl SttBackend for LoadingEngine {
        fn catalog(&self) -> Vec<ModelInfo> {
            Vec::new()
        }
        fn status(&self, _model: &ModelId) -> ModelStatus {
            self.status.lock().clone()
        }
        fn download(&self, _model: &ModelId) -> Result<()> {
            Ok(())
        }
        fn delete(&self, _model: &ModelId) -> Result<()> {
            Ok(())
        }
        fn load(&self, _model: &ModelId) -> Result<()> {
            Ok(())
        }
        fn unload(&self, _model: &ModelId, _keep_preview: bool) -> Result<()> {
            Ok(())
        }
        fn preview_ready(&self, _model: &ModelId) -> bool {
            false
        }
        fn start_stream(&self, _session: u64, _options: &SessionOptions) -> Result<()> {
            Ok(())
        }
        fn push_audio(&self, _session: u64, _samples: &[f32]) -> Result<()> {
            Ok(())
        }
        fn end_stream(&self, _session: u64) {}
        fn transcribe(&self, samples: &[f32], options: &SessionOptions) -> Result<Transcript> {
            self.calls.lock().push((options.final_model.clone(), self.status.lock().clone()));
            Ok(Transcript { text: format!("{} samples", samples.len()), model: options.final_model.clone(), elapsed_ms: 1 })
        }
        fn subscribe(&self) -> crossbeam_channel::Receiver<EngineEvent> {
            crossbeam_channel::unbounded().1
        }
    }

    fn options() -> SessionOptions {
        SessionOptions {
            final_model: ModelId::new("final"),
            preview_model: Some(ModelId::new("preview")),
            language: "en".into(),
            vocabulary: Vec::new(),
        }
    }

    #[test]
    fn a_dictation_that_stops_before_the_load_is_complete_waits_for_the_model() {
        // The idle time unloaded the model, and the dictation stopped during the load.
        let engine = LoadingEngine::new(ModelStatus::Downloaded);
        let loader = engine.clone();
        let load = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            *loader.status.lock() = ModelStatus::Optimizing;
            std::thread::sleep(Duration::from_millis(200));
            *loader.status.lock() = ModelStatus::Ready;
        });
        let pass = FinalPass::Local { engine: engine.clone(), model: ModelId::new("final") };
        let started = Instant::now();
        let Passed { transcript, note, model_wait_ms } = pass.run(&[0.0; 1600], &options()).unwrap();
        load.join().unwrap();

        assert!(started.elapsed() >= Duration::from_millis(300), "the final pass ran before the load was complete");
        // No text is lost, and the preview model did not replace the final-pass model.
        assert_eq!(transcript.text, "1600 samples");
        assert_eq!(transcript.model, ModelId::new("final"));
        assert_eq!(note, None);
        assert!(model_wait_ms >= 300, "the diagnostics do not have the wait for the model");
        assert_eq!(*engine.calls.lock(), [(ModelId::new("final"), ModelStatus::Ready)]);
    }

    #[test]
    fn a_load_that_fails_ends_the_final_pass_with_the_reason() {
        let engine = LoadingEngine::new(ModelStatus::Failed { message: "the model files are broken".into(), reason: Some(sayso_core::stt::EngineReason::LoadFailed) });
        let pass = FinalPass::Local { engine: engine.clone(), model: ModelId::new("final") };
        assert_eq!(pass.run(&[0.0; 1600], &options()).err().unwrap(), "Sayso could not load the model: the model files are broken.");
        assert!(engine.calls.lock().is_empty());
        // An engine without a reason code gives the general sentence with its message.
        let engine = LoadingEngine::new(ModelStatus::Failed { message: "the model files are broken".into(), reason: None });
        let pass = FinalPass::Local { engine: engine.clone(), model: ModelId::new("final") };
        assert_eq!(pass.run(&[0.0; 1600], &options()).err().unwrap(), "Something went wrong: the model files are broken.");
    }

    #[test]
    fn a_dictation_is_saved_only_with_history_on_and_incognito_off() {
        assert!(saves_history(true, false, false));
        assert!(!saves_history(false, false, false), "history is off");
        assert!(!saves_history(true, true, false), "incognito was on at the start");
        assert!(!saves_history(true, false, true), "incognito was on at the end");
    }
}
