package bridge

import (
	"errors"
	"log/slog"
	"math"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/watzon/sayso/internal/engine"
)

// The pipeline of one source of a live session (sayso-speech/src/live/source.rs).
//
// Two goroutines:
//
//   - intake takes the audio as it arrives. It writes the recording, sends
//     the audio to the preview stream and to the VAD, sends partial events,
//     and cuts a part of audio for each speech that ends. It never waits for
//     a final pass, so the audio does not wait either.
//   - final runs the final pass on each part, one at a time, and sends a
//     segment event for each segment, with times from the start of the
//     session. Before each part it waits while a dictation runs.
//
// The source stops when its audio queue closes, or when its capture says
// that it ended: the intake hears the rest of the audio, closes the preview
// stream and the VAD session, cuts the last part, and waits for the final
// goroutine.
//
// The times of all sources are from the start of the session. A source that
// starts late or loses audio gets silence there, so its recording and its
// times stay on the time of the other sources.

const (
	liveRate = uint64(SampleRate)
	// livePadMs of audio go before and after the speech of a part, so the
	// first and last words are complete. The audio before a part does not
	// reach into the part before it.
	livePadMs = 200
	// liveLookbackMs is how much audio the intake keeps while nobody speaks,
	// for a speech start that the engine reports late.
	liveLookbackMs = 10_000
	// liveMaxSilenceMs is the most silence that a source that starts late or
	// loses audio gets, so a computer that slept for hours does not write
	// hours of it.
	liveMaxSilenceMs = 60_000
	// liveEchoVadLagMs is how long after the audio a VAD can report the start
	// of a speech. The echo rule treats older audio without an open speech as
	// done.
	liveEchoVadLagMs = 1_000
	// liveEchoMark is how often the system source tells the echo rule how far
	// it is.
	liveEchoMark = 250 * time.Millisecond
)

type feedKind int

const (
	feedAudio feedKind = iota
	feedGap
	feedEnded
)

// feedItem is what a capture gives, in order.
type feedItem struct {
	kind  feedKind
	audio Captured
	gapMs uint64
	end   SourceEnd
}

// queueFeed is the Feed of a capture: it puts the items in the queue of its
// source.
type queueFeed struct{ q *liveQueue[feedItem] }

func (f queueFeed) Audio(c Captured) { f.q.push(feedItem{kind: feedAudio, audio: c}) }
func (f queueFeed) Gap(ms uint64)    { f.q.push(feedItem{kind: feedGap, gapMs: ms}) }
func (f queueFeed) Ended(e SourceEnd) {
	f.q.push(feedItem{kind: feedEnded, end: e})
}

// sessionClock is the time of the first audio of a session on the capture
// clock, which all its sources share.
type sessionClock struct {
	mu     sync.Mutex
	origin int64
}

func newSessionClock() *sessionClock { return &sessionClock{origin: math.MaxInt64} }

// firstAudio says that the first audio of a source has the time ms. It gives
// how late it is after the first audio of the session.
func (c *sessionClock) firstAudio(ms int64) uint64 {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.origin = min(c.origin, ms)
	return uint64(ms - c.origin)
}

type echoRole int

const (
	echoNone echoRole = iota
	echoMic
	echoSystem
)

// sourcePlan is everything that a source needs, after the session opened its
// engine sessions.
type sourcePlan struct {
	session    uint64
	source     Source
	finalModel string
	language   string
	// preview is the engine session of the preview stream, when hasPreview.
	preview    uint64
	hasPreview bool
	// vad is the engine session of the VAD, when engineVad; else Basic VAD.
	vad       uint64
	engineVad bool
	wav       *wavWriter
	clock     *sessionClock
	// echo is the echo rule of a session with both mic and system.
	echo     *echoGate
	echoRole echoRole
	// silence is the silence limit of the session, and silenceIndex the
	// place of this source in it.
	silence      *silenceWatch
	silenceIndex int
}

// sourceResult is what a source gives when it ends.
type sourceResult struct {
	source   Source
	segments []Segment
	wav      string
	// wavError says why the recording is not complete, when it is not.
	wavError string
}

