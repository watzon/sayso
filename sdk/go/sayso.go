// Package sayso is the Go SDK of Sayso: speech to text for an app, through
// Sayso when it runs (Sayso mode), or through an engine that the app ships
// (Local mode).
//
//	s, err := sayso.Connect(ctx, sayso.Options{Mode: sayso.Auto, App: "Notes",
//		Capabilities: []sayso.Capability{sayso.CapTranscribeFile},
//		Engine: sayso.FromRelease("./engine")})
//	job, err := s.TranscribeFile(ctx, "/path/meeting.wav", sayso.FileOptions{})
//	for ev := range s.Events() { ... }
//
// The modes:
//   - Sayso uses Sayso and fails when it does not run (host_not_running), or
//     starts it with Options.Start.
//   - Local never talks to Sayso.
//   - Auto uses Sayso when it runs, else Local. It never starts Sayso. It
//     changes only while no job or session runs, and each change is a
//     ModeChanged event.
package sayso

import (
	"context"
	"io"
	"slices"
	"sync"
	"time"

	"github.com/watzon/sayso/sdk/go/client"
	"github.com/watzon/sayso/sdk/go/engine"
	"github.com/watzon/sayso/sdk/go/protocol"
)

// Mode is how Connect reaches speech to text.
type Mode int

const (
	Auto Mode = iota
	Sayso
	Local
)

// ActiveMode is what a Speech uses now.
type ActiveMode int

const (
	ModeSayso ActiveMode = iota
	ModeLocal
)

func (m ActiveMode) String() string {
	if m == ModeSayso {
		return "sayso"
	}
	return "local"
}

// Names of the protocol.
type (
	Capability = protocol.Capability
	Role       = protocol.Role
	Source     = protocol.Source
	ModelInfo  = protocol.ModelInfo
	Event      = protocol.Event
)

const (
	CapModelsRead       = protocol.CapModelsRead
	CapModelsDownload   = protocol.CapModelsDownload
	CapTranscribeFile   = protocol.CapTranscribeFile
	CapSessionLive      = protocol.CapSessionLive
	CapAudioMic         = protocol.CapAudioMic
	CapAudioSystem      = protocol.CapAudioSystem
	CapAiEnhance        = protocol.CapAiEnhance
	CapDictationObserve = protocol.CapDictationObserve

	RoleFinal       = protocol.RoleFinal
	RoleStreaming   = protocol.RoleStreaming
	RoleVad         = protocol.RoleVad
	RoleDiarization = protocol.RoleDiarization

	SourceMic    = protocol.SourceMic
	SourceSystem = protocol.SourceSystem
	SourceClient = protocol.SourceClient
)

// EngineSpec is the engine of Local mode.
type EngineSpec struct {
	// Program is the engine program; Folder is a release folder (see
	// engine.Find). Program wins when both are set.
	Program string
	Folder  string
	Env     []string
	Stderr  io.Writer
}

// FromRelease is the engine of an unpacked release file in folder.
func FromRelease(folder string) EngineSpec { return EngineSpec{Folder: folder} }

// FromProgram is an engine program.
func FromProgram(path string) EngineSpec { return EngineSpec{Program: path} }

func (e EngineSpec) program() (string, error) {
	if e.Program != "" {
		return e.Program, nil
	}
	if e.Folder == "" {
		return "", protocol.Errorf(protocol.CodeUnsupported, "Local mode needs an engine: set Options.Engine")
	}
	return engine.Find(e.Folder)
}

// FindEngine is the engine program in a release folder.
func FindEngine(folder string) (string, error) { return engine.Find(folder) }

// ModelsDir is the models folder of Sayso on this system, which Local mode
// shares.
func ModelsDir() string { return protocol.ModelsDir(protocol.DataDir()) }

// ModelStore is the model store of Sayso on this system.
func ModelStore() string { return protocol.ModelStoreDir(ModelsDir()) }

// Options say how Connect works.
type Options struct {
	Mode Mode
	// Start: in Mode Sayso, start Sayso when it does not run, and wait up to
	// StartWait (default 10 s) for it.
	Start     bool
	StartWait time.Duration
	// Launchers start Sayso; nil is DefaultLaunchers.
	Launchers []Launcher
	// App is the name that Sayso shows in its question.
	App          string
	Capabilities []Capability
	// Socket is the socket or pipe of Sayso; "" finds it.
	Socket string
	// OnApprovalWaiting runs when Sayso asks the user about this app.
	OnApprovalWaiting func()
	// Engine and ModelsDir are for Local mode. ModelsDir "" shares the models
	// of Sayso.
	Engine    EngineSpec
	ModelsDir string
	// Capture is the capture of the sources mic and system in Local mode.
	Capture CaptureSources
}

