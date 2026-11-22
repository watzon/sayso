package localengine

import (
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
)

// ---------------------------------------------------------------------------
// Fake recognizer
// ---------------------------------------------------------------------------

// fakeLoader loads the fake models. It checks that the folders exist.
type fakeLoader struct {
	loads, finalLoads atomic.Int64
	fail              atomic.Bool
	mu                sync.Mutex
	diarized          []uint32
}

func (l *fakeLoader) loadFinalPass(spec *modelSpec, _ *modelDirs) (finalPass, error) {
	l.finalLoads.Add(1)
	if spec.recipe.finalPass() {
		return fakeFinal{}, nil
	}
	return nil, nil
}

func (l *fakeLoader) load(spec *modelSpec, dirs *modelDirs) (*loaded, error) {
	l.loads.Add(1)
	if l.fail.Load() {
		return nil, errors.New("a broken model")
	}
	if !isDir(dirs.main) {
		return nil, fmt.Errorf("missing %s", dirs.main)
	}
	switch spec.recipe.task() {
	case taskVad:
		return &loaded{vad: fakeVad{}}, nil
	case taskDiarization:
		return &loaded{diarizer: &fakeDiarizer{l}}, nil
	}
	out := &loaded{}
	if spec.recipe.finalPass() {
		out.finalPass = fakeFinal{}
	}
	if dirs.stream != "" {
		out.preview = fakePreview{}
	}
	return out, nil
}

type fakeVad struct{}

func (fakeVad) detector() (detector, error) { return &energyDetector{}, nil }

// fakeDiarizer: two speakers numbered 9 and 4 in its own order; the turns of 9
// overlap and are not in order. With one speaker, one turn for the whole audio.
type fakeDiarizer struct{ l *fakeLoader }

func (d *fakeDiarizer) diarize(samples []float32, speakers uint32) ([]rawTurn, error) {
	d.l.mu.Lock()
	d.l.diarized = append(d.l.diarized, speakers)
	d.l.mu.Unlock()
	if speakers == 1 {
		return []rawTurn{{0, float32(len(samples)) / 16_000, 7}}, nil
	}
	return []rawTurn{{2, 3, 4}, {0, 1, 9}, {0.5, 1.5, 9}, {2.5, 3.5, 9}}, nil
}

// fakeFinal says how many samples it heard and in which language. It finds
// German in audio above zero, English below zero, and Cantonese in loud audio
// (a mean above 0.05). Silence has no text.
type fakeFinal struct{}

func (fakeFinal) transcribe(samples []float32, language string, vocabulary []string, _ bool) (timed, error) {
	var sum float32
	silent := true
	for _, s := range samples {
		sum += s
		silent = silent && s == 0
	}
	var found string
	switch {
	case sum > 0.05*float32(len(samples)):
		found = "yue"
	case sum > 0:
		found = "de"
	case sum < 0:
		found = "en"
	}
	if silent {
		return timed{language: found}, nil
	}
	text := fmt.Sprintf("%d samples in %s", len(samples), language)
	if len(vocabulary) > 0 {
		text += " with " + strings.Join(vocabulary, "+")
	}
	return timed{text: text, language: found}, nil
}

// fakePreview gives one word per loud chunk. A silent chunk ends the segment.
type fakePreview struct{}

func (fakePreview) start() (previewStream, error) { return &fakeStream{}, nil }

type fakeStream struct {
	words  []string
	silent bool
}

func (s *fakeStream) accept(samples []float32) {
	s.silent = true
	for _, x := range samples {
		if x != 0 {
			s.silent = false
		}
	}
	if !s.silent {
		s.words = append(s.words, fmt.Sprintf("w%d", len(s.words)))
	}
}

func (s *fakeStream) decode() segment {
	seg := segment{text: strings.Join(s.words, " "), endpoint: s.silent && len(s.words) > 0}
	if seg.endpoint {
		s.words = nil
	}
	return seg
}

func (s *fakeStream) finish() segment {
	text := strings.Join(s.words, " ")
	s.words = nil
	return segment{text: text, endpoint: true}
}

