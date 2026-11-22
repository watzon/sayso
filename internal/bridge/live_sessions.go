package bridge

import (
	"errors"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

// Live sessions (docs/extensions.md, session_start, session_audio,
// session_stop, "Scheduling", "Capture", "Lifecycle", "Privacy"; ported from
// sayso-speech/src/live/mod.rs): audio that is transcribed for an app while
// it arrives.
//
// A session has one or more sources (mic, system, client), and each source is
// a pipeline of its own (live_source.go): the audio goes to the recording (a
// WAV file), to a preview stream of the engine when the session has a
// streaming model (partial events), and to a VAD (a VAD session of the
// engine, or Basic VAD). Each speech that ends is a part, and the final model
// transcribes it (segment events, with times from the start of the session).
// Stop ends the audio, waits for the last parts, runs diarization on the
// recording of each source, sends speakers events, and only then gives the
// text, so the reply of session_stop comes after the last events.

type slotState int

const (
	slotFree slotState = iota
	// slotStarting: session_start opens the captures and the engine
	// sessions, and can wait for the answer of the user.
	slotStarting
	slotRunning
)

type liveSessions struct {
	deps LiveDeps
	mu   sync.Mutex
	// state, startConnection, startCancelled, and run are the slot. A start
	// whose connection closed or whose host quits is cancelled: it ends when
	// it can, with nothing left open.
	state           slotState
	startConnection uint64
	startCancelled  bool
	run             *runningSession
	// closed: no session starts after Close. Changed under mu.
	closed            bool
	nextSession       atomic.Uint64
	nextEngineSession atomic.Uint64
	obsMu             sync.Mutex
	observer          func(*LiveInfo)
}

type runningSession struct {
	info  LiveInfo
	owner Owner
	// client is the audio of the source client while it is open.
	client   *liveQueue[feedItem]
	excludes []string
	// stopper is held until the stop.
	stopper *stopper
}

// closeClient ends the audio of the source client.
func (r *runningSession) closeClient() {
	if r.client != nil {
		r.client.close()
		r.client = nil
	}
}

// capture is a capture that runs, and the queue of its source.
type capture struct {
	once sync.Once
	stop func()
	q    *liveQueue[feedItem]
}

// end stops the capture; the source then hears the rest of its audio.
func (c *capture) end() {
	c.once.Do(func() {
		if c.stop != nil {
			c.stop()
		}
		c.q.close()
	})
}

// stopper is what a stop needs, taken out of the slot so the stop does not
// hold its lock.
type stopper struct {
	sources     []*sourceRun
	captures    []*capture
	cancel      *atomic.Bool
	sink        EventSink
	diarization string
	// temporary are the recordings to delete after the stop (no record_to).
	temporary []string
	// recordings are all the recordings, for a start that was cancelled.
	recordings []string
	// echo is the echo rule, when the session has both mic and system.
	echo *echoGate
	// releaseModels lets the models of the session go.
	releaseModels func()
}

func (s *stopper) endCaptures() {
	for _, c := range s.captures {
		c.end()
	}
}

// newLiveSessions makes the live sessions of the host.
func newLiveSessions(deps LiveDeps) LiveSessions {
	l := &liveSessions{deps: deps}
	l.nextSession.Store(1)
	l.nextEngineSession.Store(EngineSessionBase)
	return l
}

func liveErr(code ErrorCode, format string, args ...any) *LiveError {
	return &LiveError{Code: code, Message: fmt.Sprintf(format, args...)}
}

func liveEngineErr(what string, err error) *LiveError {
	code, message := engineCode(err)
	return &LiveError{Code: code, Message: what + ": " + message}
}

func openErr(source Source, err error) *LiveError {
	var code ErrorCode
	message := err.Error()
	var oe *OpenError
	if errors.As(err, &oe) {
		message = oe.Message
		switch oe.Kind {
		case OpenPermission:
			code = CodePermission
		case OpenUnsupported:
			code = CodeUnsupported
		}
	}
	return liveErr(code, "the source %s did not start: %s", source, message)
}

func startCancelled() *LiveError {
	return liveErr("", "the start of the live session was cancelled, because the app or the program that runs it closed")
}

func unknownSession(session uint64) *LiveError {
	return liveErr(CodeBadRequest, "no live session %d of this connection runs", session)
}

func (l *liveSessions) SetObserver(f func(*LiveInfo)) {
	l.obsMu.Lock()
	l.observer = f
	l.obsMu.Unlock()
}

func (l *liveSessions) notify(info *LiveInfo) {
	l.obsMu.Lock()
	f := l.observer
	l.obsMu.Unlock()
	if f != nil {
		f(info)
	}
}

func cloneInfo(info LiveInfo) LiveInfo {
	info.Sources = append([]Source(nil), info.Sources...)
	return info
}

func (l *liveSessions) Current() (LiveInfo, bool) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.state != slotRunning {
		return LiveInfo{}, false
	}
	return cloneInfo(l.run.info), true
}

