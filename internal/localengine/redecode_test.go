package localengine

import (
	"fmt"
	"math"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

// Synthetic speech: word k is 400 ms of the constant level(k), then 100 ms of
// silence (every 5th gap is 300 ms).
func level(k int) float32 { return 0.1 + 0.001*float32(k) }

func speech(words int) []float32 {
	var s []float32
	for k := range words {
		s = append(s, filled(msToSamples(400), level(k))...)
		gap := msToSamples(100)
		if k%5 == 4 {
			gap = msToSamples(300)
		}
		s = append(s, make([]float32, gap)...)
	}
	return s
}

func expectedWords(words int) string {
	var w []string
	for k := range words {
		w = append(w, fmt.Sprintf("w%d", k))
	}
	return strings.Join(w, " ")
}

// wordsIn hears one word per run of non-zero samples longer than 50 ms.
func wordsIn(samples []float32) string {
	var words []string
	run, value := 0, float32(0)
	for _, s := range append(slices.Clone(samples), 0) {
		if s != 0 {
			run++
			value = s
			continue
		}
		if run > msToSamples(50) {
			words = append(words, fmt.Sprintf("w%d", int(math.Round(float64((value-0.1)/0.001)))))
		}
		run = 0
	}
	return strings.Join(words, " ")
}

// fakeDecoder records the length of every decode.
type fakeDecoder struct {
	mu      sync.Mutex
	lengths []int
	delay   time.Duration
}

func (d *fakeDecoder) decode(samples []float32) (string, error) {
	d.mu.Lock()
	d.lengths = append(d.lengths, len(samples))
	d.mu.Unlock()
	time.Sleep(d.delay)
	return wordsIn(samples), nil
}

func (d *fakeDecoder) count() int {
	d.mu.Lock()
	defer d.mu.Unlock()
	return len(d.lengths)
}

// fakeModel is the fake as a final pass. It counts its calls, can wait in
// each one, and counts its references like a native model.
type fakeModel struct {
	calls atomic.Int64
	delay time.Duration
	refs  atomic.Int64
	freed atomic.Int64
}

func newFakeModel(delay time.Duration) *fakeModel {
	m := &fakeModel{delay: delay}
	m.refs.Store(1)
	return m
}

func (m *fakeModel) transcribe(samples []float32, _ string, _ []string, _ bool) (timed, error) {
	if m.freed.Load() > 0 {
		panic("transcribe on a freed model")
	}
	m.calls.Add(1)
	time.Sleep(m.delay)
	return untimed(wordsIn(samples)), nil
}

func (m *fakeModel) retain() { m.refs.Add(1) }
func (m *fakeModel) release() {
	n := m.refs.Add(-1)
	if n == 0 {
		m.freed.Add(1)
	}
	if n < 0 {
		panic("released more often than retained")
	}
}

// runSync feeds the audio in 160 ms chunks and decodes whenever a job is due, without goroutines.
func runSync(samples []float32, c redecodeConfig, d redecoder) ([][2]string, *redecodeState) {
	var mu sync.Mutex
	state := newRedecodeState(c)
	never := func() bool { return false }
	var partials [][2]string
	for len(samples) > 0 {
		n := min(msToSamples(160), len(samples))
		state.push(samples[:n])
		samples = samples[n:]
		partials = append(partials, driveRedecode(&mu, state, d, false, never)...)
	}
	partials = append(partials, driveRedecode(&mu, state, d, true, never)...)
	return partials, state
}

func fastConfig() redecodeConfig {
	c := defaultRedecodeConfig()
	c.interval = 20 * time.Millisecond
	return c
}

func TestShortDictationPreviewsWithoutCommits(t *testing.T) {
	d := &fakeDecoder{}
	partials, state := runSync(speech(8), defaultRedecodeConfig(), d)
	if len(partials) < 5 {
		t.Fatalf("%v", partials)
	}
	for i, p := range partials {
		if p[0] != "" {
			t.Fatalf("nothing is committed in 4 s: %v", partials)
		}
		if i > 0 && partials[i-1] == p {
			t.Fatalf("a partial without a change: %v", partials)
		}
	}
	if state.text() != expectedWords(8) || partials[len(partials)-1][1] != expectedWords(8) {
		t.Fatalf("%q", state.text())
	}
	// The end of the stream decodes the last word.
	state.push(speech(9)[len(speech(8)):])
	var mu sync.Mutex
	driveRedecode(&mu, state, d, true, func() bool { return false })
	if state.text() != expectedWords(9) {
		t.Fatalf("%q", state.text())
	}
}

func TestLongDictationCommitsAtQuietPointsAndStaysBounded(t *testing.T) {
	const words = 150 // about 80 s
	c := defaultRedecodeConfig()
	d := &fakeDecoder{}
	partials, state := runSync(speech(words), c, d)
	if state.text() != expectedWords(words) {
		t.Fatalf("a word was lost or repeated: %q", state.text())
	}
	if state.committed == "" {
		t.Fatal("nothing committed")
	}
	if longest := slices.Max(d.lengths); longest > c.commitAfter+msToSamples(160) {
		t.Fatalf("decodes stay bounded: %d", longest)
	}
	if len(state.audio) > c.commitAfter {
		t.Fatalf("pending %d", len(state.audio))
	}
	last, mixed := "", false
	for _, p := range partials {
		if !strings.HasPrefix(p[0], last) || !strings.HasPrefix(expectedWords(words), p[0]) {
			t.Fatalf("%q -> %q", last, p[0])
		}
		last = p[0]
		mixed = mixed || (p[0] != "" && p[1] != "")
	}
	if !mixed {
		t.Fatal("no partial with both committed and tentative text")
	}
}

func TestTheCutFallsInSilence(t *testing.T) {
	samples := speech(60)
	state := newRedecodeState(defaultRedecodeConfig())
	state.push(samples[:msToSamples(11_000)])
	job := state.nextJob(false)
	if job == nil || !job.commit {
		t.Fatalf("a commit is due: %+v", job)
	}
	if samples[job.cut] != 0 || job.cut < msToSamples(4_000) || job.cut > msToSamples(9_000) {
		t.Fatalf("cut at %d", job.cut)
	}
}

func TestLittleNewAudioWaits(t *testing.T) {
	state := newRedecodeState(defaultRedecodeConfig())
	state.push(filled(msToSamples(200), 0.1))
	if state.nextJob(false) != nil {
		t.Fatal("a job for 200 ms")
	}
	if job := state.nextJob(true); job == nil || job.commit || job.end != msToSamples(200) {
		t.Fatalf("%+v", job)
	}
	// Too short to decode: empty text, no decoder call.
	d := &fakeDecoder{}
	text, err := runRedecodeJob(d, &redecodeJob{end: 10, samples: filled(10, 0.1)}, msToSamples(200))
	if err != nil || text != "" || d.count() != 0 {
		t.Fatalf("%q %v %d", text, err, d.count())
	}
}

func TestAPanickingDecoderIsAnError(t *testing.T) {
	panicky := decoderFunc(func([]float32) (string, error) { panic("boom") })
	if _, err := runRedecodeJob(panicky, &redecodeJob{end: msToSamples(500), samples: filled(msToSamples(500), 0.1)}, 0); err == nil {
		t.Fatal("no error")
	}
}

func TestASessionSendsPartialsAndFinishesWithTheFullText(t *testing.T) {
	samples := speech(10)
	var mu sync.Mutex
	var partials []string
	s := startRedecodeSession(fastConfig(), &fakeDecoder{delay: 2 * time.Millisecond}, func() bool { return false }, func(c, tt string) {
		mu.Lock()
		partials = append(partials, joinTexts([]string{c, tt}))
		mu.Unlock()
	})
	for len(samples) > 0 {
		n := min(msToSamples(160), len(samples))
		s.push(samples[:n])
		samples = samples[n:]
		time.Sleep(15 * time.Millisecond)
	}
	if got := s.finish(false); got != expectedWords(10) {
		t.Fatalf("%q", got)
	}
	mu.Lock()
	defer mu.Unlock()
	if len(partials) < 3 {
		t.Fatalf("%v", partials)
	}
}

func TestAPausedSessionDoesNotDecodeAndFinishCanSkipTheLastDecode(t *testing.T) {
	d := &fakeDecoder{}
	s := startRedecodeSession(fastConfig(), d, func() bool { return true }, func(string, string) {})
	s.push(speech(4))
	time.Sleep(100 * time.Millisecond)
	if d.count() != 0 {
		t.Fatal("decoded while paused")
	}
	if got := s.finish(true); got != "" || d.count() != 0 {
		t.Fatalf("%q %d", got, d.count())
	}
}

func TestFinishStopsTheWorkerPromptly(t *testing.T) {
	s := startRedecodeSession(fastConfig(), &fakeDecoder{delay: 50 * time.Millisecond}, func() bool { return false }, func(string, string) {})
	s.push(speech(6))
	time.Sleep(40 * time.Millisecond)
	started := time.Now()
	s.finish(true)
	if took := time.Since(started); took > 500*time.Millisecond {
		t.Fatalf("%v", took)
	}
}

// updateStream runs a stream as the stream worker does: after an ended
// segment it asks again at once.
func updateStream(s previewStream, p *partials) {
	for {
		seg := s.decode()
		p.update(seg)
		if !seg.endpoint {
			return
		}
	}
}

func TestTheStreamReportsCommitsAsEndedSegmentsAndKeepsEveryWord(t *testing.T) {
	model := newFakeModel(0)
	l := attachRedecode(model)
	if l.preview == nil || l.finalPass == nil {
		t.Fatal("parts missing")
	}
	preview := &redecodePreview{model: model, running: &atomic.Int64{}, config: fastConfig()}
	stream, err := preview.start()
	if err != nil {
		t.Fatal(err)
	}
	var p partials
	const words = 30 // about 16 s: at least one commit
	samples := speech(words)
	for len(samples) > 0 {
		n := min(msToSamples(160), len(samples))
		stream.accept(samples[:n])
		samples = samples[n:]
		time.Sleep(4 * time.Millisecond)
		updateStream(stream, &p)
	}
	seen := false
	waitUntil(t, "a commit", func() bool {
		updateStream(stream, &p)
		if c, _, ok := p.changed(); ok && c != "" {
			seen = true
		}
		return seen
	})
	p.update(stream.finish())
	if p.text() != expectedWords(words) {
		t.Fatalf("%q", p.text())
	}
	if model.calls.Load() <= 2 {
		t.Fatalf("%d calls", model.calls.Load())
	}
}

func TestARunningFinalPassPausesThePreview(t *testing.T) {
	model := newFakeModel(300 * time.Millisecond)
	l := attachRedecode(model)
	stream, err := l.preview.start()
	if err != nil {
		t.Fatal(err)
	}
	stream.accept(speech(4))
	result := make(chan string, 1)
	go func() {
		got, _ := l.finalPass.transcribe(speech(2), "en", nil, false)
		result <- got.text
	}()
	waitUntil(t, "the final pass starts", func() bool { return model.calls.Load() == 1 })
	// While the final pass runs no decode starts, and the end of the stream
	// skips its last decode.
	if seg := stream.finish(); seg != (segment{endpoint: true}) {
		t.Fatalf("%+v", seg)
	}
	if got := <-result; got != expectedWords(2) {
		t.Fatalf("%q", got)
	}
	if model.calls.Load() != 1 {
		t.Fatalf("only the final pass decoded: %d", model.calls.Load())
	}
}

// The final pass and the redecode preview both hold the model, so it is
// freed once, when the engine lets go of both parts and the last stream ends.
func TestARedecodeModelIsFreedOnceAfterItsLastHolder(t *testing.T) {
	model := newFakeModel(0)
	l := attachRedecode(model)
	stream, err := l.preview.start()
	if err != nil {
		t.Fatal(err)
	}
	freeLoaded(l)
	if model.freed.Load() != 0 {
		t.Fatal("freed while a stream still decodes with it")
	}
	stream.close()
	if model.freed.Load() != 1 || model.refs.Load() != 0 {
		t.Fatalf("freed %d times, refs %d", model.freed.Load(), model.refs.Load())
	}
}