func (s *fakeStream) close() {}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

type harness struct {
	t      *testing.T
	engine *Engine
	out    *captured
	loader *fakeLoader
	worker *streamWorker
}

func newHarness(t *testing.T, baseURL string) *harness {
	t.Helper()
	dir := t.TempDir()
	c, err := configFromEnv(filepath.Join(dir, "models"))
	if err != nil {
		t.Fatal(err)
	}
	c.baseURL, c.hfEndpoint, c.decodeInterval = baseURL, baseURL, 0
	out := &captured{}
	l := &fakeLoader{}
	e, err := newEngine(c, NewOutput(out), l)
	if err != nil {
		t.Fatal(err)
	}
	return &harness{t: t, engine: e, out: out, loader: l, worker: &streamWorker{engine: e, sessions: map[uint64]*session{}}}
}

func jsonLine(t testing.TB, v any) string {
	b, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

type obj = map[string]any

// call runs one request and returns the messages it wrote.
func (h *harness) call(req obj) []map[string]any {
	h.t.Helper()
	h.out.clear()
	r, err := ParseRequest(jsonLine(h.t, req))
	if err != nil {
		h.t.Fatal(err)
	}
	h.engine.handle(r)
	return h.out.messages(h.t)
}

func (h *harness) stream(req obj) []map[string]any {
	h.t.Helper()
	h.out.clear()
	r, err := ParseRequest(jsonLine(h.t, req))
	if err != nil {
		h.t.Fatal(err)
	}
	h.worker.handle(r)
	return h.out.messages(h.t)
}

func (h *harness) modelsDir() string { return h.engine.config.modelsDir }

// install puts a model on disk as a finished download.
func (h *harness) install(model string, engine obj) {
	h.t.Helper()
	spec, err := parseSpec(json.RawMessage(jsonLine(h.t, engine)))
	if err != nil {
		h.t.Fatal(err)
	}
	for _, a := range spec.archives() {
		p, err := h.engine.folders.archiveDir(model, a)
		mkdir(h.t, p, err)
	}
	if err := h.engine.folders.markDownloaded(model, spec); err != nil {
		h.t.Fatal(err)
	}
}

func (h *harness) loadModel(model string, engine obj) {
	h.t.Helper()
	h.install(model, engine)
	reply := h.call(obj{"v": 1, "type": "load", "id": "load", "model": model, "engine": engine})
	if last(reply)["type"] != "ok" {
		h.t.Fatalf("%v", reply)
	}
}

func (h *harness) wav(name string, samples []int16) string {
	h.t.Helper()
	if err := os.MkdirAll(h.modelsDir(), 0o755); err != nil {
		h.t.Fatal(err)
	}
	p := filepath.Join(h.modelsDir(), name)
	if err := os.WriteFile(p, testWAV(1, 16_000, samples), 0o644); err != nil {
		h.t.Fatal(err)
	}
	return p
}

func specJSON(recipe, archive string, preview any) obj {
	return obj{"kind": "sherpa", "recipe": recipe, "archive": archive, "preview_archive": preview}
}

func states(messages []map[string]any) []string {
	var out []string
	for _, m := range messages {
		if m["type"] == "model_state" {
			out = append(out, m["state"].(string))
		}
	}
	return out
}

func last(messages []map[string]any) map[string]any {
	if len(messages) == 0 {
		return map[string]any{}
	}
	return messages[len(messages)-1]
}

func pcm(samples []int16) string {
	var b []byte
	for _, s := range samples {
		b = binary.LittleEndian.AppendUint16(b, uint16(s))
	}
	return base64.StdEncoding.EncodeToString(b)
}

func repeat(v int16, n int) []int16 {
	s := make([]int16, n)
	for i := range s {
		s[i] = v
	}
	return s
}

func clip(pieces ...audioPiece) []int16 {
	var out []int16
	for _, s := range audioOf(pieces...) {
		out = append(out, int16(s*32767))
	}
	return out
}

func silero() obj { return obj{"kind": "sherpa_vad", "recipe": "silero", "file": "silero_vad.onnx"} }

func diarizationEngine() obj {
	return obj{"kind": "sherpa_diarization", "segmentation": "seg-archive", "embedding": "speaker.onnx"}
}

func asJSON(t testing.TB, v any) string { return jsonLine(t, v) }

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

func TestHelloAndUnknownRequests(t *testing.T) {
	h := newHarness(t, "http://unused")
	r := h.call(obj{"v": 1, "type": "hello", "id": "r1"})
	if r[0]["type"] != "hello" || r[0]["id"] != "r1" || r[0]["protocol"] != 1.0 ||
		asJSON(t, r[0]["features"]) != `["catalog","concurrent_streams","detected_language","diarization","keep_preview","segments","store_v1","unload","vad"]` {
		t.Fatalf("%v", r)
	}
	if r := h.call(obj{"v": 1, "type": "prepare", "id": "p", "model": "m", "preview": true, "vocabulary": []string{"Sayso"}}); r[0]["type"] != "ok" {
		t.Fatalf("%v", r)
	}
	if r := h.call(obj{"v": 1, "type": "fly", "id": "r2"}); asJSON(t, r[0]) != `{"id":"r2","message":"unknown request type fly","type":"error","v":1}` {
		t.Fatalf("%v", r)
	}
	if r := h.call(obj{"v": 1, "type": "load", "id": "r3"}); r[0]["message"] != "missing string field model" {
		t.Fatalf("%v", r)
	}
	if r := h.call(obj{"v": 1, "type": "load", "id": "r4", "model": "m"}); r[0]["code"] != "unknown_model" {
		t.Fatalf("%v", r)
	}
}

func TestListModelsWithoutAListGivesTheCatalog(t *testing.T) {
	h := newHarness(t, "http://unused")
	r := h.call(obj{"v": 1, "type": "list_models", "id": "l"})
	reply := last(r)
	models, ok := reply["models"].([]any)
	if reply["type"] != "ok" || !ok || len(models) != len(h.engine.catalog.models) || len(states(r)) != len(models) {
		t.Fatalf("%v", reply)
	}
}

func TestAModelLoadsTranscribesAndIsDeleted(t *testing.T) {
	h := newHarness(t, "http://unused")
	engine := specJSON("parakeet_tdt", "main", "stream")
	list := obj{"v": 1, "type": "list_models", "id": "l", "models": []obj{{"id": "m", "engine": engine}}}
	if s := states(h.call(list)); !reflect.DeepEqual(s, []string{"not_downloaded"}) {
		t.Fatalf("%v", s)
	}
	h.install("m", engine)
	if s := states(h.call(list)); !reflect.DeepEqual(s, []string{"downloaded"}) {
		t.Fatalf("%v", s)
	}
	transcribe := obj{"v": 1, "type": "transcribe", "id": "t1", "model": "m", "path": h.wav("clip.wav", repeat(1000, 16_000)), "language": "en", "vocabulary": []string{"Sayso"}}
	if r := h.call(transcribe); r[0]["message"] != "model m is not loaded" {
		t.Fatalf("%v", r)
	}
	r := h.call(obj{"v": 1, "type": "load", "id": "o1", "model": "m", "engine": engine})
	if !reflect.DeepEqual(states(r), []string{"optimizing", "ready"}) || last(r)["type"] != "ok" {
		t.Fatalf("%v", r)
	}
	if _, ok := r[1]["load_ms"].(float64); !ok {
		t.Fatalf("no load_ms: %v", r)
	}
	r = h.call(obj{"v": 1, "type": "load", "id": "o2", "model": "m", "engine": engine})
	if !reflect.DeepEqual(states(r), []string{"ready"}) || r[0]["load_ms"] != 0.0 || h.loader.loads.Load() != 1 {
		t.Fatalf("%v", r)
	}
	if s := states(h.call(list)); !reflect.DeepEqual(s, []string{"ready"}) {
		t.Fatalf("%v", s)
	}
	r = h.call(transcribe)
	if len(r) != 1 || r[0]["type"] != "final" || r[0]["id"] != "t1" || r[0]["text"] != "16000 samples in en with Sayso" {
		t.Fatalf("the dictionary words reach the model: %v", r)
	}
	if _, ok := r[0]["elapsed_ms"].(float64); !ok {
		t.Fatalf("no elapsed_ms: %v", r)
	}
	r = h.call(obj{"v": 1, "type": "delete", "id": "x1", "model": "m", "engine": engine})
	if !reflect.DeepEqual(states(r), []string{"not_downloaded"}) || last(r)["type"] != "ok" || isDir(filepath.Join(h.modelsDir(), "m")) {
		t.Fatalf("%v", r)
	}
}

func TestUnloadTakesAModelOutOfMemoryAndKeepsTheFiles(t *testing.T) {
	h := newHarness(t, "http://unused")
	whisper, streaming := specJSON("whisper", "w", nil), specJSON("zipformer_streaming", "stream", nil)
	h.loadModel("w", whisper)
	h.loadModel("s", streaming)
	list := obj{"v": 1, "type": "list_models", "id": "l1", "models": []obj{{"id": "w", "engine": whisper}, {"id": "s", "engine": streaming}}}
	r := h.call(obj{"v": 1, "type": "unload", "id": "u1", "model": "w"})
	if asJSON(t, r) != `[{"id":"u1","model":"w","state":"downloaded","type":"model_state","v":1},{"id":"u1","type":"ok","v":1}]` {
		t.Fatalf("%v", r)
	}
	if s := states(h.call(list)); !reflect.DeepEqual(s, []string{"downloaded", "ready"}) {
		t.Fatalf("%v", s)
	}
	transcribe := obj{"v": 1, "type": "transcribe", "id": "t1", "model": "w", "path": h.wav("clip.wav", repeat(1000, 1600)), "language": "en"}
	if r := h.call(transcribe); r[0]["message"] != "model w is not loaded" {
		t.Fatalf("%v", r)
	}
	h.call(obj{"v": 1, "type": "load", "id": "o3", "model": "w", "engine": whisper})
	if h.loader.loads.Load() != 3 || h.call(transcribe)[0]["type"] != "final" {
		t.Fatal("reload")
	}
	h.call(obj{"v": 1, "type": "unload", "id": "u2", "model": "w"})
	if r := h.call(obj{"v": 1, "type": "unload", "id": "u3", "model": "w"}); asJSON(t, r) != `[{"id":"u3","type":"ok","v":1}]` {
		t.Fatalf("%v", r)
	}
	if r := h.call(obj{"v": 1, "type": "unload", "id": "u4"}); r[0]["message"] != "missing string field model" {
		t.Fatalf("%v", r)
	}
}

func TestStreamsSendPartialsAndEndWithTheText(t *testing.T) {
	h := newHarness(t, "http://unused")
	h.loadModel("s", specJSON("zipformer_streaming", "stream", nil))
	if r := h.stream(obj{"v": 1, "type": "stream_start", "id": "s1", "session": 7, "model": "s", "language": "en"}); asJSON(t, r) != `[{"id":"s1","type":"ok","v":1}]` {
		t.Fatalf("%v", r)
	}
	loud, silent := pcm(repeat(1000, 320)), pcm(repeat(0, 320))
	audio := func(p string) obj { return obj{"v": 1, "type": "stream_audio", "session": 7, "pcm": p} }
	if r := h.stream(audio(loud)); asJSON(t, r) != `[{"committed":"","session":7,"tentative":"w0","type":"partial","v":1}]` {
		t.Fatalf("%v", r)
	}
	if r := h.stream(audio(loud)); r[0]["tentative"] != "w0 w1" {
		t.Fatalf("%v", r)
	}
	if r := h.stream(audio(silent)); r[0]["committed"] != "w0 w1" || r[0]["tentative"] != "" {
		t.Fatalf("%v", r)
	}
	if r := h.stream(audio(silent)); len(r) != 0 {
		t.Fatalf("no change, no partial: %v", r)
	}
	h.stream(audio(loud))
	r := h.stream(obj{"v": 1, "type": "stream_end", "id": "e1", "session": 7})
	if r[0]["type"] != "partial" || r[0]["committed"] != "w0 w1 w0" || asJSON(t, r[1]) != `{"id":"e1","session":7,"text":"w0 w1 w0","type":"ok","v":1}` {
		t.Fatalf("%v", r)
	}
	if r := h.stream(audio(loud)); r[0]["message"] != "unknown stream session 7" {
		t.Fatalf("%v", r)
	}
	if r := h.call(obj{"v": 1, "type": "transcribe", "id": "t1", "model": "s", "path": h.wav("clip.wav", repeat(1000, 160))}); r[0]["message"] != "model s has no final pass" {
		t.Fatalf("%v", r)
	}
}

func TestAStreamPastTheLimitGetsTooManyStreams(t *testing.T) {
	h := newHarness(t, "http://unused")
	h.loadModel("s", specJSON("zipformer_streaming", "stream", nil))
	start := func(s int) obj { return obj{"v": 1, "type": "stream_start", "id": "s", "session": s, "model": "s"} }
	for s := range maxStreams {
		if r := h.stream(start(s)); r[0]["type"] != "ok" {
			t.Fatalf("%v", r)
		}
	}
	if r := h.stream(start(99)); r[0]["type"] != "error" || r[0]["code"] != "too_many_streams" {
		t.Fatalf("%v", r)
	}
	if r := h.stream(start(0)); r[0]["type"] != "ok" {
		t.Fatalf("the same session id replaces its stream: %v", r)
	}
}

func TestStreamStartNeedsALoadedModelWithALivePreview(t *testing.T) {
	h := newHarness(t, "http://unused")
	batch := specJSON("sense_voice", "batch", nil)
	h.install("b", batch)
	start := obj{"v": 1, "type": "stream_start", "id": "s1", "session": 1, "model": "b", "language": "en"}
	if r := h.stream(start); r[0]["message"] != "model b is not loaded" {
		t.Fatalf("%v", r)
	}
	h.call(obj{"v": 1, "type": "load", "id": "o1", "model": "b", "engine": batch})
	if r := h.stream(start); r[0]["message"] != "model b has no live preview" {
		t.Fatalf("%v", r)
	}
}

func TestLongRecordingsAreCutIntoPiecesWithSegments(t *testing.T) {
	h := newHarness(t, "http://unused")
	h.loadModel("w", specJSON("whisper", "w", nil))
	wav := h.wav("long.wav", repeat(1000, 16_000*40))
	r := h.call(obj{"v": 1, "type": "transcribe", "id": "t1", "model": "w", "path": wav, "language": "en", "segments": true})
	segs, _ := r[0]["segments"].([]any)
	if len(segs) != 2 || strings.Count(r[0]["text"].(string), "samples in en") != 2 {
		t.Fatalf("%v", r)
	}
	s0, s1 := segs[0].(map[string]any), segs[1].(map[string]any)
	if s0["start_ms"] != 0.0 || s0["end_ms"] != s1["start_ms"] || s1["end_ms"] != 40_000.0 || s0["words"] != nil {
		t.Fatalf("%v", segs)
	}
	if r := h.call(obj{"v": 1, "type": "transcribe", "id": "t2", "model": "w", "path": wav, "language": "en"}); r[0]["segments"] != nil {
		t.Fatalf("only when asked: %v", r)
	}
}

func transcribeWhisper(t *testing.T, samples []int16, language string) map[string]any {
	h := newHarness(t, "http://unused")
	h.loadModel("w", specJSON("whisper", "w", nil))
	return h.call(obj{"v": 1, "type": "transcribe", "id": "t1", "model": "w", "path": h.wav("clip.wav", samples), "language": language})[0]
}

func TestAutoGivesTheLanguageOfTheMostAudio(t *testing.T) {
	german := append(append(repeat(1000, 16_000*24), repeat(0, 16_000)...), repeat(-1000, 16_000*30)...)
	r := transcribeWhisper(t, german, "auto")
	if strings.Count(r["text"].(string), "samples in auto") != 3 || r["language"] != "en" {
		t.Fatalf("30 s of English against 24 s of German: %v", r)
	}
	more := append(append(repeat(1000, 16_000*24), repeat(0, 16_000)...), repeat(-1000, 16_000*10)...)
	if r := transcribeWhisper(t, more, ""); r["language"] != "de" {
		t.Fatalf("an empty language is auto: %v", r)
	}
}

func TestANamedLanguageOrNoTextGivesNoLanguage(t *testing.T) {
	for _, named := range []string{"en", "de", "en-US"} {
		if r := transcribeWhisper(t, repeat(1000, 32_000), named); r["type"] != "final" || r["language"] != nil {
			t.Fatalf("%s: %v", named, r)
		}
	}
	if r := transcribeWhisper(t, repeat(0, 32_000), "auto"); r["text"] != "" || r["language"] != nil {
		t.Fatalf("%v", r)
	}
}

func TestServeRoutesRequestsAndStopsOnShutdown(t *testing.T) {
	h := newHarness(t, "http://unused")
	input := strings.Join([]string{
		`{"v":1,"type":"hello","id":"a"}`, "", "garbage",
		`{"v":1,"type":"stream_end","id":"b","session":4}`,
		`{"v":1,"type":"shutdown","id":"c"}`,
		`{"v":1,"type":"hello","id":"never"}`,
	}, "\n")
	h.engine.serve(strings.NewReader(input))
	byID := func(id string) map[string]any {
		for _, m := range h.out.messages(t) {
			if m["id"] == id {
				return m
			}
		}
		return nil
	}
	waitUntil(t, "the replies", func() bool { return byID("a") != nil && byID("b") != nil })
	if byID("a")["type"] != "hello" || byID("b")["message"] != "unknown stream session 4" || byID("c")["type"] != "ok" || byID("never") != nil {
		t.Fatalf("%v", h.out.messages(t))
	}
	found := false
	for _, m := range h.out.messages(t) {
		found = found || (m["message"] == "invalid JSON line" && m["id"] == nil)
	}
	if !found {
		t.Fatal("no error for the garbage line")
	}
}

func TestServeRunsUnloadBeforeTheLoadThatFollows(t *testing.T) {
	h := newHarness(t, "http://unused")
	whisper := specJSON("whisper", "w", nil)
	h.loadModel("w", whisper)
	h.out.clear()
	h.engine.serve(strings.NewReader(jsonLine(t, obj{"v": 1, "type": "unload", "id": "u", "model": "w"}) + "\n" +
		jsonLine(t, obj{"v": 1, "type": "load", "id": "o", "model": "w", "engine": whisper})))
	waitUntil(t, "the load", func() bool {
		for _, m := range h.out.messages(t) {
			if m["id"] == "o" && m["type"] == "ok" {
				return true
			}
		}
		return false
	})
	if s := states(h.out.messages(t)); !reflect.DeepEqual(s, []string{"downloaded", "optimizing", "ready"}) || h.loader.loads.Load() != 2 {
		t.Fatalf("%v", s)
	}
}

func TestTheVADRequestAndAVADSessionFindTheSameParts(t *testing.T) {
	h := newHarness(t, "http://unused")
	h.loadModel("v", silero())
	samples := clip(audioPiece{800, q}, audioPiece{2500, l}, audioPiece{1200, q}, audioPiece{600, l}, audioPiece{300, q}, audioPiece{900, l}, audioPiece{1500, q})
	r := h.call(obj{"v": 1, "type": "vad", "id": "v1", "model": "v", "path": h.wav("speech.wav", samples)})
	speech, _ := r[0]["speech"].([]any)
	if len(r) != 1 || len(speech) != 2 || r[0]["id"] != "v1" {
		t.Fatalf("the 300 ms pause does not end the second part: %v", r)
	}
	if r := h.stream(obj{"v": 1, "type": "vad_start", "id": "s1", "session": 9, "model": "v"}); asJSON(t, r) != `[{"id":"s1","type":"ok","v":1}]` {
		t.Fatalf("%v", r)
	}
	var events []map[string]any
	for rest := samples; len(rest) > 0; {
		n := min(2560, len(rest))
		events = append(events, h.stream(obj{"v": 1, "type": "stream_audio", "session": 9, "pcm": pcm(rest[:n])})...)
		rest = rest[n:]
	}
	end := h.stream(obj{"v": 1, "type": "stream_end", "id": "e1", "session": 9})
	if asJSON(t, last(end)) != `{"id":"e1","session":9,"type":"ok","v":1}` {
		t.Fatalf("%v", end)
	}
	events = append(events, end[:len(end)-1]...)
	var types []string
	var ends []map[string]any
	for _, e := range events {
		if e["id"] != nil || e["session"] != 9.0 {
			t.Fatalf("%v", e)
		}
		types = append(types, e["type"].(string))
		if e["type"] == "speech_end" {
			ends = append(ends, e)
		}
	}
	if !reflect.DeepEqual(types, []string{"speech_start", "speech_end", "speech_start", "speech_end"}) {
		t.Fatalf("%v", events)
	}
	for i, part := range speech {
		p := part.(map[string]any)
		if ends[i]["start_ms"] != p["start_ms"] || ends[i]["end_ms"] != p["end_ms"] || events[2*i]["start_ms"] != events[2*i+1]["start_ms"] {
			t.Fatalf("%v vs %v", speech, events)
		}
	}
}

func TestAVADRequestOnAudioWithoutSpeechGetsAnEmptyList(t *testing.T) {
	h := newHarness(t, "http://unused")
	h.loadModel("v", silero())
	if r := h.call(obj{"v": 1, "type": "vad", "id": "v1", "model": "v", "path": h.wav("quiet.wav", clip(audioPiece{2000, q}))}); asJSON(t, r[0]["speech"]) != `[]` {
		t.Fatalf("%v", r)
	}
	if r := h.call(obj{"v": 1, "type": "vad", "id": "v2", "model": "v", "path": filepath.Join(h.modelsDir(), "missing.wav")}); r[0]["type"] != "error" {
		t.Fatalf("%v", r)
	}
}

func TestDiarizeNumbersTheSpeakersAndJoinsTheirTurns(t *testing.T) {
	h := newHarness(t, "http://unused")
	h.loadModel("d", diarizationEngine())
	wav := h.wav("talk.wav", repeat(1000, 16_000*4))
	r := h.call(obj{"v": 1, "type": "diarize", "id": "z1", "model": "d", "path": wav})
	if len(r) != 1 || r[0]["id"] != "z1" || r[0]["type"] != "ok" || r[0]["speakers"] != 2.0 ||
		asJSON(t, r[0]["turns"]) != `[{"end_ms":1500,"speaker":0,"start_ms":0},{"end_ms":3000,"speaker":1,"start_ms":2000},{"end_ms":3500,"speaker":0,"start_ms":2500}]` {
		t.Fatalf("%v", r)
	}
	r = h.call(obj{"v": 1, "type": "diarize", "id": "z2", "model": "d", "path": wav, "speakers": 1})
	if r[0]["speakers"] != 1.0 || asJSON(t, r[0]["turns"]) != `[{"end_ms":4000,"speaker":0,"start_ms":0}]` {
		t.Fatalf("%v", r)
	}
	if r := h.call(obj{"v": 1, "type": "diarize", "id": "z3", "model": "d", "path": wav, "speakers": nil}); r[0]["type"] != "ok" {
		t.Fatalf("%v", r)
	}
	if !reflect.DeepEqual(h.loader.diarized, []uint32{0, 1, 0}) {
		t.Fatalf("the model gets the number of speakers: %v", h.loader.diarized)
	}
}

func TestDiarizeChecksSpeakersAndHandlesAudioWithoutSpeech(t *testing.T) {
	h := newHarness(t, "http://unused")
	h.loadModel("d", diarizationEngine())
	wav := h.wav("talk.wav", repeat(1000, 16_000))
	for _, bad := range []any{0, -1, 1.5, "2", true, []int{2}} {
		r := h.call(obj{"v": 1, "type": "diarize", "id": "z", "model": "d", "path": wav, "speakers": bad})
		if len(r) != 1 || r[0]["type"] != "error" || r[0]["message"] != "speakers must be an integer of 1 or more" {
			t.Fatalf("%v: %v", bad, r)
		}
	}
	if len(h.loader.diarized) != 0 {
		t.Fatal("a bad number reached the model")
	}
	if r := h.call(obj{"v": 1, "type": "diarize", "id": "z", "model": "d", "path": h.wav("empty.wav", nil)}); r[0]["speakers"] != 0.0 || asJSON(t, r[0]["turns"]) != `[]` {
		t.Fatalf("%v", r)
	}
}

func TestARedecodeModelGivesItsLivePreviewFromTheFinalPass(t *testing.T) {
	h := newHarness(t, "http://unused")
	engine := specJSON("parakeet_tdt", "main", nil)
	engine["redecode_preview"] = true
	h.loadModel("r", engine)
	if r := h.stream(obj{"v": 1, "type": "stream_start", "id": "s", "session": 1, "model": "r"}); r[0]["type"] != "ok" {
		t.Fatalf("%v", r)
	}
	h.stream(obj{"v": 1, "type": "stream_audio", "session": 1, "pcm": pcm(repeat(1000, 16_000))})
	r := h.stream(obj{"v": 1, "type": "stream_end", "id": "e", "session": 1})
	if text, _ := last(r)["text"].(string); !strings.HasSuffix(text, "samples in auto") {
		t.Fatalf("%v", r)
	}
	if r := h.call(obj{"v": 1, "type": "unload", "id": "u", "model": "r"}); last(r)["type"] != "ok" {
		t.Fatalf("%v", r)
	}
}

func TestAFailedLoadReportsFailed(t *testing.T) {
	h := newHarness(t, "http://unused")
	whisper := specJSON("whisper", "w", nil)
	h.install("w", whisper)
	h.loader.fail.Store(true)
	r := h.call(obj{"v": 1, "type": "load", "id": "o", "model": "w", "engine": whisper})
	if s := states(r); len(s) == 0 || s[len(s)-1] != "failed" || last(r)["type"] != "error" {
		t.Fatalf("%v", r)
	}
}

func TestAFailedDownloadReportsFailedOnceAndLeavesNothingComplete(t *testing.T) {
	srv := httptest.NewServer(http.NotFoundHandler())
	defer srv.Close()
	h := newHarness(t, srv.URL)
	engine := specJSON("whisper", "sherpa-onnx-missing", nil)
	r := h.call(obj{"v": 1, "type": "download", "id": "d1", "model": "m", "engine": engine})
	if !reflect.DeepEqual(states(r), []string{"downloading", "failed"}) {
		t.Fatalf("%v", r)
	}
	errorsSent := 0
	for _, m := range r {
		if m["state"] == "failed" && m["reason"] != "download_failed" {
			t.Fatalf("%v", m)
		}
		if m["type"] == "error" {
			errorsSent++
		}
	}
	if e := last(r); errorsSent != 1 || e["type"] != "error" || e["id"] != "d1" || !strings.Contains(e["message"].(string), "404") {
		t.Fatalf("one error reply with the status: %v", r)
	}
	if isFile(filepath.Join(h.modelsDir(), "m", ".complete")) {
		t.Fatal("marked complete")
	}
	if r := h.call(obj{"v": 1, "type": "load", "id": "o1", "model": "m", "engine": engine}); r[0]["message"] != "model m is not downloaded" {
		t.Fatalf("%v", r)
	}
}
