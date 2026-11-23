package dictation

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"math"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/watzon/sayso/internal/bridge"
	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/platform"
	"github.com/watzon/sayso/internal/providers"
	"github.com/watzon/sayso/internal/store"
)

// The controller of a dictation (crates/sayso-app/src/dictation.rs without
// GPUI): one goroutine owns the machine and the sessions; effects that
// block run on their own goroutine and send their result back as an event.

// SelfBundleID is the app id that a dictation into Sayso itself records.
const SelfBundleID = "dev.sayso.Sayso"

// Message is a text for the user: an i18n key and its arguments. The UI
// translates it.
type Message struct {
	Key  string
	Args map[string]any
}

func msg(key string, kv ...any) Message {
	m := Message{Key: key}
	if len(kv) > 0 {
		m.Args = map[string]any{}
		for i := 0; i+1 < len(kv); i += 2 {
			m.Args[fmt.Sprint(kv[i])] = kv[i+1]
		}
	}
	return m
}

// String is the key with its arguments, for logs and the machine.
func (m Message) String() string {
	if len(m.Args) == 0 {
		return m.Key
	}
	keys := make([]string, 0, len(m.Args))
	for k := range m.Args {
		keys = append(keys, k)
	}
	slices.Sort(keys)
	parts := make([]string, len(keys))
	for i, k := range keys {
		parts[i] = fmt.Sprintf("%s=%v", k, m.Args[k])
	}
	return m.Key + "{" + strings.Join(parts, ",") + "}"
}

// SessionOptions are the models and words of a dictation.
type SessionOptions struct {
	FinalModel   models.ModelID
	PreviewModel models.ModelID // "" for none
	Language     string
	Vocabulary   []string
}

// Transcript is the text of a final pass.
type Transcript struct {
	Text      string
	Model     models.ModelID
	ElapsedMs uint64
}

// Engine is the local speech engine for the final pass and the live preview.
type Engine interface {
	Status(model string) engine.ModelStatus
	// WaitReady waits until the model is in memory, failed, or the deadline.
	WaitReady(model string, deadline time.Time) engine.ModelStatus
	// Load starts loading the model (the idle time can unload it).
	Load(model string) error
	Transcribe(samples []float32, opts SessionOptions) (Transcript, error)
	// PreviewReady is true when the live preview of model is in memory.
	PreviewReady(model string) bool
	StartStream(session uint64, opts SessionOptions) error
	PushAudio(session uint64, samples []float32) error
	EndStream(session uint64)
}

// History is where dictations go (*store.Store).
type History interface {
	InsertEntry(e store.HistoryEntry) (int64, error)
	SaveAudio(samples []float32) (string, error)
	RecordReplacementUses(applied []store.AppliedReplacement) error
}

// Host tells connected apps about dictations and holds their jobs.
type Host interface {
	DictationDone(done bridge.DictationDone, incognitoAtStart, incognitoAtEnd bool)
	DictationGate() *bridge.DictationGate
}

// StyleJob is one style step.
type StyleJob struct {
	Style        *enhance.Style
	Provider     config.Provider
	Text         string
	Replacements []store.AppliedReplacement
	App, Site    string
	Language     string
	SpeechModel  models.ModelID
	Vocabulary   []string
	BasePrompt   string
	// Context ends when the dictation is cancelled; a local cleanup model
	// stops then. nil means none.
	Context context.Context
}

// Styler runs a style step. It blocks.
type Styler interface {
	Run(job StyleJob) enhance.PipelineOutput
}

// Preparer is a Styler that can get a provider ready while the user speaks:
// a spare claude process, or the cleanup model loaded. Prepare returns at once.
type Preparer interface {
	Prepare(style *enhance.Style, provider config.Provider)
}

// Observer hears what the overlay shows. Calls come from the controller
// goroutine and must not block.
type Observer interface {
	StateChanged(State)
	Notice(Message)
	Level(rms float32)
	Preview(committed, tentative string)
	// HistoryChanged: a dictation's history entry and its replacement
	// counts were written. The write runs on its own goroutine and ends
	// after the dictation's last state change, so a view that read the
	// history for that change reads it again now. Called on that goroutine.
	HistoryChanged()
}

// Deps is what the controller uses. Config and Styles are read at each use.
type Deps struct {
	Platform   platform.Services
	Engine     Engine // nil: no local engine
	Secrets    providers.SecretStore
	History    History // nil: no history
	Host       Host    // nil: no Host API
	Styler     Styler  // nil: DefaultStyler
	Config     func() *config.Config
	Styles     func() *enhance.StyleLibrary
	Replacer   func() *enhance.Replacer
	Vocabulary func() []string
	Incognito  func() bool
	// SoundData is the WAV of a cue (internal/theme has them).
	SoundData func(platform.SoundCue) []byte
	// StyleName is the shown name of a style; nil uses Style.Name.
	StyleName  func(*enhance.Style) string
	System     func() store.SystemInfo
	AppVersion string
	Observer   Observer
	Timing     *Timing
	// Now is the clock; nil is time.Now.
	Now func() time.Time
	// TickEvery is the period of Tick; 0 is 100 ms.
	TickEvery time.Duration
}

type session struct {
	app          *platform.ActiveApp
	ruleStyle    string
	pickedStyle  string
	samplesMu    sync.Mutex
	samples      []float32
	capturing    bool
	streaming    atomic.Bool
	opening      bool
	started      time.Time
	stopped      time.Time
	durationMs   uint64
	transcript   *string
	transcribeMs uint64
	model        models.ModelID
	replacements []store.AppliedReplacement
	styleID      string
	enhance      *store.EnhanceOutcome
	finalText    *string
	insert       *store.InsertOutcome
	incognito    bool
	diagnostics  store.Diagnostics
	cancelStyle  context.CancelFunc // ends the context of a running style step
}