// Features are what the active mode can do.
type Features struct {
	Mode             ActiveMode
	Enhance          bool
	Subscribe        bool
	DownloadQuestion bool
	MicCapture       bool
	SystemCapture    bool
	// Host is hello.features of Sayso (Sayso mode); Engine is the features of
	// the engine (Local mode).
	Host   []string
	Engine []string
}

// ModeChanged is the event of a change of mode in Auto.
type ModeChanged struct {
	Mode     ActiveMode
	Features Features
}

func (ModeChanged) Kind() string { return "mode_changed" }

// FileOptions are the options of TranscribeFile. Empty fields take the
// choice of the user (Sayso) or the first model that fits (Local).
type FileOptions struct {
	Model, VadModel, Language, DiarizationModel string
	Diarize                                     bool
	Speakers                                    uint32
}

// SessionOptions are the options of StartSession.
type SessionOptions struct {
	Sources                                                   []Source
	Model, PreviewModel, VadModel, Language, DiarizationModel string
	Diarize                                                   bool
	// RecordTo is a folder for the WAV files of the sources.
	RecordTo     string
	SilenceLimit time.Duration
}

// Session is a live session that started.
type Session struct {
	ID       uint64
	Excludes []string
}

// EnhanceOptions are the options of Enhance: a style of the user, or a
// prompt, and a provider.
type EnhanceOptions struct {
	Style, Prompt, Provider string
}

// EnhanceResult is the text of Enhance.
type EnhanceResult = protocol.EnhanceReply

type backend interface {
	mode() ActiveMode
	features() Features
	listModels(ctx context.Context, role *Role) ([]ModelInfo, error)
	downloadModel(ctx context.Context, model, reason string, onEvent func(Event)) error
	transcribeFile(ctx context.Context, req protocol.TranscribeFile) (uint64, error)
	cancelJob(ctx context.Context, job uint64) error
	sessionStart(ctx context.Context, req protocol.SessionStart) (protocol.SessionReply, error)
	sessionAudio(session uint64, samples []int16) error
	sessionStop(ctx context.Context, session uint64) (string, error)
	enhance(ctx context.Context, req protocol.Enhance) (protocol.EnhanceReply, error)
	subscribe(ctx context.Context, events []string) ([]string, error)
	events() <-chan Event
	alive() bool
	close()
}

// Speech is speech to text for an app.
type Speech struct {
	opts Options

	mu      sync.Mutex
	active  backend
	local   *localBackend
	jobs    map[uint64]bool
	session map[uint64]bool
	closed  bool

	out  *eventQueue
	wait sync.WaitGroup
}

// Connect reaches speech to text in the mode of opts.
func Connect(ctx context.Context, opts Options) (*Speech, error) {
	s := &Speech{opts: opts, jobs: map[uint64]bool{}, session: map[uint64]bool{}, out: newEventQueue()}
	var b backend
	var err error
	switch opts.Mode {
	case Sayso:
		b, err = s.connectSayso(ctx, opts.Start)
	case Local:
		b, err = s.startLocal(ctx)
	default:
		b, err = s.connectSayso(ctx, false)
		if isCode(err, protocol.CodeHostNotRunning) {
			b, err = s.startLocal(ctx)
		}
	}
	if err != nil {
		s.out.close()
		return nil, err
	}
	s.use(b)
	return s, nil
}

func isCode(err error, code protocol.ErrorCode) bool {
	e, ok := err.(*Error)
	return ok && e.Code == code
}

func (s *Speech) connectSayso(ctx context.Context, start bool) (backend, error) {
	b, err := dialSayso(ctx, s.opts)
	if err == nil || !start || !isCode(err, protocol.CodeHostNotRunning) {
		return b, err
	}
	if err := StartSayso(ctx, s.opts.Launchers); err != nil {
		return nil, err
	}
	wait := s.opts.StartWait
	if wait == 0 {
		wait = 10 * time.Second
	}
	deadline := time.Now().Add(wait)
	for {
		b, err = dialSayso(ctx, s.opts)
		if err == nil || !isCode(err, protocol.CodeHostNotRunning) || time.Now().After(deadline) {
			return b, err
		}
		select {
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-time.After(100 * time.Millisecond):
		}
	}
}

func (s *Speech) startLocal(ctx context.Context) (backend, error) {
	s.mu.Lock()
	l := s.local
	s.mu.Unlock()
	if l != nil && l.alive() {
		return l, nil
	}
	l, err := startLocal(ctx, s.opts)
	if err != nil {
		return nil, err
	}
	s.mu.Lock()
	s.local = l
	s.mu.Unlock()
	return l, nil
}