// sourceRun is a source that runs.
type sourceRun struct{ done chan sourceResult }

// join waits until the source ended. Its audio queue must be closed first.
func (r *sourceRun) join() sourceResult { return <-r.done }

// engineWatch is the subscription of a source to the events of the engine.
type engineWatch struct {
	events  <-chan engine.Event
	unwatch func()
	sync    func()
}

func watchEngine(b *bus) engineWatch {
	if b == nil {
		return engineWatch{unwatch: func() {}, sync: func() {}}
	}
	events, unwatch := b.watch()
	return engineWatch{events: events, unwatch: unwatch, sync: b.sync}
}

// startSource starts the goroutines of a source. The watch must exist before
// the engine sessions opened, so no event is lost. cancel stops the final
// passes that wait.
func startSource(plan sourcePlan, audio *liveQueue[feedItem], watch engineWatch, eng Engine, sink EventSink, gate *DictationGate, cancel *atomic.Bool) *sourceRun {
	parts := newLiveQueue[partMessage]()
	segments := make(chan []Segment, 1)
	fp := &finalPass{
		session: plan.session, source: plan.source, model: plan.finalModel, language: plan.language,
		engine: eng, sink: sink, gate: gate, cancel: cancel, echo: plan.echo, echoRole: plan.echoRole,
	}
	go func() { segments <- fp.run(parts) }()
	in := newIntake(plan, eng, sink, parts)
	run := &sourceRun{done: make(chan sourceResult, 1)}
	go func() { run.done <- in.run(audio, watch, segments) }()
	return run
}

// partAudio is the audio of one part, for the final pass.
type partAudio struct {
	// offsetMs is the time of the first sample, from the start of the session.
	offsetMs uint64
	samples  []float32
	// clockOffset is from a time of the session to the capture clock.
	clockOffset int64
}

// partMessage is what the intake sends to the final pass, in order: a part,
// or (doneUntil) for the system source, that all speech before this time on
// the capture clock is in the parts before it.
type partMessage struct {
	part      *partAudio
	doneUntil int64
}

type intake struct {
	session    uint64
	source     Source
	preview    uint64
	hasPreview bool
	// basic is the Basic VAD, or nil for the VAD session vad of the engine
	// with the start of its open speech.
	basic    *BasicVad
	vad      uint64
	open     bool
	openFrom uint64
	engine   Engine
	sink     EventSink
	wav      *wavWriter
	wavPath  string
	wavError string
	// kept is the audio that a part can still need; it starts at sample keptFrom.
	kept     []float32
	keptFrom uint64
	// heard is all the samples heard.
	heard uint64
	// lastEndMs is the end of the last part.
	lastEndMs uint64
	parts     *liveQueue[partMessage]
	clock     *sessionClock
	// timed is true after the first audio with a capture time.
	timed       bool
	clockOffset int64
	// echo is the gate when this is the system source of the echo rule.
	echo         *echoGate
	silence      *silenceWatch
	silenceIndex int
	// lastSpeechMs is the last time that this source had speech, in ms of audio.
	lastSpeechMs uint64
}

func newIntake(plan sourcePlan, eng Engine, sink EventSink, parts *liveQueue[partMessage]) *intake {
	in := &intake{
		session: plan.session, source: plan.source, preview: plan.preview, hasPreview: plan.hasPreview,
		vad: plan.vad, engine: eng, sink: sink, wav: plan.wav, wavPath: plan.wav.path, parts: parts,
		clock: plan.clock, silence: plan.silence, silenceIndex: plan.silenceIndex,
	}
	if !plan.engineVad {
		in.basic = NewBasicVad()
	}
	if plan.echoRole == echoSystem {
		in.echo = plan.echo
	}
	return in
}

