package localengine

import (
	"fmt"
	"os"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

// A live preview for a model with no streaming decoder: decode the growing
// recording again with the final pass (redecode_preview in the catalog).
//
// A worker decodes the uncommitted audio about every 600 ms, when at least
// 300 ms of new audio arrived. Once the uncommitted audio passes 10 s, the
// older part is decoded one last time and committed. The cut is at the
// quietest 100 ms window between 7 s and 2 s before the end. committed then
// only grows, and tentative is the text of the last 2 to 7 s.
//
// The preview and the final pass share one model. While a final pass runs,
// no preview decode starts, and the end of a stream skips its last decode.

// attachRedecode gives a loaded final pass a redecode live preview.
func attachRedecode(fp finalPass) *loaded {
	running := &atomic.Int64{}
	// Both parts hold the model and each lets go of it once: the reference of
	// the engine covers the final pass, and this one covers the preview.
	retainPart(fp)
	return &loaded{
		finalPass: &countedFinalPass{inner: fp, running: running},
		preview:   &redecodePreview{model: fp, running: running, config: defaultRedecodeConfig()},
	}
}

// countedFinalPass counts the final passes in progress.
type countedFinalPass struct {
	inner   finalPass
	running *atomic.Int64
}

func (c *countedFinalPass) transcribe(samples []float32, language string, vocabulary []string, times bool) (timed, error) {
	c.running.Add(1)
	defer c.running.Add(-1)
	return c.inner.transcribe(samples, language, vocabulary, times)
}

func (c *countedFinalPass) retain()  { retainPart(c.inner) }
func (c *countedFinalPass) release() { releasePart(c.inner) }

type redecodePreview struct {
	model   finalPass
	running *atomic.Int64
	config  redecodeConfig
}

func (p *redecodePreview) retain()  { retainPart(p.model) }
func (p *redecodePreview) release() { releasePart(p.model) }

func (p *redecodePreview) start() (previewStream, error) {
	// The stream decodes with the model until it ends.
	retainPart(p.model)
	stream := &redecodeStream{running: p.running, model: p.model}
	decoder := decoderFunc(func(samples []float32) (string, error) {
		// Parakeet detects the language itself, and the preview never uses the dictionary.
		t, err := p.model.transcribe(samples, "auto", nil, false)
		return t.text, err
	})
	stream.session = startRedecodeSession(p.config, decoder,
		func() bool { return p.running.Load() > 0 },
		func(committed, tentative string) {
			stream.mu.Lock()
			stream.committed, stream.tentative = committed, tentative
			stream.mu.Unlock()
		})
	return stream, nil
}

// redecodeStream is one stream. The worker decodes; decode only reads its last text.
type redecodeStream struct {
	session              *redecodeSession
	mu                   sync.Mutex
	committed, tentative string
	// reported is the committed text that went out as ended segments.
	reported string
	running  *atomic.Int64
	model    finalPass
}

func (s *redecodeStream) unreported(committed string) string {
	rest, ok := strings.CutPrefix(committed, s.reported)
	if !ok {
		rest = committed
	}
	return strings.TrimSpace(rest)
}

func (s *redecodeStream) accept(samples []float32) {
	if s.session != nil {
		s.session.push(samples)
	}
}

// decode sends newly committed text as an ended segment first. The engine then
// asks again at once and gets the tentative text.
func (s *redecodeStream) decode() segment {
	s.mu.Lock()
	committed, tentative := s.committed, s.tentative
	s.mu.Unlock()
	if fresh := s.unreported(committed); fresh != "" {
		s.reported = committed
		return segment{text: fresh, endpoint: true}
	}
	return segment{text: tentative}
}

func (s *redecodeStream) finish() segment {
	if s.session == nil {
		return segment{endpoint: true}
	}
	session := s.session
	s.session = nil
	// A final pass that runs now has the processor; the preview text it has stays.
	text := session.finish(s.running.Load() > 0)
	releasePart(s.model)
	rest := s.unreported(text)
	s.reported = text
	return segment{text: rest, endpoint: true}
}

func (s *redecodeStream) close() {
	if s.session != nil {
		s.session.stopWorker()
		s.session = nil
		releasePart(s.model)
	}
}

// redecoder turns samples into text.
type redecoder interface {
	decode(samples []float32) (string, error)
}

type decoderFunc func([]float32) (string, error)

func (f decoderFunc) decode(samples []float32) (string, error) { return f(samples) }

type redecodeConfig struct {
	// interval is the time between two decodes.
	interval time.Duration
	// minNew is the new audio needed for another decode, in samples.
	minNew int
	// commitAfter is the uncommitted audio that starts a commit, in samples.
	commitAfter int
	// keep is the audio that always stays uncommitted after a commit.
	keep int
	// search is the length of the region before keep where the cut can fall.
	search int
	// minDecode: shorter audio is not decoded (its text is empty).
	minDecode int
}

func defaultRedecodeConfig() redecodeConfig {
	return redecodeConfig{
		interval:    600 * time.Millisecond,
		minNew:      msToSamples(300),
		commitAfter: msToSamples(10_000),
		keep:        msToSamples(2_000),
		search:      msToSamples(5_000),
		minDecode:   msToSamples(200),
	}
}

// redecodeJob is one decode the preview needs: a commit of the audio before
// cut, or a tail decode of all uncommitted audio (end samples).
type redecodeJob struct {
	commit  bool
	cut     int
	end     int
	samples []float32
}

// redecodeState is the text state of one preview, without threads.
type redecodeState struct {
	config               redecodeConfig
	audio                []float32
	committed, tentative string
	decoded              int
	stale                bool
	sent                 *[2]string
}

func newRedecodeState(c redecodeConfig) *redecodeState { return &redecodeState{config: c} }

func (p *redecodeState) push(samples []float32) { p.audio = append(p.audio, samples...) }

// nextJob is the next decode, if one is due. finishing asks for a decode of
// any new audio, however little.
func (p *redecodeState) nextJob(finishing bool) *redecodeJob {
	n := len(p.audio)
	c := p.config
	if n > c.commitAfter {
		end := max(n-c.keep, 0)
		cut := max(quietestPoint(p.audio, max(end-c.search, 0), end), 1)
		return &redecodeJob{commit: true, cut: cut, samples: append([]float32(nil), p.audio[:cut]...)}
	}
	fresh := max(n-p.decoded, 0)
	if p.stale || fresh >= c.minNew || (finishing && fresh > 0) {
		return &redecodeJob{end: n, samples: append([]float32(nil), p.audio...)}
	}
	return nil
}

// apply takes the text of a job and returns the partial to send, if it changed.
func (p *redecodeState) apply(job *redecodeJob, text string) (committed, tentative string, ok bool) {
	if job.commit {
		p.committed = joinTexts([]string{p.committed, text})
		p.audio = append(p.audio[:0], p.audio[job.cut:]...)
		p.decoded = 0
		p.stale = true
		return "", "", false
	}
	p.tentative = strings.TrimSpace(text)
	p.decoded = job.end
	p.stale = false
	pair := [2]string{p.committed, p.tentative}
	if p.sent != nil && *p.sent == pair {
		return "", "", false
	}
	p.sent = &pair
	return pair[0], pair[1], true
}

// skip is a failed decode: keep the old text and wait for new audio.
func (p *redecodeState) skip(job *redecodeJob) {
	if !job.commit {
		p.decoded = job.end
	}
}

func (p *redecodeState) text() string { return joinTexts([]string{p.committed, p.tentative}) }

func runRedecodeJob(d redecoder, job *redecodeJob, minDecode int) (text string, err error) {
	if len(job.samples) < minDecode {
		return "", nil
	}
	defer catchPanic("the decoder", &err)
	return d.decode(job.samples)
}

// driveRedecode decodes every due job (a commit is followed by a tail decode
// at once) and returns the partials to send.
func driveRedecode(mu *sync.Mutex, state *redecodeState, d redecoder, finishing bool, stop func() bool) [][2]string {
	mu.Lock()
	minDecode := state.config.minDecode
	mu.Unlock()
	var out [][2]string
	// At most a commit and a tail per round, plus slack for a commit that
	// leaves more than commitAfter behind.
	for range 4 {
		if stop() {
			break
		}
		mu.Lock()
		job := state.nextJob(finishing)
		mu.Unlock()
		if job == nil {
			break
		}
		text, err := runRedecodeJob(d, job, minDecode)
		mu.Lock()
		if err != nil {
			fmt.Fprintf(os.Stderr, "sayso-engine: a live preview decode failed: %v\n", err)
			state.skip(job)
			mu.Unlock()
			break
		}
		if c, t, ok := state.apply(job, text); ok {
			out = append(out, [2]string{c, t})
		}
		mu.Unlock()
		if !job.commit {
			break
		}
	}
	return out
}

// redecodeSession is a running preview: the audio and its worker goroutine.
type redecodeSession struct {
	mu      sync.Mutex
	state   *redecodeState
	decoder redecoder
	stopMu  sync.Mutex
	stopped bool
	wake    chan struct{}
	done    chan struct{}
}

// startRedecodeSession starts the worker. paused is asked before each decode;
// while it is true no decode starts. onPartial gets the text when it changed.
func startRedecodeSession(config redecodeConfig, d redecoder, paused func() bool, onPartial func(string, string)) *redecodeSession {
	s := &redecodeSession{state: newRedecodeState(config), decoder: d, wake: make(chan struct{}), done: make(chan struct{})}
	go func() {
		defer close(s.done)
		interval := config.interval
		next := time.Now().Add(interval)
		for {
			if wait := time.Until(next); wait > 0 {
				timer := time.NewTimer(wait)
				select {
				case <-s.wake:
					timer.Stop()
				case <-timer.C:
				}
			}
			if s.isStopped() {
				return
			}
			started := time.Now()
			next = started.Add(interval)
			if paused() {
				continue
			}
			stop := func() bool { return s.isStopped() || paused() }
			for _, p := range driveRedecode(&s.mu, s.state, s.decoder, false, stop) {
				onPartial(p[0], p[1])
			}
			// A decode longer than the interval is followed by a rest of half its
			// time, so the preview never takes more than two thirds of the time.
			took := time.Since(started)
			if rest := time.Now().Add(took / 2); rest.After(next) {
				next = rest
			}
		}
	}()
	return s
}

func (s *redecodeSession) isStopped() bool {
	s.stopMu.Lock()
	defer s.stopMu.Unlock()
	return s.stopped
}

func (s *redecodeSession) push(samples []float32) {
	s.mu.Lock()
	s.state.push(samples)
	s.mu.Unlock()
}

// stopWorker ends the worker after the decode that runs now, if any.
func (s *redecodeSession) stopWorker() {
	s.stopMu.Lock()
	if !s.stopped {
		s.stopped = true
		close(s.wake)
	}
	s.stopMu.Unlock()
	<-s.done
}

// finish stops the worker, then decodes the new audio one last time unless
// skipDecode. It returns the full text.
func (s *redecodeSession) finish(skipDecode bool) string {
	s.stopWorker()
	if !skipDecode {
		driveRedecode(&s.mu, s.state, s.decoder, true, func() bool { return false })
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.state.text()
}
