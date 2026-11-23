package bridge

import (
	"fmt"
	"log/slog"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/enhance"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/store"
)

// ---- list_models (models.rs) ----

// ModelRoles are the roles of a model in the protocol.
func ModelRoles(m models.ModelInfo) []Role {
	roles := []Role{}
	if m.FinalPass {
		roles = append(roles, RoleFinal)
	}
	if m.LivePreview {
		roles = append(roles, RoleStreaming)
	}
	switch m.Task() {
	case models.TaskVad:
		roles = append(roles, RoleVad)
	case models.TaskDiarization:
		roles = append(roles, RoleDiarization)
	}
	return roles
}

func stateOf(s engine.ModelStatus) ModelState {
	switch s.State {
	case engine.StateDownloading:
		return StateDownloading
	case engine.StateOptimizing:
		return StateOptimizing
	case engine.StateDownloaded:
		return StateDownloaded
	case engine.StateReady:
		return StateReady
	case engine.StateFailed:
		return StateFailed
	}
	return StateNotDownloaded
}

type chosenRole struct {
	id   models.ModelID
	role Role
}

func chosen(cfg *config.Config, onDisk func(models.ModelID) bool) []chosenRole {
	d := cfg.Dictation
	out := []chosenRole{{d.Model, RoleFinal}}
	if active, ok := models.FindWith(cfg.Speech.Catalog(), d.Model); ok {
		if preview, ok := models.PreviewFor(active, d.PreviewModel, d.Language, onDisk); ok {
			out = append(out, chosenRole{preview, RoleStreaming})
		}
	}
	if v := models.VadFor(nil, cfg.Models.Vad, onDisk); !v.Basic && v.Model != "" {
		out = append(out, chosenRole{v.Model, RoleVad})
	}
	if id, ok := models.DiarizationFor(nil, cfg.Models.Diarization, onDisk); ok {
		out = append(out, chosenRole{id, RoleDiarization})
	}
	return out
}

// ListModelInfos is the list of list_models: each local model, its state,
// and the roles that the host uses it for.
func ListModelInfos(catalog []models.ModelInfo, status func(models.ModelID) engine.ModelStatus, cfg *config.Config, role *Role) []ModelInfo {
	ch := chosen(cfg, func(id models.ModelID) bool { return status(id).IsOnDisk() })
	out := []ModelInfo{}
	for _, m := range catalog {
		roles := ModelRoles(m)
		if role != nil && !slices.Contains(roles, *role) {
			continue
		}
		var choice []Role
		for _, c := range ch {
			if c.id == m.ID {
				choice = append(choice, c.role)
			}
		}
		langs := slices.Clone(m.Languages)
		if langs == nil {
			langs = []string{}
		}
		out = append(out, ModelInfo{ID: string(m.ID), Name: m.Name, Roles: roles, State: stateOf(status(m.ID)), Languages: langs, Timestamps: TimestampLevel(m.Engine.Timestamps()), SizeBytes: m.SizeBytes, Choice: choice})
	}
	return out
}

func noEngine() Reply {
	return ReplyError(CodeUnsupported, "the speech engine does not run, so Sayso cannot list or download models")
}

func listModels(s *shared, req *ListModels, r *responder) {
	eng, reply := readyEngine(s, r)
	if reply != nil {
		r.reply(reply)
		return
	}
	cfg := s.services.Config()
	list := ListModelInfos(models.LocalModels(), func(id models.ModelID) engine.ModelStatus { return eng.Status(string(id)) }, cfg, req.Role)
	r.reply(ReplyOK(ListModelsReply{Models: list}))
}

// ---- download_model (download.rs) ----

const (
	engineStartWait   = 10 * time.Second
	downloadStartWait = 30 * time.Second
	downloadTick      = 250 * time.Millisecond
	maxReasonChars    = 300
)