func (in *intake) run(audio *liveQueue[feedItem], watch engineWatch, finalDone <-chan []Segment) sourceResult {
	events := watch.events
	lastTrim, lastMark := time.Now(), time.Now()
	var ended *SourceEnd
loop:
	for {
		events = in.takeEvents(events)
		item, ok, done := audio.tryPop()
		if !ok {
			if done {
				break
			}
			select {
			case <-audio.signal:
			case e, open := <-events:
				if open {
					in.event(e)
				} else {
					events = nil
				}
			}
		} else {
			switch item.kind {
			case feedAudio:
				in.captured(item.audio)
			case feedGap:
				in.gap(item.gapMs)
			case feedEnded:
				end := item.end
				ended = &end
				break loop
			}
		}
		if time.Since(lastTrim) >= time.Second {
			in.trimIdle()
			lastTrim = time.Now()
		}
		if in.echo != nil && time.Since(lastMark) >= liveEchoMark {
			in.echoMark()
			lastMark = time.Now()
		}
	}
	in.finish(events, watch.sync)
	watch.unwatch()
	// The final goroutine ends when the last part is done.
	in.parts.close()
	segments := <-finalDone
	if ended != nil {
		reason := EndFailed
		if ended.Reason == EndPermission {
			reason = EndPermission
		} else {
			slog.Warn("host: live session: a source ended", "session", in.session, "source", in.source, "message", ended.Message)
		}
		in.sink.Event(SourceEnded{Session: in.session, Source: in.source, Reason: reason})
	}
	return sourceResult{source: in.source, segments: segments, wav: in.wavPath, wavError: in.wavError}
}

// takeEvents handles the events of the engine that wait, without waiting. It
// gives nil when the channel closed.
func (in *intake) takeEvents(events <-chan engine.Event) <-chan engine.Event {
	for events != nil {
		select {
		case e, open := <-events:
			if !open {
				return nil
			}
			in.event(e)
		default:
			return events
		}
	}
	return nil
}

func (in *intake) heardMs() uint64 { return in.heard * 1000 / liveRate }

func (in *intake) captured(c Captured) {
	if c.Timed {
		ms := c.Time.Milliseconds()
		if !in.timed {
			in.timed = true
			in.silenceFor(in.clock.firstAudio(ms))
		}
		in.clockOffset = ms - int64(in.heardMs())
	}
	in.hear(c.Samples)
}

// gap says that the capture lost ms of audio: silence goes there, in the
// recording and for the VAD.
func (in *intake) gap(ms uint64) {
	in.silenceFor(ms)
	in.sink.Event(SourceGap{Session: in.session, Source: in.source, Ms: ms})
}

func (in *intake) silenceFor(ms uint64) {
	zeros := make([]float32, liveRate/50)
	for i := uint64(0); i < min(ms, liveMaxSilenceMs)/20; i++ {
		in.hear(zeros)
	}
}

// echoMark tells the echo rule how far the system source is. The message
// goes through the queue of parts, so the parts before it are done when it
// arrives.
func (in *intake) echoMark() {
	now := int64(in.heardMs()) + in.clockOffset
	in.echo.systemHeard(now)
	if !in.speaking() {
		in.parts.push(partMessage{doneUntil: now - liveEchoVadLagMs})
	}
}

func (in *intake) speaking() bool {
	if in.basic != nil {
		return in.basic.Speaking()
	}
	return in.open
}

func (in *intake) hear(samples []float32) {
	if len(samples) == 0 {
		return
	}
	if in.wav != nil {
		if err := in.wav.write(samples); err != nil {
			slog.Warn("host: live session: cannot write the recording", "session", in.session, "path", in.wavPath, "error", err)
			in.wavError = "cannot write " + in.wavPath + ": " + err.Error()
			in.wav.abandon()
			in.wav = nil
		}
	}
	in.kept = append(in.kept, samples...)
	in.heard += uint64(len(samples))
	if in.hasPreview {
		if err := in.engine.PushAudio(in.preview, samples); err != nil {
			slog.Debug("host: live session: preview audio", "session", in.session, "error", err)
		}
	}
	if in.basic != nil {
		for _, p := range in.basic.Accept(samples) {
			in.cut(p.StartMs, p.EndMs)
		}
	} else if err := in.engine.PushAudio(in.vad, samples); err != nil {
		slog.Warn("host: live session: VAD audio", "session", in.session, "error", err)
	}
	in.watchSilence()
}