// Controller runs dictations.
type Controller struct {
	d        Deps
	machine  *Machine
	sessions map[SessionID]*session
	work     chan func()
	quit     chan struct{}
	done     chan struct{}
	epoch    time.Time
	// media pauses and resumes the media player in order, off this
	// goroutine; nil without a media service. mediaDone closes once
	// mediaLoop ended, after Close.
	media     chan bool
	mediaDone chan struct{}
	// trackedRecording is the recording state the hotkeys and the media
	// were last told.
	trackedRecording bool
	pending          string
	stopOnce         sync.Once

	stateMu sync.Mutex
	state   State
	last    string
}

// New starts a controller.
func New(d Deps) *Controller {
	if d.Now == nil {
		d.Now = time.Now
	}
	if d.TickEvery == 0 {
		d.TickEvery = 100 * time.Millisecond
	}
	if d.Styler == nil {
		d.Styler = &DefaultStyler{Secrets: d.Secrets, Config: d.Config}
	}
	if d.Incognito == nil {
		d.Incognito = func() bool { return false }
	}
	t := DefaultTiming()
	if d.Timing != nil {
		t = *d.Timing
	}
	c := &Controller{d: d, machine: NewMachine(t), sessions: map[SessionID]*session{}, work: make(chan func(), 256), quit: make(chan struct{}), done: make(chan struct{}), epoch: d.Now()}
	if d.Platform.Media != nil {
		c.media, c.mediaDone = make(chan bool, 16), make(chan struct{})
		go func() {
			defer close(c.mediaDone)
			mediaLoop(d.Platform.Media, c.media, c.quit)
		}()
	}
	go c.loop()
	return c
}

func (c *Controller) loop() {
	defer close(c.done)
	// The clock runs only while the machine can time out (needsTick), so
	// the app does not wake up ten times a second at rest.
	tick := time.NewTicker(c.d.TickEvery)
	defer tick.Stop()
	ticking := c.needsTick()
	if !ticking {
		tick.Stop()
	}
	for {
		select {
		case f := <-c.work:
			f()
		case <-tick.C:
			c.dispatch(Event{Kind: EvTick})
		case <-c.quit:
			return
		}
		switch need := c.needsTick(); {
		case need && !ticking:
			tick.Reset(c.d.TickEvery)
			ticking = true
		case !need && ticking:
			tick.Stop()
			ticking = false
		}
	}
}

// needsTick is true in the states that EvTick can end: a recording (its
// maximum duration) and a notice that goes away by itself.
func (c *Controller) needsTick() bool {
	s := c.machine.State
	return s.Kind == StateRecording || s.Kind == StateShowing && !s.Stays
}

// do runs f on the controller goroutine.
func (c *Controller) do(f func()) {
	select {
	case c.work <- f:
	case <-c.quit:
	}
}

// Close stops the controller. A recording stops without a trace.
func (c *Controller) Close() {
	c.stopOnce.Do(func() {
		wait := make(chan struct{})
		c.do(func() {
			for id, s := range c.sessions {
				c.stopMic(s)
				if s.streaming.Load() && c.d.Engine != nil {
					c.d.Engine.EndStream(id)
				}
			}
			close(wait)
		})
		select {
		case <-wait:
		case <-time.After(time.Second):
		}
		close(c.quit)
		<-c.done
		// The resume of the music a dictation paused runs before the app
		// exits, within reason: the macOS player takes a second or two.
		if c.mediaDone != nil {
			select {
			case <-c.mediaDone:
			case <-time.After(mediaQuitWait):
			}
		}
	})
}

// mediaQuitWait bounds how long Close waits for the media player.
const mediaQuitWait = 3 * time.Second

// Dispatch sends an event to the machine.
func (c *Controller) Dispatch(e Event) { c.do(func() { c.dispatch(e) }) }

func (c *Controller) Toggle()         { c.Dispatch(Event{Kind: EvToggle}) }
func (c *Controller) PushToTalkDown() { c.Dispatch(Event{Kind: EvPushToTalkDown}) }
func (c *Controller) PushToTalkUp()   { c.Dispatch(Event{Kind: EvPushToTalkUp}) }
func (c *Controller) Cancel()         { c.Dispatch(Event{Kind: EvCancel}) }
func (c *Controller) Undo()           { c.Dispatch(Event{Kind: EvUndo}) }
func (c *Controller) Dismiss()        { c.Dispatch(Event{Kind: EvDismiss}) }

// PickStyle chooses the style of the dictation that records now, or of the
// next one.
func (c *Controller) PickStyle(id string) {
	c.do(func() {
		if c.machine.State.Kind == StateRecording {
			if s := c.sessions[c.machine.State.Session]; s != nil {
				s.pickedStyle = id
				return
			}
		}
		c.pending = id
	})
}

// State is the state now.
func (c *Controller) State() State {
	c.stateMu.Lock()
	defer c.stateMu.Unlock()
	return c.state
}

// LastText is the text of the last insert.
func (c *Controller) LastText() string {
	c.stateMu.Lock()
	defer c.stateMu.Unlock()
	return c.last
}

// EnginePartial passes a live preview of the engine to the overlay.
func (c *Controller) EnginePartial(session uint64, committed, tentative string) {
	c.do(func() {
		if c.machine.State.Kind == StateRecording && c.machine.State.Session == session && c.d.Observer != nil {
			c.d.Observer.Preview(committed, tentative)
		}
	})
}

// ModelReady starts a late live preview when the model of the preview
// loaded during a recording.
func (c *Controller) ModelReady(model models.ModelID) {
	c.do(func() {
		if c.machine.State.Kind != StateRecording {
			return
		}
		id := c.machine.State.Session
		if c.previewModel() != model {
			return
		}
		if s := c.sessions[id]; s != nil && !s.streaming.Load() && !s.opening {
			c.startPreview(id, false)
		}
	})
}

