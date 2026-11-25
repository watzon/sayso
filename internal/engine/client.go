package engine

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strconv"
	"sync"
	"sync/atomic"
	"time"
)

// Options configure a Client. The defaults suit the real engine.
type Options struct {
	// EnginePath is the engine program. "" uses DefaultEnginePath.
	EnginePath string
	// ModelsDir is SAYSO_MODELS_DIR.
	ModelsDir string
	// CacheDir holds the temporary WAV files of TranscribeSamples.
	CacheDir string
	// StoreDir, when set, is SAYSO_MODEL_STORE (an engine without store_v1 ignores it).
	StoreDir string
	// Env is more environment for the engine, also after a restart ("K=V").
	Env []string
	// BeforeLaunch runs before each launch and gives more environment, for
	// example the result of the model migration (models.ModelStoreEnv).
	BeforeLaunch func() []string
	// BackoffInitial is the first restart delay after a crash; it doubles up to BackoffMax.
	BackoffInitial time.Duration
	BackoffMax     time.Duration
	// HelloTimeout is how long the first hello may take.
	HelloTimeout time.Duration
	// Args are extra program arguments (tests).
	Args []string
}

// EngineProgram is the engine name beside the app executable.
func EngineProgram() string {
	switch runtime.GOOS {
	case "darwin":
		return "SaysoEngine"
	case "windows":
		return "sayso-engine.exe"
	}
	return "sayso-engine"
}

// DefaultEnginePath is the engine next to the running executable.
func DefaultEnginePath() string {
	exe, err := os.Executable()
	if err != nil {
		return EngineProgram()
	}
	return filepath.Join(filepath.Dir(exe), EngineProgram())
}

// ErrStopped is the error of a request after Close or while the engine is down.
var ErrStopped = errors.New("engine: the engine is not running")

// ErrCrashed is the error of a request that was running when the engine stopped.
var ErrCrashed = errors.New("engine: the engine stopped")

type pending struct {
	reply   chan Message
	onEvent func(Message)
	typ     string
}

type process struct {
	cmd    *exec.Cmd
	stdin  io.WriteCloser
	writes chan []byte
	done   chan struct{}
}

// Client runs an engine process and talks to it. It restarts the engine with
// backoff when it stops, says hello again, refreshes the model states, and
// loads again the models that were loaded.
type Client struct {
	opts   Options
	events chan Event

	mu       sync.Mutex
	proc     *process
	pending  map[string]*pending
	hello    Hello
	statuses map[string]ModelStatus
	engines  map[string]json.RawMessage // the engine object of each model seen
	loaded   map[string]bool
	closed   bool

	nextID   atomic.Uint64
	attempts atomic.Uint32
	dropped  atomic.Uint64
}

// Start launches the engine, says hello, and refreshes nothing else. Events
// arrive on the returned channel; read it, or events are dropped when it is full.
func Start(opts Options) (*Client, <-chan Event, error) {
	if opts.EnginePath == "" {
		opts.EnginePath = DefaultEnginePath()
	}
	if opts.BackoffInitial == 0 {
		opts.BackoffInitial = 500 * time.Millisecond
	}
	if opts.BackoffMax == 0 {
		opts.BackoffMax = 5 * time.Second
	}
	if opts.HelloTimeout == 0 {
		opts.HelloTimeout = 30 * time.Second
	}
	c := &Client{
		opts: opts, events: make(chan Event, 1024), pending: map[string]*pending{},
		statuses: map[string]ModelStatus{}, engines: map[string]json.RawMessage{}, loaded: map[string]bool{},
	}
	if err := c.launch(); err != nil {
		c.emit(Event{Kind: EventStartFailed, Message: err.Error()})
		return nil, c.events, err
	}
	ctx, cancel := context.WithTimeout(context.Background(), opts.HelloTimeout)
	defer cancel()
	if _, err := c.Hello(ctx); err != nil {
		c.emit(Event{Kind: EventStartFailed, Message: err.Error()})
		c.Close()
		return nil, c.events, err
	}
	c.emit(Event{Kind: EventStarted})
	return c, c.events, nil
}

func (c *Client) emit(e Event) {
	select {
	case c.events <- e:
	default:
		slog.Warn("engine event dropped: the channel is full", "kind", e.Kind)
	}
}

