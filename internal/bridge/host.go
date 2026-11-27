package bridge

import (
	"errors"
	"log/slog"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"sync"
	"sync/atomic"
	"time"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/paths"
	"github.com/watzon/sayso/internal/update"
)

// HostVersion is the version that hello gives when HostConfig names none.
var HostVersion = update.Version

// EngineStart is how far the speech engine got.
type EngineStart int

const (
	EngineStarting EngineStart = iota
	EngineStarted
	EngineFailed
)

// EngineError is an error of the engine with an optional protocol code.
type EngineError struct {
	Code    ErrorCode
	Message string
}

func (e *EngineError) Error() string { return e.Message }

// RawTurn is a turn of the engine, with a number for the speaker.
type RawTurn struct {
	StartMs, EndMs uint64
	Speaker        uint32
}

// Engine is the speech engine of the app (SttBackend in sayso-platform):
// what the host needs of it. Errors can be *EngineError to carry a code.
type Engine interface {
	StartState() EngineStart
	Features() []string
	Status(model string) engine.ModelStatus
	Download(model string) error
	Load(model string) error
	Transcribe(samples []float32, model, language string) (string, error)
	TranscribeSegments(samples []float32, model, language string) ([]Segment, error)
	DetectSpeech(model string, samples []float32) ([]SpeechPart, error)
	Diarize(model string, samples []float32, speakers *uint32) ([]RawTurn, error)
	StartStream(session uint64, model, language string) error
	StartVad(session uint64, model string) error
	PushAudio(session uint64, samples []float32) error
	EndStream(session uint64)
	EndVad(session uint64) error
	// Subscribe gives the events of the engine until stop runs.
	Subscribe() (events <-chan engine.Event, stop func())
}

// DownloadQuestion asks the user about a download for an app.
type DownloadQuestion struct {
	AppID, AppName     string
	ModelID, ModelName string
	SizeBytes          uint64
	Reason             string
}

// StyleRequest asks the app to run a style on text for an app.
type StyleRequest struct {
	Style      *enhance.Style
	Provider   config.Provider
	BasePrompt string
	Text       string
}

// Services is what the app gives the host (HostServices).
type Services interface {
	// Engine is the speech engine, or nil when it does not run.
	Engine() Engine
	Config() *config.Config
	// AskDownload asks the user and blocks until the answer. withdrawn
	// turns true when the question can go away.
	AskDownload(q DownloadQuestion, withdrawn func() bool) bool
	// AskApproval asks the user about an app. answered is false when the
	// question went away without an answer.
	AskApproval(q ApprovalQuestion, withdrawn func() bool) (allow, answered bool)
	// Styles are the styles of the user, or nil when the app runs no AI.
	Styles() *enhance.StyleLibrary
	// RunStyle runs a style for enhance.
	RunStyle(r StyleRequest) enhance.PipelineOutput
	// Microphone and SystemAudio are the sources of live sessions, or nil.
	Microphone() Microphone
	SystemAudio() SystemAudio
	// LiveTempDir holds the recordings of live sessions.
	LiveTempDir() string
}

// BaseServices has the defaults of the optional services. Embed it.
type BaseServices struct{}

func (BaseServices) Styles() *enhance.StyleLibrary { return nil }
func (BaseServices) RunStyle(r StyleRequest) enhance.PipelineOutput {
	err := &enhance.Error{Kind: enhance.ErrNotConfigured, Message: "this host cannot run styles"}
	return enhance.PipelineOutput{Text: r.Text, Enhance: storeFailed(r.Provider.ID, err.Error()), EnhanceError: err, ErrorFrom: r.Provider.ID}
}
func (BaseServices) Microphone() Microphone   { return nil }
func (BaseServices) SystemAudio() SystemAudio { return nil }
func (BaseServices) LiveTempDir() string      { return filepath.Join(os.TempDir(), "sayso-live") }

// StyleNamer is an optional service: the shown (translated) name of a
// style, which enhance also matches.
type StyleNamer interface {
	StyleDisplayName(*enhance.Style) string
}

// HostConfig is how the host starts.
type HostConfig struct {
	Socket           string
	Apps             *AppsFile
	HostVersion      string
	Identify         Identify
	ApprovalCooldown time.Duration
}

// NewHostConfig is the default config for a socket and apps.toml.
func NewHostConfig(socket, appsFile string) HostConfig {
	return HostConfig{Socket: socket, Apps: NewAppsFile(appsFile), HostVersion: HostVersion, Identify: SystemIdentify{}, ApprovalCooldown: approvalCooldown}
}

