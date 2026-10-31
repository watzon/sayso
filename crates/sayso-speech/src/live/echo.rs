//! The echo rule (`docs/sdk.md`, "System audio"): without headphones the
//! microphone hears the call too, so a session with the sources `mic` and
//! `system` drops a microphone segment that a system segment covers in time
//! and in words.
//!
//! A system segment can come after the microphone segment that it covers,
//! because each source has its own final pass. So [`EchoGate`] holds each
//! microphone segment until the system source has transcribed the audio of
//! that time, and decides then. The segments of the system go out at once.

use crate::EventSink;
use parking_lot::Mutex;
use sayso_protocol::{Event, Segment, SegmentEvent, Source, is_unspaced};
use std::collections::VecDeque;
use std::sync::Arc;

/// The part of the words of a microphone segment that must be in the system
/// audio of the same time. `docs/sdk.md`, "System audio", says how the test
/// data in `tests/fixtures/echo_pairs.tsv` chose it.
pub const ECHO_LIMIT: f32 = 0.6;
/// A microphone segment with this many words or fewer is an echo only when it
/// is also most of the system words of its time. "Yes" or "I think so" in a long
/// sentence is a real reply.
const SHORT_WORDS: usize = 3;
const SHORT_LIMIT: f32 = 0.5;
/// How far the times of the two sources can differ: the path through the room
/// and the output buffers, the padding of a part, and the times of a model.
const TIME_SLACK_MS: i64 = 1_000;
/// At least this part of a microphone segment must be in the time of system segments.
const MIN_TIME_COVER: f32 = 0.5;
/// A microphone segment waits at most this much audio of the system. Its final
/// pass can be far behind, and the session must not hold a reply for long.
const MAX_HOLD_MS: i64 = 60_000;
/// How much system text the gate keeps, in audio time.
const KEEP_SYSTEM_MS: i64 = 600_000;