func (c *Controller) nowMs() uint64 { return uint64(c.d.Now().Sub(c.epoch).Milliseconds()) }

func (c *Controller) config() *config.Config {
	if c.d.Config == nil {
		cfg := config.Default()
		return &cfg
	}
	return c.d.Config()
}

func (c *Controller) notice(m Message) {
	slog.Info("dictation: notice", "message", m.String())
	if c.d.Observer != nil {
		c.d.Observer.Notice(m)
	}
}

func (c *Controller) dispatch(e Event) {
	starting := (e.Kind == EvToggle || e.Kind == EvPushToTalkDown) && c.machine.State.Kind != StateRecording && c.machine.State.Kind != StateProcessing
	if starting && !c.canStart() {
		return
	}
	wasRecording := c.machine.IsRecording()
	effects := c.machine.Handle(e, c.nowMs())
	// The overlay shows the new state at once: the effects can take a while
	// (the focused app, opening or closing the microphone), and a failure
	// comes as an event of its own.
	c.track(c.machine.State, c.machine.IsRecording(), wasRecording)
	c.publish(c.machine.State)
	for _, fx := range effects {
		c.run(fx)
	}
	c.track(c.machine.State, c.machine.IsRecording(), wasRecording)
	c.publish(c.machine.State)
}

// track holds the dictation gate while a dictation runs, and tells the
// hotkeys and the media player when the recording starts or stops. It
// does each change once.
func (c *Controller) track(st State, recording, wasRecording bool) {
	if c.d.Host != nil {
		if g := c.d.Host.DictationGate(); g != nil {
			if st.Kind == StateRecording || st.Kind == StateProcessing {
				g.Begin()
			} else {
				g.End()
			}
		}
	}
	if recording == wasRecording || recording == c.trackedRecording {
		return
	}
	c.trackedRecording = recording
	if c.d.Platform.Hotkeys != nil {
		c.d.Platform.Hotkeys.SetRecording(recording)
	}
	if c.media != nil && (!recording || c.config().Audio.PauseMedia) {
		select {
		case c.media <- recording:
		case <-c.quit:
		}
	}
}

// publish tells the observer the state, when it changed.
func (c *Controller) publish(st State) {
	c.stateMu.Lock()
	changed := c.state != st
	c.state = st
	c.stateMu.Unlock()
	if changed && c.d.Observer != nil {
		c.d.Observer.StateChanged(st)
	}
}

// mediaLoop pauses the media player for true and resumes it for false, in
// order. The macOS adapter runs a program for each, which can take a
// second or more, so it never runs on the controller goroutine: the
// overlay would wait for it. On quit it resumes what it paused, whether or
// not the resume queued behind the pause was taken first.
func mediaLoop(media platform.MediaController, ops <-chan bool, quit <-chan struct{}) {
	was := false
	for {
		select {
		case <-quit:
			if was {
				if err := media.Resume(); err != nil {
					slog.Warn("dictation: media resume failed", "err", err)
				}
			}
			return
		case pause := <-ops:
			switch {
			case pause && !was:
				w, err := media.Pause()
				if err != nil {
					slog.Warn("dictation: media pause failed", "err", err)
				}
				was = w
			case !pause:
				// The resume plays only what the pause stopped.
				if was {
					if err := media.Resume(); err != nil {
						slog.Warn("dictation: media resume failed", "err", err)
					}
				}
				was = false
			}
		}
	}
}

// canStart checks what a dictation needs and shows a notice when something
// is missing.
func (c *Controller) canStart() bool {
	if p := c.d.Platform.Permissions; p != nil && !p.Status().Microphone {
		if err := p.RequestMicrophone(); err != nil {
			slog.Warn("dictation: microphone permission", "err", err)
		}
		c.notice(msg("overlay.notice.needs_microphone"))
		return false
	}
	cfg := c.config()
	model := c.activeModel(cfg)
	if model.IsRemote() {
		if !c.cloudUsable(cfg, model.ID) {
			c.notice(msg("overlay.notice.set_up_cloud_provider"))
			return false
		}
		return true
	}
	if c.d.Engine == nil {
		c.notice(msg("overlay.notice.no_engine"))
		return false
	}
	if !c.d.Engine.Status(string(model.ID)).IsOnDisk() {
		c.notice(msg("overlay.notice.download_model"))
		return false
	}
	return true
}

func (c *Controller) cloudUsable(cfg *config.Config, id models.ModelID) bool {
	providerID, _, ok := SplitModelID(id)
	if !ok {
		return false
	}
	p, ok := cfg.Speech.Provider(providerID)
	if !ok {
		return false
	}
	return p.APIKeyAccount == nil || (c.d.Secrets != nil && c.d.Secrets.Get(*p.APIKeyAccount) != "")
}

// SplitModelID splits "<provider id>:<model>".
func SplitModelID(id models.ModelID) (provider, model string, ok bool) {
	return strings.Cut(string(id), ":")
}

func (c *Controller) activeModel(cfg *config.Config) models.ModelInfo {
	if m, ok := models.FindWith(cfg.Speech.Catalog(), cfg.Dictation.Model); ok {
		return m
	}
	if _, _, ok := SplitModelID(cfg.Dictation.Model); ok {
		return models.ModelInfo{ID: cfg.Dictation.Model, Name: string(cfg.Dictation.Model), Engine: models.EngineKind{Kind: models.KindRemote}, FinalPass: true}
	}
	return models.ModelInfo{ID: cfg.Dictation.Model, Name: string(cfg.Dictation.Model), FinalPass: true}
}

