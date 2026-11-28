package sayso

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"slices"
	"sync/atomic"

	"github.com/watzon/sayso/sdk/go/engine"
	"github.com/watzon/sayso/sdk/go/protocol"
	"github.com/watzon/sayso/sdk/go/speech"
)

// localBackend is Local mode: the engine of the app, with the jobs and live
// sessions of the speech package.
type localBackend struct {
	eng       *engine.Engine
	adapter   *speech.EngineAdapter
	bus       *speech.Bus
	jobs      *speech.JobQueue
	live      speech.LiveSessions
	modelsDir string
	tmp       string
	out       *eventQueue
	capture   captureSources

	closed   atomic.Bool
	followed chan struct{}
}

// localConnection is the one connection of Local mode, for the job queue
// and the live sessions.
const localConnection = 1

func startLocal(ctx context.Context, opts Options) (*localBackend, error) {
	program, err := opts.Engine.program()
	if err != nil {
		return nil, err
	}
	models := opts.ModelsDir
	if models == "" {
		models = ModelsDir()
	}
	eng, err := engine.Start(ctx, engine.Options{Program: program, ModelsDir: models, Env: opts.Engine.Env, Stderr: opts.Engine.Stderr})
	if err != nil {
		return nil, err
	}
	tmp, err := os.MkdirTemp("", "sayso-local")
	if err != nil {
		eng.Close()
		return nil, err
	}
	l := &localBackend{eng: eng, modelsDir: models, tmp: tmp, out: newEventQueue(), followed: make(chan struct{}), capture: opts.Capture.resolve()}
	l.adapter = speech.FromEngine(eng, tmp)
	l.bus = speech.StartBus(l.adapter)
	uses := speech.NewModelUse()
	l.jobs = speech.StartJobQueue(speech.NewJobRunner(l.adapter, uses), nil)
	l.live = speech.NewLiveSessions(speech.LiveDeps{
		Engine: l.adapter, Bus: l.bus, Microphone: l.capture.mic, SystemAudio: l.capture.system,
		Models: uses, RecordingDir: filepath.Join(tmp, "live"),
	})
	go l.follow()
	return l, nil
}

// follow ends the work when the engine stops: each job gets its one end, and
// the session gets session_ended.
func (l *localBackend) follow() {
	defer close(l.followed)
	<-l.adapter.Done()
	l.jobs.Close()
	if cur, ok := l.live.Current(); ok {
		code := protocol.CodeUnavailable
		l.live.ConnectionClosed(localConnection)
		l.out.push(&protocol.SessionEnded{Session: cur.Session, Code: &code})
	}
}

func (l *localBackend) mode() ActiveMode { return ModeLocal }

func (l *localBackend) features() Features {
	return Features{Mode: ModeLocal, Engine: slices.Clone(l.eng.Hello().Features), MicCapture: l.capture.mic != nil, SystemCapture: l.capture.system != nil}
}

func (l *localBackend) alive() bool {
	select {
	case <-l.eng.Done():
		return false
	default:
		return true
	}
}

func roles(names []string) []Role {
	out := make([]Role, len(names))
	for i, n := range names {
		out[i] = Role(n)
	}
	return out
}

func (l *localBackend) catalog(ctx context.Context) ([]ModelInfo, error) {
	list, _, states, err := l.eng.Catalog(ctx)
	if err != nil {
		var ee *engine.Error
		if errors.As(err, &ee) {
			return nil, protocol.Errorf(protocol.CodeUnsupported, "the engine has no catalog: %s", ee.Message)
		}
		return nil, err
	}
	state := map[string]string{}
	for _, s := range states {
		state[s.Model] = s.State
		l.adapter.SetStatus(s.Model, speech.ModelStatus{State: s.State, Message: s.Message, Reason: s.Reason})
	}
	out := make([]ModelInfo, 0, len(list))
	for _, m := range list {
		st := protocol.ModelState(state[m.ID])
		if st == "" {
			st = protocol.StateNotDownloaded
		}
		langs := m.Languages
		if langs == nil {
			langs = []string{}
		}
		out = append(out, ModelInfo{ID: m.ID, Name: m.Name, Roles: roles(m.Roles), State: st, Languages: langs, Timestamps: protocol.TimestampLevel(m.Timestamps), SizeBytes: m.SizeBytes})
	}
	return out, nil
}

func (l *localBackend) listModels(ctx context.Context, role *Role) ([]ModelInfo, error) {
	all, err := l.catalog(ctx)
	if err != nil || role == nil {
		return all, err
	}
	var out []ModelInfo
	for _, m := range all {
		if slices.Contains(m.Roles, *role) {
			out = append(out, m)
		}
	}
	return out, nil
}