// checkSources allows mic and system (when the app can capture them) and
// client, each at most once.
func checkSources(sources []Source, hasMicrophone, hasSystemAudio bool) *LiveError {
	if len(sources) == 0 {
		return liveErr(CodeBadRequest, "a live session needs at least one source")
	}
	for i, source := range sources {
		for _, before := range sources[:i] {
			if before == source {
				return liveErr(CodeBadRequest, "the source %s is in the list twice", source)
			}
		}
		switch source {
		case SourceClient:
		case SourceMic:
			if !hasMicrophone {
				return liveErr(CodeUnsupported, "this program cannot capture the source %s", source)
			}
		case SourceSystem:
			if !hasSystemAudio {
				return liveErr(CodeUnsupported, "this program cannot capture the source %s", source)
			}
		default:
			return liveErr(CodeBadRequest, "unknown source %s", source)
		}
	}
	return nil
}

func hasSource(sources []Source, s Source) bool {
	for _, x := range sources {
		if x == s {
			return true
		}
	}
	return false
}

func (l *liveSessions) Start(owner Owner, spec SessionSpec, sink EventSink) (Started, *LiveError) {
	if err := checkSources(spec.Sources, l.deps.Microphone != nil, l.deps.SystemAudio != nil); err != nil {
		return Started{}, err
	}
	l.mu.Lock()
	if l.closed {
		l.mu.Unlock()
		return Started{}, startCancelled()
	}
	if l.state != slotFree {
		l.mu.Unlock()
		return Started{}, liveErr(CodeBusy, "another live session runs, and only one can run at a time")
	}
	l.state, l.startConnection, l.startCancelled = slotStarting, owner.Connection, false
	l.mu.Unlock()
	// Each way out of here without a session frees the slot, also a panic.
	defer func() {
		l.mu.Lock()
		if l.state == slotStarting {
			l.state = slotFree
		}
		l.mu.Unlock()
	}()

	session := l.nextSession.Add(1) - 1
	run, err := l.open(session, owner, spec, sink)
	if err != nil {
		return Started{}, err
	}
	l.mu.Lock()
	if !(l.state == slotStarting && l.startCancelled) {
		l.state, l.run = slotRunning, run
		info := cloneInfo(run.info)
		excludes := append([]string(nil), run.excludes...)
		l.mu.Unlock()
		slog.Info("host: live session started", "session", session, "app", owner.AppID, "sources", spec.Sources)
		l.notify(&info)
		return Started{Session: session, Excludes: excludes}, nil
	}
	l.mu.Unlock()
	// The app or the host went while this opened. The slot stays taken until
	// the sources closed.
	run.closeClient()
	st := run.stopper
	st.cancel.Store(true)
	st.temporary = st.recordings
	l.finish(session, st, true)
	return Started{}, startCancelled()
}