func readyEngine(s *shared, r *responder) (Engine, Reply) {
	if s.engine == nil {
		return nil, noEngine()
	}
	deadline := time.Now().Add(engineStartWait)
	for {
		switch s.engine.StartState() {
		case EngineStarted:
			return s.engine, nil
		case EngineFailed:
			return nil, ReplyError(CodeUnsupported, "the speech engine did not start")
		}
		if time.Now().After(deadline) || !r.isOpen() || s.isClosing() {
			return nil, ReplyError(CodeUnsupported, "the speech engine is still starting. Try again in a moment")
		}
		time.Sleep(50 * time.Millisecond)
	}
}

type look int

const (
	lookOnDisk look = iota
	lookRunning
	lookIdle
)

func lookAt(eng Engine, id string, starting map[string]bool) look {
	st := eng.Status(id)
	switch {
	case st.IsOnDisk():
		return lookOnDisk
	case st.State == engine.StateDownloading || starting[id]:
		return lookRunning
	}
	return lookIdle
}

func (s *shared) look(eng Engine, id string) look {
	s.downloadsMu.Lock()
	defer s.downloadsMu.Unlock()
	return lookAt(eng, id, s.downloads)
}

func downloadModel(s *shared, appID string, req *DownloadModel, r *responder) {
	r.reply(runDownload(s, appID, req, r))
}

func runDownload(s *shared, appID string, req *DownloadModel, r *responder) Reply {
	info, ok := models.FindLocal(models.ModelID(req.Model))
	if !ok {
		return ReplyError(CodeUnknownModel, fmt.Sprintf("there is no model %s for this computer", req.Model))
	}
	eng, reply := readyEngine(s, r)
	if reply != nil {
		return reply
	}
	id := string(info.ID)
	events, unwatch := s.bus.watch()
	defer unwatch()
	switch s.look(eng, id) {
	case lookOnDisk:
		return ReplyOK(nil)
	case lookRunning:
		return follow(s, eng, events, id, r)
	}
	gate := s.questionGate(appID)
	gate.Lock()
	var release func()
	var early Reply
	switch s.look(eng, id) {
	case lookOnDisk:
		early = ReplyOK(nil)
	case lookRunning:
	case lookIdle:
		release, early = askAndStart(s, appID, req, info, eng, r)
	}
	gate.Unlock()
	if early != nil {
		return early
	}
	reply = follow(s, eng, events, id, r)
	if release != nil {
		release()
	}
	return reply
}

func askAndStart(s *shared, appID string, req *DownloadModel, info models.ModelInfo, eng Engine, r *responder) (func(), Reply) {
	if !r.isOpen() {
		return nil, ReplyError("", "the connection is closed")
	}
	appName := VisibleText(appID)
	if rec := s.apps.Current().Record(appID); rec != nil {
		appName = rec.ShownName()
	}
	q := DownloadQuestion{AppID: appID, AppName: appName, ModelID: string(info.ID), ModelName: info.Name, SizeBytes: info.SizeBytes, Reason: shortReason(req.Reason)}
	slog.Info("host: an app asks to download a model", "app", appID, "model", info.ID)
	accepted := s.services.AskDownload(q, func() bool { return !r.isOpen() || s.isClosing() })
	slog.Info("host: the user answered a download", "app", appID, "model", info.ID, "yes", accepted)
	if !accepted {
		return nil, ReplyError(CodeDeclined, "the user said no to the download")
	}
	if !r.isOpen() {
		return nil, ReplyError("", "the connection is closed")
	}
	s.downloadsMu.Lock()
	defer s.downloadsMu.Unlock()
	switch lookAt(eng, string(info.ID), s.downloads) {
	case lookOnDisk:
		return nil, ReplyOK(nil)
	case lookRunning:
		return nil, nil
	}
	if err := eng.Download(string(info.ID)); err != nil {
		return nil, ReplyError("", fmt.Sprintf("the engine cannot start the download: %v", err))
	}
	slog.Info("host: a download starts for an app", "model", info.ID, "app", appID)
	id := string(info.ID)
	s.downloads[id] = true
	return func() {
		s.downloadsMu.Lock()
		delete(s.downloads, id)
		s.downloadsMu.Unlock()
	}, nil
}

