//! Run one job against the engine: read the file, load the models, find the
//! speech, transcribe it one chunk at a time, and find the speakers.

use super::audio::{self, Audio};
use super::model_use::ModelUse;
use super::plan::{self, Chunk, Stopped, VAD_WINDOW_MS};
use super::queue::{JobRun, RunJob};
use super::speakers;
use super::JobSpec;
use sayso_core::models::{ModelId, Timestamps};
use sayso_core::stt::SessionOptions;
use sayso_platform::SttBackend;
use sayso_protocol::{Event, Segment, SegmentEvent, SpeakersEvent, Word};
use std::sync::Arc;

/// With diarization, the transcription is this share of the progress, and
/// diarization the rest: it runs after the last chunk and can take some time.
pub const TRANSCRIPTION_SHARE: f64 = 0.9;

/// Runs jobs on the engine of the app.
pub struct JobRunner {
    backend: Arc<dyn SttBackend>,
    models: Arc<ModelUse>,
    vad_window_ms: u64,
}

impl JobRunner {
    /// `models` tells the memory policy of the app which models the jobs use.
    pub fn new(backend: Arc<dyn SttBackend>, models: Arc<ModelUse>) -> JobRunner {
        JobRunner { backend, models, vad_window_ms: VAD_WINDOW_MS }
    }

    /// A runner whose VAD model gets windows of `window_ms`, for the tests.
    #[cfg(test)]
    pub(crate) fn with_vad_window(mut self, window_ms: u64) -> JobRunner {
        self.vad_window_ms = window_ms;
        self
    }

    /// The job up to its last event. Gives the text of `job_done`.
    fn run_job(&self, job: &JobSpec, run: &JobRun<'_>) -> Result<String, Stopped> {
        let audio = audio::read(&job.path).map_err(Stopped::Failed)?;
        if run.cancelled() {
            return Err(Stopped::Cancelled);
        }
        // Hold before the load, so the app does not unload a model between the two.
        let _hold = self.models.hold(job.models());
        for model in job.models() {
            run.before_engine_call()?;
            if !self.backend.status(&model).is_usable() {
                self.backend.load(&model).map_err(|e| Stopped::Failed(format!("Sayso cannot load the model {model}: {e}")))?;
            }
        }

        let parts = plan::speech_parts(&audio, &job.vad, &*self.backend, self.vad_window_ms, &mut || run.before_engine_call())?;
        let chunks = plan::chunks(&parts, audio.len_ms(), plan::max_chunk_ms(job.timestamps));
        let options = SessionOptions {
            final_model: job.model.clone(),
            preview_model: None,
            language: job.language.clone(),
            vocabulary: Vec::new(),
        };
        let share = if job.diarization.is_some() { TRANSCRIPTION_SHARE } else { 1.0 };
        let len_ms = audio.len_ms().max(1);
        let mut times = Vec::new();
        let mut texts = Vec::new();
        for (i, chunk) in chunks.iter().enumerate() {
            run.before_engine_call()?;
            for segment in self.transcribe(&audio, *chunk, &options, job.timestamps)? {
                times.push((segment.start_ms, segment.end_ms));
                texts.push(segment.text.clone());
                run.event(Event::Segment(SegmentEvent { job: Some(run.id), session: None, source: None, segment }));
            }
            // The silence after the last chunk is done too.
            let done = if i + 1 == chunks.len() { len_ms } else { chunk.end_ms.min(len_ms) };
            run.event(Event::JobProgress { job: run.id, fraction: share * done as f64 / len_ms as f64 });
        }

        if let Some(model) = &job.diarization {
            run.before_engine_call()?;
            let turns = self.diarize(model, &audio, job.speakers)?;
            let speakers = speakers::assign(&times, &turns);
            let turns = speakers::segment_turns(&times, &speakers);
            run.event(Event::Speakers(SpeakersEvent { job: Some(run.id), session: None, source: None, turns }));
        }
        Ok(texts.join(" "))
    }

    /// The segments of one chunk, with times from the start of the file.
    fn transcribe(&self, audio: &Audio, chunk: Chunk, options: &SessionOptions, timestamps: Timestamps) -> Result<Vec<Segment>, Stopped> {
        let samples = audio.floats(chunk.start_ms, chunk.end_ms);
        let failed = |e| Stopped::Failed(format!("Transcription failed: {e}"));
        if timestamps == Timestamps::None {
            // No times from the model: the chunk is the segment.
            let text = self.backend.transcribe(&samples, options).map_err(failed)?.text;
            let text = text.trim();
            return Ok(if text.is_empty() {
                Vec::new()
            } else {
                vec![Segment { start_ms: chunk.start_ms, end_ms: chunk.end_ms, text: text.to_string(), words: None }]
            });
        }
        let found = self.backend.transcribe_segments(&samples, options).map_err(failed)?;
        let shift = chunk.start_ms;
        Ok(found
            .segments
            .into_iter()
            .filter(|s| !s.text.trim().is_empty())
            .map(|s| Segment {
                start_ms: s.start_ms + shift,
                end_ms: s.end_ms + shift,
                text: s.text.trim().to_string(),
                words: (!s.words.is_empty()).then(|| {
                    s.words.into_iter().map(|w| Word { start_ms: w.start_ms + shift, end_ms: w.end_ms + shift, text: w.text }).collect()
                }),
            })
            .collect())
    }

    /// The turns of the whole file. Without a number of speakers, small
    /// speakers are folded (`speakers::fold_small_speakers`).
    fn diarize(&self, model: &ModelId, audio: &Audio, speakers: Option<u32>) -> Result<Vec<sayso_core::stt::Turn>, Stopped> {
        let found = self
            .backend
            .diarize(model, &audio.all_floats(), speakers)
            .map_err(|e| Stopped::Failed(format!("Diarization failed: {e}")))?;
        Ok(if speakers.is_none() { speakers::fold_small_speakers(&found.turns) } else { found.turns })
    }
}

impl RunJob for JobRunner {
    fn run(&self, job: &JobSpec, run: &JobRun<'_>) {
        match self.run_job(job, run) {
            Ok(text) => run.event(Event::JobDone { job: run.id, text }),
            Err(stopped) => run.stopped(stopped),
        }
    }
}
