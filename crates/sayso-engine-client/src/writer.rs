//! The queue between callers and the writer thread of a sidecar process.
//!
//! Requests and live preview audio share one FIFO, so the engine sees them in
//! protocol order: `stream_start`, the audio, `stream_end`, then the final pass.
//! Callers never wait on the pipe. The queue holds audio up to a duration, and
//! it can drop the queued audio of one stream. That is how a stop skips a
//! backlog of preview audio that the engine has not read yet.

use parking_lot::{Condvar, Mutex};
use std::collections::VecDeque;

/// Audio that may wait in the queue before new audio is dropped: 10 s of 16 kHz
/// samples. The live preview can lose audio. The final pass reads the full
/// recording from a file.
pub(crate) const MAX_QUEUED_AUDIO_SAMPLES: usize = 16_000 * 10;

/// How many ended streams the queue remembers. Dictations run one at a time, so
/// the audio of an old stream is late by a few frames at most.
const REMEMBERED_ENDED: usize = 16;

/// What the writer thread sends to the sidecar. Audio is encoded on the writer
/// thread, so `push_audio` only converts samples and queues them.
pub(crate) enum Outgoing {
    Line(String),
    Audio { session: u64, pcm: Vec<i16> },
}

/// The queue is closed: the sidecar is gone or going.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Closed;

/// What happened to a frame of preview audio.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AudioPush {
    Queued,
    /// The queue holds the most audio it may. The frame is dropped.
    Full,
    /// The stream of the session ended. The frame is dropped.
    Ended,
}

#[derive(Default)]
struct State {
    items: VecDeque<Outgoing>,
    queued_samples: usize,
    closed: bool,
    /// Sessions whose stream ended, oldest first.
    ended: VecDeque<u64>,
}

impl State {
    fn mark_ended(&mut self, session: u64) {
        if !self.ended.contains(&session) {
            if self.ended.len() == REMEMBERED_ENDED {
                self.ended.pop_front();
            }
            self.ended.push_back(session);
        }
    }
}

#[derive(Default)]
pub(crate) struct WriterQueue {
    state: Mutex<State>,
    ready: Condvar,
}