func (l *liveSessions) startIsCancelled() bool {
	l.mu.Lock()
	defer l.mu.Unlock()
	return l.state == slotStarting && l.startCancelled
}

func (l *liveSessions) engineSession() uint64 { return l.nextEngineSession.Add(1) - 1 }

// recordingPaths gives the recording of each source: <record_to>/<source>.wav,
// else a file in the folder of temporary recordings. It makes the folder.
func (l *liveSessions) recordingPaths(session uint64, recordTo string, sources []Source) ([]string, *LiveError) {
	dir := l.deps.RecordingDir
	if recordTo != "" {
		if !filepath.IsAbs(recordTo) {
			return nil, liveErr(CodeBadRequest, "record_to must be an absolute path, not %s", recordTo)
		}
		dir = recordTo
	}
	if dir == "" {
		dir = filepath.Join(os.TempDir(), "sayso-live")
	}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, liveErr(CodeBadRequest, "cannot make the folder %s: %v", dir, err)
	}
	out := make([]string, len(sources))
	for i, source := range sources {
		if recordTo != "" {
			out[i] = filepath.Join(dir, RecordingName(source))
		} else {
			out[i] = filepath.Join(dir, fmt.Sprintf("sayso-live-%d-%d-%s", os.Getpid(), session, RecordingName(source)))
		}
	}
	return out, nil
}

// RecordingName is the file name of the recording of a source in record_to.
func RecordingName(source Source) string { return string(source) + ".wav" }

// opened is what a start opened so far. A start that fails closes it all:
// the sources that run end, the engine sessions and watches that no source
// owns yet close, and the recordings that it made go.
type opened struct {
	eng      Engine
	sources  []*sourceRun
	client   *liveQueue[feedItem]
	captures []*capture
	streams  []uint64
	vads     []uint64
	watches  []engineWatch
	written  []string
	release  func()
}

func (o *opened) undo() {
	// The audio ends, so the sources that run end too.
	if o.client != nil {
		o.client.close()
	}
	for _, c := range o.captures {
		c.end()
	}
	for _, s := range o.sources {
		s.join()
	}
	for _, id := range o.streams {
		o.eng.EndStream(id)
	}
	for _, id := range o.vads {
		o.eng.EndVad(id)
	}
	for _, w := range o.watches {
		w.unwatch()
	}
	for _, p := range o.written {
		os.Remove(p)
	}
	if o.release != nil {
		o.release()
	}
}

func (l *liveSessions) loadModel(model string) error {
	if l.deps.Engine.Status(model).IsUsable() {
		return nil
	}
	return l.deps.Engine.Load(model)
}

