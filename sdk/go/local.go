package sayso

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"sync/atomic"

	"github.com/watzon/sayso/sdk/go/audio"
	"github.com/watzon/sayso/sdk/go/engine"
	"github.com/watzon/sayso/sdk/go/protocol"
)

// localSessionBase is the first engine session of Local mode, as in the
// host, so the ids stay exact in an engine that reads JSON numbers as
// doubles.
const localSessionBase uint64 = 1 << 40

// localBackend is Local mode: the engine of the app, with jobs and sessions
// of the source client.
type localBackend struct {
	eng       *engine.Engine
	modelsDir string
	tmp       string
	out       *eventQueue

	mu       sync.Mutex
	loads    sync.Mutex
	nextJob  uint64
	jobs     map[uint64]context.CancelFunc
	nextSess uint64
	sessions map[uint64]*localSession
	byEngine map[uint64]uint64
	closed   atomic.Bool
	followed chan struct{}
}

type localSession struct {
	id       uint64
	engineID uint64
	model    string
	preview  string
	recordTo string
	samples  []int16
}

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
	l := &localBackend{eng: eng, modelsDir: models, tmp: tmp, out: newEventQueue(), jobs: map[uint64]context.CancelFunc{}, sessions: map[uint64]*localSession{}, byEngine: map[uint64]uint64{}, followed: make(chan struct{})}
	go l.follow()
	return l, nil
}

// follow passes the partials of the engine on as partials of sessions.
func (l *localBackend) follow() {
	for m := range l.eng.Events() {
		if m.Type != "partial" {
			continue
		}
		var p engine.Partial
		if m.Decode(&p) != nil {
			continue
		}
		l.mu.Lock()
		id, ok := l.byEngine[p.Session]
		l.mu.Unlock()
		if ok {
			src := SourceClient
			l.out.push(&protocol.PartialEvent{Session: id, Source: &src, Committed: p.Committed, Tentative: p.Tentative})
		}
	}
	l.mu.Lock()
	var jobs []uint64
	for job := range l.jobs {
		jobs = append(jobs, job)
	}
	sessions := l.sessions
	l.sessions = map[uint64]*localSession{}
	l.mu.Unlock()
	code := protocol.CodeUnavailable
	for _, job := range jobs {
		l.endJob(job, &protocol.JobFailed{Job: job, Message: "the engine stopped", Code: &code})
	}
	for id := range sessions {
		l.out.push(&protocol.SessionEnded{Session: id, Code: &code})
	}
	close(l.followed)
}

func (l *localBackend) mode() ActiveMode { return ModeLocal }

func (l *localBackend) features() Features {
	return Features{Mode: ModeLocal, Engine: slices.Clone(l.eng.Hello().Features)}
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

// pick is the model of a request, or the first model of the catalog that
// has the role and is on disk.
func (l *localBackend) pick(ctx context.Context, named *string, role Role) (string, error) {
	all, err := l.catalog(ctx)
	if err != nil {
		return "", err
	}
	for _, m := range all {
		if named != nil && m.ID != *named {
			continue
		}
		if !slices.Contains(m.Roles, role) {
			if named != nil {
				return "", protocol.Errorf(protocol.CodeWrongRole, "the model %s cannot do %s", m.ID, role)
			}
			continue
		}
		if m.State == protocol.StateNotDownloaded || m.State == protocol.StateDownloading || m.State == protocol.StateFailed {
			if named != nil {
				return "", protocol.Errorf(protocol.CodeModelNotDownloaded, "the model %s is not downloaded", m.ID)
			}
			continue
		}
		return m.ID, nil
	}
	if named != nil {
		return "", protocol.Errorf(protocol.CodeUnknownModel, "the engine has no model %s", *named)
	}
	return "", protocol.Errorf(protocol.CodeModelNotDownloaded, "no %s model is downloaded", role)
}

func (l *localBackend) load(ctx context.Context, model string) error {
	l.loads.Lock()
	defer l.loads.Unlock()
	_, err := l.eng.Load(ctx, model, nil)
	return engineError(err)
}

func (l *localBackend) transcribeFile(ctx context.Context, req protocol.TranscribeFile) (uint64, error) {
	if !filepath.IsAbs(req.Path) {
		return 0, protocol.Errorf(protocol.CodeBadRequest, "path must be absolute: %s", req.Path)
	}
	if req.Speakers != nil && *req.Speakers == 0 {
		return 0, protocol.Errorf(protocol.CodeBadRequest, "speakers must be 1 or more")
	}
	model, err := l.pick(ctx, req.Model, RoleFinal)
	if err != nil {
		return 0, err
	}
	diarize := (req.Diarize != nil && *req.Diarize) || req.DiarizationModel != nil
	var diar string
	if diarize {
		if !l.eng.Has("diarization") {
			return 0, protocol.Errorf(protocol.CodeUnsupported, "the engine has no diarization")
		}
		if diar, err = l.pick(ctx, req.DiarizationModel, RoleDiarization); err != nil {
			return 0, err
		}
	}
	jobCtx, cancel := context.WithCancel(context.Background())
	l.mu.Lock()
	l.nextJob++
	job := l.nextJob
	l.jobs[job] = cancel
	l.out.push(&protocol.JobProgress{Job: job, Fraction: 0})
	l.mu.Unlock()
	go l.runJob(jobCtx, job, req, model, diar)
	return job, nil
}

// emit sends an event of a job that has not ended.
func (l *localBackend) emit(job uint64, ev Event) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if _, ok := l.jobs[job]; ok {
		l.out.push(ev)
	}
}