impl WriterQueue {
    /// Queue a request line.
    pub(crate) fn send(&self, item: Outgoing) -> Result<(), Closed> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(Closed);
        }
        if let Outgoing::Audio { pcm, .. } = &item {
            state.queued_samples += pcm.len();
        }
        state.items.push_back(item);
        self.ready.notify_one();
        Ok(())
    }

    /// Queue a frame of preview audio, unless the queue is full or the stream ended.
    pub(crate) fn push_audio(&self, session: u64, pcm: Vec<i16>) -> Result<AudioPush, Closed> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(Closed);
        }
        if state.ended.contains(&session) {
            return Ok(AudioPush::Ended);
        }
        if state.queued_samples >= MAX_QUEUED_AUDIO_SAMPLES {
            return Ok(AudioPush::Full);
        }
        state.queued_samples += pcm.len();
        state.items.push_back(Outgoing::Audio { session, pcm });
        self.ready.notify_one();
        Ok(AudioPush::Queued)
    }

    /// A stream starts for `session`. It may reuse the id of a stream that ended.
    pub(crate) fn begin_stream(&self, session: u64) {
        self.state.lock().ended.retain(|&ended| ended != session);
    }

    /// Close the stream of `session`: drop its queued audio and refuse more.
    /// The caller then queues `stream_end`, which follows every frame that the
    /// writer already took. Returns the number of dropped frames.
    pub(crate) fn end_stream(&self, session: u64) -> usize {
        let mut state = self.state.lock();
        state.mark_ended(session);
        let before = state.items.len();
        let mut dropped_samples = 0;
        state.items.retain(|item| match item {
            Outgoing::Audio { session: s, pcm } if *s == session => {
                dropped_samples += pcm.len();
                false
            }
            _ => true,
        });
        state.queued_samples -= dropped_samples;
        before - state.items.len()
    }

    /// Close the stream of `session`: refuse more audio, but keep the audio that
    /// waits. For a VAD session, where each frame counts: a missing frame moves
    /// all later times. `stream_end` then follows the last frame.
    pub(crate) fn finish_stream(&self, session: u64) {
        self.state.lock().mark_ended(session);
    }

    /// Stop taking items. The writer sends what is queued, then `next` gives None.
    pub(crate) fn close(&self) {
        self.state.lock().closed = true;
        self.ready.notify_all();
    }

    /// The next item, in order. Waits for one. None when the queue is closed and empty.
    pub(crate) fn next(&self) -> Option<Outgoing> {
        let mut state = self.state.lock();
        loop {
            if let Some(item) = state.items.pop_front() {
                if let Outgoing::Audio { pcm, .. } = &item {
                    state.queued_samples -= pcm.len();
                }
                return Some(item);
            }
            if state.closed {
                return None;
            }
            self.ready.wait(&mut state);
        }
    }

    #[cfg(test)]
    fn queued_samples(&self) -> usize {
        self.state.lock().queued_samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> Outgoing {
        Outgoing::Line(text.into())
    }

    fn audio(queue: &WriterQueue, session: u64, samples: usize) -> AudioPush {
        queue.push_audio(session, vec![0; samples]).unwrap()
    }

    /// Everything queued, in order, as text: `L:<line>` or `A:<session>`.
    fn drain(queue: &WriterQueue) -> Vec<String> {
        queue.close();
        std::iter::from_fn(|| queue.next())
            .map(|item| match item {
                Outgoing::Line(text) => format!("L:{text}"),
                Outgoing::Audio { session, .. } => format!("A:{session}"),
            })
            .collect()
    }

    #[test]
    fn items_come_out_in_the_order_they_went_in() {
        let queue = WriterQueue::default();
        queue.send(line("start")).unwrap();
        audio(&queue, 1, 320);
        queue.send(line("middle")).unwrap();
        audio(&queue, 1, 320);
        queue.send(line("end")).unwrap();
        assert_eq!(
            drain(&queue),
            ["L:start", "A:1", "L:middle", "A:1", "L:end"]
        );
    }

    #[test]
    fn audio_is_bounded_by_its_duration_not_by_its_frame_count() {
        let queue = WriterQueue::default();
        // 20 ms frames: 500 fill 10 s.
        for _ in 0..500 {
            assert_eq!(audio(&queue, 1, 320), AudioPush::Queued);
        }
        assert_eq!(audio(&queue, 1, 320), AudioPush::Full);
        // One long frame uses as much of the bound as the many short ones.
        let queue = WriterQueue::default();
        assert_eq!(audio(&queue, 1, 100_000), AudioPush::Queued);
        assert_eq!(audio(&queue, 1, 60_000), AudioPush::Queued);
        assert_eq!(audio(&queue, 1, 320), AudioPush::Full);
        // Short frames do not stop at a count of 500 while the audio is shorter than 10 s.
        let queue = WriterQueue::default();
        for _ in 0..2_000 {
            assert_eq!(audio(&queue, 1, 80), AudioPush::Queued);
        }
        assert_eq!(queue.queued_samples(), 160_000);
    }

    #[test]
    fn taking_audio_makes_room_again() {
        let queue = WriterQueue::default();
        while audio(&queue, 1, 320) == AudioPush::Queued {}
        assert!(matches!(queue.next(), Some(Outgoing::Audio { .. })));
        assert_eq!(audio(&queue, 1, 320), AudioPush::Queued);
    }

    #[test]
    fn ending_a_stream_drops_its_audio_and_keeps_the_requests_in_order() {
        let queue = WriterQueue::default();
        queue.send(line("stream_start 1")).unwrap();
        audio(&queue, 1, 320);
        audio(&queue, 2, 320);
        queue.send(line("load")).unwrap();
        audio(&queue, 1, 320);
        assert_eq!(queue.end_stream(1), 2);
        queue.send(line("stream_end 1")).unwrap();
        queue.send(line("transcribe")).unwrap();
        assert_eq!(
            queue.queued_samples(),
            320,
            "the frame of session 2 stays counted"
        );
        assert_eq!(
            drain(&queue),
            [
                "L:stream_start 1",
                "A:2",
                "L:load",
                "L:stream_end 1",
                "L:transcribe"
            ]
        );
    }

    #[test]
    fn finishing_a_stream_keeps_its_audio_and_refuses_more() {
        let queue = WriterQueue::default();
        queue.send(line("vad_start 1")).unwrap();
        audio(&queue, 1, 320);
        audio(&queue, 1, 320);
        queue.finish_stream(1);
        queue.send(line("stream_end 1")).unwrap();
        assert_eq!(audio(&queue, 1, 320), AudioPush::Ended);
        assert_eq!(drain(&queue), ["L:vad_start 1", "A:1", "A:1", "L:stream_end 1"]);
    }

    #[test]
    fn a_frame_the_writer_already_took_stays_before_stream_end() {
        let queue = WriterQueue::default();
        audio(&queue, 1, 320);
        audio(&queue, 1, 320);
        // The writer takes the first frame, then the stream ends.
        assert!(matches!(
            queue.next(),
            Some(Outgoing::Audio { session: 1, .. })
        ));
        assert_eq!(queue.end_stream(1), 1);
        queue.send(line("stream_end 1")).unwrap();
        assert_eq!(drain(&queue), ["L:stream_end 1"]);
    }

    #[test]
    fn audio_for_an_ended_stream_is_refused_until_the_id_starts_again() {
        let queue = WriterQueue::default();
        queue.end_stream(7);
        assert_eq!(audio(&queue, 7, 320), AudioPush::Ended);
        assert_eq!(audio(&queue, 8, 320), AudioPush::Queued);
        queue.begin_stream(7);
        assert_eq!(audio(&queue, 7, 320), AudioPush::Queued);
    }

    #[test]
    fn the_queue_forgets_old_ended_streams_and_stays_small() {
        let queue = WriterQueue::default();
        for session in 0..100 {
            queue.end_stream(session);
        }
        assert_eq!(queue.state.lock().ended.len(), REMEMBERED_ENDED);
        assert_eq!(audio(&queue, 99, 320), AudioPush::Ended);
        assert_eq!(audio(&queue, 0, 320), AudioPush::Queued);
    }

    #[test]
    fn a_closed_queue_refuses_items_and_ends_after_the_queued_ones() {
        let queue = WriterQueue::default();
        queue.send(line("shutdown")).unwrap();
        queue.close();
        assert_eq!(queue.send(line("late")), Err(Closed));
        assert_eq!(queue.push_audio(1, vec![0; 320]), Err(Closed));
        assert!(matches!(queue.next(), Some(Outgoing::Line(text)) if text == "shutdown"));
        assert!(queue.next().is_none());
    }

    #[test]
    fn next_waits_for_an_item() {
        let queue = std::sync::Arc::new(WriterQueue::default());
        let writer = queue.clone();
        let handle = std::thread::spawn(move || writer.next().is_some());
        std::thread::sleep(std::time::Duration::from_millis(50));
        queue.send(line("late")).unwrap();
        assert!(handle.join().unwrap());
    }
}