/// The words of `text`, in lower case and without punctuation. A character of
/// a script without spaces (Chinese, Japanese, Thai, ...) counts as one word.
pub(crate) fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        if is_unspaced(c) {
            if !word.is_empty() {
                out.push(std::mem::take(&mut word));
            }
            out.push(c.to_string());
        } else if c.is_alphanumeric() {
            word.extend(c.to_lowercase());
        } else if c != '\'' && c != '’' && !word.is_empty() {
            out.push(std::mem::take(&mut word));
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

/// The same word, or a word with one wrong letter, as two transcripts of one
/// sound often have ("there" and "their" are not that close, and do not count).
fn same_word(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len().min(b.len()) < 4 || a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let start = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let end = a[start..].iter().rev().zip(b[start..].iter().rev()).take_while(|(x, y)| x == y).count();
    a.len().max(b.len()) - start - end <= 1
}

/// The number of words of `a` that are in `b` in the same order.
fn common_words(a: &[String], b: &[String]) -> usize {
    let mut row = vec![0usize; b.len() + 1];
    for word in a {
        let mut diagonal = 0;
        for (j, other) in b.iter().enumerate() {
            let up = row[j + 1];
            row[j + 1] = if same_word(word, other) { diagonal + 1 } else { up.max(row[j]) };
            diagonal = up;
        }
    }
    row[b.len()]
}

/// The decision on the words alone. `limit` is a parameter for the test data.
pub(crate) fn words_echo(mic: &[String], system: &[String], limit: f32) -> bool {
    if mic.is_empty() || system.is_empty() {
        return false;
    }
    let common = common_words(mic, system) as f32;
    common / mic.len() as f32 >= limit && (mic.len() > SHORT_WORDS || common / system.len() as f32 >= SHORT_LIMIT)
}

/// A system segment, with times on the capture clock.
struct SystemSegment {
    start: i64,
    end: i64,
    /// Each word, with its start.
    words: Vec<(i64, String)>,
}

impl SystemSegment {
    fn new(segment: &Segment, clock_offset: i64) -> SystemSegment {
        let start = segment.start_ms as i64 + clock_offset;
        let end = segment.end_ms as i64 + clock_offset;
        let words = match segment.words.as_ref().filter(|words| !words.is_empty()) {
            Some(timed) => timed.iter().flat_map(|word| words(&word.text).into_iter().map(move |text| (word.start_ms as i64 + clock_offset, text))).collect(),
            // No times for the words: they are about evenly spread over the segment.
            None => {
                let all = words(&segment.text);
                let count = all.len().max(1) as i64;
                all.into_iter().enumerate().map(|(i, text)| (start + (end - start) * i as i64 / count, text)).collect()
            }
        };
        SystemSegment { start, end, words }
    }
}

/// True when the system segments cover a microphone segment in time and in words.
fn covered(start: i64, end: i64, mic: &[String], system: &[SystemSegment]) -> bool {
    let near: Vec<&SystemSegment> = system.iter().filter(|s| s.start - TIME_SLACK_MS < end && s.end + TIME_SLACK_MS > start).collect();
    let mut spans: Vec<(i64, i64)> = near.iter().map(|s| ((s.start - TIME_SLACK_MS).max(start), (s.end + TIME_SLACK_MS).min(end))).collect();
    spans.sort_unstable();
    let mut in_time = 0;
    let mut reached = start;
    for (from, to) in spans {
        in_time += (to - from.max(reached)).max(0);
        reached = reached.max(to);
    }
    if (in_time as f32) < (end - start).max(1) as f32 * MIN_TIME_COVER {
        return false;
    }
    let heard: Vec<String> = near.iter().flat_map(|s| &s.words).filter(|(at, _)| *at >= start - TIME_SLACK_MS && *at <= end + TIME_SLACK_MS).map(|(_, word)| word.clone()).collect();
    words_echo(mic, &heard, ECHO_LIMIT)
}

struct Held {
    segment: Segment,
    start: i64,
    end: i64,
}

#[derive(Default)]
struct State {
    system: Vec<SystemSegment>,
    /// The system final pass has finished all the speech before this time.
    done_until: i64,
    /// The system source has heard the audio up to this time.
    heard: i64,
    ended: bool,
    held: VecDeque<Held>,
    kept: Vec<Segment>,
}

/// Decides on the microphone segments of one session. The final passes and
/// the system intake call it; no call waits for another source.
pub(crate) struct EchoGate {
    session: u64,
    sink: Arc<dyn EventSink>,
    state: Mutex<State>,
}

impl EchoGate {
    pub fn new(session: u64, sink: Arc<dyn EventSink>) -> EchoGate {
        EchoGate { session, sink, state: Mutex::new(State { done_until: i64::MIN, heard: i64::MIN, ..State::default() }) }
    }

    /// A microphone segment with `clock_offset` from its times to the capture clock.
    pub fn mic_segment(&self, segment: Segment, clock_offset: i64) {
        let mut state = self.state.lock();
        let (start, end) = (segment.start_ms as i64 + clock_offset, segment.end_ms as i64 + clock_offset);
        state.held.push_back(Held { segment, start, end });
        self.release(&mut state);
    }

    /// A segment of the system source, after its event went out.
    pub fn system_segment(&self, segment: &Segment, clock_offset: i64) {
        let mut state = self.state.lock();
        state.system.push(SystemSegment::new(segment, clock_offset));
        let newest = state.system.last().map_or(0, |s| s.end);
        state.system.retain(|s| s.end >= newest - KEEP_SYSTEM_MS);
    }

    /// The system final pass has finished all the speech before `clock_ms`.
    pub fn system_done_until(&self, clock_ms: i64) {
        let mut state = self.state.lock();
        state.done_until = state.done_until.max(clock_ms);
        self.release(&mut state);
    }

    /// The system source has heard the audio up to `clock_ms`.
    pub fn system_heard(&self, clock_ms: i64) {
        let mut state = self.state.lock();
        state.heard = state.heard.max(clock_ms);
        self.release(&mut state);
    }

    /// No more system segments come. It can be called more than once.
    pub fn system_ended(&self) {
        let mut state = self.state.lock();
        state.ended = true;
        self.release(&mut state);
    }

    /// The microphone segments that stayed, after the sources ended.
    pub fn kept(&self) -> Vec<Segment> {
        self.state.lock().kept.clone()
    }

    /// Decide on the held segments whose time has come, in order. The event
    /// goes out under the lock, so two threads cannot change the order.
    fn release(&self, state: &mut State) {
        while let Some(front) = state.held.front() {
            let ready = state.ended || state.done_until >= front.end + TIME_SLACK_MS || state.heard >= front.end + MAX_HOLD_MS;
            if !ready {
                return;
            }
            let Some(held) = state.held.pop_front() else { return };
            if covered(held.start, held.end, &words(&held.segment.text), &state.system) {
                log::debug!("host: live session {}: a microphone segment of {} ms is an echo", self.session, held.end - held.start);
                continue;
            }
            self.sink.event(Event::Segment(SegmentEvent { job: None, session: Some(self.session), source: Some(Source::Mic), segment: held.segment.clone() }));
            state.kept.push(held.segment);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(text: &str) -> Vec<String> {
        words(text)
    }

    fn segment(start: u64, end: u64, text: &str) -> Segment {
        Segment { start_ms: start, end_ms: end, text: text.into(), words: None }
    }

    #[derive(Default)]
    struct Events(Mutex<Vec<String>>);

    impl EventSink for Events {
        fn event(&self, event: Event) {
            if let Event::Segment(segment) = event {
                self.0.lock().push(segment.segment.text);
            }
        }
    }

    fn gate() -> (Arc<Events>, EchoGate) {
        let sink = Arc::new(Events::default());
        (sink.clone(), EchoGate::new(1, sink))
    }

    #[test]
    fn words_ignore_case_and_punctuation_and_split_scripts_without_spaces() {
        assert_eq!(w("Don't stop, the 2 BIG-ones!"), ["dont", "stop", "the", "2", "big", "ones"]);
        assert_eq!(w("今天 hello世界"), ["今", "天", "hello", "世", "界"]);
        assert!(w(" ... ").is_empty());
    }

    #[test]
    fn one_wrong_letter_in_a_longer_word_is_the_same_word() {
        assert!(same_word("transcribe", "transcribes"));
        assert!(same_word("meeting", "meating"));
        assert!(!same_word("the", "she"));
        assert!(!same_word("there", "their"));
        assert!(!same_word("monday", "sunday"));
    }

    #[test]
    fn a_short_segment_must_be_most_of_the_system_words() {
        assert!(words_echo(&w("Yes."), &w("yes"), ECHO_LIMIT));
        assert!(!words_echo(&w("Yes."), &w("yes we should ship the release on Friday"), ECHO_LIMIT));
    }

    #[test]
    fn a_reply_of_three_words_stays_when_it_is_a_small_part_of_the_system_words() {
        assert!(!words_echo(&w("I think so"), &w("I think we should ship it"), ECHO_LIMIT));
        assert!(!words_echo(&w("I think so"), &w("I think so and we should ship it on Friday"), ECHO_LIMIT));
        assert!(words_echo(&w("I think so"), &w("I think so"), ECHO_LIMIT));
        assert!(words_echo(&w("I think so"), &w("yes I think so too"), ECHO_LIMIT));
        // Four words: only the limit of the words of the microphone counts.
        assert!(words_echo(&w("I think so too"), &w("I think so too and we should ship it on Friday"), ECHO_LIMIT));
    }

    #[test]
    fn a_microphone_segment_waits_for_the_system_and_an_echo_is_dropped() {
        let (sink, gate) = gate();
        gate.mic_segment(segment(1_000, 3_000, "we should ship the release on friday"), 0);
        gate.system_done_until(3_500);
        assert!(sink.0.lock().is_empty(), "the system can still have a segment of this time");
        gate.system_segment(&segment(900, 3_100, "We should ship the release on Friday."), 0);
        gate.system_done_until(4_000);
        assert!(sink.0.lock().is_empty());
        assert!(gate.kept().is_empty());
    }

    #[test]
    fn the_own_words_of_the_user_stay_and_keep_their_order() {
        let (sink, gate) = gate();
        gate.mic_segment(segment(1_000, 2_000, "I can take that one"), 0);
        gate.mic_segment(segment(2_500, 3_500, "and the notes too"), 0);
        gate.system_segment(&segment(1_000, 3_500, "does anyone want to review the budget numbers"), 0);
        gate.system_done_until(10_000);
        assert_eq!(*sink.0.lock(), ["I can take that one", "and the notes too"]);
        assert_eq!(gate.kept().len(), 2);
    }

    #[test]
    fn the_same_words_at_another_time_are_not_an_echo() {
        let (sink, gate) = gate();
        gate.system_segment(&segment(1_000, 3_000, "we should ship the release on friday"), 0);
        gate.mic_segment(segment(30_000, 32_000, "we should ship the release on friday"), 0);
        gate.system_done_until(40_000);
        assert_eq!(sink.0.lock().len(), 1);
    }

    #[test]
    fn the_clock_offsets_line_up_the_two_sources() {
        let (sink, gate) = gate();
        // The segment times differ by 20 s, and the capture clock says that they are the same time.
        gate.system_segment(&segment(1_000, 3_000, "we should ship the release on friday"), 100_000);
        gate.mic_segment(segment(21_000, 23_000, "we should ship the release on friday"), 80_000);
        gate.system_done_until(110_000);
        assert!(sink.0.lock().is_empty());
    }

    #[test]
    fn only_the_words_of_the_same_time_in_a_long_system_segment_count() {
        let (sink, gate) = gate();
        // 30 s of speech with these words at its start. The user says them near its end.
        let long = format!("i think we should go {}", "and talk about many other things here ".repeat(10));
        gate.system_segment(&segment(0, 30_000, &long), 0);
        gate.mic_segment(segment(27_000, 28_500, "I think we should go"), 0);
        gate.system_done_until(40_000);
        assert_eq!(sink.0.lock().len(), 1);
    }

    #[test]
    fn the_end_of_the_system_releases_what_waits() {
        let (sink, gate) = gate();
        gate.mic_segment(segment(1_000, 2_000, "this one stays"), 0);
        gate.system_ended();
        assert_eq!(sink.0.lock().len(), 1);
        gate.mic_segment(segment(5_000, 6_000, "and this one too"), 0);
        assert_eq!(sink.0.lock().len(), 2, "no wait after the end");
    }

    #[test]
    fn a_segment_does_not_wait_longer_than_the_limit_of_system_audio() {
        let (sink, gate) = gate();
        gate.mic_segment(segment(1_000, 2_000, "this one stays"), 0);
        gate.system_heard(2_000 + MAX_HOLD_MS - 1);
        assert!(sink.0.lock().is_empty());
        gate.system_heard(2_000 + MAX_HOLD_MS);
        assert_eq!(sink.0.lock().len(), 1);
    }

    /// One line of `echo_pairs.tsv`: `echo` or `other`, the microphone text, the system text.
    fn pairs() -> Vec<(bool, Vec<String>, Vec<String>)> {
        include_str!("../../tests/fixtures/echo_pairs.tsv")
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .map(|line| {
                let mut fields = line.split('\t');
                let (kind, mic, system) = (fields.next().unwrap(), fields.next().unwrap(), fields.next().unwrap());
                (kind == "echo", w(mic), w(system))
            })
            .collect()
    }

    /// The test data of the limit. `cargo test -p sayso-speech echo_limit -- --nocapture`
    /// prints the table of `docs/sdk.md`.
    #[test]
    fn echo_limit_keeps_real_replies_and_drops_most_echoes() {
        let pairs = pairs();
        let (echoes, others) = (pairs.iter().filter(|p| p.0).count(), pairs.iter().filter(|p| !p.0).count());
        let rates = |limit: f32| {
            let dropped = |want: bool| pairs.iter().filter(|p| p.0 == want && words_echo(&p.1, &p.2, limit)).count();
            (dropped(true) as f32 / echoes as f32, dropped(false) as f32 / others as f32)
        };
        println!("{echoes} echoes, {others} others");
        println!("limit  echoes dropped  real text dropped");
        for limit in [0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9] {
            let (echo, other) = rates(limit);
            println!("{limit:.1}    {:5.1} %          {:5.1} %", echo * 100.0, other * 100.0);
        }
        let (echo, other) = rates(ECHO_LIMIT);
        assert!(echo >= 0.9, "the limit drops {echo} of the echoes");
        assert!(other <= 0.02, "the limit drops {other} of the real text");
    }
}