func (c *Client) environment() []string {
	env := append(os.Environ(), "SAYSO_MODELS_DIR="+c.opts.ModelsDir)
	if c.opts.StoreDir != "" {
		env = append(env, "SAYSO_MODEL_STORE="+c.opts.StoreDir)
	}
	env = append(env, c.opts.Env...)
	if c.opts.BeforeLaunch != nil {
		env = append(env, c.opts.BeforeLaunch()...)
	}
	return env
}

func (c *Client) launch() error {
	if c.opts.ModelsDir != "" {
		_ = os.MkdirAll(c.opts.ModelsDir, 0o755)
	}
	cmd := exec.Command(c.opts.EnginePath, c.opts.Args...)
	hideConsoleWindow(cmd)
	cmd.Env = c.environment()
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return err
	}
	stderr, err := cmd.StderrPipe()
	if err != nil {
		return err
	}
	if err := cmd.Start(); err != nil {
		return fmt.Errorf("cannot start the engine %s: %w", c.opts.EnginePath, err)
	}
	p := &process{cmd: cmd, stdin: stdin, writes: make(chan []byte, 4096), done: make(chan struct{})}
	c.mu.Lock()
	c.proc = p
	c.mu.Unlock()
	go func() {
		for line := range p.writes {
			if _, err := stdin.Write(line); err != nil {
				break
			}
		}
		stdin.Close()
	}()
	go func() {
		s := bufio.NewScanner(stderr)
		s.Buffer(make([]byte, 64*1024), 4*1024*1024)
		for s.Scan() {
			slog.Debug("engine stderr", "line", s.Text())
		}
	}()
	go c.readLoop(p, stdout)
	return nil
}

func (c *Client) readLoop(p *process, stdout io.Reader) {
	r := bufio.NewReaderSize(stdout, 1<<20)
	for {
		line, err := r.ReadBytes('\n')
		if len(line) > 0 {
			c.handleLine(line)
		}
		if err != nil {
			break
		}
	}
	waitErr := p.cmd.Wait()
	close(p.done)
	c.onExit(p, waitErr)
}

func (c *Client) handleLine(line []byte) {
	m, err := parseMessage(line)
	if err != nil {
		slog.Warn("engine sent a line that is not a message", "err", err)
		return
	}
	switch m.Type {
	case "hello", "ok", "final", "error":
		if m.ID == "" {
			slog.Warn("engine error without a request", "message", m.Message)
			return
		}
		c.mu.Lock()
		p := c.pending[m.ID]
		delete(c.pending, m.ID)
		c.mu.Unlock()
		if p != nil {
			p.reply <- m
		}
		return
	}
	// An event: first the caller's handler, then the event channel.
	if m.ID != "" {
		c.mu.Lock()
		p := c.pending[m.ID]
		c.mu.Unlock()
		if p != nil && p.onEvent != nil {
			p.onEvent(m)
		}
	}
	c.event(m)
}

func statusOf(m Message) ModelStatus {
	s := ModelStatus{State: m.str("state"), Message: m.Message, Reason: m.Reason}
	m.Field("preview", &s.Preview)
	s.LoadMs = m.u64("load_ms")
	if s.State == StateDownloading {
		m.Field("fraction", &s.Fraction)
		s.BytesDone, s.BytesTotal = m.u64("bytes_done"), m.u64("bytes_total")
	}
	return s
}