func shortReason(reason string) string {
	reason = strings.NewReplacer("\n", " ", "\r", " ", "\t", " ").Replace(reason)
	reason = strings.TrimSpace(withoutHidden(reason))
	runes := []rune(reason)
	if len(runes) <= maxReasonChars {
		return reason
	}
	return string(runes[:maxReasonChars]) + "..."
}

type progress struct {
	model     string
	r         *responder
	announced ModelState
}

func (p *progress) announce(state ModelState) { p.stateEvent(state, nil, nil) }

func (p *progress) stateEvent(state ModelState, message, reason *string) {
	if p.announced == state && message == nil {
		return
	}
	p.r.event(ModelStateEvent{Model: p.model, State: state, Message: message, Reason: reason})
	p.announced = state
}

func (p *progress) status(st engine.ModelStatus, seenDownloading bool) Reply {
	switch st.State {
	case engine.StateDownloading:
		p.announce(StateDownloading)
		p.r.event(DownloadProgress{Model: p.model, Fraction: F64(st.Fraction), BytesDone: st.BytesDone, BytesTotal: st.BytesTotal})
		return nil
	case engine.StateDownloaded, engine.StateOptimizing, engine.StateReady:
		p.announce(stateOf(st))
		return ReplyOK(nil)
	case engine.StateFailed:
		var reason *string
		if st.Reason != "" {
			reason = strp(st.Reason)
		}
		msg := st.Message
		p.stateEvent(StateFailed, &msg, reason)
		return &ErrorReply{Message: "the download failed: " + msg, Reason: reason}
	}
	if seenDownloading {
		p.announce(StateNotDownloaded)
		return ReplyError("", "the download stopped")
	}
	return nil
}

func follow(s *shared, eng Engine, events <-chan engine.Event, id string, r *responder) Reply {
	p := &progress{model: id, r: r}
	seen := false
	if cur := eng.Status(id); cur.State == engine.StateDownloading {
		seen = true
		if reply := p.status(cur, seen); reply != nil {
			return reply
		}
	}
	giveUp := time.Now().Add(downloadStartWait)
	tick := time.NewTicker(downloadTick)
	defer tick.Stop()
	for {
		if !r.isOpen() || s.isClosing() {
			return ReplyError("", "the connection is closed")
		}
		select {
		case e, ok := <-events:
			if !ok {
				return ReplyError("", "Sayso is closing")
			}
			switch e.Kind {
			case engine.EventModelStatus:
				if e.Model != id {
					continue
				}
				if reply := p.status(e.Status, seen); reply != nil {
					return reply
				}
				seen = seen || e.Status.State == engine.StateDownloading
			case engine.EventCrashed, engine.EventStartFailed:
				p.announce(StateNotDownloaded)
				return ReplyError("", "the speech engine stopped during the download")
			}
		case <-tick.C:
			if !seen && time.Now().After(giveUp) {
				return ReplyError("", "the engine did not report the download")
			}
		}
	}
}

// ---- enhance (enhance.rs) ----

// PromptStyleID is the id of the style of a prompt of an app.
const PromptStyleID = "app-prompt"

func enhanceRequest(s *shared, appID string, req *Enhance, r *responder) {
	reply, refusal := runEnhance(s, req)
	if refusal != nil {
		slog.Info("host: enhance of an app failed", "app", appID, "err", refusal.Message)
		r.reply(ReplyError(refusal.Code, refusal.Message))
		return
	}
	r.reply(reply)
}

