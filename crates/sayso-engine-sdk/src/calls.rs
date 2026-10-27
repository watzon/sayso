//! One typed method for each request of the engine protocol.

use crate::wav::TempWav;
use crate::{Engine, Error, Result};
use sayso_protocol::engine::{self, CatalogModel, DiarizeReply, Download, GenerateReply, LanguageModelReply, ListModels, ListModelsReply, ListedModel, ModelRef, PartState, Prepare, Request, Retired, StreamAudio, StreamEnd, StreamEndReply, StreamStart, Transcribe, Unload, Vad, VadReply, VadStart};
use sayso_protocol::{DownloadProgress, Fields, ModelState, ModelStateEvent, SpeechPart, parse_ok, pcm};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A model by id. An id from the built-in catalog needs nothing more. A model
/// that the catalog does not have needs its `engine` object (see "Models" in
/// the spec).
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub id: String,
    pub engine: Option<Fields>,
}

impl Model {
    pub fn with_engine(id: impl Into<String>, engine: Fields) -> Model {
        Model { id: id.into(), engine: Some(engine) }
    }
}

impl From<&str> for Model {
    fn from(id: &str) -> Model {
        Model { id: id.to_owned(), engine: None }
    }
}

impl From<String> for Model {
    fn from(id: String) -> Model {
        Model { id, engine: None }
    }
}

impl From<&Model> for Model {
    fn from(model: &Model) -> Model {
        model.clone()
    }
}

impl From<Model> for ModelRef {
    fn from(model: Model) -> ModelRef {
        ModelRef { model: model.id, engine: model.engine }
    }
}

impl From<&Model> for ListedModel {
    fn from(model: &Model) -> ListedModel {
        ListedModel { id: model.id.clone(), engine: model.engine.clone() }
    }
}

/// The reply to [`Engine::list_models`]: the built-in catalog, and the state of each model on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct Catalog {
    pub models: Vec<CatalogModel>,
    /// Ids that left the catalog.
    pub retired: Vec<Retired>,
    /// The `model_state` events that came before the reply, one for each model.
    pub states: Vec<ModelStateEvent>,
}

/// The result of [`Engine::load`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loaded {
    /// The time of the load in milliseconds, from the `ready` event.
    pub load_ms: Option<u64>,
}

/// The result of [`Engine::transcribe`]: `text`, `elapsed_ms`, and `segments`
/// when you asked for them and the engine has the feature `segments`.
pub type Transcript = engine::Final;

/// The result of [`Engine::stream_end`]. A VAD session has no `text`.
pub type StreamEnded = StreamEndReply;

/// The settings of a `transcribe` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscribeOptions {
    /// A language code such as `en`, or `auto`.
    pub language: String,
    /// Words that the model should prefer. A model that cannot use them ignores them.
    pub vocabulary: Vec<String>,
    /// Ask for `segments` with times (feature `segments`).
    pub segments: bool,
}

impl Default for TranscribeOptions {
    fn default() -> TranscribeOptions {
        TranscribeOptions { language: "auto".into(), vocabulary: Vec::new(), segments: false }
    }
}

impl TranscribeOptions {
    pub fn with_segments(mut self) -> TranscribeOptions {
        self.segments = true;
        self
    }
}

fn transcribe_request(model: &str, path: &Path, options: &TranscribeOptions) -> Request {
    Request::Transcribe(Transcribe {
        model: model.to_owned(),
        path: path.to_owned(),
        language: options.language.clone(),
        vocabulary: options.vocabulary.clone(),
        // The field is there only when it is on: an engine without the feature never sees it.
        segments: options.segments.then_some(true),
    })
}

fn unload_request(model: &str, keep_preview: bool) -> Request {
    // The field is there only when it is on: an engine without the feature never sees it.
    Request::Unload(Unload { model: model.to_owned(), keep_preview: keep_preview.then_some(true) })
}

/// The typed body of an `ok` reply.
fn ok_body<T: serde::de::DeserializeOwned>(reply: engine::Reply, request: &str) -> Result<T> {
    match reply {
        engine::Reply::Ok(fields) => parse_ok(&fields).map_err(|e| Error::Protocol(format!("the engine sent an unexpected reply to {request}: {e}"))),
        other => Err(Error::Protocol(format!("the engine replied {} to {request}, and not ok", sayso_protocol::Kind::kind(&other)))),
    }
}