// watchSilence tells the silence watch how long nobody has spoken in this
// source.
func (in *intake) watchSilence() {
	if in.silence == nil {
		return
	}
	heardMs := in.heardMs()
	if in.speaking() {
		in.lastSpeechMs = heardMs
	}
	in.silence.heard(in.silenceIndex, satSub(heardMs, in.lastSpeechMs))
}

func (in *intake) event(e engine.Event) {
	switch e.Kind {
	case engine.EventPartial:
		if in.hasPreview && e.Session == in.preview {
			source := in.source
			in.sink.Event(PartialEvent{Session: in.session, Source: &source, Committed: e.Committed, Tentative: e.Tentative})
		}
	case engine.EventSpeechStart:
		if in.basic == nil && e.Session == in.vad {
			in.open, in.openFrom = true, e.StartMs
		}
	case engine.EventSpeechEnd:
		if in.basic == nil && e.Session == in.vad {
			in.open = false
			in.lastSpeechMs = max(in.lastSpeechMs, e.EndMs)
			in.cut(e.StartMs, e.EndMs)
		}
	}
}

// cut sends the audio of a speech from startMs to endMs that ended to the
// final pass.
func (in *intake) cut(startMs, endMs uint64) {
	startMs = max(startMs, in.lastEndMs)
	if endMs <= startMs {
		return
	}
	padBefore := min(uint64(livePadMs), startMs-in.lastEndMs)
	from := max(sampleAt(startMs-padBefore), in.keptFrom)
	to := min(sampleAt(endMs+livePadMs), in.heard)
	if from >= to {
		slog.Debug("host: live session: the audio of a part is gone", "session", in.session, "start_ms", startMs, "end_ms", endMs)
		return
	}
	samples := append([]float32(nil), in.kept[from-in.keptFrom:to-in.keptFrom]...)
	in.lastEndMs = endMs
	// A later part starts at the end of this one or after it.
	in.dropBefore(sampleAt(endMs))
	in.parts.push(partMessage{part: &partAudio{offsetMs: from * 1000 / liveRate, samples: samples, clockOffset: in.clockOffset}})
}

// trimIdle forgets the audio older than liveLookbackMs while nobody speaks.
func (in *intake) trimIdle() {
	var keepFrom uint64
	switch {
	case in.basic != nil && in.basic.Speaking():
		return
	case in.basic == nil && in.open:
		keepFrom = sampleAt(satSub(in.openFrom, livePadMs))
	default:
		keepFrom = satSub(in.heard, sampleAt(liveLookbackMs))
	}
	in.dropBefore(max(keepFrom, sampleAt(in.lastEndMs)))
}

func (in *intake) dropBefore(sample uint64) {
	sample = min(sample, in.heard)
	if sample > in.keptFrom {
		in.kept = append(in.kept[:0:0], in.kept[sample-in.keptFrom:]...)
		in.keptFrom = sample
	}
}

// finish closes the engine sessions, cuts the last part, and closes the
// recording, after the audio ended.
func (in *intake) finish(events <-chan engine.Event, sync func()) {
	if in.hasPreview {
		in.engine.EndStream(in.preview)
	}
	if in.basic != nil {
		// The partials that came already. The session does not wait for the
		// last partial of the engine: the segments have the text.
		in.takeEvents(events)
		for _, p := range in.basic.Finish() {
			in.cut(p.StartMs, p.EndMs)
		}
	} else {
		// The engine sends the last speech_end before its reply, and the bus
		// passes it on at sync. A backend that cannot wait for the reply
		// leaves the speech open: it ends at the end of the audio.
		if err := in.engine.EndVad(in.vad); err != nil {
			slog.Warn("host: live session: the end of the VAD session failed", "session", in.session, "error", err)
		}
		sync()
		in.takeEvents(events)
		if in.open {
			in.cut(in.openFrom, in.heardMs())
		}
	}
	if in.wav != nil {
		if _, err := in.wav.finish(); err != nil {
			slog.Warn("host: live session: cannot close the recording", "session", in.session, "path", in.wavPath, "error", err)
			in.wavError = "cannot write " + in.wavPath + ": " + err.Error()
		}
		in.wav = nil
	}
}