func (c *Controller) previewModel() models.ModelID {
	cfg := c.config()
	if !cfg.Dictation.LivePreview {
		return ""
	}
	active := c.activeModel(cfg)
	onDisk := func(id models.ModelID) bool { return c.d.Engine != nil && c.d.Engine.Status(string(id)).IsOnDisk() }
	if id, ok := models.PreviewFor(active, cfg.Dictation.PreviewModel, cfg.Dictation.Language, onDisk); ok {
		return id
	}
	return ""
}

func (c *Controller) vocabulary() []string {
	if c.d.Vocabulary == nil {
		return nil
	}
	return c.d.Vocabulary()
}

func (c *Controller) sessionOptions() SessionOptions {
	cfg := c.config()
	return SessionOptions{FinalModel: c.activeModel(cfg).ID, PreviewModel: c.previewModel(), Language: cfg.Dictation.Language, Vocabulary: c.vocabulary()}
}

func (c *Controller) session(id SessionID) *session {
	s, ok := c.sessions[id]
	if !ok {
		s = &session{}
		c.sessions[id] = s
	}
	return s
}

func (c *Controller) run(fx Effect) {
	switch fx.Kind {
	case FxStartCapture:
		c.startCapture(fx.Session)
	case FxStopCapture:
		c.stopCapture(fx.Session)
	case FxTranscribe:
		c.transcribe(fx.Session)
	case FxApplyStyle:
		c.applyStyle(fx.Session, fx.Text)
	case FxInsert:
		c.insert(fx.Session, fx.Text)
	case FxDiscard:
		if s, ok := c.sessions[fx.Session]; ok {
			delete(c.sessions, fx.Session)
			if s.cancelStyle != nil {
				s.cancelStyle()
			}
			c.stopMic(s)
			if s.streaming.Load() && c.d.Engine != nil {
				c.d.Engine.EndStream(fx.Session)
			}
		}
	case FxSaveHistory:
		c.saveHistory(fx.Session)
	case FxPlaySound:
		c.play(fx.Sound)
	}
}

func (c *Controller) play(s Sound) {
	cfg := c.config().Sounds
	var cue platform.SoundCue
	var on bool
	switch s {
	case SoundStart:
		cue, on = platform.CueStart, cfg.Start
	case SoundStop:
		cue, on = platform.CueStop, cfg.Stop
	case SoundCancel:
		cue, on = platform.CueCancel, cfg.Cancel
	case SoundInsert:
		cue, on = platform.CueInsert, cfg.Insert
	}
	if !cfg.Enabled || !on || c.d.Platform.Sounds == nil {
		return
	}
	var wav []byte
	if c.d.SoundData != nil {
		wav = c.d.SoundData(cue)
	}
	go func() {
		if err := c.d.Platform.Sounds.Play(cue, wav, cfg.Volume); err != nil {
			slog.Debug("dictation: sound failed", "cue", cue, "err", err)
		}
	}()
}

func (c *Controller) stopMic(s *session) {
	if s.capturing && c.d.Platform.Audio != nil {
		if err := c.d.Platform.Audio.Stop(); err != nil {
			slog.Warn("dictation: microphone stop failed", "err", err)
		}
	}
	s.capturing = false
}

func (c *Controller) startCapture(id SessionID) {
	requested := time.Now()
	s := c.session(id)
	if c.pending != "" {
		s.pickedStyle, c.pending = c.pending, ""
	}
	if ctx := c.d.Platform.Context; ctx != nil {
		if app, err := ctx.ActiveApp(); err == nil && (app.BundleID != "" || app.Name != "") {
			a := app
			s.app = &a
		}
	}
	cfg := c.config()
	if c.d.Engine != nil {
		model := c.activeModel(cfg)
		if !model.IsRemote() && !c.d.Engine.Status(string(model.ID)).IsUsable() {
			go func() {
				if err := c.d.Engine.Load(string(model.ID)); err != nil {
					slog.Warn("dictation: model load failed", "model", model.ID, "err", err)
				}
			}()
		}
	}
	s.incognito = c.d.Incognito()
	appID, site := "", ""
	if s.app != nil {
		appID, site = s.app.BundleID, s.app.Site
	}
	if style, ok := cfg.AI.StyleRules.Resolve(appID, site); ok {
		s.ruleStyle = style
	}
	var recording bool
	if audio := c.d.Platform.Audio; audio != nil {
		device := ""
		if cfg.Audio.InputDevice != nil {
			device = *cfg.Audio.InputDevice
		}
		observer := c.d.Observer
		eng := c.d.Engine
		err := audio.Start(context.Background(), device, func(chunk platform.AudioChunk) {
			if observer != nil {
				observer.Level(chunk.RMS)
			}
			s.samplesMu.Lock()
			if s.streaming.Load() && eng != nil {
				_ = eng.PushAudio(id, chunk.Samples)
			}
			s.samples = append(s.samples, chunk.Samples...)
			s.samplesMu.Unlock()
		})
		if err != nil {
			slog.Error("dictation: microphone", "err", err)
			go c.Dispatch(Event{Kind: EvCaptureFailed, Session: id, Message: msg("overlay.notice.microphone_failed").String()})
		} else {
			recording = true
			s.capturing = true
		}
	} else {
		go c.Dispatch(Event{Kind: EvCaptureFailed, Session: id, Message: msg("overlay.notice.microphone_failed").String()})
	}
	s.started = time.Now()
	ms := uint64(time.Since(requested).Milliseconds())
	s.diagnostics.StartToCaptureMs = &ms
	if recording {
		c.startPreview(id, true)
		if p, ok := c.d.Styler.(Preparer); ok {
			style := c.sessionStyle(id)
			if provider, ok := c.providerFor(style); ok {
				go p.Prepare(style, provider)
			}
		}
	}
}