// use makes b the active backend and forwards its events.
func (s *Speech) use(b backend) {
	s.mu.Lock()
	s.active = b
	s.mu.Unlock()
	s.wait.Add(1)
	go func() {
		defer s.wait.Done()
		for ev := range b.events() {
			s.track(ev)
			s.out.push(ev)
		}
	}()
}

func (s *Speech) track(ev Event) {
	s.mu.Lock()
	defer s.mu.Unlock()
	switch e := ev.(type) {
	case *protocol.JobDone:
		delete(s.jobs, e.Job)
	case *protocol.JobFailed:
		delete(s.jobs, e.Job)
	case *protocol.SessionEnded:
		delete(s.session, e.Session)
	}
}

func (s *Speech) idle() bool { return len(s.jobs) == 0 && len(s.session) == 0 }

// backend is the backend for new work. In Auto it changes mode while idle.
func (s *Speech) backend(ctx context.Context) (backend, error) {
	s.mu.Lock()
	if s.closed {
		s.mu.Unlock()
		return nil, protocol.Errorf(protocol.CodeConnectionLost, "the Speech is closed")
	}
	b, idle := s.active, s.idle()
	s.mu.Unlock()
	switch {
	case s.opts.Mode == Local:
		return b, nil
	case s.opts.Mode == Sayso:
		if b.alive() {
			return b, nil
		}
		if !idle {
			return nil, protocol.Errorf(protocol.CodeConnectionLost, "the connection to Sayso ended")
		}
		nb, err := s.connectSayso(ctx, false)
		if err != nil {
			return nil, err
		}
		b.close()
		s.use(nb)
		return nb, nil
	}
	if !idle {
		return b, nil
	}
	var next backend
	switch {
	case b.mode() == ModeLocal:
		nb, err := dialSayso(ctx, s.opts)
		if err != nil {
			if isCode(err, protocol.CodeHostNotRunning) {
				return b, nil
			}
			return nil, err
		}
		next = nb
	case !b.alive():
		nb, err := s.startLocal(ctx)
		if err != nil {
			return nil, err
		}
		next = nb
	default:
		return b, nil
	}
	if b.mode() == ModeSayso {
		b.close()
	}
	s.use(next)
	s.out.push(ModeChanged{Mode: next.mode(), Features: next.features()})
	return next, nil
}

// Mode is the mode in use.
func (s *Speech) Mode() ActiveMode {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.active.mode()
}

// Features are what the mode in use can do.
func (s *Speech) Features() Features {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.active.features()
}

// Events are the events of jobs and sessions, dictation_done, host_closing,
// and ModeChanged, in order. It closes after Close.
func (s *Speech) Events() <-chan Event { return s.out.out }

// ListModels lists the models; role nil lists all.
func (s *Speech) ListModels(ctx context.Context, role *Role) ([]ModelInfo, error) {
	b, err := s.backend(ctx)
	if err != nil {
		return nil, err
	}
	return b.listModels(ctx, role)
}

// DownloadModel downloads a model. In Sayso mode the user is asked first.
// onEvent gets its model_state and download_progress events.
func (s *Speech) DownloadModel(ctx context.Context, model, reason string, onEvent func(Event)) error {
	b, err := s.backend(ctx)
	if err != nil {
		return err
	}
	return b.downloadModel(ctx, model, reason, onEvent)
}

func optional[T comparable](v T) *T {
	var zero T
	if v == zero {
		return nil
	}
	return &v
}

// TranscribeFile starts a job on a file at an absolute path. Its events come
// on Events, with exactly one end: job_done or job_failed.
func (s *Speech) TranscribeFile(ctx context.Context, path string, o FileOptions) (uint64, error) {
	b, err := s.backend(ctx)
	if err != nil {
		return 0, err
	}
	req := protocol.TranscribeFile{Path: path, Model: optional(o.Model), VadModel: optional(o.VadModel), Language: optional(o.Language), Diarize: optional(o.Diarize), DiarizationModel: optional(o.DiarizationModel), Speakers: optional(o.Speakers)}
	s.mu.Lock()
	defer s.mu.Unlock()
	job, err := b.transcribeFile(ctx, req)
	if err == nil {
		s.jobs[job] = true
	}
	return job, err
}

// CancelJob cancels a job; it ends with job_failed.
func (s *Speech) CancelJob(ctx context.Context, job uint64) error {
	s.mu.Lock()
	b := s.active
	s.mu.Unlock()
	return b.cancelJob(ctx, job)
}