func sampleAt(ms uint64) uint64 { return ms * liveRate / 1000 }

type finalPass struct {
	session  uint64
	source   Source
	model    string
	language string
	engine   Engine
	sink     EventSink
	gate     *DictationGate
	cancel   *atomic.Bool
	echo     *echoGate
	echoRole echoRole
}

func (f *finalPass) run(parts *liveQueue[partMessage]) []Segment {
	var all []Segment
	cancelled := func() bool { return f.cancel.Load() }
	for {
		m, ok := parts.pop()
		if !ok {
			break
		}
		if m.part == nil {
			if f.echoRole == echoSystem {
				f.echo.systemDoneUntil(m.doneUntil)
			}
			continue
		}
		part := m.part
		if cancelled() || (f.gate != nil && !f.gate.WaitIdle(cancelled)) {
			continue
		}
		release := func() {}
		if f.gate != nil {
			release = f.gate.LivePart()
		}
		segments, err := liveTranscribe(f.engine, part.samples, f.model, f.language)
		release()
		if err != nil {
			slog.Warn("host: live session: the final pass of a part failed", "session", f.session, "error", err)
			if !cancelled() {
				// The app needs this event to tell an engine error from
				// silence. It does not use the echo gate: no segment comes
				// from this part.
				code, message := engineCode(err)
				var c *ErrorCode
				if code != "" {
					c = &code
				}
				f.sink.Event(PartFailed{
					Session: f.session, Source: f.source, StartMs: part.offsetMs,
					EndMs:   part.offsetMs + uint64(len(part.samples))*1000/liveRate,
					Message: message, Code: c,
				})
			}
			continue
		}
		for _, s := range segments {
			s = shiftSegment(s, part.offsetMs)
			if strings.TrimSpace(s.Text) == "" {
				continue
			}
			if f.echoRole == echoMic {
				// The gate sends it when the system source has caught up,
				// unless it is an echo.
				f.echo.micSegment(s, part.clockOffset)
				continue
			}
			session, source := f.session, f.source
			f.sink.Event(SegmentEvent{Session: &session, Source: &source, Segment: s})
			if f.echoRole == echoSystem {
				f.echo.systemSegment(s, part.clockOffset)
			}
			all = append(all, s)
		}
	}
	if f.echoRole == echoSystem {
		f.echo.systemEnded()
	}
	return all
}

// liveTranscribe gives the segments of a part. An engine without segments
// gives one segment for the whole part (backend.rs).
func liveTranscribe(eng Engine, samples []float32, model, language string) ([]Segment, error) {
	if !hasFeature(eng.Features(), "segments") {
		text, err := eng.Transcribe(samples, model, language)
		if err != nil {
			return nil, err
		}
		return []Segment{{StartMs: 0, EndMs: uint64(len(samples)) * 1000 / liveRate, Text: strings.TrimSpace(text)}}, nil
	}
	found, err := eng.TranscribeSegments(samples, model, language)
	if err != nil {
		return nil, err
	}
	out := make([]Segment, len(found))
	for i, s := range found {
		out[i] = Segment{StartMs: s.StartMs, EndMs: s.EndMs, Text: strings.TrimSpace(s.Text)}
		if len(s.Words) > 0 {
			out[i].Words = append([]Word(nil), s.Words...)
		}
	}
	return out, nil
}

// engineCode is the protocol code and the message of an engine error.
func engineCode(err error) (ErrorCode, string) {
	var e *EngineError
	if errors.As(err, &e) {
		return e.Code, e.Message
	}
	return "", err.Error()
}

// shiftSegment gives a segment with times from the start of the session.
func shiftSegment(s Segment, offsetMs uint64) Segment {
	s.StartMs += offsetMs
	s.EndMs += offsetMs
	if len(s.Words) > 0 {
		words := make([]Word, len(s.Words))
		for i, w := range s.Words {
			w.StartMs += offsetMs
			w.EndMs += offsetMs
			words[i] = w
		}
		s.Words = words
	}
	return s
}