// endJob sends the one end of a job.
func (l *localBackend) endJob(job uint64, ev Event) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if cancel, ok := l.jobs[job]; ok {
		delete(l.jobs, job)
		cancel()
		l.out.push(ev)
	}
}

func (l *localBackend) runJob(ctx context.Context, job uint64, req protocol.TranscribeFile, model, diar string) {
	fail := func(err error) {
		e := engineError(err)
		msg := e.Error()
		var code *protocol.ErrorCode
		var pe *Error
		if errors.As(e, &pe) {
			msg = pe.Message
			if pe.Code != "" {
				code = &pe.Code
			}
		}
		l.endJob(job, &protocol.JobFailed{Job: job, Message: msg, Code: code})
	}
	a, err := audio.ReadFile(req.Path)
	if err != nil {
		fail(err)
		return
	}
	wav := filepath.Join(l.tmp, fmt.Sprintf("job-%d.wav", job))
	if err := audio.WriteWAV(wav, a.Samples()); err != nil {
		fail(err)
		return
	}
	defer os.Remove(wav)
	if err := l.load(ctx, model); err != nil {
		fail(err)
		return
	}
	lang := "auto"
	if req.Language != nil {
		lang = *req.Language
	}
	final, err := l.eng.Transcribe(ctx, engine.TranscribeRequest{Model: model, Path: wav, Language: lang, Segments: l.eng.Has("segments")})
	if ctx.Err() != nil {
		return
	}
	if err != nil {
		fail(err)
		return
	}
	for _, s := range final.Segments {
		ev := &protocol.SegmentEvent{Job: &job, Segment: protocol.Segment{StartMs: s.StartMs, EndMs: s.EndMs, Text: s.Text}}
		for _, w := range s.Words {
			ev.Words = append(ev.Words, protocol.Word{StartMs: w.StartMs, EndMs: w.EndMs, Text: w.Text})
		}
		l.emit(job, ev)
	}
	if final.Segments == nil && final.Text != "" {
		l.emit(job, &protocol.SegmentEvent{Job: &job, Segment: protocol.Segment{StartMs: 0, EndMs: a.LenMs(), Text: final.Text}})
	}
	if diar != "" {
		l.emit(job, &protocol.JobProgress{Job: job, Fraction: 0.9})
		if err := l.load(ctx, diar); err != nil {
			fail(err)
			return
		}
		var speakers uint32
		if req.Speakers != nil {
			speakers = *req.Speakers
		}
		d, err := l.eng.Diarize(ctx, diar, wav, speakers)
		if ctx.Err() != nil {
			return
		}
		if err != nil {
			fail(err)
			return
		}
		turns := make([]protocol.Turn, len(d.Turns))
		for i, t := range d.Turns {
			turns[i] = protocol.Turn{StartMs: t.StartMs, EndMs: t.EndMs, Speaker: fmt.Sprint(t.Speaker + 1)}
		}
		l.emit(job, &protocol.SpeakersEvent{Job: &job, Turns: turns})
	}
	l.emit(job, &protocol.JobProgress{Job: job, Fraction: 1})
	l.endJob(job, &protocol.JobDone{Job: job, Text: strings.TrimSpace(final.Text)})
}

func (l *localBackend) cancelJob(_ context.Context, job uint64) error {
	l.mu.Lock()
	_, ok := l.jobs[job]
	l.mu.Unlock()
	if !ok {
		return protocol.Errorf(protocol.CodeBadRequest, "this connection has no job %d", job)
	}
	l.endJob(job, &protocol.JobFailed{Job: job, Message: "The job was cancelled."})
	return nil
}