func (c *Client) event(m Message) {
	switch m.Type {
	case "model_state":
		model, s := m.str("model"), statusOf(m)
		c.mu.Lock()
		// Engines send model_state downloading after each download_progress;
		// one without the progress keeps what download_progress said.
		if prev := c.statuses[model]; s.State == StateDownloading && prev.State == StateDownloading && s.BytesDone == 0 && s.Fraction == 0 {
			s.Fraction, s.BytesDone, s.BytesTotal = prev.Fraction, prev.BytesDone, prev.BytesTotal
		}
		c.statuses[model] = s
		switch s.State {
		case StateReady:
			c.loaded[model] = true
		case StateDownloaded, StateNotDownloaded, StateFailed:
			delete(c.loaded, model)
		}
		c.mu.Unlock()
		c.emit(Event{Kind: EventModelStatus, Model: model, Status: s})
	case "download_progress":
		var f float32
		m.Field("fraction", &f)
		s := ModelStatus{State: StateDownloading, Fraction: f, BytesDone: m.u64("bytes_done"), BytesTotal: m.u64("bytes_total")}
		model := m.str("model")
		c.mu.Lock()
		c.statuses[model] = s
		c.mu.Unlock()
		c.emit(Event{Kind: EventModelStatus, Model: model, Status: s})
	case "partial":
		c.emit(Event{Kind: EventPartial, Session: m.u64("session"), Committed: m.str("committed"), Tentative: m.str("tentative")})
	case "speech_start":
		c.emit(Event{Kind: EventSpeechStart, Session: m.u64("session"), StartMs: m.u64("start_ms")})
	case "speech_end":
		c.emit(Event{Kind: EventSpeechEnd, Session: m.u64("session"), StartMs: m.u64("start_ms"), EndMs: m.u64("end_ms")})
	case "part_state":
		c.emit(Event{Kind: EventPartState, Model: m.str("model"), Part: m.str("part"), State: m.str("state")})
	case "log":
		level := m.str("level")
		slog.Debug("engine log", "level", level, "message", m.Message)
		c.emit(Event{Kind: EventLog, Level: level, Message: m.Message})
	}
}

func (c *Client) onExit(p *process, waitErr error) {
	c.mu.Lock()
	if c.proc != p {
		c.mu.Unlock()
		return
	}
	failed := c.pending
	c.pending = map[string]*pending{}
	closed := c.closed
	c.proc = nil
	c.mu.Unlock()
	close(p.writes)
	for _, pd := range failed {
		pd.reply <- Message{Type: "error", Message: ErrCrashed.Error(), Code: "crashed"}
	}
	if closed {
		return
	}
	msg := "the engine stopped"
	if waitErr != nil {
		msg = "the engine stopped: " + waitErr.Error()
	}
	c.emit(Event{Kind: EventCrashed, Message: msg})
	go c.restart()
}

func (c *Client) nextBackoff() time.Duration {
	attempt := min(c.attempts.Add(1)-1, 16)
	d := c.opts.BackoffInitial << attempt
	if d <= 0 || d > c.opts.BackoffMax {
		d = c.opts.BackoffMax
	}
	return d
}

func (c *Client) restart() {
	for {
		time.Sleep(c.nextBackoff())
		c.mu.Lock()
		closed := c.closed
		c.mu.Unlock()
		if closed {
			return
		}
		if err := c.launch(); err != nil {
			slog.Error("cannot restart the engine", "err", err)
			continue
		}
		if c.recover() {
			return
		}
		// The engine did not answer; kill it, and onExit restarts it again.
		c.kill()
		return
	}
}

// recover: hello, refresh the states, load again what was loaded, Restarted.
func (c *Client) recover() bool {
	ctx, cancel := context.WithTimeout(context.Background(), c.opts.HelloTimeout)
	defer cancel()
	if _, err := c.Hello(ctx); err != nil {
		slog.Warn("the engine did not answer after a restart", "err", err)
		return false
	}
	c.attempts.Store(0)
	c.mu.Lock()
	var refs []ModelRef
	for id, eng := range c.engines {
		refs = append(refs, ModelRef{ID: id, Engine: eng})
	}
	var reload []string
	for id := range c.loaded {
		reload = append(reload, id)
	}
	c.loaded = map[string]bool{}
	c.mu.Unlock()
	sort.Slice(refs, func(i, j int) bool { return refs[i].ID < refs[j].ID })
	sort.Strings(reload)
	if len(refs) > 0 {
		if err := c.ListModels(context.Background(), refs); err != nil {
			slog.Warn("cannot refresh the model states after a restart", "err", err)
		}
	}
	for _, id := range reload {
		c.mu.Lock()
		eng := c.engines[id]
		c.mu.Unlock()
		if _, err := c.Load(context.Background(), id, eng); err != nil {
			slog.Warn("cannot load a model again after a restart", "model", id, "err", err)
		}
	}
	c.emit(Event{Kind: EventRestarted})
	return true
}

func (c *Client) kill() {
	c.mu.Lock()
	p := c.proc
	c.mu.Unlock()
	if p != nil && p.cmd.Process != nil {
		_ = p.cmd.Process.Kill()
	}
}

