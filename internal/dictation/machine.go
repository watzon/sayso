// Package dictation is the dictation of Sayso: the state machine and the
// controller that runs its effects (microphone,
// live preview, final pass, replacements, spoken punctuation, style,
// insertion, history, sounds, media pause).
package dictation

import "strings"

// SessionID names one dictation.
type SessionID = uint64

// IncognitoDuring is true when incognito was on at the start or at the end
// of a dictation. Such a dictation leaves no trace: no history entry and no
// dictation_done event.
func IncognitoDuring(atStart, atEnd bool) bool { return atStart || atEnd }

// Trigger is how a recording started.
type Trigger int

const (
	// TriggerToggle is the toggle hotkey, a pill click, or a popover button.
	TriggerToggle Trigger = iota
	TriggerPushToTalk
)

// Stage is the step of a dictation that is processed.
type Stage int

const (
	StageTranscribing Stage = iota
	StageEnhancing
	StageInserting
)

// NoticeKind is the kind of a notice in the overlay.
type NoticeKind int

const (
	NoticeInserted NoticeKind = iota
	// NoticeInsertedRaw: the style failed and the transcript went in as it was.
	NoticeInsertedRaw
	// NoticeInsertFailed: the card stays until Copy or Dismiss.
	NoticeInsertFailed
	NoticeCancelled
	NoticeError
	// NoticeNoSpeech: the recording had no speech.
	NoticeNoSpeech
	// NoticeDiscarded: a dictation that was too short.
	NoticeDiscarded
)

// Notice is a short message in the overlay.
type Notice struct {
	Kind      NoticeKind
	StyleName string // Inserted, "" when no style applied
	Reason    string // InsertedRaw, InsertFailed
	Text      string // InsertFailed
	Message   string // Error
}

// StateKind is the kind of a State.
type StateKind int

const (
	StateIdle StateKind = iota
	StateRecording
	StateProcessing
	// StateShowing is a notice; UntilMs 0 with Stays means it stays until dismissed.
	StateShowing
)

// State is the state of the machine.
type State struct {
	Kind       StateKind
	Session    SessionID
	Trigger    Trigger // Recording
	StartedMs  uint64  // Recording
	Stage      Stage   // Processing
	RecordedMs uint64  // Processing
	Notice     Notice  // Showing
	UntilMs    uint64  // Showing
	Stays      bool    // Showing: no UntilMs
}

// Phase is the name of the state: idle, recording, processing, inserting,
// done, cancelled, or failed.
func (s State) Phase() string {
	switch s.Kind {
	case StateRecording:
		return "recording"
	case StateProcessing:
		if s.Stage == StageInserting {
			return "inserting"
		}
		return "processing"
	case StateShowing:
		switch s.Notice.Kind {
		case NoticeInserted, NoticeInsertedRaw:
			return "done"
		case NoticeCancelled:
			return "cancelled"
		case NoticeError, NoticeInsertFailed:
			return "failed"
		}
		return "done"
	}
	return "idle"
}

// EventKind is the kind of an Event.
type EventKind int

const (
	EvToggle EventKind = iota
	EvPushToTalkDown
	EvPushToTalkUp
	EvCancel
	// EvCaptureFailed: the microphone did not start.
	EvCaptureFailed
	EvUndo
	EvDismiss
	// EvTranscribed: the final transcript after replacements. Empty means silence.
	EvTranscribed
	EvTranscribeFailed
	// EvEnhanceStarted: the pipeline needs the AI step.
	EvEnhanceStarted
	// EvReadyToInsert: the text to insert is known.
	EvReadyToInsert
	EvInserted
	EvInsertFailed
	// EvTick expires notices and enforces the maximum duration.
	EvTick
)

// Event is an input of the machine.
type Event struct {
	Kind          EventKind
	Session       SessionID
	Message       string // CaptureFailed, TranscribeFailed
	Text          string // Transcribed, ReadyToInsert, InsertFailed
	StyleName     string // ReadyToInsert
	EnhanceError  string // ReadyToInsert, "" for none
	HasEnhanceErr bool   // ReadyToInsert
	Reason        string // InsertFailed
}

// Sound is a cue.
type Sound int

const (
	SoundStart Sound = iota
	SoundStop
	SoundCancel
	SoundInsert
)

// EffectKind is the kind of an Effect.
type EffectKind int

const (
	// FxStartCapture starts the microphone and the preview stream.
	FxStartCapture EffectKind = iota
	// FxStopCapture stops the microphone and keeps the audio for the final pass or Undo.
	FxStopCapture
	// FxTranscribe transcribes the audio, applies replacements, then sends Transcribed.
	FxTranscribe
	// FxApplyStyle runs the style (or skips it) and sends ReadyToInsert.
	FxApplyStyle
	FxInsert
	// FxDiscard drops the audio and the preview stream.
	FxDiscard
	// FxSaveHistory writes the history entry.
	FxSaveHistory
	FxPlaySound
)