// open opens the captures, the engine sessions, and the recordings, and
// starts the sources.
func (l *liveSessions) open(session uint64, owner Owner, spec SessionSpec, sink EventSink) (run *runningSession, lerr *LiveError) {
	deps := l.deps
	eng := deps.Engine
	o := &opened{eng: eng}
	defer func() {
		if run == nil {
			o.undo()
		}
	}()
	recordings, lerr := l.recordingPaths(session, spec.RecordTo, spec.Sources)
	if lerr != nil {
		return nil, lerr
	}
	// Hold before the load, so the app does not unload a model between the two.
	if deps.Models != nil {
		o.release = deps.Models.Hold(spec.Models())
	}
	load := []string{spec.FinalModel}
	if spec.PreviewModel != "" {
		load = append(load, spec.PreviewModel)
	}
	if !spec.Vad.Basic && spec.Vad.Model != "" {
		load = append(load, spec.Vad.Model)
	}
	for _, m := range load {
		if err := l.loadModel(m); err != nil {
			return nil, liveEngineErr("cannot load "+m, err)
		}
	}

	cancel := &atomic.Bool{}
	clock := newSessionClock()
	var echo *echoGate
	if hasSource(spec.Sources, SourceMic) && hasSource(spec.Sources, SourceSystem) {
		echo = newEchoGate(session, sink)
	}
	// The captures first, and the system audio first of them. It can wait for
	// the answer of the user, and the microphone must not run during that
	// wait, when Sayso shows no recording state yet. A refusal stops the
	// start with no microphone and no recording.
	var excludes []string
	feeds := map[Source]*liveQueue[feedItem]{}
	if hasSource(spec.Sources, SourceSystem) {
		q := newLiveQueue[feedItem]()
		stop, ex, err := deps.SystemAudio.Open(owner.PID, queueFeed{q})
		if err != nil {
			return nil, openErr(SourceSystem, err)
		}
		o.captures = append(o.captures, &capture{stop: stop, q: q})
		excludes = ex
		feeds[SourceSystem] = q
		if l.startIsCancelled() {
			return nil, startCancelled()
		}
	}
	if hasSource(spec.Sources, SourceMic) {
		q := newLiveQueue[feedItem]()
		stop, err := deps.Microphone.Open(queueFeed{q})
		if err != nil {
			return nil, openErr(SourceMic, err)
		}
		o.captures = append(o.captures, &capture{stop: stop, q: q})
		feeds[SourceMic] = q
	}
	var silence *silenceWatch
	if spec.SilenceLimit > 0 {
		silence = newSilenceWatch(spec.SilenceLimit, len(spec.Sources), l.onSilenceLimit(session, spec.SilenceLimit, sink))
	}
	for index, source := range spec.Sources {
		// Watch before the engine sessions open, so no event is lost.
		watch := watchEngine(deps.Bus)
		o.watches = append(o.watches, watch)
		plan := sourcePlan{
			session: session, source: source, finalModel: spec.FinalModel, language: spec.Language,
			clock: clock, silence: silence, silenceIndex: index,
		}
		if spec.PreviewModel != "" {
			id := l.engineSession()
			if err := eng.StartStream(id, spec.PreviewModel, spec.Language); err != nil {
				return nil, liveEngineErr("cannot open the preview stream", err)
			}
			o.streams = append(o.streams, id)
			plan.preview, plan.hasPreview = id, true
		}
		if !spec.Vad.Basic && spec.Vad.Model != "" {
			id := l.engineSession()
			if err := eng.StartVad(id, spec.Vad.Model); err != nil {
				return nil, liveEngineErr("cannot open the VAD session", err)
			}
			o.vads = append(o.vads, id)
			plan.vad, plan.engineVad = id, true
		}
		wav, err := createWav(recordings[index])
		if err != nil {
			return nil, liveErr(CodeBadRequest, "cannot write %s: %v", recordings[index], err)
		}
		o.written = append(o.written, recordings[index])
		plan.wav = wav
		audio, captured := feeds[source]
		if !captured {
			audio = newLiveQueue[feedItem]()
			o.client = audio
		}
		if echo != nil {
			switch source {
			case SourceMic:
				plan.echo, plan.echoRole = echo, echoMic
			case SourceSystem:
				plan.echo, plan.echoRole = echo, echoSystem
			}
		}
		o.sources = append(o.sources, startSource(plan, audio, watch, eng, sink, deps.Gate, cancel))
		// The source ends its engine sessions and its watch now.
		o.streams, o.vads, o.watches = nil, nil, nil
	}
	st := &stopper{
		sources: o.sources, captures: o.captures, cancel: cancel, sink: sink,
		diarization: spec.Diarization, recordings: recordings, echo: echo, releaseModels: o.release,
	}
	if spec.RecordTo == "" {
		st.temporary = append([]string(nil), recordings...)
	}
	return &runningSession{
		info: LiveInfo{
			Session: session, AppID: owner.AppID, Sources: append([]Source(nil), spec.Sources...),
			CapturesMic: hasSource(spec.Sources, SourceMic),
		},
		owner: owner, client: o.client, excludes: excludes, stopper: st,
	}, nil
}