impl Engine {
    /// `list_models` without `models`: the built-in catalog (feature `catalog`)
    /// and the state of each model on disk.
    pub fn list_models(&self) -> Result<Catalog> {
        let mut states = Vec::new();
        let reply = self.run(&Request::ListModels(ListModels { models: None }), self.timeouts().control, |event| collect_state(event, &mut states))?;
        let body: ListModelsReply = ok_body(reply, "list_models")?;
        Ok(Catalog { models: body.models, retired: body.retired, states })
    }

    /// `list_models` with a list: the state on disk of these models. The
    /// engine remembers their `engine` objects for later requests.
    pub fn check_models(&self, models: &[Model]) -> Result<Vec<ModelStateEvent>> {
        let mut states = Vec::new();
        let request = Request::ListModels(ListModels { models: Some(models.iter().map(ListedModel::from).collect()) });
        self.run(&request, self.timeouts().control, |event| collect_state(event, &mut states))?;
        Ok(states)
    }

    /// Download a model. The progress events come on [`Engine::events`]. Use
    /// [`Engine::download_with`] to get them here.
    pub fn download(&self, model: impl Into<Model>) -> Result<()> {
        self.download_with(model, None, |_| {})
    }

    /// Download a model and call `on_progress` for each `download_progress`
    /// event, in the thread of the caller. `size_bytes` helps an engine that
    /// knows only a fraction.
    pub fn download_with(&self, model: impl Into<Model>, size_bytes: Option<u64>, mut on_progress: impl FnMut(&DownloadProgress)) -> Result<()> {
        let model = model.into();
        let request = Request::Download(Download { model: model.id, engine: model.engine, size_bytes });
        self.run(&request, self.timeouts().download, |event| {
            if let engine::Event::DownloadProgress(progress) = event {
                on_progress(progress);
            }
        })?;
        Ok(())
    }

    /// Delete the files of a model.
    pub fn delete(&self, model: impl Into<Model>) -> Result<()> {
        self.run(&Request::Delete(model.into().into()), self.timeouts().control, |_| {})?;
        Ok(())
    }

    /// Read a model into memory.
    pub fn load(&self, model: impl Into<Model>) -> Result<Loaded> {
        let model = model.into();
        let mut load_ms = None;
        let id = model.id.clone();
        self.run(&Request::Load(model.into()), self.timeouts().load, |event| {
            if let engine::Event::ModelState(state) = event
                && state.model == id
                && state.state == ModelState::Ready
            {
                load_ms = state.load_ms;
            }
        })?;
        Ok(Loaded { load_ms })
    }

    /// Take a model out of memory (feature `unload`). With `keep_preview`, an
    /// engine with the feature `keep_preview` keeps the live preview part.
    pub fn unload(&self, model: &str, keep_preview: bool) -> Result<()> {
        self.run(&unload_request(model, keep_preview), self.timeouts().control, |_| {})?;
        Ok(())
    }

    /// Load the optional parts of a loaded model (feature `prepare`). The
    /// result has the `part_state` events of the parts.
    pub fn prepare(&self, model: &str, preview: bool, vocabulary: &[String]) -> Result<Vec<PartState>> {
        let mut parts = Vec::new();
        let request = Request::Prepare(Prepare { model: model.to_owned(), preview, vocabulary: vocabulary.to_vec() });
        self.run(&request, self.timeouts().load, |event| {
            if let engine::Event::PartState(part) = event {
                parts.push(part.clone());
            }
        })?;
        Ok(parts)
    }

    /// The final pass on a WAV file: 16 kHz, mono, 16-bit PCM (see [`crate::wav`]).
    /// The engine does not delete the file.
    pub fn transcribe(&self, model: &str, path: impl AsRef<Path>, options: &TranscribeOptions) -> Result<Transcript> {
        let reply = self.call(&transcribe_request(model, path.as_ref(), options), self.timeouts().process)?;
        match reply {
            engine::Reply::Final(transcript) => Ok(transcript),
            other => Err(Error::Protocol(format!("the engine replied {} to transcribe, and not final", sayso_protocol::Kind::kind(&other)))),
        }
    }