// startPreview opens the live preview stream off the controller goroutine.
// With backlog, the audio from before the stream opened goes first.
func (c *Controller) startPreview(id SessionID, backlog bool) {
	opts := c.sessionOptions()
	eng := c.d.Engine
	if eng == nil || opts.PreviewModel == "" || !c.config().Overlay.ShowPreview {
		return
	}
	p := string(opts.PreviewModel)
	if !eng.Status(p).IsUsable() && !eng.PreviewReady(p) {
		return
	}
	s := c.sessions[id]
	if s == nil {
		return
	}
	s.opening = true
	go func() {
		opened := eng.StartStream(id, opts) == nil
		c.do(func() {
			s.opening = false
			if !opened {
				return
			}
			if !(c.machine.State.Kind == StateRecording && c.machine.State.Session == id) {
				eng.EndStream(id)
				return
			}
			s.samplesMu.Lock()
			if backlog {
				for i := 0; i < len(s.samples); i += bridge.SampleRate / 10 {
					_ = eng.PushAudio(id, s.samples[i:min(i+bridge.SampleRate/10, len(s.samples))])
				}
			}
			s.streaming.Store(true)
			s.samplesMu.Unlock()
		})
	}()
}

func (c *Controller) stopCapture(id SessionID) {
	s := c.session(id)
	requested := time.Now()
	s.stopped = requested
	if !s.started.IsZero() {
		s.durationMs = uint64(requested.Sub(s.started).Milliseconds())
	}
	c.stopMic(s)
	ms := uint64(time.Since(requested).Milliseconds())
	s.diagnostics.StopToCaptureEndMs = &ms
	streamed := s.streaming.Swap(false)
	s.diagnostics.Transcribe.LivePreview = streamed
	if streamed && c.d.Engine != nil {
		c.d.Engine.EndStream(id)
	}
}

// finalPass is how the final pass of the active model runs.
type finalPass struct {
	local    Engine
	model    models.ModelID
	cloud    *config.SpeechProvider
	info     models.ModelInfo
	apiKey   string
	timeout  time.Duration
	fallback *models.ModelInfo
}

type passed struct {
	transcript  Transcript
	note        *Message
	modelWaitMs uint64
	sent        time.Time
}

func (c *Controller) finalPass() (*finalPass, *Message) {
	cfg := c.config()
	info := c.activeModel(cfg)
	if providerID, model, ok := SplitModelID(info.ID); ok {
		p, ok := cfg.Speech.Provider(providerID)
		if !ok {
			m := msg("overlay.error.cloud_provider_not_set_up")
			return nil, &m
		}
		fp := &finalPass{cloud: &p, model: models.ModelID(model), info: info, timeout: time.Duration(cfg.Speech.TimeoutMs) * time.Millisecond, local: c.d.Engine}
		if p.APIKeyAccount != nil && c.d.Secrets != nil {
			fp.apiKey = c.d.Secrets.Get(*p.APIKeyAccount)
		}
		if c.d.Engine != nil {
			if preview := c.previewModel(); preview != "" {
				if m, ok := models.Find(preview); ok && m.FinalPass && c.d.Engine.Status(string(m.ID)).IsUsable() {
					fp.fallback = &m
				}
			}
		}
		return fp, nil
	}
	if c.d.Engine == nil {
		m := msg("overlay.error.engine_not_running")
		return nil, &m
	}
	return &finalPass{local: c.d.Engine, model: info.ID, info: info}, nil
}

func (p *finalPass) run(samples []float32, opts SessionOptions) (passed, *Message) {
	if p.cloud == nil {
		wait := time.Now()
		st := p.local.WaitReady(string(p.model), wait.Add(300*time.Second))
		switch st.State {
		case engine.StateReady:
		case engine.StateFailed:
			m := msg("engine.status."+orDefault(st.Reason, "failed"), "message", st.Message)
			return passed{}, &m
		default:
			m := msg("overlay.error.model_load_timeout")
			return passed{}, &m
		}
		waitMs := uint64(time.Since(wait).Milliseconds())
		sent := time.Now()
		t, err := p.local.Transcribe(samples, opts)
		if err != nil {
			m := Message{Key: err.Error()}
			return passed{}, &m
		}
		return passed{transcript: t, modelWaitMs: waitMs, sent: sent}, nil
	}
	started := time.Now()
	language := p.info.LanguageFor(opts.Language)
	if language == "auto" {
		language = ""
	}
	var vocab []string
	if p.info.Vocabulary {
		vocab = opts.Vocabulary
	}
	pcm := engine.ToPCM16(samples)
	sent := time.Now()
	text, err := providers.Transcribe(*p.cloud, p.apiKey, providers.Request{Model: string(p.model), WAV: engine.EncodeWAV(pcm), Language: language, Vocabulary: vocab, Timeout: p.timeout})
	if err == nil {
		return passed{transcript: Transcript{Text: text, Model: p.info.ID, ElapsedMs: uint64(time.Since(started).Milliseconds())}, sent: sent}, nil
	}
	slog.Warn("dictation: cloud final pass failed", "provider", p.cloud.ID, "err", err)
	m := cloudError(p.cloud.Name, err)
	if p.fallback == nil || p.local == nil {
		return passed{}, &m
	}
	local := opts
	local.FinalModel = p.fallback.ID
	t, ferr := p.local.Transcribe(samples, local)
	if ferr != nil {
		return passed{}, &m
	}
	note := msg("overlay.notice.provider_fallback", "provider", p.cloud.Name, "local", p.fallback.Name)
	return passed{transcript: t, note: &note, sent: sent}, nil
}

func cloudError(provider string, err error) Message {
	var se *config.SpeechError
	if errors.As(err, &se) {
		return msg(se.UserMessageKey(), "provider", provider)
	}
	return msg("core.speech.error.network", "provider", provider)
}

func orDefault(s, d string) string {
	if s == "" {
		return d
	}
	return s
}