// onSilenceLimit is what runs when the silence limit of session passes: end
// the audio of the sources and send the event. It is false while the session
// still starts, so the watch asks again at the next audio. session_stop ends
// the session as usual, and the last parts come before its reply.
func (l *liveSessions) onSilenceLimit(session uint64, limit time.Duration, sink EventSink) func() bool {
	return func() bool {
		l.mu.Lock()
		switch {
		case l.state == slotStarting:
			l.mu.Unlock()
			return false
		case l.state == slotRunning && l.run.info.Session == session:
			l.run.closeClient()
			if l.run.stopper != nil {
				l.run.stopper.endCaptures()
			}
		default:
			l.mu.Unlock()
			return true
		}
		l.mu.Unlock()
		slog.Info("host: live session: no speech for the silence limit, so its audio stops", "session", session, "ms", limit.Milliseconds())
		sink.Event(SilenceLimit{Session: session, SilenceMs: uint64(limit.Milliseconds())})
		return true
	}
}

func (l *liveSessions) Audio(connection, session uint64, pcm string) *LiveError {
	samples, err := DecodeF32(pcm)
	if err != nil {
		return liveErr(CodeBadRequest, "%s", err.Error())
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.state != slotRunning || l.run.info.Session != session || l.run.owner.Connection != connection {
		return unknownSession(session)
	}
	if l.run.client == nil {
		if l.run.info.Stopping {
			return liveErr(CodeBadRequest, "the live session stops")
		}
		return liveErr(CodeBadRequest, "the live session has no source client")
	}
	l.run.client.push(feedItem{kind: feedAudio, audio: Captured{Samples: samples}})
	return nil
}

// takeStopper marks the session as stopping and takes what its stop needs.
// Called with mu held.
func (l *liveSessions) takeStopper() *stopper {
	st := l.run.stopper
	l.run.stopper = nil
	l.run.closeClient()
	l.run.info.Stopping = true
	return st
}

func (l *liveSessions) Stop(connection, session uint64) (string, *LiveError) {
	l.mu.Lock()
	if l.state != slotRunning || l.run.info.Session != session || l.run.owner.Connection != connection {
		l.mu.Unlock()
		return "", unknownSession(session)
	}
	if l.run.stopper == nil {
		l.mu.Unlock()
		return "", liveErr(CodeBadRequest, "the live session stops already")
	}
	st := l.takeStopper()
	info := cloneInfo(l.run.info)
	l.mu.Unlock()
	l.notify(&info)
	text, err := l.finish(session, st, false)
	l.free(session)
	return text, err
}

func (l *liveSessions) StopForUser(session uint64) bool {
	l.mu.Lock()
	if l.state != slotRunning || l.run.info.Session != session || l.run.stopper == nil {
		l.mu.Unlock()
		return false
	}
	st := l.takeStopper()
	info := cloneInfo(l.run.info)
	l.mu.Unlock()
	l.notify(&info)
	go func() {
		text, err := l.finish(session, st, false)
		if err != nil {
			slog.Warn("host: the stop of a live session failed", "session", session, "error", err.Message)
			text = ""
		}
		l.free(session)
		st.sink.Event(SessionEnded{Session: session, Text: text})
	}()
	return true
}

func (l *liveSessions) ConnectionClosed(connection uint64) { l.cancelSession(&connection) }

func (l *liveSessions) Close() { l.cancelSession(nil) }

// cancelSession cancels the session of connection, or any session. A session
// in its start cannot stop now, for example during a question of the system;
// it ends when the question ends.
func (l *liveSessions) cancelSession(connection *uint64) {
	l.mu.Lock()
	if connection == nil {
		l.closed = true
	}
	var st *stopper
	var session uint64
	switch {
	case l.state == slotStarting && (connection == nil || l.startConnection == *connection):
		l.startCancelled = true
	case l.state == slotRunning && (connection == nil || l.run.owner.Connection == *connection):
		session = l.run.info.Session
		st = l.takeStopper()
	}
	l.mu.Unlock()
	if st == nil {
		return
	}
	slog.Info("host: live session is cancelled: its connection or the host closes", "session", session)
	st.cancel.Store(true)
	go func() {
		l.finish(session, st, true)
		l.free(session)
	}()
}

func (l *liveSessions) finish(session uint64, st *stopper, cancelled bool) (string, *LiveError) {
	if st.releaseModels != nil {
		defer st.releaseModels()
	}
	// The audio of the captures ends here; the client queue closed with the slot.
	st.endCaptures()
	results := make([]sourceResult, 0, len(st.sources))
	for _, s := range st.sources {
		results = append(results, s.join())
	}
	if st.echo != nil {
		// The rule sends what still waits, and it has the segments that stayed.
		st.echo.systemEnded()
		for i := range results {
			if results[i].source == SourceMic {
				results[i].segments = st.echo.keptSegments()
			}
		}
	}
	var text string
	var err *LiveError
	if !cancelled {
		text, err = l.speakersAndText(session, results, st.diarization, st.sink)
	}
	for _, p := range st.temporary {
		if e := os.Remove(p); e != nil && !errors.Is(e, os.ErrNotExist) {
			slog.Warn("host: cannot delete a live recording", "path", p, "error", e)
		}
	}
	slog.Info("host: live session ended", "session", session)
	return text, err
}

func (l *liveSessions) speakersAndText(session uint64, results []sourceResult, diarization string, sink EventSink) (string, *LiveError) {
	for _, r := range results {
		if r.wavError != "" {
			return "", liveErr("", "%s", r.wavError)
		}
	}
	if diarization != "" {
		eng, gate := l.deps.Engine, l.deps.Gate
		for _, r := range results {
			if gate != nil {
				gate.WaitIdle(func() bool { return false })
			}
			if err := l.loadModel(diarization); err != nil {
				return "", liveEngineErr("cannot load "+diarization, err)
			}
			samples, err := readLiveWav(r.wav)
			if err != nil {
				return "", liveErr("", "%s", err.Error())
			}
			release := func() {}
			if gate != nil {
				release = gate.LivePart()
			}
			found, err := eng.Diarize(diarization, samples, nil)
			release()
			if err != nil {
				return "", liveEngineErr("diarization failed", err)
			}
			// The rules of jobs: a session has no number of speakers, so
			// small speakers fold.
			found = foldSmallSpeakers(found)
			times := make([][2]uint64, len(r.segments))
			for i, s := range r.segments {
				times[i] = [2]uint64{s.StartMs, s.EndMs}
			}
			turns := segmentTurns(times, assignSpeakers(times, found))
			id, source := session, r.source
			sink.Event(SpeakersEvent{Session: &id, Source: &source, Turns: turns})
		}
	}
	return sessionText(results), nil
}

func (l *liveSessions) free(session uint64) {
	l.mu.Lock()
	if l.state == slotRunning && l.run.info.Session == session {
		l.state, l.run = slotFree, nil
		l.mu.Unlock()
		l.notify(nil)
		return
	}
	l.mu.Unlock()
}

// sessionText is the text of the session: the segments of all sources in the
// order of time.
func sessionText(results []sourceResult) string {
	type piece struct {
		start  uint64
		source int
		text   string
	}
	var all []piece
	for i, r := range results {
		for _, s := range r.segments {
			all = append(all, piece{s.StartMs, i, strings.TrimSpace(s.Text)})
		}
	}
	sort.SliceStable(all, func(a, b int) bool {
		if all[a].start != all[b].start {
			return all[a].start < all[b].start
		}
		return all[a].source < all[b].source
	})
	var texts []string
	for _, p := range all {
		if p.text != "" {
			texts = append(texts, p.text)
		}
	}
	return strings.Join(texts, " ")
}
