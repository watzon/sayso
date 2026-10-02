//! The dictation state machine.
//!
//! [`Machine::handle`] is pure: it takes an event and the current time and
//! returns effects for the controller to run. This keeps every transition
//! testable without audio, an engine, or a UI.
//!
//! The final pass always transcribes the full captured audio. The streaming
//! session only feeds the live preview. This is why "Undo" after a cancel can
//! still produce text: the audio is kept until the undo window closes.

use serde::{Deserialize, Serialize};

pub type SessionId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Trigger {
    /// Toggle hotkey, pill click, popover button.
    Toggle,
    PushToTalk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Transcribing,
    Enhancing,
    Inserting,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Notice {
    Inserted { style_name: Option<String> },
    /// The style failed. The transcript went in as it was.
    InsertedRaw { reason: String },
    /// Text could not be inserted. The card stays until Copy or Dismiss.
    InsertFailed { text: String, reason: String },
    Cancelled,
    Error { message: String },
    /// A dictation was too short (accidental press). No sound, no history.
    Discarded,
}

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Idle,
    Recording { session: SessionId, trigger: Trigger, started_ms: u64 },
    Processing { session: SessionId, stage: Stage, recorded_ms: u64 },
    /// A short message in the overlay. `until_ms` None means it stays until dismissed.
    Showing { session: SessionId, notice: Notice, until_ms: Option<u64> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Toggle,
    PushToTalkDown,
    PushToTalkUp,
    Cancel,
    Undo,
    Dismiss,
    /// The final transcript after replacements. Empty text means silence.
    Transcribed { session: SessionId, text: String },
    TranscribeFailed { session: SessionId, message: String },
    /// The pipeline needs the AI step for this session.
    EnhanceStarted { session: SessionId },
    /// The text to insert is known (style applied, or skipped, or failed).
    ReadyToInsert { session: SessionId, text: String, style_name: Option<String>, enhance_error: Option<String> },
    Inserted { session: SessionId },
    InsertFailed { session: SessionId, text: String, reason: String },
    /// Called on a timer. Expires notices and enforces the maximum duration.
    Tick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    Start,
    Stop,
    Cancel,
    Insert,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Start the microphone and the preview stream.
    StartCapture { session: SessionId },
    /// Stop the microphone. Keep the audio buffer for the final pass or Undo.
    StopCapture { session: SessionId },
    /// Transcribe the captured audio, apply replacements, then send `Transcribed`.
    Transcribe { session: SessionId },
    /// Run the style (or skip it) and send `ReadyToInsert`.
    ApplyStyle { session: SessionId, transcript: String },
    Insert { session: SessionId, text: String },
    /// Drop the audio buffer and the preview stream.
    Discard { session: SessionId },
    /// Write the history entry for this session.
    SaveHistory { session: SessionId },
    PlaySound(Sound),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    pub min_duration_ms: u64,
    pub max_duration_ms: u64,
    pub inserted_ms: u64,
    pub inserted_raw_ms: u64,
    pub cancelled_ms: u64,
    pub error_ms: u64,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            min_duration_ms: 300,
            max_duration_ms: 600_000,
            inserted_ms: 1200,
            inserted_raw_ms: 2600,
            cancelled_ms: 5000,
            error_ms: 4000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Machine {
    pub state: State,
    pub timing: Timing,
    next_session: SessionId,
    /// The session whose audio is still kept for Undo.
    undo_session: Option<(SessionId, u64)>,
    /// The notice to show once the insert succeeds.
    pending_notice: Option<Notice>,
}

impl Default for Machine {
    fn default() -> Self {
        Self::new(Timing::default())
    }
}

impl Machine {
    pub fn new(timing: Timing) -> Self {
        Self { state: State::Idle, timing, next_session: 1, undo_session: None, pending_notice: None }
    }

    pub fn is_recording(&self) -> bool {
        matches!(self.state, State::Recording { .. })
    }

    pub fn current_session(&self) -> Option<SessionId> {
        match self.state {
            State::Idle => None,
            State::Recording { session, .. } | State::Processing { session, .. } | State::Showing { session, .. } => {
                Some(session)
            }
        }
    }

    pub fn handle(&mut self, event: Event, now_ms: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        match (self.state.clone(), event) {
            // Start a dictation from Idle, or over a notice.
            (State::Idle | State::Showing { .. }, Event::Toggle) => self.start(Trigger::Toggle, now_ms, &mut fx),
            (State::Idle | State::Showing { .. }, Event::PushToTalkDown) => {
                self.start(Trigger::PushToTalk, now_ms, &mut fx)
            }

            // Stop recording.
            (State::Recording { session, trigger: Trigger::Toggle, started_ms }, Event::Toggle)
            | (State::Recording { session, trigger: Trigger::PushToTalk, started_ms }, Event::PushToTalkUp) => {
                self.finish(session, started_ms, now_ms, &mut fx)
            }

            // Cancel while recording or processing. The audio is kept for Undo.
            (State::Recording { session, .. }, Event::Cancel) => {
                fx.push(Effect::StopCapture { session });
                self.cancel(session, now_ms, &mut fx);
            }
            (State::Processing { session, .. }, Event::Cancel) => self.cancel(session, now_ms, &mut fx),

            (State::Showing { session, notice: Notice::Cancelled, .. }, Event::Undo) => {
                if self.undo_session.is_some_and(|(s, _)| s == session) {
                    self.undo_session = None;
                    self.state = State::Processing { session, stage: Stage::Transcribing, recorded_ms: 0 };
                    fx.push(Effect::Transcribe { session });
                }
            }

            (State::Processing { session, recorded_ms, .. }, Event::Transcribed { session: s, text }) if s == session => {
                if text.trim().is_empty() {
                    self.state = State::Showing {
                        session,
                        notice: Notice::Error { message: "No speech was heard.".into() },
                        until_ms: Some(now_ms + self.timing.error_ms),
                    };
                    fx.push(Effect::Discard { session });
                } else {
                    self.state = State::Processing { session, stage: Stage::Transcribing, recorded_ms };
                    fx.push(Effect::ApplyStyle { session, transcript: text });
                }
            }
            (State::Processing { session, .. }, Event::TranscribeFailed { session: s, message }) if s == session => {
                self.state = State::Showing {
                    session,
                    notice: Notice::Error { message },
                    until_ms: Some(now_ms + self.timing.error_ms),
                };
                fx.push(Effect::SaveHistory { session });
            }
            (State::Processing { session, recorded_ms, .. }, Event::EnhanceStarted { session: s }) if s == session => {
                self.state = State::Processing { session, stage: Stage::Enhancing, recorded_ms };
            }
            (
                State::Processing { session, recorded_ms, .. },
                Event::ReadyToInsert { session: s, text, style_name, enhance_error },
            ) if s == session => {
                self.state = State::Processing { session, stage: Stage::Inserting, recorded_ms };
                self.pending_notice = Some(match enhance_error {
                    Some(reason) => Notice::InsertedRaw { reason },
                    None => Notice::Inserted { style_name },
                });
                fx.push(Effect::Insert { session, text });
            }
            (State::Processing { session, .. }, Event::Inserted { session: s }) if s == session => {
                let notice = self.pending_notice.take().unwrap_or(Notice::Inserted { style_name: None });
                let duration = match notice {
                    Notice::InsertedRaw { .. } => self.timing.inserted_raw_ms,
                    _ => self.timing.inserted_ms,
                };
                self.state = State::Showing { session, notice, until_ms: Some(now_ms + duration) };
                fx.push(Effect::SaveHistory { session });
                fx.push(Effect::PlaySound(Sound::Insert));
            }
            (State::Processing { session, .. }, Event::InsertFailed { session: s, text, reason }) if s == session => {
                self.pending_notice = None;
                self.state = State::Showing { session, notice: Notice::InsertFailed { text, reason }, until_ms: None };
                fx.push(Effect::SaveHistory { session });
            }

            (State::Showing { .. }, Event::Dismiss) => self.state = State::Idle,

            (State::Recording { session, started_ms, .. }, Event::Tick) => {
                if now_ms.saturating_sub(started_ms) >= self.timing.max_duration_ms {
                    self.finish(session, started_ms, now_ms, &mut fx);
                }
            }
            (State::Showing { session, until_ms: Some(until), notice }, Event::Tick) if now_ms >= until => {
                if notice == Notice::Cancelled
                    && let Some((s, _)) = self.undo_session.take() {
                        fx.push(Effect::Discard { session: s });
                    }
                let _ = session;
                self.state = State::Idle;
            }

            // Everything else is ignored: a toggle while processing, a stale
            // result from a cancelled session, a release without a press.
            _ => {}
        }
        fx
    }

    fn start(&mut self, trigger: Trigger, now_ms: u64, fx: &mut Vec<Effect>) {
        // Starting over a Cancelled notice ends its undo window.
        if let Some((s, _)) = self.undo_session.take() {
            fx.push(Effect::Discard { session: s });
        }
        self.pending_notice = None;
        let session = self.next_session;
        self.next_session += 1;
        self.state = State::Recording { session, trigger, started_ms: now_ms };
        fx.push(Effect::StartCapture { session });
        fx.push(Effect::PlaySound(Sound::Start));
    }

    fn finish(&mut self, session: SessionId, started_ms: u64, now_ms: u64, fx: &mut Vec<Effect>) {
        let recorded_ms = now_ms.saturating_sub(started_ms);
        fx.push(Effect::StopCapture { session });
        if recorded_ms < self.timing.min_duration_ms {
            fx.push(Effect::Discard { session });
            self.state = State::Idle;
            return;
        }
        fx.push(Effect::PlaySound(Sound::Stop));
        fx.push(Effect::Transcribe { session });
        self.state = State::Processing { session, stage: Stage::Transcribing, recorded_ms };
    }

    fn cancel(&mut self, session: SessionId, now_ms: u64, fx: &mut Vec<Effect>) {
        self.pending_notice = None;
        self.undo_session = Some((session, now_ms));
        self.state =
            State::Showing { session, notice: Notice::Cancelled, until_ms: Some(now_ms + self.timing.cancelled_ms) };
        fx.push(Effect::PlaySound(Sound::Cancel));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m() -> Machine {
        Machine::default()
    }

    fn has(fx: &[Effect], want: &Effect) -> bool {
        fx.contains(want)
    }

    /// Drive a full happy path and return the machine.
    fn record_and_transcribe(machine: &mut Machine, text: &str) -> SessionId {
        machine.handle(Event::Toggle, 0);
        machine.handle(Event::Toggle, 2000);
        let s = machine.current_session().unwrap();
        machine.handle(Event::Transcribed { session: s, text: text.into() }, 2100);
        s
    }

    #[test]
    fn toggle_records_and_stops() {
        let mut machine = m();
        let fx = machine.handle(Event::Toggle, 0);
        assert!(has(&fx, &Effect::StartCapture { session: 1 }));
        assert!(has(&fx, &Effect::PlaySound(Sound::Start)));
        assert!(machine.is_recording());
        let fx = machine.handle(Event::Toggle, 1500);
        assert!(has(&fx, &Effect::StopCapture { session: 1 }));
        assert!(has(&fx, &Effect::Transcribe { session: 1 }));
        assert_eq!(machine.state, State::Processing { session: 1, stage: Stage::Transcribing, recorded_ms: 1500 });
    }

    #[test]
    fn happy_path_inserts_and_saves() {
        let mut machine = m();
        let s = record_and_transcribe(&mut machine, "hello");
        machine.handle(Event::EnhanceStarted { session: s }, 2150);
        assert!(matches!(machine.state, State::Processing { stage: Stage::Enhancing, .. }));
        let fx = machine.handle(
            Event::ReadyToInsert { session: s, text: "Hello.".into(), style_name: Some("Clean".into()), enhance_error: None },
            2500,
        );
        assert!(has(&fx, &Effect::Insert { session: s, text: "Hello.".into() }));
        let fx = machine.handle(Event::Inserted { session: s }, 2600);
        assert!(has(&fx, &Effect::SaveHistory { session: s }));
        assert_eq!(
            machine.state,
            State::Showing { session: s, notice: Notice::Inserted { style_name: Some("Clean".into()) }, until_ms: Some(3800) }
        );
        machine.handle(Event::Tick, 3799);
        assert!(matches!(machine.state, State::Showing { .. }));
        machine.handle(Event::Tick, 3800);
        assert_eq!(machine.state, State::Idle);
    }

    #[test]
    fn style_failure_shows_raw_notice_longer() {
        let mut machine = m();
        let s = record_and_transcribe(&mut machine, "hello");
        machine.handle(
            Event::ReadyToInsert {
                session: s,
                text: "hello".into(),
                style_name: None,
                enhance_error: Some("OpenRouter timed out after 4 s".into()),
            },
            2200,
        );
        machine.handle(Event::Inserted { session: s }, 2300);
        assert!(matches!(
            machine.state,
            State::Showing { notice: Notice::InsertedRaw { .. }, until_ms: Some(4900), .. }
        ));
    }

    #[test]
    fn push_to_talk_records_while_held() {
        let mut machine = m();
        machine.handle(Event::PushToTalkDown, 0);
        // A toggle press while holding push-to-talk is ignored.
        assert!(machine.handle(Event::Toggle, 100).is_empty());
        let fx = machine.handle(Event::PushToTalkUp, 900);
        assert!(has(&fx, &Effect::Transcribe { session: 1 }));
    }

    #[test]
    fn short_press_is_discarded_silently() {
        let mut machine = m();
        machine.handle(Event::PushToTalkDown, 0);
        let fx = machine.handle(Event::PushToTalkUp, 120);
        assert!(has(&fx, &Effect::Discard { session: 1 }));
        assert!(!fx.iter().any(|e| matches!(e, Effect::PlaySound(_))));
        assert_eq!(machine.state, State::Idle);
    }

    #[test]
    fn cancel_while_recording_then_undo_transcribes() {
        let mut machine = m();
        machine.handle(Event::Toggle, 0);
        let fx = machine.handle(Event::Cancel, 1000);
        assert!(has(&fx, &Effect::StopCapture { session: 1 }));
        assert!(!fx.iter().any(|e| matches!(e, Effect::Discard { .. })), "audio is kept for undo");
        let fx = machine.handle(Event::Undo, 2000);
        assert!(has(&fx, &Effect::Transcribe { session: 1 }));
        assert!(matches!(machine.state, State::Processing { session: 1, .. }));
    }

    #[test]
    fn cancel_undo_window_expires_and_discards() {
        let mut machine = m();
        machine.handle(Event::Toggle, 0);
        machine.handle(Event::Cancel, 1000);
        let fx = machine.handle(Event::Tick, 6000);
        assert!(has(&fx, &Effect::Discard { session: 1 }));
        assert_eq!(machine.state, State::Idle);
        // Undo after the window does nothing.
        assert!(machine.handle(Event::Undo, 6100).is_empty());
    }

    #[test]
    fn cancel_during_processing_ignores_late_results() {
        let mut machine = m();
        machine.handle(Event::Toggle, 0);
        machine.handle(Event::Toggle, 1000);
        machine.handle(Event::Cancel, 1100);
        assert!(machine.handle(Event::Transcribed { session: 1, text: "late".into() }, 1200).is_empty());
        assert!(matches!(machine.state, State::Showing { notice: Notice::Cancelled, .. }));
    }

    #[test]
    fn starting_over_a_cancel_discards_the_old_audio() {
        let mut machine = m();
        machine.handle(Event::Toggle, 0);
        machine.handle(Event::Cancel, 1000);
        let fx = machine.handle(Event::Toggle, 1500);
        assert!(has(&fx, &Effect::Discard { session: 1 }));
        assert!(has(&fx, &Effect::StartCapture { session: 2 }));
    }

    #[test]
    fn insert_failure_stays_until_dismissed() {
        let mut machine = m();
        let s = record_and_transcribe(&mut machine, "hi");
        machine.handle(Event::ReadyToInsert { session: s, text: "Hi.".into(), style_name: None, enhance_error: None }, 2200);
        let fx = machine.handle(
            Event::InsertFailed { session: s, text: "Hi.".into(), reason: "No text field is focused".into() },
            2300,
        );
        assert!(has(&fx, &Effect::SaveHistory { session: s }));
        machine.handle(Event::Tick, 999_999);
        assert!(matches!(machine.state, State::Showing { notice: Notice::InsertFailed { .. }, until_ms: None, .. }));
        machine.handle(Event::Dismiss, 1_000_000);
        assert_eq!(machine.state, State::Idle);
    }

    #[test]
    fn a_new_dictation_replaces_a_failure_card() {
        let mut machine = m();
        let s = record_and_transcribe(&mut machine, "hi");
        machine.handle(Event::ReadyToInsert { session: s, text: "Hi.".into(), style_name: None, enhance_error: None }, 2200);
        machine.handle(Event::InsertFailed { session: s, text: "Hi.".into(), reason: "x".into() }, 2300);
        let fx = machine.handle(Event::Toggle, 3000);
        assert!(has(&fx, &Effect::StartCapture { session: 2 }));
    }

    #[test]
    fn silence_shows_an_error_and_discards() {
        let mut machine = m();
        let fx = {
            machine.handle(Event::Toggle, 0);
            machine.handle(Event::Toggle, 1000);
            machine.handle(Event::Transcribed { session: 1, text: "  ".into() }, 1100)
        };
        assert!(has(&fx, &Effect::Discard { session: 1 }));
        assert!(matches!(machine.state, State::Showing { notice: Notice::Error { .. }, .. }));
    }

    #[test]
    fn engine_failure_saves_history_so_audio_can_be_retried() {
        let mut machine = m();
        machine.handle(Event::Toggle, 0);
        machine.handle(Event::Toggle, 1000);
        let fx = machine.handle(Event::TranscribeFailed { session: 1, message: "Engine stopped".into() }, 1100);
        assert!(has(&fx, &Effect::SaveHistory { session: 1 }));
    }

    #[test]
    fn max_duration_stops_recording() {
        let mut machine = Machine::new(Timing { max_duration_ms: 10_000, ..Timing::default() });
        machine.handle(Event::Toggle, 0);
        assert!(machine.handle(Event::Tick, 9_999).is_empty());
        let fx = machine.handle(Event::Tick, 10_000);
        assert!(has(&fx, &Effect::Transcribe { session: 1 }));
    }

    #[test]
    fn stale_events_are_ignored() {
        let mut machine = m();
        assert!(machine.handle(Event::PushToTalkUp, 0).is_empty());
        assert!(machine.handle(Event::Undo, 0).is_empty());
        assert!(machine.handle(Event::Inserted { session: 9 }, 0).is_empty());
        assert_eq!(machine.state, State::Idle);
    }
}