// send queues a line. Audio lines are dropped (and counted) when the queue is full.
func (c *Client) send(line []byte, droppable bool) error {
	c.mu.Lock()
	p := c.proc
	c.mu.Unlock()
	if p == nil {
		return ErrStopped
	}
	defer func() { _ = recover() }() // the queue closed under us: the engine stopped
	if droppable {
		select {
		case p.writes <- line:
		default:
			c.dropped.Add(1)
		}
		return nil
	}
	select {
	case p.writes <- line:
		return nil
	case <-p.done:
		return ErrStopped
	}
}

// DroppedAudioFrames counts stream_audio lines dropped because the engine was slow.
func (c *Client) DroppedAudioFrames() uint64 { return c.dropped.Load() }

func requestLine(typ, id string, fields map[string]any) []byte {
	m := map[string]any{"v": ProtocolVersion, "type": typ}
	if id != "" {
		m["id"] = id
	}
	for k, v := range fields {
		m[k] = v
	}
	b, _ := json.Marshal(m)
	return append(b, '\n')
}

// Call sends a request and waits for its reply. onEvent gets the events
// with the request's id. An error reply becomes *Error.
func (c *Client) Call(ctx context.Context, typ string, fields map[string]any, onEvent func(Message)) (Message, error) {
	id := typ[:1] + strconv.FormatUint(c.nextID.Add(1), 10)
	pd := &pending{reply: make(chan Message, 1), onEvent: onEvent, typ: typ}
	c.mu.Lock()
	if c.closed {
		c.mu.Unlock()
		return Message{}, ErrStopped
	}
	c.pending[id] = pd
	c.mu.Unlock()
	if err := c.send(requestLine(typ, id, fields), false); err != nil {
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
		return Message{}, err
	}
	select {
	case m := <-pd.reply:
		if m.Type == "error" {
			if m.Code == "crashed" {
				return m, ErrCrashed
			}
			return m, &Error{m.Message, m.Code, m.Reason}
		}
		return m, nil
	case <-ctx.Done():
		c.mu.Lock()
		delete(c.pending, id)
		c.mu.Unlock()
		return Message{}, ctx.Err()
	}
}

// Hello says hello and checks the protocol.
func (c *Client) Hello(ctx context.Context) (Hello, error) {
	m, err := c.Call(ctx, "hello", nil, nil)
	if err != nil {
		return Hello{}, err
	}
	h := Hello{Version: m.str("version")}
	m.Field("protocol", &h.Protocol)
	m.Field("features", &h.Features)
	if h.Protocol != ProtocolVersion {
		return h, fmt.Errorf("engine: protocol %d, this client speaks %d", h.Protocol, ProtocolVersion)
	}
	c.mu.Lock()
	c.hello = h
	c.mu.Unlock()
	return h, nil
}

// Features is the hello of the running engine.
func (c *Client) Features() Hello {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.hello
}

// Status is the last known state of a model.
func (c *Client) Status(model string) (ModelStatus, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	s, ok := c.statuses[model]
	return s, ok
}

// ModelRef is a model id with its engine object (nil uses the engine's catalog).
type ModelRef struct {
	ID     string          `json:"id"`
	Engine json.RawMessage `json:"engine,omitempty"`
}

func (c *Client) remember(id string, eng json.RawMessage) {
	if len(eng) == 0 {
		return
	}
	c.mu.Lock()
	c.engines[id] = eng
	c.mu.Unlock()
}

// ListModels asks for the state of each model; model_state events update Status.
func (c *Client) ListModels(ctx context.Context, refs []ModelRef) error {
	for _, r := range refs {
		c.remember(r.ID, r.Engine)
	}
	if refs == nil {
		refs = []ModelRef{}
	}
	_, err := c.Call(ctx, "list_models", map[string]any{"models": refs}, nil)
	return err
}

// Catalog is list_models without models (feature catalog): the engine's own
// catalog with the states.
func (c *Client) Catalog(ctx context.Context) ([]CatalogModel, []Retired, error) {
	m, err := c.Call(ctx, "list_models", nil, nil)
	if err != nil {
		return nil, nil, err
	}
	var models []CatalogModel
	var retired []Retired
	m.Field("models", &models)
	m.Field("retired", &retired)
	return models, retired, nil
}