// HostConfigForPaths uses the socket of this user and <data>/apps.toml.
func HostConfigForPaths(p paths.Paths) (HostConfig, error) {
	socket, err := paths.HostSocketPath()
	if err != nil {
		return HostConfig{}, err
	}
	return NewHostConfig(socket, p.AppsFile()), nil
}

const closeWait = 3 * time.Second

// Host serves connected apps.
type Host struct {
	socket   string
	shared   *shared
	listener *listener
	accepted chan struct{}
}

type shared struct {
	hostVersion string
	identify    Identify
	apps        *AppsFile
	approvals   *approvals
	services    Services
	engine      Engine
	bus         *bus
	downloadsMu sync.Mutex
	downloads   map[string]bool
	questionsMu sync.Mutex
	questions   map[string]*sync.Mutex
	gate        *DictationGate
	modelUse    *ModelUse
	jobs        *JobQueue
	live        LiveSessions
	closing     atomic.Bool
	connMu      sync.Mutex
	connections map[uint64]*entry
	nextConn    atomic.Uint64
	observers   *observers
	serving     sync.WaitGroup
}

func (s *shared) isClosing() bool { return s.closing.Load() }

func (s *shared) questionGate(appID string) *sync.Mutex {
	s.questionsMu.Lock()
	defer s.questionsMu.Unlock()
	m, ok := s.questions[appID]
	if !ok {
		m = &sync.Mutex{}
		s.questions[appID] = m
	}
	return m
}

// stopWorkWithoutGrant stops a live session or the jobs of an app that
// lost a grant that they need.
func (s *shared) stopWorkWithoutGrant() {
	apps := s.apps.Current()
	hasAll := func(appID string, needed []Capability) bool {
		var granted []Capability
		if r := apps.Record(appID); r != nil && r.Allowed {
			granted = r.Capabilities()
		}
		for _, c := range needed {
			if !slices.Contains(granted, c) {
				return false
			}
		}
		return true
	}
	if s.live != nil {
		if info, ok := s.live.Current(); ok && !info.Stopping && !hasAll(info.AppID, SessionCapabilities(info.Sources)) {
			slog.Info("host: live session stops: the grants of its app changed", "session", info.Session)
			s.live.StopForUser(info.Session)
		}
	}
	if s.jobs == nil {
		return
	}
	numbers := s.jobs.Connections()
	sort.Slice(numbers, func(i, j int) bool { return numbers[i] < numbers[j] })
	numbers = slices.Compact(numbers)
	var without []uint64
	s.connMu.Lock()
	for _, n := range numbers {
		if e, ok := s.connections[n]; ok && e.appID != "" && !hasAll(e.appID, []Capability{CapTranscribeFile}) {
			without = append(without, n)
		}
	}
	s.connMu.Unlock()
	for _, n := range without {
		slog.Info("host: the jobs of a connection stop: the grants of its app changed", "connection", n)
		s.jobs.GrantGone(n)
	}
}

// StartHost binds the socket and serves apps.
func StartHost(cfg HostConfig, services Services) (*Host, error) {
	if cfg.Apps == nil {
		return nil, errors.New("host: no apps file")
	}
	if cfg.Identify == nil {
		cfg.Identify = SystemIdentify{}
	}
	if cfg.HostVersion == "" {
		cfg.HostVersion = HostVersion
	}
	l, err := bindListener(cfg.Socket)
	if err != nil {
		return nil, err
	}
	eng := services.Engine()
	gate := NewDictationGate()
	modelUse := NewModelUse()
	obs := &observers{}
	s := &shared{
		hostVersion: cfg.HostVersion,
		identify:    cfg.Identify,
		apps:        cfg.Apps,
		approvals:   newApprovals(cfg.ApprovalCooldown),
		services:    services,
		engine:      eng,
		downloads:   map[string]bool{},
		questions:   map[string]*sync.Mutex{},
		gate:        gate,
		modelUse:    modelUse,
		connections: map[uint64]*entry{},
		observers:   obs,
	}
	s.nextConn.Store(1)
	s.bus = startBus(eng)
	if eng != nil {
		s.jobs = StartJobQueue(NewJobRunner(eng, modelUse), gate)
		s.jobs.SetOnChange(obs.holdsChanged)
		s.live = newLiveSessions(LiveDeps{Engine: eng, Bus: s.bus, Microphone: services.Microphone(), SystemAudio: services.SystemAudio(), Gate: gate, Models: modelUse, RecordingDir: services.LiveTempDir()})
		if s.live != nil {
			s.live.SetObserver(obs.liveChanged)
		}
	}
	h := &Host{socket: cfg.Socket, shared: s, listener: l, accepted: make(chan struct{})}
	go h.acceptLoop()
	slog.Info("host: apps connect", "socket", cfg.Socket)
	return h, nil
}

