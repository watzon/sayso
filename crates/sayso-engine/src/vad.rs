//! Voice activity detection (`docs/engine-protocol.md`, "VAD").
//!
//! The model comes from a [`Detector`] (sherpa-onnx in [`crate::sherpa`], a fake
//! in the tests). [`Tracker`] turns what the detector says into the events of the
//! protocol, and it holds the rules of the protocol. The `vad` request and a VAD
//! session both use it, so the same audio gives the same parts.
//!
//! The rules, and what sherpa-onnx 1.13.8 does for them:
//! - Speech under 250 ms is left out. The detector does this (`min_speech_duration`).
//! - A pause under 500 ms does not end speech. The detector does this
//!   (`min_silence_duration`).
//! - A part is at most 30 s. The detector does not do this: its `max_speech_duration`
//!   only makes it split at a short pause, and it does not split speech without a pause.
//!   A split at a short pause leaves a gap under 500 ms between the parts. So the
//!   detector gets a very long `max_speech_duration` (see [`crate::sherpa`]), and the
//!   tracker cuts the speech at 30 s.

use crate::audio::{self, SAMPLE_RATE};

/// Speech under this is left out.
pub const MIN_SPEECH_MS: usize = 250;
/// A pause under this does not end speech.
pub const MIN_SILENCE_MS: usize = 500;
/// The longest part. Longer speech is cut into parts of this length.
pub const MAX_PART_MS: usize = 30_000;

/// A voice activity detector that hears the audio one window at a time.
///
/// It follows the detector of sherpa-onnx. A window gives a decision, so a caller
/// that gives exactly one window to each `accept` gets the same result for any
/// way of cutting the audio. Positions are in samples from the first window.
pub trait Detector: Send {
    /// The number of samples in a window.
    fn window(&self) -> usize;
    /// Hear one window.
    fn accept(&mut self, window: &[f32]);
    /// True while speech is open: it started and the pause that ends it is not long enough yet.
    fn detected(&self) -> bool;
    /// The oldest speech that ended since the last call: its start and its end.
    /// The end is where the pause began.
    fn next_segment(&mut self) -> Option<(u64, u64)>;
    /// End the speech that is open. It ends at the last sample the detector heard.
    fn flush(&mut self);
}

/// An event of a VAD session. The times are in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Start { start_ms: u64 },
    End { start_ms: u64, end_ms: u64 },
}

fn to_ms(samples: u64) -> u64 {
    samples * 1000 / SAMPLE_RATE as u64
}

/// Feeds audio to a [`Detector`] and reports the speech parts.
pub struct Tracker {
    detector: Box<dyn Detector>,
    window: usize,
    /// Audio that does not fill a window yet.
    pending: Vec<f32>,
    /// The samples of the caller.
    given: u64,
    /// The samples that the detector heard: `given`, then silence at the end.
    heard: u64,
    /// The start of the part that is open.
    open: Option<u64>,
    /// The end of the last part.
    last_end: u64,
}

impl Tracker {
    pub fn new(detector: Box<dyn Detector>) -> Self {
        Tracker {
            window: detector.window(),
            detector,
            pending: Vec::new(),
            given: 0,
            heard: 0,
            open: None,
            last_end: 0,
        }
    }

    /// Hear more audio (16 kHz mono). Any chunk size gives the same events.
    pub fn accept(&mut self, samples: &[f32]) -> Vec<Event> {
        self.given += samples.len() as u64;
        self.pending.extend_from_slice(samples);
        let mut events = Vec::new();
        self.drain(&mut events);
        events
    }

    /// No more audio comes. Closes the part that is open. After this call, do not use the tracker.
    pub fn finish(&mut self) -> Vec<Event> {
        // The detector ends speech when it hears a pause. Give it the rest of the last
        // window and a pause, so that speech at the end of the audio ends as other speech does.
        let rest = (self.window - self.pending.len() % self.window) % self.window;
        let silence = rest + audio::ms(MIN_SILENCE_MS) + 3 * self.window;
        self.pending.resize(self.pending.len() + silence, 0.0);
        let mut events = Vec::new();
        self.drain(&mut events);
        self.detector.flush();
        self.collect(&mut events);
        if let Some(start) = self.open.take() {
            self.end_part(start, self.given, &mut events);
        }
        events
    }

    /// Give every whole window in `pending` to the detector.
    fn drain(&mut self, events: &mut Vec<Event>) {
        let pending = std::mem::take(&mut self.pending);
        let windows = pending.chunks_exact(self.window);
        let rest = windows.remainder().to_vec();
        for window in windows {
            self.detector.accept(window);
            self.heard += window.len() as u64;
            self.collect(events);
            if self.detector.detected() {
                self.open_part(events);
                self.cut_long_part(events);
            }
        }
        self.pending = rest;
    }