func (l *localBackend) sessionStart(ctx context.Context, req protocol.SessionStart) (protocol.SessionReply, error) {
	if !slices.Equal(req.Sources, []Source{SourceClient}) {
		return protocol.SessionReply{}, protocol.Errorf(protocol.CodeUnsupported, "Local mode of this SDK takes only the source client")
	}
	if req.RecordTo != nil && !filepath.IsAbs(*req.RecordTo) {
		return protocol.SessionReply{}, protocol.Errorf(protocol.CodeBadRequest, "record_to must be absolute: %s", *req.RecordTo)
	}
	l.mu.Lock()
	busy := len(l.sessions) > 0
	l.mu.Unlock()
	if busy {
		return protocol.SessionReply{}, protocol.Errorf(protocol.CodeBusy, "a live session runs")
	}
	model, err := l.pick(ctx, req.Model, RoleFinal)
	if err != nil {
		return protocol.SessionReply{}, err
	}
	s := &localSession{model: model}
	if req.RecordTo != nil {
		s.recordTo = *req.RecordTo
	}
	preview := req.PreviewModel
	if preview == nil && slices.Contains(l.mustRoles(ctx, model), string(RoleStreaming)) {
		preview = &model
	}
	l.mu.Lock()
	l.nextSess++
	s.id = l.nextSess
	s.engineID = localSessionBase + s.id
	l.mu.Unlock()
	if preview != nil {
		p, err := l.pick(ctx, preview, RoleStreaming)
		if err != nil {
			return protocol.SessionReply{}, err
		}
		if err := l.load(ctx, p); err != nil {
			return protocol.SessionReply{}, err
		}
		if err := engineError(l.eng.StreamStart(ctx, s.engineID, p, deref(req.Language))); err != nil {
			return protocol.SessionReply{}, err
		}
		s.preview = p
	}
	l.mu.Lock()
	l.sessions[s.id] = s
	if s.preview != "" {
		l.byEngine[s.engineID] = s.id
	}
	l.mu.Unlock()
	return protocol.SessionReply{Session: s.id, Excludes: []string{}}, nil
}

func (l *localBackend) mustRoles(ctx context.Context, model string) []string {
	all, err := l.catalog(ctx)
	if err != nil {
		return nil
	}
	for _, m := range all {
		if m.ID == model {
			out := make([]string, len(m.Roles))
			for i, r := range m.Roles {
				out[i] = string(r)
			}
			return out
		}
	}
	return nil
}

func deref(s *string) string {
	if s == nil {
		return ""
	}
	return *s
}

func (l *localBackend) sessionAudio(session uint64, samples []int16) error {
	l.mu.Lock()
	s := l.sessions[session]
	if s != nil {
		s.samples = append(s.samples, samples...)
	}
	l.mu.Unlock()
	if s == nil {
		return protocol.Errorf(protocol.CodeBadRequest, "there is no session %d", session)
	}
	if s.preview != "" {
		return engineError(l.eng.StreamAudio(s.engineID, samples))
	}
	return nil
}

func (l *localBackend) sessionStop(ctx context.Context, session uint64) (string, error) {
	l.mu.Lock()
	s := l.sessions[session]
	l.mu.Unlock()
	if s == nil {
		return "", protocol.Errorf(protocol.CodeBadRequest, "there is no session %d", session)
	}
	defer func() {
		l.mu.Lock()
		delete(l.sessions, session)
		delete(l.byEngine, s.engineID)
		l.mu.Unlock()
	}()
	if s.preview != "" {
		if _, err := l.eng.StreamEnd(ctx, s.engineID); err != nil {
			return "", engineError(err)
		}
	}
	l.mu.Lock()
	samples := s.samples
	l.mu.Unlock()
	wav := filepath.Join(l.tmp, fmt.Sprintf("session-%d.wav", session))
	if s.recordTo != "" {
		if err := os.MkdirAll(s.recordTo, 0o700); err != nil {
			return "", err
		}
		wav = filepath.Join(s.recordTo, "client.wav")
	} else {
		defer os.Remove(wav)
	}
	if err := audio.WriteWAV(wav, samples); err != nil {
		return "", err
	}
	if err := l.load(ctx, s.model); err != nil {
		return "", err
	}
	final, err := l.eng.Transcribe(ctx, engine.TranscribeRequest{Model: s.model, Path: wav, Language: "auto", Segments: l.eng.Has("segments")})
	if err != nil {
		return "", engineError(err)
	}
	src := SourceClient
	for _, seg := range final.Segments {
		l.out.push(&protocol.SegmentEvent{Session: &session, Source: &src, Segment: protocol.Segment{StartMs: seg.StartMs, EndMs: seg.EndMs, Text: seg.Text}})
	}
	return strings.TrimSpace(final.Text), nil
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
	l.eng.Close()
	<-l.followed
	os.RemoveAll(l.tmp)
	l.out.close()
}
