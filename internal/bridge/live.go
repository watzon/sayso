package bridge

// Live sessions (docs/extensions.md, session_start, session_audio,
// session_stop): the types that the host and the app share.

// Microphone is the shared microphone of the app for live sessions.
type Microphone interface {
	// Open starts the microphone. Each chunk is 16 kHz mono floats. The
	// function that it gives stops it.
	Open(onChunk func([]float32), onEnd func(SourceEndReason)) (stop func(), err error)
}

// SystemAudio is the capture of the sound that the computer plays.
type SystemAudio interface {
	// Open starts the capture without the sound of the app with pid when
	// it can. excludes says what it leaves out: "sayso", "app".
	Open(appPID *uint32, onChunk func([]float32), onEnd func(SourceEndReason)) (stop func(), excludes []string, err error)
}

// Owner is the connection and app of a live session.
type Owner struct {
	Connection uint64
	AppID      string
	PID        *uint32
}

// LiveInfo is the live session that runs.
type LiveInfo struct {
	Session  uint64
	AppID    string
	Sources  []Source
	Stopping bool
}

// Started is a live session that started.
type Started struct {
	Session  uint64
	Excludes []string
}

// LiveError is a live request that failed.
type LiveError struct {
	Code    ErrorCode
	Message string
}

func (e *LiveError) Error() string { return e.Message }

// Reply is the error reply.
func (e *LiveError) Reply() Reply { return ReplyError(e.Code, e.Message) }

// LiveDeps is what live sessions use.
type LiveDeps struct {
	Engine       Engine
	Bus          *bus
	Microphone   Microphone
	SystemAudio  SystemAudio
	Gate         *DictationGate
	Models       *ModelUse
	RecordingDir string
}

// LiveSessions runs the one live session of the host.
type LiveSessions interface {
	Start(owner Owner, spec SessionSpec, sink EventSink) (Started, *LiveError)
	Audio(connection, session uint64, pcm string) *LiveError
	Stop(connection, session uint64) (string, *LiveError)
	StopForUser(session uint64) bool
	ConnectionClosed(connection uint64)
	Current() (LiveInfo, bool)
	SetObserver(func(*LiveInfo))
	Close()
}