    /// Take the parts that the detector ended.
    fn collect(&mut self, events: &mut Vec<Event>) {
        while let Some((start, end)) = self.detector.next_segment() {
            let start = match self.open.take() {
                Some(start) => start,
                None => {
                    let start = start.max(self.last_end).min(self.given);
                    events.push(Event::Start { start_ms: to_ms(start) });
                    start
                }
            };
            self.end_part(start, end, events);
        }
    }

    fn end_part(&mut self, start: u64, end: u64, events: &mut Vec<Event>) {
        let end = end.min(self.given).max(start);
        events.push(Event::End { start_ms: to_ms(start), end_ms: to_ms(end) });
        self.last_end = end;
    }

    /// Speech is open now. When it just began, report its start. The detector decides after
    /// the shortest speech and two windows, so the speech began that long ago. The detector
    /// reports the same start at the end of the part.
    fn open_part(&mut self, events: &mut Vec<Event>) {
        if self.open.is_some() {
            return;
        }
        let back = (2 * self.window + audio::ms(MIN_SPEECH_MS)) as u64;
        let start = self.heard.saturating_sub(back).max(self.last_end).min(self.given);
        self.open = Some(start);
        events.push(Event::Start { start_ms: to_ms(start) });
    }

    /// Cut the part that is open when it is 30 s long. The next part starts at the cut.
    ///
    /// The cut waits until the detector has the speech open a pause and a shortest speech
    /// after the cut point. The speech has gone on past the cut point then, so the next part
    /// is not shorter than the shortest speech, and it is not empty.
    fn cut_long_part(&mut self, events: &mut Vec<Event>) {
        let max = audio::ms(MAX_PART_MS) as u64;
        let wait = (audio::ms(MIN_SILENCE_MS) + audio::ms(MIN_SPEECH_MS)) as u64;
        while let Some(start) = self.open {
            if self.heard < start + max + wait {
                break;
            }
            let cut = start + max;
            events.push(Event::End { start_ms: to_ms(start), end_ms: to_ms(cut) });
            events.push(Event::Start { start_ms: to_ms(cut) });
            self.open = Some(cut);
            self.last_end = cut;
        }
    }
}