func modelFields(model string, eng json.RawMessage) map[string]any {
	f := map[string]any{"model": model}
	if len(eng) > 0 {
		f["engine"] = eng
	}
	return f
}

// Download downloads a model. Progress arrives as model status events.
func (c *Client) Download(ctx context.Context, model string, eng json.RawMessage, sizeBytes uint64) error {
	c.remember(model, eng)
	f := modelFields(model, eng)
	if sizeBytes > 0 {
		f["size_bytes"] = sizeBytes
	}
	_, err := c.Call(ctx, "download", f, nil)
	return err
}

// Delete removes a model's files.
func (c *Client) Delete(ctx context.Context, model string, eng json.RawMessage) error {
	c.remember(model, eng)
	_, err := c.Call(ctx, "delete", modelFields(model, eng), nil)
	return err
}

// Load reads a model into memory and gives the load time in ms.
func (c *Client) Load(ctx context.Context, model string, eng json.RawMessage) (uint64, error) {
	c.remember(model, eng)
	var loadMs uint64
	_, err := c.Call(ctx, "load", modelFields(model, eng), func(m Message) {
		if m.Type == "model_state" && m.str("state") == StateReady {
			loadMs = m.u64("load_ms")
		}
	})
	if err == nil {
		c.mu.Lock()
		c.loaded[model] = true
		c.mu.Unlock()
	}
	return loadMs, err
}

// Unload takes a model out of memory. keepPreview keeps the live preview part
// (feature keep_preview).
func (c *Client) Unload(ctx context.Context, model string, keepPreview bool) error {
	f := map[string]any{"model": model}
	if keepPreview {
		f["keep_preview"] = true
	}
	_, err := c.Call(ctx, "unload", f, nil)
	if err == nil && !keepPreview {
		c.mu.Lock()
		delete(c.loaded, model)
		c.mu.Unlock()
	}
	return err
}

// Prepare loads the optional parts of a loaded model (feature prepare).
func (c *Client) Prepare(ctx context.Context, model string, preview bool, vocabulary []string) error {
	if vocabulary == nil {
		vocabulary = []string{}
	}
	_, err := c.Call(ctx, "prepare", map[string]any{"model": model, "preview": preview, "vocabulary": vocabulary}, nil)
	return err
}

// Transcribe runs the final pass on a WAV file (16 kHz mono 16-bit).
func (c *Client) Transcribe(ctx context.Context, model, wavPath, language string, vocabulary []string, segments bool) (Final, error) {
	if vocabulary == nil {
		vocabulary = []string{}
	}
	f := map[string]any{"model": model, "path": wavPath, "language": language, "vocabulary": vocabulary}
	if segments {
		f["segments"] = true
	}
	m, err := c.Call(ctx, "transcribe", f, nil)
	if err != nil {
		return Final{}, err
	}
	out := Final{Text: m.str("text"), ElapsedMs: m.u64("elapsed_ms"), Language: m.str("language")}
	m.Field("segments", &out.Segments)
	return out, nil
}

// TranscribeSamples writes the samples to a temporary WAV in CacheDir, runs
// Transcribe, and deletes the file.
func (c *Client) TranscribeSamples(ctx context.Context, model string, samples []float32, language string, vocabulary []string) (Final, error) {
	dir := c.opts.CacheDir
	if dir == "" {
		dir = os.TempDir()
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return Final{}, err
	}
	f, err := os.CreateTemp(dir, "dictation-*.wav")
	if err != nil {
		return Final{}, err
	}
	path := f.Name()
	defer os.Remove(path)
	_, werr := f.Write(EncodeWAV(ToPCM16(samples)))
	if cerr := f.Close(); werr == nil {
		werr = cerr
	}
	if werr != nil {
		return Final{}, werr
	}
	return c.Transcribe(ctx, model, path, language, vocabulary, false)
}

// StreamStart opens a live preview stream; partials arrive as events.
func (c *Client) StreamStart(ctx context.Context, session uint64, model, language string) error {
	f := map[string]any{"session": session, "model": model}
	if language != "" {
		f["language"] = language
	}
	_, err := c.Call(ctx, "stream_start", f, nil)
	return err
}