// Effect is an output of the machine for the controller to run.
type Effect struct {
	Kind    EffectKind
	Session SessionID
	Text    string // ApplyStyle: transcript; Insert: text
	Sound   Sound
}

// Timing are the times of the machine in milliseconds.
type Timing struct {
	MinDurationMs, MaxDurationMs                    uint64
	InsertedMs, InsertedRawMs, CancelledMs, ErrorMs uint64
}

// DefaultTiming is the timing of the app.
func DefaultTiming() Timing {
	return Timing{MinDurationMs: 300, MaxDurationMs: 600_000, InsertedMs: 1200, InsertedRawMs: 2600, CancelledMs: 5000, ErrorMs: 4000}
}

// Machine is the dictation state machine. Handle is pure: it takes an event
// and the time and gives effects.
type Machine struct {
	State   State
	Timing  Timing
	next    SessionID
	undo    *SessionID
	pending *Notice
}

// NewMachine is an idle machine.
func NewMachine(t Timing) *Machine { return &Machine{Timing: t, next: 1} }

// IsRecording is true while the microphone records.
func (m *Machine) IsRecording() bool { return m.State.Kind == StateRecording }

// CurrentSession is the session of the state, or false when idle.
func (m *Machine) CurrentSession() (SessionID, bool) {
	if m.State.Kind == StateIdle {
		return 0, false
	}
	return m.State.Session, true
}

func satSub(a, b uint64) uint64 {
	if a < b {
		return 0
	}
	return a - b
}

// Handle runs one event.
func (m *Machine) Handle(e Event, nowMs uint64) []Effect {
	var fx []Effect
	s := m.State
	switch {
	case (s.Kind == StateIdle || s.Kind == StateShowing) && e.Kind == EvToggle:
		m.start(TriggerToggle, nowMs, &fx)
	case (s.Kind == StateIdle || s.Kind == StateShowing) && e.Kind == EvPushToTalkDown:
		m.start(TriggerPushToTalk, nowMs, &fx)
	case s.Kind == StateRecording && ((s.Trigger == TriggerToggle && e.Kind == EvToggle) || (s.Trigger == TriggerPushToTalk && e.Kind == EvPushToTalkUp)):
		m.finish(s.Session, s.StartedMs, nowMs, &fx)
	case s.Kind == StateRecording && e.Kind == EvCancel:
		fx = append(fx, Effect{Kind: FxStopCapture, Session: s.Session})
		m.cancel(s.Session, nowMs, &fx)
	case s.Kind == StateProcessing && e.Kind == EvCancel:
		m.cancel(s.Session, nowMs, &fx)
	case s.Kind == StateRecording && e.Kind == EvCaptureFailed && e.Session == s.Session:
		m.State = State{Kind: StateShowing, Session: s.Session, Notice: Notice{Kind: NoticeError, Message: e.Message}, UntilMs: nowMs + m.Timing.ErrorMs}
		fx = append(fx, Effect{Kind: FxDiscard, Session: s.Session})
	case s.Kind == StateShowing && s.Notice.Kind == NoticeCancelled && e.Kind == EvUndo:
		if m.undo != nil && *m.undo == s.Session {
			m.undo = nil
			m.State = State{Kind: StateProcessing, Session: s.Session, Stage: StageTranscribing}
			fx = append(fx, Effect{Kind: FxTranscribe, Session: s.Session})
		}
	case s.Kind == StateProcessing && e.Kind == EvTranscribed && e.Session == s.Session:
		if strings.TrimSpace(e.Text) == "" {
			m.State = State{Kind: StateShowing, Session: s.Session, Notice: Notice{Kind: NoticeNoSpeech}, UntilMs: nowMs + m.Timing.ErrorMs}
			fx = append(fx, Effect{Kind: FxDiscard, Session: s.Session})
		} else {
			m.State = State{Kind: StateProcessing, Session: s.Session, Stage: StageTranscribing, RecordedMs: s.RecordedMs}
			fx = append(fx, Effect{Kind: FxApplyStyle, Session: s.Session, Text: e.Text})
		}
	case s.Kind == StateProcessing && e.Kind == EvTranscribeFailed && e.Session == s.Session:
		m.State = State{Kind: StateShowing, Session: s.Session, Notice: Notice{Kind: NoticeError, Message: e.Message}, UntilMs: nowMs + m.Timing.ErrorMs}
		fx = append(fx, Effect{Kind: FxSaveHistory, Session: s.Session})
	case s.Kind == StateProcessing && e.Kind == EvEnhanceStarted && e.Session == s.Session:
		m.State = State{Kind: StateProcessing, Session: s.Session, Stage: StageEnhancing, RecordedMs: s.RecordedMs}
	case s.Kind == StateProcessing && e.Kind == EvReadyToInsert && e.Session == s.Session:
		// A cleanup model answers only filler words with no text: insert nothing.
		if strings.TrimSpace(e.Text) == "" {
			m.State = State{Kind: StateShowing, Session: s.Session, Notice: Notice{Kind: NoticeNoSpeech}, UntilMs: nowMs + m.Timing.ErrorMs}
			return append(fx, Effect{Kind: FxDiscard, Session: s.Session})
		}
		m.State = State{Kind: StateProcessing, Session: s.Session, Stage: StageInserting, RecordedMs: s.RecordedMs}
		if e.HasEnhanceErr {
			m.pending = &Notice{Kind: NoticeInsertedRaw, Reason: e.EnhanceError}
		} else {
			m.pending = &Notice{Kind: NoticeInserted, StyleName: e.StyleName}
		}
		fx = append(fx, Effect{Kind: FxInsert, Session: s.Session, Text: e.Text})
	case s.Kind == StateProcessing && e.Kind == EvInserted && e.Session == s.Session:
		notice := Notice{Kind: NoticeInserted}
		if m.pending != nil {
			notice = *m.pending
			m.pending = nil
		}
		d := m.Timing.InsertedMs
		if notice.Kind == NoticeInsertedRaw {
			d = m.Timing.InsertedRawMs
		}
		m.State = State{Kind: StateShowing, Session: s.Session, Notice: notice, UntilMs: nowMs + d}
		fx = append(fx, Effect{Kind: FxSaveHistory, Session: s.Session}, Effect{Kind: FxPlaySound, Sound: SoundInsert})
	case s.Kind == StateProcessing && e.Kind == EvInsertFailed && e.Session == s.Session:
		m.pending = nil
		m.State = State{Kind: StateShowing, Session: s.Session, Notice: Notice{Kind: NoticeInsertFailed, Text: e.Text, Reason: e.Reason}, Stays: true}
		fx = append(fx, Effect{Kind: FxSaveHistory, Session: s.Session})
	case s.Kind == StateShowing && e.Kind == EvDismiss:
		m.State = State{Kind: StateIdle}
	case s.Kind == StateRecording && e.Kind == EvTick:
		if satSub(nowMs, s.StartedMs) >= m.Timing.MaxDurationMs {
			m.finish(s.Session, s.StartedMs, nowMs, &fx)
		}
	case s.Kind == StateShowing && !s.Stays && e.Kind == EvTick && nowMs >= s.UntilMs:
		if s.Notice.Kind == NoticeCancelled && m.undo != nil {
			fx = append(fx, Effect{Kind: FxDiscard, Session: *m.undo})
			m.undo = nil
		}
		m.State = State{Kind: StateIdle}
	}
	// Everything else is ignored: a toggle while processing, a stale result
	// from a cancelled session, a release without a press.
	return fx
}