/// The parts with speech, as `(start_ms, end_ms)`, from the events of a whole audio.
pub fn parts(events: &[Event]) -> Vec<(u64, u64)> {
    events
        .iter()
        .filter_map(|event| match *event {
            Event::End { start_ms, end_ms } => Some((start_ms, end_ms)),
            Event::Start { .. } => None,
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::collections::VecDeque;

    /// A detector for tests that follows the detector of sherpa-onnx: a window with
    /// a root mean square above 0.01 is speech. Speech starts after `MIN_SPEECH_MS` of it,
    /// and ends after `MIN_SILENCE_MS` of silence.
    pub struct EnergyDetector {
        heard: u64,
        loud: usize,
        quiet: usize,
        triggered: bool,
        start: Option<u64>,
        done: VecDeque<(u64, u64)>,
    }

    impl EnergyDetector {
        pub fn new() -> Self {
            EnergyDetector { heard: 0, loud: 0, quiet: 0, triggered: false, start: None, done: VecDeque::new() }
        }
    }

    const WINDOW: usize = 512;

    impl Detector for EnergyDetector {
        fn window(&self) -> usize {
            WINDOW
        }

        fn accept(&mut self, window: &[f32]) {
            self.heard += window.len() as u64;
            let min_speech = audio::ms(MIN_SPEECH_MS);
            let min_silence = audio::ms(MIN_SILENCE_MS);
            if audio::rms(window) > 0.01 {
                self.quiet = 0;
                self.loud += window.len();
                if !self.triggered && self.loud >= min_speech {
                    self.triggered = true;
                    self.start = Some(self.heard.saturating_sub((2 * WINDOW + min_speech) as u64));
                }
                return;
            }
            if !self.triggered {
                self.loud = 0;
                return;
            }
            self.quiet += window.len();
            if self.quiet >= min_silence {
                let end = self.heard - min_silence as u64;
                self.done.push_back((self.start.take().unwrap_or(0), end));
                self.triggered = false;
                self.loud = 0;
                self.quiet = 0;
            }
        }

        fn detected(&self) -> bool {
            self.start.is_some()
        }

        fn next_segment(&mut self) -> Option<(u64, u64)> {
            self.done.pop_front()
        }

        fn flush(&mut self) {
            if let Some(start) = self.start.take() {
                self.done.push_back((start, self.heard));
                self.triggered = false;
            }
        }
    }

    /// Audio for the tests: `(milliseconds, loud)` pieces.
    pub fn audio_of(pieces: &[(usize, bool)]) -> Vec<f32> {
        pieces
            .iter()
            .flat_map(|&(length, loud)| std::iter::repeat_n(if loud { 0.5 } else { 0.0 }, audio::ms(length)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{EnergyDetector, audio_of};
    use super::*;

    fn tracker() -> Tracker {
        Tracker::new(Box::new(EnergyDetector::new()))
    }

    /// All events for `samples`, fed in chunks of `chunk` samples.
    fn run(samples: &[f32], chunk: usize) -> Vec<Event> {
        let mut tracker = tracker();
        let mut events = Vec::new();
        for piece in samples.chunks(chunk) {
            events.extend(tracker.accept(piece));
        }
        events.extend(tracker.finish());
        events
    }

    fn near(actual: u64, expected: u64) -> bool {
        actual.abs_diff(expected) <= 100
    }

    #[test]
    fn speech_with_a_long_pause_is_two_parts() {
        let samples = audio_of(&[(1000, false), (2000, true), (1500, false), (1000, true), (1000, false)]);
        let found = parts(&run(&samples, 2560));
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(near(found[0].0, 1000) && near(found[0].1, 3000), "{found:?}");
        assert!(near(found[1].0, 4500) && near(found[1].1, 5500), "{found:?}");
    }

    #[test]
    fn a_short_pause_does_not_end_speech_and_short_speech_is_left_out() {
        let samples = audio_of(&[(500, false), (1000, true), (300, false), (1000, true), (800, false), (200, true), (1000, false)]);
        let found = parts(&run(&samples, 2560));
        assert_eq!(found.len(), 1, "one part, and the 200 ms are left out: {found:?}");
        assert!(near(found[0].0, 500) && near(found[0].1, 2800), "{found:?}");
    }

    #[test]
    fn silence_and_noise_without_speech_give_no_part() {
        assert!(parts(&run(&audio_of(&[(3000, false)]), 2560)).is_empty());
        assert!(parts(&run(&[], 2560)).is_empty());
        assert!(parts(&run(&audio_of(&[(100, true)]), 2560)).is_empty());
    }

    #[test]
    fn events_do_not_depend_on_the_chunk_size() {
        let samples = audio_of(&[(700, false), (3000, true), (900, false), (40_000, true), (1000, false), (400, true)]);
        let all = run(&samples, samples.len());
        for chunk in [1, 160, 2560, 3333, 16_000] {
            assert_eq!(run(&samples, chunk), all, "chunk of {chunk} samples");
        }
    }

    #[test]
    fn long_speech_is_cut_into_parts_of_at_most_30_s_that_touch() {
        let samples = audio_of(&[(1000, false), (75_000, true), (1000, false)]);
        let found = parts(&run(&samples, 2560));
        assert_eq!(found.len(), 3, "{found:?}");
        assert!(near(found[0].0, 1000));
        assert_eq!(found[0].1 - found[0].0, 30_000, "{found:?}");
        assert_eq!(found[1], (found[0].1, found[0].1 + 30_000), "{found:?}");
        assert_eq!(found[2].0, found[1].1, "{found:?}");
        assert!(near(found[2].1, 76_000), "{found:?}");
    }

    #[test]
    fn a_session_reports_each_start_before_its_end_and_every_end_by_the_finish() {
        let samples = audio_of(&[(500, false), (2000, true), (1000, false), (1000, true)]);
        let mut tracker = tracker();
        let mut events = Vec::new();
        let mut seen_before_finish = 0;
        for piece in samples.chunks(2560) {
            events.extend(tracker.accept(piece));
            seen_before_finish = events.len();
        }
        // The first part ended, and the second part has begun. Its end waits for a pause.
        assert!(matches!(events[0], Event::Start { .. }), "{events:?}");
        assert!(matches!(events[1], Event::End { .. }), "{events:?}");
        assert!(matches!(events[2], Event::Start { .. }), "{events:?}");
        assert_eq!(seen_before_finish, 3, "{events:?}");
        let rest = tracker.finish();
        assert_eq!(rest.len(), 1, "{rest:?}");
        let Event::End { start_ms, end_ms } = rest[0] else { panic!("{rest:?}") };
        assert!(end_ms > start_ms && end_ms <= 4500, "the end is not after the audio: {rest:?}");
    }

    #[test]
    fn speech_that_runs_to_the_end_of_the_audio_ends_inside_it() {
        let samples = audio_of(&[(500, false), (2000, true)]);
        let found = parts(&run(&samples, 2560));
        assert_eq!(found.len(), 1);
        assert!(found[0].1 <= 2500, "{found:?}");
        assert!(near(found[0].1, 2500), "{found:?}");
    }

    #[test]
    fn every_event_list_is_ordered_and_without_overlap() {
        let samples = audio_of(&[(300, true), (900, false), (5000, true), (600, false), (35_000, true), (2000, false), (900, true)]);
        let events = run(&samples, 1000);
        let mut open: Option<u64> = None;
        let mut last_end = 0;
        for event in events {
            match event {
                Event::Start { start_ms } => {
                    assert!(open.is_none() && start_ms >= last_end, "{start_ms}");
                    open = Some(start_ms);
                }
                Event::End { start_ms, end_ms } => {
                    assert_eq!(open.take(), Some(start_ms));
                    assert!(end_ms > start_ms);
                    last_end = end_ms;
                }
            }
        }
        assert!(open.is_none());
    }
}