// StreamAudio sends audio to a stream or VAD session. It never blocks; a
// chunk is dropped when the engine falls behind.
func (c *Client) StreamAudio(session uint64, samples []float32) error {
	return c.send(requestLine("stream_audio", "", map[string]any{"session": session, "pcm": PCMBase64(ToPCM16(samples))}), true)
}

// StreamEnd closes a stream and gives its text ("" for a VAD session).
func (c *Client) StreamEnd(ctx context.Context, session uint64) (string, error) {
	m, err := c.Call(ctx, "stream_end", map[string]any{"session": session}, nil)
	return m.str("text"), err
}

// Vad finds the speech in a WAV file (feature vad).
func (c *Client) Vad(ctx context.Context, model, wavPath string) ([]SpeechPart, error) {
	m, err := c.Call(ctx, "vad", map[string]any{"model": model, "path": wavPath}, nil)
	if err != nil {
		return nil, err
	}
	parts := []SpeechPart{}
	m.Field("speech", &parts)
	return parts, nil
}

// VadStart opens a live VAD session; speech events arrive as events.
func (c *Client) VadStart(ctx context.Context, session uint64, model string) error {
	_, err := c.Call(ctx, "vad_start", map[string]any{"session": session, "model": model}, nil)
	return err
}

// Diarize finds who speaks when (feature diarization). speakers 0 lets the engine find the number.
func (c *Client) Diarize(ctx context.Context, model, wavPath string, speakers uint32) (Diarization, error) {
	f := map[string]any{"model": model, "path": wavPath}
	if speakers > 0 {
		f["speakers"] = speakers
	}
	m, err := c.Call(ctx, "diarize", f, nil)
	if err != nil {
		return Diarization{}, err
	}
	var d Diarization
	m.Field("speakers", &d.Speakers)
	m.Field("turns", &d.Turns)
	if d.Turns == nil {
		d.Turns = []Turn{}
	}
	return d, nil
}

// LanguageModel asks for the system language model (feature language_model).
func (c *Client) LanguageModel(ctx context.Context) (LanguageModelState, error) {
	m, err := c.Call(ctx, "language_model", nil, nil)
	if err != nil {
		return LanguageModelState{}, err
	}
	s := LanguageModelState{Model: m.str("model"), Message: m.Message, Reason: m.Reason}
	m.Field("available", &s.Available)
	return s, nil
}

// generateGrace is the time the engine gets beyond its own timeout.
const generateGrace = 500 * time.Millisecond

// Generate runs Apple Intelligence on the prompt (feature language_model).
// Errors carry the code unavailable, refused, or timeout.
func (c *Client) Generate(ctx context.Context, instructions, prompt string, timeout time.Duration, temperature *float64) (Generated, error) {
	f := map[string]any{"instructions": instructions, "prompt": prompt, "timeout_ms": timeout.Milliseconds()}
	if temperature != nil {
		f["temperature"] = *temperature
	}
	ctx, cancel := context.WithTimeout(ctx, timeout+generateGrace)
	defer cancel()
	m, err := c.Call(ctx, "generate", f, nil)
	if errors.Is(err, context.DeadlineExceeded) {
		return Generated{}, &Error{Message: "the language model gave no reply in time", Code: ReasonTimeout, Reason: ReasonTimeout}
	}
	if err != nil {
		return Generated{}, err
	}
	return Generated{Text: m.str("text"), Model: m.str("model")}, nil
}

// Close asks the engine to shut down, waits briefly, and kills it if needed.
// The client does not restart after Close.
func (c *Client) Close() error {
	c.mu.Lock()
	if c.closed {
		c.mu.Unlock()
		return nil
	}
	c.closed = true
	p := c.proc
	c.mu.Unlock()
	if p == nil {
		return nil
	}
	id := "shutdown"
	c.mu.Lock()
	c.pending[id] = &pending{reply: make(chan Message, 1)}
	c.mu.Unlock()
	func() {
		defer func() { _ = recover() }()
		select {
		case p.writes <- requestLine("shutdown", id, nil):
		default:
		}
	}()
	select {
	case <-p.done:
	case <-time.After(2 * time.Second):
		if p.cmd.Process != nil {
			_ = p.cmd.Process.Kill()
		}
		<-p.done
	}
	return nil
}