func (l *localBackend) downloadModel(ctx context.Context, model, _ string, onEvent func(Event)) error {
	all, err := l.catalog(ctx)
	if err != nil {
		return err
	}
	var size uint64
	found := false
	for _, m := range all {
		if m.ID == model {
			size, found = m.SizeBytes, true
		}
	}
	if !found {
		return protocol.Errorf(protocol.CodeUnknownModel, "the engine has no model %s", model)
	}
	err = l.eng.Download(ctx, model, nil, size, func(p engine.DownloadProgress) {
		if onEvent != nil {
			onEvent(&protocol.DownloadProgress{Model: p.Model, Fraction: protocol.F64(p.Fraction), BytesDone: p.BytesDone, BytesTotal: p.BytesTotal})
		}
	}, func(s engine.ModelState) {
		if onEvent != nil {
			ev := &protocol.ModelStateEvent{Model: s.Model, State: protocol.ModelState(s.State)}
			if s.Message != "" {
				ev.Message = &s.Message
			}
			if s.Reason != "" {
				ev.Reason = &s.Reason
			}
			onEvent(ev)
		}
	})
	return engineError(err)
}

func engineError(err error) error {
	var ee *engine.Error
	switch {
	case err == nil:
		return nil
	case errors.As(err, &ee):
		out := &Error{Code: protocol.ErrorCode(ee.Code), Message: ee.Message, Reason: ee.Reason}
		return out
	case errors.Is(err, engine.ErrStopped):
		return protocol.Errorf(protocol.CodeUnavailable, "the engine stopped")
	}
	return err
}

// resolveCatalog is the catalog of the engine as resolve reads it.
func (l *localBackend) resolveCatalog(ctx context.Context) (speech.Catalog, error) {
	all, err := l.catalog(ctx)
	if err != nil {
		return speech.Catalog{}, err
	}
	return speech.Catalog{Models: all, Features: l.eng.Hello().Features}, nil
}

func refusal(r *speech.Refusal) error { return &Error{Code: r.Code, Message: r.Message} }

func liveError(e *speech.LiveError) error {
	if e == nil {
		return nil
	}
	return &Error{Code: e.Code, Message: e.Message}
}

type sinkFunc func(Event)

func (f sinkFunc) Event(e Event) { f(e) }

func (l *localBackend) transcribeFile(ctx context.Context, req protocol.TranscribeFile) (uint64, error) {
	if !filepath.IsAbs(req.Path) {
		return 0, protocol.Errorf(protocol.CodeBadRequest, "path must be absolute: %s", req.Path)
	}
	cat, err := l.resolveCatalog(ctx)
	if err != nil {
		return 0, err
	}
	spec, r := speech.ResolveJob(&req, cat)
	if r != nil {
		return 0, refusal(r)
	}
	job, ok := l.jobs.Submit(localConnection, spec, sinkFunc(l.out.push), func(uint64) {})
	if !ok {
		return 0, protocol.Errorf(protocol.CodeUnavailable, "the engine stopped")
	}
	return job, nil
}

func (l *localBackend) cancelJob(_ context.Context, job uint64) error {
	if !l.jobs.Cancel(localConnection, job) {
		return protocol.Errorf(protocol.CodeBadRequest, "this connection has no job %d", job)
	}
	return nil
}

func (l *localBackend) sessionStart(ctx context.Context, req protocol.SessionStart) (protocol.SessionReply, error) {
	if req.RecordTo != nil && !filepath.IsAbs(*req.RecordTo) {
		return protocol.SessionReply{}, protocol.Errorf(protocol.CodeBadRequest, "record_to must be absolute: %s", *req.RecordTo)
	}
	cat, err := l.resolveCatalog(ctx)
	if err != nil {
		return protocol.SessionReply{}, err
	}
	spec, r := speech.ResolveSession(&req, cat)
	if r != nil {
		return protocol.SessionReply{}, refusal(r)
	}
	started, lerr := l.live.Start(speech.Owner{Connection: localConnection, AppID: "local"}, spec, sinkFunc(l.out.push))
	if lerr != nil {
		return protocol.SessionReply{}, liveError(lerr)
	}
	excludes := started.Excludes
	if excludes == nil {
		excludes = []string{}
	}
	return protocol.SessionReply{Session: started.Session, Excludes: excludes}, nil
}

func (l *localBackend) sessionAudio(session uint64, samples []int16) error {
	return liveError(l.live.Audio(localConnection, session, protocol.EncodeI16(samples)))
}

func (l *localBackend) sessionStop(_ context.Context, session uint64) (string, error) {
	text, err := l.live.Stop(localConnection, session)
	return text, liveError(err)
}

func (l *localBackend) enhance(context.Context, protocol.Enhance) (protocol.EnhanceReply, error) {
	return protocol.EnhanceReply{}, protocol.Errorf(protocol.CodeUnsupported, "Local mode cannot enhance; it needs Sayso")
}

func (l *localBackend) subscribe(context.Context, []string) ([]string, error) {
	return nil, protocol.Errorf(protocol.CodeUnsupported, "Local mode cannot subscribe; it needs Sayso")
}

func (l *localBackend) events() <-chan Event { return l.out.out }

func (l *localBackend) close() {
	if l.closed.Swap(true) {
		return
	}
	l.live.Close()
	l.jobs.Close()
	l.eng.Close()
	<-l.followed
	l.bus.Stop()
	os.RemoveAll(l.tmp)
	l.out.close()
}