func runEnhance(s *shared, req *Enhance) (Reply, *Refusal) {
	text, r := enhanceText(req)
	if r != nil {
		return nil, r
	}
	styles := s.services.Styles()
	if styles == nil {
		return nil, refuse(CodeUnavailable, "this version of Sayso has no AI providers")
	}
	cfg := s.services.Config()
	var namer StyleNamer
	if n, ok := s.services.(StyleNamer); ok {
		namer = n
	}
	style, provider, r := PlanEnhance(cfg, styles, req, namer)
	if r != nil {
		return nil, r
	}
	out := s.services.RunStyle(StyleRequest{Style: style, Provider: provider, BasePrompt: styles.BasePrompt(), Text: text})
	return EnhanceReplyOf(provider, out)
}

func enhanceText(req *Enhance) (string, *Refusal) {
	if req.Text == nil || strings.TrimSpace(*req.Text) == "" {
		return "", refuse(CodeBadRequest, "enhance needs text that is not empty")
	}
	if req.Style != nil && req.Prompt != nil {
		return "", refuse(CodeBadRequest, "send style or prompt, not both")
	}
	if req.Prompt != nil && strings.TrimSpace(*req.Prompt) == "" {
		return "", refuse(CodeBadRequest, "the prompt is empty")
	}
	return *req.Text, nil
}

// PlanEnhance picks the style and the provider of an enhance request.
func PlanEnhance(cfg *config.Config, styles *enhance.StyleLibrary, req *Enhance, namer StyleNamer) (*enhance.Style, config.Provider, *Refusal) {
	var style enhance.Style
	switch {
	case req.Style != nil:
		found := findStyle(styles, *req.Style, namer)
		if found == nil {
			return nil, config.Provider{}, refuse(CodeBadRequest, "the user has no style %s", *req.Style)
		}
		style = *found
	case req.Prompt != nil:
		style = promptStyle(*req.Prompt)
	default:
		found, ok := styles.Get(cfg.AI.ActiveStyle)
		if !ok {
			found, ok = styles.Get(enhance.DefaultStyleID)
		}
		if !ok {
			return nil, config.Provider{}, refuse(CodeUnavailable, "the user has no styles")
		}
		style = *found
	}
	if style.IsRaw() {
		return nil, config.Provider{}, refuse(CodeBadRequest, "the style %s uses no AI", style.ID)
	}
	var provider config.Provider
	if req.Provider != nil {
		p, ok := findProvider(cfg, *req.Provider)
		if !ok {
			return nil, config.Provider{}, refuse(CodeBadRequest, "the user has no AI provider %s", *req.Provider)
		}
		if style.Provider == nil || *style.Provider != p.ID {
			style.Model = nil
		}
		provider = p
	} else {
		p, ok := cfg.AI.StyleProvider(style.Provider)
		if !ok {
			return nil, config.Provider{}, refuse(CodeUnavailable, "no AI provider is set up for the style %s", style.ID)
		}
		provider = p
	}
	if !cfg.AI.Enabled {
		return nil, config.Provider{}, refuse(CodeUnavailable, "AI is off in Sayso. The user can turn it on in Settings")
	}
	if provider.NeedsModel() {
		return nil, config.Provider{}, refuse(CodeUnavailable, "the AI provider %s has no model chosen", provider.ID)
	}
	if req.Prompt != nil && provider.Kind == config.KindCleanupModel {
		return nil, config.Provider{}, refuse(CodeUnsupported, "the AI provider %s is a cleanup model and cannot follow a prompt", provider.ID)
	}
	if style.RunPlanOn(provider).Kind == enhance.PlanNothing {
		return nil, config.Provider{}, refuse(CodeUnavailable, "the AI provider %s cannot run the style %s", provider.ID, style.ID)
	}
	return &style, provider, nil
}