func (m *Machine) start(t Trigger, nowMs uint64, fx *[]Effect) {
	// Starting over a Cancelled notice ends its undo window.
	if m.undo != nil {
		*fx = append(*fx, Effect{Kind: FxDiscard, Session: *m.undo})
		m.undo = nil
	}
	m.pending = nil
	session := m.next
	m.next++
	m.State = State{Kind: StateRecording, Session: session, Trigger: t, StartedMs: nowMs}
	// The cue first: a USB microphone takes 75 to 120 ms to open.
	*fx = append(*fx, Effect{Kind: FxPlaySound, Sound: SoundStart}, Effect{Kind: FxStartCapture, Session: session})
}

func (m *Machine) finish(session SessionID, startedMs, nowMs uint64, fx *[]Effect) {
	recorded := satSub(nowMs, startedMs)
	*fx = append(*fx, Effect{Kind: FxStopCapture, Session: session})
	if recorded < m.Timing.MinDurationMs {
		*fx = append(*fx, Effect{Kind: FxDiscard, Session: session})
		m.State = State{Kind: StateIdle}
		return
	}
	*fx = append(*fx, Effect{Kind: FxPlaySound, Sound: SoundStop}, Effect{Kind: FxTranscribe, Session: session})
	m.State = State{Kind: StateProcessing, Session: session, Stage: StageTranscribing, RecordedMs: recorded}
}

func (m *Machine) cancel(session SessionID, nowMs uint64, fx *[]Effect) {
	m.pending = nil
	s := session
	m.undo = &s
	m.State = State{Kind: StateShowing, Session: session, Notice: Notice{Kind: NoticeCancelled}, UntilMs: nowMs + m.Timing.CancelledMs}
	*fx = append(*fx, Effect{Kind: FxPlaySound, Sound: SoundCancel})
}
