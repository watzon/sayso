package speech

import "time"

// Live sessions (docs/extensions.md, session_start, session_audio,
// session_stop): the types that the host and the app share. The sessions
// themselves are in live_sessions.go (sayso-speech/src/live).

// EngineSessionBase is the first engine session id of live sessions. A
// dictation numbers its engine sessions from 1 up; live sessions take theirs
// from 2^40 up, so the two never meet. 2^40 is under 2^53, so an engine that
// reads JSON numbers as doubles keeps the ids exact.
const EngineSessionBase uint64 = 1 << 40

// Captured is audio of a capture: 16 kHz mono floats.
type Captured struct {
	Samples []float32
	// Time is the time of the first sample on the capture clock, which the
	// sources of one session share. Timed is false when the source has no
	// such clock, and its times are its samples.
	Time  time.Duration
	Timed bool
}

// SourceEnd is why a capture ended before the session.
type SourceEnd struct {
	// Reason is EndPermission (the user took the permission back) or
	// EndFailed (the capture failed and did not come back).
	Reason SourceEndReason
	// Message says why it failed, in English.
	Message string
}

// Feed takes the audio of one capture, in the order of capture. Its methods
// never wait, and calls after the capture stopped are ignored.
type Feed interface {
	Audio(Captured)
	// Gap says that the capture lost this much audio, for example at a
	// change of the device.
	Gap(ms uint64)
	// Ended says that the capture ended before the session. No more audio
	// comes.
	Ended(SourceEnd)
}

// OpenErrorKind is why a capture did not start.
type OpenErrorKind int

const (
	OpenFailed OpenErrorKind = iota
	// OpenPermission: the system refused the permission.
	OpenPermission
	// OpenUnsupported: this system cannot capture it.
	OpenUnsupported
)

// OpenError is a capture that did not start. The message is English.
type OpenError struct {
	Kind    OpenErrorKind
	Message string
}

func (e *OpenError) Error() string { return e.Message }

// Microphone is the shared microphone of the app for live sessions.
type Microphone interface {
	// Open starts the microphone. Its audio goes to feed from now on. stop
	// ends it. err can be an *OpenError.
	Open(feed Feed) (stop func(), err error)
}

// SystemAudio is the capture of the sound that the computer plays.
type SystemAudio interface {
	// Open starts the capture, as Microphone.Open does, without the sound of
	// the app with appPID when it can. It can wait for the answer of the user
	// to a question of the system. excludes says what it leaves out:
	// "sayso", "app".
	Open(appPID *uint32, feed Feed) (stop func(), excludes []string, err error)
}

// Owner is the connection and app of a live session.
type Owner struct {
	Connection uint64
	AppID      string
	// PID is the process of the app, when it is not this process. The
	// capture of the system audio leaves it out.
	PID *uint32
}

// LiveInfo is the live session that runs.
type LiveInfo struct {
	Session uint64
	AppID   string
	Sources []Source
	// CapturesMic is true while it captures the microphone. Sayso shows that
	// it records.
	CapturesMic bool
	// Stopping is true after session_stop, while the last parts and
	// diarization run.
	Stopping bool
}

// Started is a live session that started.
type Started struct {
	Session  uint64
	Excludes []string
}

// LiveError is a live request that failed. Code is empty for none.
type LiveError struct {
	Code    ErrorCode
	Message string
}

func (e *LiveError) Error() string { return e.Message }

// LiveDeps is what live sessions use.
type LiveDeps struct {
	Engine Engine
	Bus    *Bus
	// Microphone and SystemAudio are nil when this app cannot capture them.
	Microphone  Microphone
	SystemAudio SystemAudio
	Gate        *DictationGate
	// Models holds the models of a session while it runs, or nil.
	Models *ModelUse
	// RecordingDir holds the recordings of sessions without record_to.
	RecordingDir string
}

// LiveSessions runs the one live session of the host.
type LiveSessions interface {
	// Start is session_start. Its events go to sink. It blocks while the
	// models load, and while the user answers a question of the system.
	Start(owner Owner, spec SessionSpec, sink EventSink) (Started, *LiveError)
	// Audio is session_audio: base64 of 16 kHz mono 16-bit PCM for the
	// source client.
	Audio(connection, session uint64, pcm string) *LiveError
	// Stop is session_stop: it ends the audio, waits for the last parts,
	// diarizes, and gives the text. The events go to the sink before it
	// returns.
	Stop(connection, session uint64) (string, *LiveError)
	// StopForUser ends the session as Stop does, with session_ended as its
	// end on the sink. It returns at once, false when the session no longer
	// runs or stops already.
	StopForUser(session uint64) bool
	// ConnectionClosed stops the session of the connection without
	// diarization. It returns at once.
	ConnectionClosed(connection uint64)
	Current() (LiveInfo, bool)
	SetObserver(func(*LiveInfo))
	// Close stops the session as ConnectionClosed does; no session starts
	// after it.
	Close()
}