    /// Write the samples (16 kHz mono) to a temporary WAV file, transcribe it, and remove the file.
    pub fn transcribe_i16(&self, model: &str, samples: &[i16], options: &TranscribeOptions) -> Result<Transcript> {
        let wav = TempWav::from_i16(samples)?;
        self.transcribe(model, wav.path(), options)
    }

    /// The same as [`Engine::transcribe_i16`] for samples from -1 to 1.
    pub fn transcribe_f32(&self, model: &str, samples: &[f32], options: &TranscribeOptions) -> Result<Transcript> {
        let wav = TempWav::from_f32(samples)?;
        self.transcribe(model, wav.path(), options)
    }

    /// Open a stream for a live preview. The `partial` events come on
    /// [`Engine::events`]. `session` is a number that you choose (see [`Engine::new_session`]).
    pub fn stream_start(&self, session: u64, model: &str, language: Option<&str>) -> Result<()> {
        let request = Request::StreamStart(StreamStart { session, model: model.to_owned(), language: language.map(str::to_owned) });
        self.run(&request, self.timeouts().control, |_| {})?;
        Ok(())
    }

    /// Send audio (16 kHz mono) to a stream or a VAD session. Chunks of 160 ms work well. There is no reply:
    /// an error for a session that is not open comes as a log of the engine and not here.
    pub fn stream_audio_i16(&self, session: u64, samples: &[i16]) -> Result<()> {
        self.stream_audio_pcm(session, pcm::encode_i16(samples))
    }

    /// The same as [`Engine::stream_audio_i16`] for samples from -1 to 1.
    pub fn stream_audio_f32(&self, session: u64, samples: &[f32]) -> Result<()> {
        self.stream_audio_pcm(session, pcm::encode_f32(samples))
    }

    /// Send `pcm`, the base64 text of 16 kHz mono little-endian 16-bit PCM (see [`sayso_protocol::pcm`]).
    pub fn stream_audio_pcm(&self, session: u64, pcm: String) -> Result<()> {
        self.connection.send(&Request::StreamAudio(StreamAudio { session, pcm }))
    }

    /// Close a stream or a VAD session. The engine decodes the rest of the
    /// audio first, so the last `partial` or `speech_end` comes before this returns.
    pub fn stream_end(&self, session: u64) -> Result<StreamEnded> {
        let reply = self.call(&Request::StreamEnd(StreamEnd { session }), self.timeouts().control)?;
        ok_body(reply, "stream_end")
    }

    /// Find the speech in a WAV file (feature `vad`).
    pub fn vad(&self, model: &str, path: impl AsRef<Path>) -> Result<Vec<SpeechPart>> {
        let request = Request::Vad(Vad { model: model.to_owned(), path: PathBuf::from(path.as_ref()) });
        let body: VadReply = ok_body(self.call(&request, self.timeouts().process)?, "vad")?;
        Ok(body.speech)
    }

    /// Open a VAD session on live audio (feature `vad`). Send audio with
    /// [`Engine::stream_audio_i16`], read `speech_start` and `speech_end` on
    /// [`Engine::events`], and close it with [`Engine::stream_end`].
    pub fn vad_start(&self, session: u64, model: &str) -> Result<()> {
        self.run(&Request::VadStart(VadStart { session, model: model.to_owned() }), self.timeouts().control, |_| {})?;
        Ok(())
    }

    /// Find who speaks when in a WAV file (feature `diarization`). `speakers`
    /// is the number of speakers, when you know it.
    pub fn diarize(&self, model: &str, path: impl AsRef<Path>, speakers: Option<u32>) -> Result<DiarizeReply> {
        let request = Request::Diarize(engine::Diarize { model: model.to_owned(), path: PathBuf::from(path.as_ref()), speakers });
        ok_body(self.call(&request, self.timeouts().process)?, "diarize")
    }

    /// Ask for the language model of the system (feature `language_model`).
    pub fn language_model(&self) -> Result<LanguageModelReply> {
        ok_body(self.call(&Request::LanguageModel, self.timeouts().control)?, "language_model")
    }