// StartSession starts a live session. Its events come on Events.
func (s *Speech) StartSession(ctx context.Context, o SessionOptions) (Session, error) {
	b, err := s.backend(ctx)
	if err != nil {
		return Session{}, err
	}
	if err := gateSession(b.features(), o); err != nil {
		return Session{}, err
	}
	req := protocol.SessionStart{Sources: o.Sources, Model: optional(o.Model), PreviewModel: optional(o.PreviewModel), VadModel: optional(o.VadModel), Language: optional(o.Language), Diarize: optional(o.Diarize), DiarizationModel: optional(o.DiarizationModel), RecordTo: optional(o.RecordTo)}
	if o.SilenceLimit > 0 {
		ms := uint64(o.SilenceLimit / time.Millisecond)
		req.SilenceLimitMs = &ms
	}
	if req.Sources == nil {
		req.Sources = []Source{SourceClient}
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	r, err := b.sessionStart(ctx, req)
	if err != nil {
		return Session{}, err
	}
	s.session[r.Session] = true
	return Session{ID: r.Session, Excludes: r.Excludes}, nil
}

// SendAudio sends 16 kHz mono audio of the source client.
func (s *Speech) SendAudio(session uint64, samples []int16) error {
	s.mu.Lock()
	b := s.active
	s.mu.Unlock()
	return b.sessionAudio(session, samples)
}

// StopSession stops a live session and gives its text.
func (s *Speech) StopSession(ctx context.Context, session uint64) (string, error) {
	s.mu.Lock()
	b := s.active
	s.mu.Unlock()
	text, err := b.sessionStop(ctx, session)
	s.mu.Lock()
	delete(s.session, session)
	s.mu.Unlock()
	return text, err
}

// Enhance runs a style of the user on text (Sayso mode only).
func (s *Speech) Enhance(ctx context.Context, text string, o EnhanceOptions) (EnhanceResult, error) {
	b, err := s.backend(ctx)
	if err != nil {
		return EnhanceResult{}, err
	}
	if !b.features().Enhance {
		return EnhanceResult{}, unsupported(b, "enhance")
	}
	return b.enhance(ctx, protocol.Enhance{Text: &text, Style: optional(o.Style), Prompt: optional(o.Prompt), Provider: optional(o.Provider)})
}

// Subscribe asks for events of Sayso, such as dictation_done (Sayso mode
// only), and gives the ones that the connection gets now.
func (s *Speech) Subscribe(ctx context.Context, events []string) ([]string, error) {
	b, err := s.backend(ctx)
	if err != nil {
		return nil, err
	}
	f := b.features()
	if !f.Subscribe || (len(f.Host) > 0 && slices.Contains(events, "dictation_done") && !slices.Contains(f.Host, protocol.FeatureDictationDone)) {
		return nil, unsupported(b, "subscribe to dictation_done")
	}
	return b.subscribe(ctx, events)
}

func unsupported(b backend, what string) error {
	if b.mode() == ModeLocal {
		return protocol.Errorf(protocol.CodeUnsupported, "Local mode cannot %s; it needs Sayso", what)
	}
	return protocol.Errorf(protocol.CodeUnsupported, "this version of Sayso cannot %s", what)
}

// gateSession refuses what the host says it lacks. A host with no features
// is older than the list, and gets the request.
func gateSession(f Features, o SessionOptions) error {
	need := map[string]bool{}
	for _, src := range o.Sources {
		switch src {
		case SourceMic:
			need[protocol.FeatureMicCapture] = true
		case SourceSystem:
			need[protocol.FeatureSystemCapture] = true
		}
	}
	if f.Mode == ModeLocal {
		switch {
		case need[protocol.FeatureMicCapture] && !f.MicCapture:
			return protocol.Errorf(protocol.CodeUnsupported, "Local mode captures no microphone here; send the audio with the source client")
		case need[protocol.FeatureSystemCapture] && !f.SystemCapture:
			return protocol.Errorf(protocol.CodeUnsupported, "Local mode cannot capture the sound of this system")
		}
		return nil
	}
	if len(f.Host) == 0 {
		return nil
	}
	if o.RecordTo != "" {
		need[protocol.FeatureRecordTo] = true
	}
	if o.SilenceLimit > 0 {
		need[protocol.FeatureSilenceLimit] = true
	}
	for _, feature := range protocol.HostFeatures {
		if need[feature] && !slices.Contains(f.Host, feature) {
			return protocol.Errorf(protocol.CodeUnsupported, "this Sayso does not serve %s", feature)
		}
	}
	return nil
}

// Close ends the connection or stops the engine, and then the events.
func (s *Speech) Close() error {
	s.mu.Lock()
	if s.closed {
		s.mu.Unlock()
		return nil
	}
	s.closed = true
	b, l := s.active, s.local
	s.mu.Unlock()
	b.close()
	if l != nil && l != b {
		l.close()
	}
	s.wait.Wait()
	s.out.close()
	return nil
}

// saysoBackend is Sayso mode: a client of the Host API.
type saysoBackend struct {
	c     *client.Client
	hello protocol.HelloReply
}

func dialSayso(ctx context.Context, opts Options) (*saysoBackend, error) {
	dctx, cancel := context.WithTimeout(ctx, 2*time.Second)
	c, err := client.Dial(dctx, opts.Socket)
	cancel()
	if err != nil {
		return nil, err
	}
	sdk := "sayso-go/" + Version
	var app *protocol.AppName
	if opts.App != "" {
		app = &protocol.AppName{Name: opts.App}
	}
	hello, err := c.Hello(ctx, protocol.Hello{SDK: &sdk, App: app, Capabilities: opts.Capabilities}, opts.OnApprovalWaiting)
	if err != nil {
		c.Close()
		return nil, err
	}
	return &saysoBackend{c: c, hello: hello}, nil
}

func (b *saysoBackend) mode() ActiveMode { return ModeSayso }

func (b *saysoBackend) features() Features {
	host := b.hello.Features
	has := func(f string) bool { return len(host) == 0 || slices.Contains(host, f) }
	return Features{
		Mode:             ModeSayso,
		Enhance:          has(protocol.FeatureEnhance),
		Subscribe:        true,
		DownloadQuestion: has(protocol.FeatureDownloadQuestion),
		MicCapture:       has(protocol.FeatureMicCapture),
		SystemCapture:    has(protocol.FeatureSystemCapture),
		Host:             slices.Clone(host),
	}
}

func (b *saysoBackend) listModels(ctx context.Context, role *Role) ([]ModelInfo, error) {
	return b.c.ListModels(ctx, role)
}

func (b *saysoBackend) downloadModel(ctx context.Context, model, reason string, onEvent func(Event)) error {
	return b.c.DownloadModel(ctx, model, reason, onEvent)
}

func (b *saysoBackend) transcribeFile(ctx context.Context, req protocol.TranscribeFile) (uint64, error) {
	return b.c.TranscribeFile(ctx, req)
}

func (b *saysoBackend) cancelJob(ctx context.Context, job uint64) error {
	return b.c.CancelJob(ctx, job)
}

func (b *saysoBackend) sessionStart(ctx context.Context, req protocol.SessionStart) (protocol.SessionReply, error) {
	return b.c.SessionStart(ctx, req)
}

func (b *saysoBackend) sessionAudio(session uint64, samples []int16) error {
	return b.c.SessionAudio(session, samples)
}

func (b *saysoBackend) sessionStop(ctx context.Context, session uint64) (string, error) {
	return b.c.SessionStop(ctx, session)
}

func (b *saysoBackend) enhance(ctx context.Context, req protocol.Enhance) (protocol.EnhanceReply, error) {
	return b.c.Enhance(ctx, req)
}

func (b *saysoBackend) subscribe(ctx context.Context, events []string) ([]string, error) {
	return b.c.Subscribe(ctx, events)
}

func (b *saysoBackend) events() <-chan Event { return b.c.Events() }

func (b *saysoBackend) alive() bool {
	select {
	case <-b.c.Done():
		return false
	default:
		return true
	}
}

func (b *saysoBackend) close() { b.c.Close() }

// eventQueue passes events on in order; its channel closes after close and
// the last event.
type eventQueue struct {
	mu     sync.Mutex
	cond   *sync.Cond
	items  []Event
	closed bool
	out    chan Event
}

func newEventQueue() *eventQueue {
	q := &eventQueue{out: make(chan Event)}
	q.cond = sync.NewCond(&q.mu)
	go func() {
		defer close(q.out)
		for {
			q.mu.Lock()
			for len(q.items) == 0 && !q.closed {
				q.cond.Wait()
			}
			if len(q.items) == 0 {
				q.mu.Unlock()
				return
			}
			ev := q.items[0]
			q.items = q.items[1:]
			q.mu.Unlock()
			q.out <- ev
		}
	}()
	return q
}

func (q *eventQueue) push(ev Event) {
	q.mu.Lock()
	q.items = append(q.items, ev)
	q.mu.Unlock()
	q.cond.Signal()
}

func (q *eventQueue) close() {
	q.mu.Lock()
	q.closed = true
	q.mu.Unlock()
	q.cond.Signal()
}