func (c *Controller) transcribe(id SessionID) {
	pass, failure := c.finalPass()
	if failure != nil {
		c.dispatch(Event{Kind: EvTranscribeFailed, Session: id, Message: failure.String()})
		return
	}
	started := time.Now()
	s := c.session(id)
	s.samplesMu.Lock()
	samples := slices.Clone(s.samples)
	s.samplesMu.Unlock()
	opts := c.sessionOptions()
	var replacer *enhance.Replacer
	if c.d.Replacer != nil {
		replacer = c.d.Replacer()
	}
	spoken := c.config().Dictation.SpokenPunctuation
	cloud := pass.cloud != nil
	go func() {
		p, failure := pass.run(samples, opts)
		if failure != nil {
			c.Dispatch(Event{Kind: EvTranscribeFailed, Session: id, Message: failure.String()})
			return
		}
		text := strings.TrimSpace(p.transcript.Text)
		var applied []store.AppliedReplacement
		if replacer != nil {
			text, applied = replacer.Apply(text)
		}
		if spoken {
			text = enhance.SpokenPunctuation(text)
		}
		timings := store.TranscribeTimings{StepMs: uint64(time.Since(started).Milliseconds()), ModelWaitMs: p.modelWaitMs, Cloud: cloud, Fallback: p.note != nil, Language: opts.Language}
		c.do(func() {
			s := c.session(id)
			if !s.stopped.IsZero() {
				ms := uint64(max(p.sent.Sub(s.stopped), 0).Milliseconds())
				s.diagnostics.StopToRequestMs = &ms
			}
			raw := p.transcript.Text
			s.transcript = &raw
			s.transcribeMs = p.transcript.ElapsedMs
			s.model = p.transcript.Model
			s.replacements = applied
			timings.LivePreview = s.diagnostics.Transcribe.LivePreview
			s.diagnostics.Transcribe = timings
			if p.note != nil {
				c.notice(*p.note)
			}
			c.dispatch(Event{Kind: EvTranscribed, Session: id, Text: text})
		})
	}()
}

func (c *Controller) styles() *enhance.StyleLibrary {
	if c.d.Styles == nil {
		return nil
	}
	return c.d.Styles()
}

// sessionStyle is the picked style, else the style of a rule, else the
// active style.
func (c *Controller) sessionStyle(id SessionID) *enhance.Style {
	lib := c.styles()
	if lib == nil {
		return nil
	}
	if s := c.sessions[id]; s != nil {
		for _, want := range []string{s.pickedStyle, s.ruleStyle} {
			if want == "" {
				continue
			}
			if st, ok := lib.Get(want); ok {
				return st
			}
		}
	}
	cfg := c.config()
	if st, ok := lib.Get(cfg.AI.ActiveStyle); ok {
		return st
	}
	if st, ok := lib.Get(enhance.DefaultStyleID); ok {
		return st
	}
	return nil
}

func (c *Controller) providerFor(style *enhance.Style) (config.Provider, bool) {
	cfg := c.config()
	if !cfg.AI.Enabled || style == nil {
		return config.Provider{}, false
	}
	return cfg.AI.StyleProvider(style.Provider)
}

func (c *Controller) styleName(st *enhance.Style) string {
	if c.d.StyleName != nil {
		return c.d.StyleName(st)
	}
	return st.Name
}

func (c *Controller) applyStyle(id SessionID, text string) {
	style := c.sessionStyle(id)
	s := c.session(id)
	if style != nil {
		s.styleID = style.ID
	}
	provider, ok := c.providerFor(style)
	if !ok || style.RunPlanOn(provider).Kind == enhance.PlanNothing {
		styleID := ""
		if style != nil {
			styleID = style.ID
		}
		slog.Info("enhance: not used", "style", styleID, "ai_enabled", c.config().AI.Enabled, "provider_found", ok, "provider", provider.ID)
		s.enhance = &store.EnhanceOutcome{Result: store.EnhanceNotUsed}
		c.dispatch(Event{Kind: EvReadyToInsert, Session: id, Text: text})
		return
	}
	if provider.NeedsModel() {
		slog.Warn("enhance: no model is chosen", "provider", provider.ID, "kind", provider.Kind)
		s.enhance = &store.EnhanceOutcome{Result: store.EnhanceFailed, ProviderID: provider.ID, Reason: "no model is chosen for it"}
		c.dispatch(Event{Kind: EvReadyToInsert, Session: id, Text: text, HasEnhanceErr: true, EnhanceError: msg("overlay.error.no_model_chosen", "provider", provider.ID).String()})
		return
	}
	c.dispatch(Event{Kind: EvEnhanceStarted, Session: id})
	name := c.styleName(style)
	cfg := c.config()
	job := StyleJob{Style: style, Provider: provider, Text: text, Replacements: slices.Clone(s.replacements), Language: s.diagnostics.Transcribe.Language, SpeechModel: s.model, Vocabulary: c.vocabulary()}
	if job.Language == "" {
		job.Language = cfg.Dictation.Language
	}
	if job.SpeechModel == "" {
		job.SpeechModel = cfg.Dictation.Model
	}
	if s.app != nil {
		job.App, job.Site = s.app.Name, s.app.Site
	}
	if lib := c.styles(); lib != nil {
		job.BasePrompt = lib.BasePrompt()
	}
	ctx, cancel := context.WithCancel(context.Background())
	job.Context, s.cancelStyle = ctx, cancel
	styler := c.d.Styler
	go func() {
		out := styler.Run(job)
		cancel()
		c.do(func() {
			s := c.session(id)
			e := out.Enhance
			s.enhance = &e
			s.diagnostics.Enhance = out.Timings
			var enhanceErr string
			if out.EnhanceError != nil {
				enhanceErr = styleErrorMessage(providerName(cfg, out.ErrorFrom), out.EnhanceError).String()
			}
			if n := out.Notice; n != nil {
				c.notice(cleanupNoticeMessage(n, name))
			}
			applied := e.Result == store.EnhanceApplied && !(out.Notice != nil && out.Notice.Kind == "cleanup_only")
			ev := Event{Kind: EvReadyToInsert, Session: id, Text: out.Text, HasEnhanceErr: out.EnhanceError != nil, EnhanceError: enhanceErr}
			if applied {
				ev.StyleName = name
			}
			c.dispatch(ev)
		})
	}()
}