func findStyle(styles *enhance.StyleLibrary, wanted string, namer StyleNamer) *enhance.Style {
	wanted = strings.TrimSpace(wanted)
	if s, ok := styles.Get(wanted); ok {
		return s
	}
	for _, s := range styles.Styles() {
		shown := s.Name
		if namer != nil {
			shown = namer.StyleDisplayName(&s)
		}
		if strings.EqualFold(s.Name, wanted) || strings.EqualFold(shown, wanted) {
			found := s
			return &found
		}
	}
	return nil
}

func findProvider(cfg *config.Config, wanted string) (config.Provider, bool) {
	wanted = strings.TrimSpace(wanted)
	for _, p := range cfg.AI.Providers {
		if p.ID == wanted {
			return p, true
		}
	}
	for _, p := range cfg.AI.Providers {
		if strings.EqualFold(p.Name, wanted) {
			return p, true
		}
	}
	return config.Provider{}, false
}

func promptStyle(prompt string) enhance.Style {
	return enhance.Style{ID: PromptStyleID, Name: "Prompt", Prompt: strings.TrimSpace(prompt), Standalone: true, Cleanup: enhance.CleanupField{State: enhance.CleanupOff}}
}

// EnhanceReplyOf is the reply to enhance for what the style run gave.
func EnhanceReplyOf(provider config.Provider, out enhance.PipelineOutput) (Reply, *Refusal) {
	if out.EnhanceError != nil {
		return nil, EnhanceRefusal(out.ErrorFrom, out.EnhanceError)
	}
	switch out.Enhance.Result {
	case store.EnhanceApplied:
		return ReplyOK(EnhanceReply{Text: out.Text, Provider: out.Enhance.ProviderID, Model: out.Enhance.Model}), nil
	case store.EnhanceFailed:
		return nil, refuse(CodeUnavailable, "the AI provider %s failed", out.Enhance.ProviderID)
	}
	msg := fmt.Sprintf("the AI provider %s did not run", provider.ID)
	if n := out.Notice; n != nil {
		switch n.Kind {
		case "language":
			msg = fmt.Sprintf("the cleanup model %s does not support the language %s", n.Model, n.Language)
		case "unknown_language":
			msg = fmt.Sprintf("the cleanup model %s cannot tell the language of the text", n.Model)
		}
	}
	return nil, refuse(CodeUnavailable, "%s", msg)
}

// EnhanceRefusal is the error of an enhance that failed.
func EnhanceRefusal(providerID string, err error) *Refusal {
	e, ok := err.(*enhance.Error)
	if !ok {
		return refuse(CodeUnavailable, "the AI provider %s failed to run", providerID)
	}
	var code ErrorCode
	var what string
	switch e.Kind {
	case enhance.ErrTimeout:
		code, what = CodeTimeout, fmt.Sprintf("gave no reply in %d s", (e.Ms+999)/1000)
	case enhance.ErrRefused:
		code, what = CodeRefused, "refused the text"
	case enhance.ErrNotConfigured:
		code, what = CodeUnavailable, "is not set up"
	case enhance.ErrHTTP:
		code, what = CodeUnavailable, fmt.Sprintf("replied with HTTP %d", e.Status)
	case enhance.ErrNetwork:
		code, what = CodeUnavailable, "cannot be reached"
	case enhance.ErrCLI:
		code, what = CodeUnavailable, "failed to run"
	case enhance.ErrInvalidOutput:
		what = "gave a reply that Sayso cannot use"
	}
	return refuse(code, "the AI provider %s %s", providerID, what)
}

func storeFailed(providerID, reason string) store.EnhanceOutcome {
	return store.EnhanceOutcome{Result: store.EnhanceFailed, ProviderID: providerID, Reason: reason}
}

// ---- transcribe_file and cancel_job (job_requests.rs) ----

const maxJobsWithFile = 16