func (h *Host) acceptLoop() {
	defer close(h.accepted)
	for {
		c, err := h.listener.accept()
		if h.shared.isClosing() {
			if c != nil {
				c.closer()
				if c.release != nil {
					c.release()
				}
			}
			return
		}
		if err != nil {
			slog.Warn("host: accept failed", "err", err)
			time.Sleep(200 * time.Millisecond)
			continue
		}
		h.serveConnection(c)
	}
}

func (h *Host) serveConnection(c *conn) {
	s := h.shared
	s.connMu.Lock()
	n := len(s.connections)
	s.connMu.Unlock()
	if n >= maxConnections {
		slog.Warn("host: too many connections, one more is closed")
		c.closer()
		if c.release != nil {
			c.release()
		}
		return
	}
	number := s.nextConn.Add(1) - 1
	s.serving.Add(1)
	go func() {
		defer s.serving.Done()
		serve(s, number, c)
	}()
}

// ConnectedApps are the ids of the apps that are connected now.
func (h *Host) ConnectedApps() []string {
	h.shared.connMu.Lock()
	var ids []string
	for _, e := range h.shared.connections {
		if e.appID != "" {
			ids = append(ids, e.appID)
		}
	}
	h.shared.connMu.Unlock()
	sort.Strings(ids)
	return ids
}

// Close says host_closing to each app, ends jobs and live sessions, and
// removes the socket. It runs once.
func (h *Host) Close() {
	s := h.shared
	s.connMu.Lock()
	if s.closing.Swap(true) {
		s.connMu.Unlock()
		return
	}
	if s.jobs != nil {
		s.jobs.Close()
	}
	if s.live != nil {
		s.live.Close()
	}
	var writers []chan struct{}
	for _, e := range s.connections {
		if w := e.closeWithHostClosing(); w != nil {
			writers = append(writers, w)
		}
	}
	s.connMu.Unlock()
	if h.listener.wake() {
		<-h.accepted
	}
	for _, w := range writers {
		<-w
	}
	done := make(chan struct{})
	go func() { s.serving.Wait(); close(done) }()
	select {
	case <-done:
	case <-time.After(closeWait):
		slog.Warn("host: a connection did not end in time")
	}
	h.listener.remove()
	s.bus.stop()
	slog.Info("host: closed")
}

// DictationGate holds jobs while a dictation runs. The app calls Begin and
// End around each dictation.
func (h *Host) DictationGate() *DictationGate { return h.shared.gate }

// ModelUse says which models the jobs and live sessions use, for the
// memory policy of the app.
func (h *Host) ModelUse() *ModelUse { return h.shared.modelUse }

// LiveSession is the live session that runs now.
func (h *Host) LiveSession() (LiveInfo, bool) {
	if h.shared.live == nil {
		return LiveInfo{}, false
	}
	return h.shared.live.Current()
}

// StopLiveSession stops a live session for the user.
func (h *Host) StopLiveSession(session uint64) bool {
	return h.shared.live != nil && h.shared.live.StopForUser(session)
}

// StopWorkWithoutGrant runs after the user changes the grants of an app.
func (h *Host) StopWorkWithoutGrant() { h.shared.stopWorkWithoutGrant() }

// Apps is apps.toml.
func (h *Host) Apps() *AppsFile { return h.shared.apps }

// SetLiveObserver runs after each change of the live session.
func (h *Host) SetLiveObserver(f func(*LiveInfo)) { h.shared.observers.setLive(f) }

// Holds are the apps that hold the engine now.
func (h *Host) Holds() []Hold { return holds(h.shared) }

// SetHoldsObserver runs after each change of the holds.
func (h *Host) SetHoldsObserver(f func()) { h.shared.observers.setHolds(f) }

// DictationDone sends a dictation that ended to the apps that subscribed,
// unless incognito was on at its start or end.
func (h *Host) DictationDone(done DictationDone, incognitoAtStart, incognitoAtEnd bool) {
	if incognitoAtStart || incognitoAtEnd {
		return
	}
	sendToSubscribers(h.shared, dictationDoneEvent, done)
}

// Socket is the path of the socket or pipe.
func (h *Host) Socket() string { return h.socket }