    /// Run the language model of the system (feature `language_model`). The
    /// engine stops the model after `timeout`. This call waits 10 seconds longer for the reply.
    pub fn generate(&self, instructions: &str, prompt: &str, timeout: Duration, temperature: Option<f64>) -> Result<GenerateReply> {
        let request = Request::Generate(engine::Generate { instructions: instructions.to_owned(), prompt: prompt.to_owned(), timeout_ms: timeout.as_millis() as u64, temperature });
        ok_body(self.call(&request, timeout + Duration::from_secs(10))?, "generate")
    }

    /// Send a request, give its events to `on_event`, and check for `ok`.
    fn run(&self, request: &Request, timeout: Duration, mut on_event: impl FnMut(&engine::Event)) -> Result<engine::Reply> {
        let name = sayso_protocol::Kind::kind(request);
        let reply = self.connection.call(request, name, timeout, Some(&mut on_event))?;
        match reply {
            engine::Reply::Ok(_) => Ok(reply),
            other => Err(Error::Protocol(format!("the engine replied {} to {name}, and not ok", sayso_protocol::Kind::kind(&other)))),
        }
    }
}

fn collect_state(event: &engine::Event, states: &mut Vec<ModelStateEvent>) {
    if let engine::Event::ModelState(state) = event {
        states.push(state.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(request: &Request, id: Option<&str>) -> serde_json::Value {
        serde_json::from_str(&sayso_protocol::encode(id, request)).unwrap()
    }

    #[test]
    fn transcribe_sends_segments_only_when_asked() {
        let path = Path::new("/tmp/a.wav");
        let plain = line(&transcribe_request("m1", path, &TranscribeOptions::default()), Some("r1"));
        assert_eq!(plain, json!({"v": 1, "type": "transcribe", "id": "r1", "model": "m1", "path": "/tmp/a.wav", "language": "auto", "vocabulary": []}));
        let options = TranscribeOptions { language: "de".into(), vocabulary: vec!["Sayso".into()], segments: true };
        let timed = line(&transcribe_request("m1", path, &options), Some("r2"));
        assert_eq!(timed["segments"], true);
        assert_eq!(timed["language"], "de");
        assert_eq!(timed["vocabulary"], json!(["Sayso"]));
    }

    #[test]
    fn a_model_by_id_leaves_out_the_engine_object() {
        let by_id: ModelRef = Model::from("parakeet-tdt-v3").into();
        assert_eq!(line(&Request::Load(by_id), Some("r1")), json!({"v": 1, "type": "load", "id": "r1", "model": "parakeet-tdt-v3"}));
        let engine = serde_json::from_value(json!({"kind": "sherpa_vad", "recipe": "silero", "file": "silero_vad.onnx"})).unwrap();
        let custom: ModelRef = Model::with_engine("my-vad", engine).into();
        assert_eq!(line(&Request::Delete(custom), Some("r2"))["engine"]["recipe"], "silero");
    }

    #[test]
    fn an_ok_body_needs_an_ok_reply() {
        let reply = engine::Reply::Ok(serde_json::from_value(json!({"speech": [{"start_ms": 1, "end_ms": 2}]})).unwrap());
        let body: VadReply = ok_body(reply, "vad").unwrap();
        assert_eq!(body.speech, vec![SpeechPart { start_ms: 1, end_ms: 2 }]);
        let wrong = engine::Reply::Final(engine::Final { text: String::new(), elapsed_ms: 0, segments: None });
        assert!(matches!(ok_body::<VadReply>(wrong, "vad"), Err(Error::Protocol(_))));
        let bad = engine::Reply::Ok(serde_json::from_value(json!({"speech": "none"})).unwrap());
        assert!(matches!(ok_body::<VadReply>(bad, "vad"), Err(Error::Protocol(_))));
    }

    #[test]
    fn stream_audio_has_no_id() {
        let request = Request::StreamAudio(StreamAudio { session: 3, pcm: pcm::encode_i16(&[1, -2]) });
        assert_eq!(line(&request, None), json!({"v": 1, "type": "stream_audio", "session": 3, "pcm": "AQD+/w=="}));
    }

    #[test]
    fn unload_sends_keep_preview_only_when_asked() {
        assert_eq!(line(&unload_request("m", false), Some("r1")), json!({"v": 1, "type": "unload", "id": "r1", "model": "m"}));
        assert_eq!(line(&unload_request("m", true), Some("r2"))["keep_preview"], true);
    }
}