func providerName(cfg *config.Config, id string) string {
	for _, p := range cfg.AI.Providers {
		if p.ID == id && p.Name != "" {
			return p.Name
		}
	}
	return id
}

func cleanupNoticeMessage(n *enhance.CleanupNotice, style string) Message {
	switch n.Kind {
	case "cleanup_only":
		return msg("overlay.notice.cleanup_only", "style", style)
	case "language":
		name := n.Model
		if m, ok := enhance.FindCleanupModel(n.Model); ok {
			name = m.Name
		}
		return msg("overlay.notice.cleanup_language", "model", name, "language", n.Language)
	}
	return msg("overlay.notice.cleanup_unknown_language")
}

func styleErrorMessage(provider string, err error) Message {
	e, ok := err.(*enhance.Error)
	if !ok {
		return msg("overlay.style_error.failed", "provider", provider)
	}
	switch e.Kind {
	case enhance.ErrTimeout:
		return msg("overlay.style_error.timeout", "provider", provider, "seconds", uint64(math.Round(float64(e.Ms)/1000)))
	case enhance.ErrNotConfigured:
		return msg("overlay.style_error.not_configured", "provider", provider)
	case enhance.ErrHTTP:
		return msg("overlay.style_error.http", "provider", provider, "status", e.Status)
	case enhance.ErrNetwork:
		return msg("overlay.style_error.network", "provider", provider)
	case enhance.ErrInvalidOutput:
		return msg("overlay.style_error.invalid_output", "provider", provider)
	case enhance.ErrRefused:
		return msg("overlay.style_error.refused", "provider", provider)
	}
	return msg("overlay.style_error.failed", "provider", provider)
}

func (c *Controller) insert(id SessionID, text string) {
	s := c.session(id)
	cfg := c.config()
	typed := s.app != nil && slices.Contains(cfg.Insertion.TypeInApps, s.app.BundleID)
	s.finalText = &text
	c.stateMu.Lock()
	c.last = text
	c.stateMu.Unlock()
	toInsert := cfg.Insertion.TextToInsert(text)
	opts := platform.InsertOptions{RestoreClipboard: cfg.Insertion.RestoreClipboard, RestoreDelay: time.Duration(cfg.Insertion.RestoreDelayMs) * time.Millisecond, FallbackToType: typed}
	inserter := c.d.Platform.Inserter
	go func() {
		started := time.Now()
		var r platform.InsertResult
		if inserter == nil {
			r = platform.InsertResult{Method: "failed", Err: errors.New("no inserter")}
		} else {
			r = inserter.Insert(toInsert, opts)
		}
		method := "paste"
		if typed {
			method = "type"
		}
		timings := store.InsertTimings{ElapsedMs: uint64(time.Since(started).Milliseconds()), Method: method}
		if !typed && opts.RestoreClipboard {
			d := uint64(opts.RestoreDelay.Milliseconds())
			timings.RestoreDelayMs = &d
		}
		if r.Backend != "" {
			b := r.Backend
			timings.Backend = &b
		}
		c.do(func() {
			s := c.session(id)
			s.diagnostics.Insert = &timings
			if r.Err != nil || r.Method == "failed" || r.Method == "not_inserted" {
				reason := "the text could not be inserted"
				if r.Err != nil {
					reason = r.Err.Error()
				}
				s.insert = &store.InsertOutcome{Result: store.InsertFailed, Reason: reason}
				c.dispatch(Event{Kind: EvInsertFailed, Session: id, Text: text, Reason: reason})
				return
			}
			if r.Method == "typed" {
				s.insert = &store.InsertOutcome{Result: store.InsertTyped}
			} else {
				s.insert = &store.InsertOutcome{Result: store.InsertPasted, ClipboardRestored: r.ClipboardRestored}
			}
			if !s.stopped.IsZero() {
				ms := uint64(time.Since(s.stopped).Milliseconds())
				s.diagnostics.StopToInsertMs = &ms
			}
			c.tellApps(id)
			c.dispatch(Event{Kind: EvInserted, Session: id})
		})
	}()
}

// tellApps sends dictation_done to the apps that observe dictation.
func (c *Controller) tellApps(id SessionID) {
	if c.d.Host == nil {
		return
	}
	s := c.session(id)
	done := bridge.DictationDone{Time: c.d.Now().UTC().Format(time.RFC3339)}
	if s.finalText != nil {
		done.Text = *s.finalText
	}
	if s.transcript != nil {
		done.Transcript = *s.transcript
	}
	if s.app != nil {
		name := s.app.Name
		done.App = &name
		if s.app.Site != "" {
			site := s.app.Site
			done.Site = &site
		}
	}
	if s.styleID != "" {
		style := s.styleID
		done.Style = &style
	}
	c.d.Host.DictationDone(done, s.incognito, c.d.Incognito())
}

// SavesHistory is true when a dictation goes to history: history on and
// incognito off at its start and end.
func SavesHistory(enabled, incognitoAtStart, incognitoAtEnd bool) bool {
	return enabled && !IncognitoDuring(incognitoAtStart, incognitoAtEnd)
}