func transcribeFile(s *shared, connection uint64, mu *sync.Mutex, owned map[uint64]bool, req *TranscribeFile, r *responder) {
	if s.engine == nil || s.jobs == nil {
		r.reply(ReplyError(CodeUnsupported, "the speech engine of Sayso does not run"))
		return
	}
	job, refusal := ResolveJob(req, choicesOf(s.services.Config()), viewOf(s.engine))
	if refusal != nil {
		r.reply(ReplyError(refusal.Code, refusal.Message))
		return
	}
	id, ok := s.jobs.Submit(connection, job, r.notifier, func(id uint64) { r.reply(ReplyOK(JobReply{Job: id})) })
	if ok {
		mu.Lock()
		owned[id] = true
		mu.Unlock()
		s.stopWorkWithoutGrant()
	}
}

func cancelJob(s *shared, connection uint64, mu *sync.Mutex, owned map[uint64]bool, req *CancelJob, r *responder) {
	mu.Lock()
	has := owned[req.Job]
	mu.Unlock()
	if !has {
		r.reply(ReplyError(CodeBadRequest, fmt.Sprintf("this connection has no job %d", req.Job)))
		return
	}
	if s.jobs != nil {
		s.jobs.Cancel(connection, req.Job)
	}
	r.reply(ReplyOK(nil))
}

// ---- session_start, session_audio, session_stop (live_requests.rs) ----

func noSpeechEngine() Reply {
	return ReplyError(CodeUnsupported, "the speech engine of Sayso does not run")
}

// noLive is the reply when the engine runs and this build has no live
// sessions.
func noLive(s *shared) Reply {
	if s.engine == nil {
		return noSpeechEngine()
	}
	return ReplyError(CodeUnsupported, "this version of Sayso cannot run live sessions yet")
}

func sessionStart(s *shared, connection uint64, appID string, pid *uint32, req *SessionStart, r *responder) {
	if s.engine == nil || s.live == nil {
		r.reply(noLive(s))
		return
	}
	spec, refusal := ResolveSession(req, choicesOf(s.services.Config()), viewOf(s.engine))
	if refusal != nil {
		r.reply(ReplyError(refusal.Code, refusal.Message))
		return
	}
	sink := newAfterReply(r.notifier)
	started, err := s.live.Start(Owner{Connection: connection, AppID: appID, PID: pid}, spec, sink)
	if err != nil {
		r.reply(err.Reply())
		return
	}
	excludes := started.Excludes
	if excludes == nil {
		excludes = []string{}
	}
	r.reply(ReplyOK(SessionReply{Session: started.Session, Excludes: excludes}))
	sink.release()
	s.stopWorkWithoutGrant()
	if !r.notifier.isOpen() {
		s.live.ConnectionClosed(connection)
	}
}

func sessionAudio(s *shared, connection uint64, req *SessionAudio, r *responder) {
	if s.live == nil {
		r.reply(noLive(s))
		return
	}
	if err := s.live.Audio(connection, req.Session, req.PCM); err != nil {
		r.reply(err.Reply())
		return
	}
	if r.hasID() {
		r.reply(ReplyOK(nil))
	} else {
		r.noReply()
	}
}

func sessionStop(s *shared, connection uint64, req *SessionStop, r *responder) {
	if s.live == nil {
		r.reply(noLive(s))
		return
	}
	text, err := s.live.Stop(connection, req.Session)
	if err != nil {
		r.reply(err.Reply())
		return
	}
	r.reply(ReplyOK(TextReply{Text: text}))
}

// afterReply holds the events of a session until the reply to its start
// went out.
type afterReply struct {
	n    notifier
	mu   sync.Mutex
	held []Event
	open bool
}

func newAfterReply(n notifier) *afterReply { return &afterReply{n: n, open: true} }

func (a *afterReply) release() {
	a.mu.Lock()
	defer a.mu.Unlock()
	for _, e := range a.held {
		a.n.send(e)
	}
	a.held, a.open = nil, false
}

func (a *afterReply) Event(e Event) {
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.open {
		a.held = append(a.held, e)
		return
	}
	a.n.send(e)
}