func (c *Controller) saveHistory(id SessionID) {
	s, ok := c.sessions[id]
	if !ok {
		return
	}
	delete(c.sessions, id)
	if c.d.History == nil {
		return
	}
	cfg := c.config()
	if !SavesHistory(cfg.History.Enabled, s.incognito, c.d.Incognito()) {
		// No entry, no audio, and no replacement counts.
		return
	}
	s.samplesMu.Lock()
	samples := s.samples
	s.samples = nil
	s.samplesMu.Unlock()
	saveAudio := cfg.History.SaveAudio && len(samples) > 0
	model := s.model
	if model == "" {
		model = cfg.Dictation.Model
	}
	transcript := ""
	if s.transcript != nil {
		transcript = *s.transcript
	}
	entry := store.HistoryEntry{
		CreatedAt:    c.d.Now().UTC(),
		DurationMs:   s.durationMs,
		Transcript:   transcript,
		FinalText:    transcript,
		StyleID:      s.styleID,
		Model:        string(model),
		TranscribeMs: s.transcribeMs,
		Replacements: s.replacements,
		Enhance:      store.EnhanceOutcome{Result: store.EnhanceNotUsed},
		Insert:       store.InsertOutcome{Result: store.InsertNotInserted},
		Waveform:     store.WaveformSummary(samples, 96),
	}
	if s.finalText != nil {
		entry.FinalText = *s.finalText
	}
	if entry.StyleID == "" {
		entry.StyleID = cfg.AI.ActiveStyle
	}
	if entry.Replacements == nil {
		entry.Replacements = []store.AppliedReplacement{}
	}
	if s.enhance != nil {
		entry.Enhance = *s.enhance
	}
	if s.insert != nil {
		entry.Insert = *s.insert
	}
	if s.app != nil {
		entry.App = &store.TargetApp{BundleID: s.app.BundleID, Name: s.app.Name, Site: s.app.Site}
	}
	diag := s.diagnostics
	diag.AppVersion = c.d.AppVersion
	entry.Diagnostics = &diag
	history, system, observer := c.d.History, c.d.System, c.d.Observer
	go func() {
		if system != nil {
			entry.Diagnostics.System = system()
		}
		if saveAudio {
			if name, err := history.SaveAudio(samples); err != nil {
				slog.Warn("dictation: could not save audio", "err", err)
			} else {
				entry.AudioFile = name
			}
		}
		if _, err := history.InsertEntry(entry); err != nil {
			slog.Error("dictation: could not save history", "err", err)
		}
		if err := history.RecordReplacementUses(entry.Replacements); err != nil {
			slog.Warn("dictation: could not count replacements", "err", err)
		}
		if observer != nil {
			observer.HistoryChanged()
		}
	}()
}

// DefaultStyler runs a style with the enhancers of internal/enhance.
type DefaultStyler struct {
	Secrets       providers.SecretStore
	Config        func() *config.Config
	LanguageModel enhance.LanguageModel
	// Cleaner runs cleanup models, or nil.
	Cleaner enhance.Cleaner
	// SpeechCatalog gives the languages of a cloud final model.
	SpeechCatalog func() []models.ModelInfo
}

// StyleTimeout is the time a provider gets for the style step.
func StyleTimeout(ai config.AI, p config.Provider) time.Duration {
	switch p.Kind {
	case config.KindOpenAICompatible, config.KindAppleIntelligence, config.KindCleanupModel:
		return time.Duration(ai.HTTPTimeoutMs) * time.Millisecond
	}
	return time.Duration(ai.CLITimeoutMs) * time.Millisecond
}

// Preloader is a Cleaner that can load a cleanup model ahead.
type Preloader interface {
	Preload(model string)
}

// Prepare starts a spare process for a provider that runs a program, and
// loads the cleanup model of a cleanup provider while the microphone records
// (a hint: the style can still change, and the style step then loads the
// right model).
func (d *DefaultStyler) Prepare(style *enhance.Style, provider config.Provider) {
	switch provider.Kind {
	case config.KindClaudeCLI:
		enhance.Prepare(provider)
	case config.KindCleanupModel:
		if pl, ok := d.Cleaner.(Preloader); ok && provider.Model != nil && style != nil && style.RunPlanOn(provider).Kind != enhance.PlanNothing {
			pl.Preload(*provider.Model)
		}
	}
}

// Run runs one style step.
func (d *DefaultStyler) Run(job StyleJob) enhance.PipelineOutput {
	started := time.Now()
	cfg := config.Default()
	if d.Config != nil {
		cfg = *d.Config()
	}
	p := &enhance.TextPipeline{Replacer: enhance.NewReplacer(nil), Style: job.Style, BasePrompt: job.BasePrompt, Vocabulary: job.Vocabulary, Timeout: StyleTimeout(cfg.AI, job.Provider),
		ProviderKind: string(job.Provider.Kind)}
	var setupMs uint64
	if job.Provider.Kind == config.KindCleanupModel && job.Provider.Model != nil {
		if info, ok := enhance.FindCleanupModel(*job.Provider.Model); ok && d.Cleaner != nil {
			var langs []string
			if m, ok := models.FindWith(cfg.Speech.Catalog(), job.SpeechModel); ok {
				langs = m.Languages
			}
			p.Cleaner = &enhance.CleanerRun{Cleaner: d.Cleaner, ProviderID: job.Provider.ID, Model: info, Plan: job.Style.RunPlanOn(job.Provider), LanguageSetting: job.Language, FinalModelLanguages: langs, App: job.App, Site: job.Site, Context: job.Context}
		}
	}
	if p.Cleaner == nil {
		p.Enhancer = enhance.BuildEnhancer(job.Provider, d.Secrets, cfg.AI, d.LanguageModel)
		setupMs = uint64(time.Since(started).Milliseconds())
	}
	out := p.RunStyle(job.Text, job.Replacements)
	if out.Timings != nil {
		out.Timings.SetupMs = setupMs
		out.Timings.StepMs = uint64(time.Since(started).Milliseconds())
		out.Timings.ProviderKind = string(job.Provider.Kind)
		if t := job.Provider.ThinkingSent(); t != "" {
			out.Timings.Thinking = &t
		}
	}
	return out
}
