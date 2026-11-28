package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/watzon/sayso/sdk/go/audio"
)

const version = "0.18.0"

type role string

const (
	roleFinal       role = "final"
	roleStreaming   role = "streaming"
	roleVad         role = "vad"
	roleDiarization role = "diarization"
)

type catalogModel struct {
	ID         string          `json:"id"`
	Name       string          `json:"name"`
	Engine     json.RawMessage `json:"engine"`
	SizeBytes  uint64          `json:"size_bytes"`
	Languages  []string        `json:"languages"`
	Roles      []role          `json:"roles"`
	Timestamps string          `json:"timestamps"`
	Detects    bool            `json:"detects_language"`
	Recommend  bool            `json:"recommended"`
}

var catalog = []catalogModel{
	{ID: "fake-final", Name: "Fake final", Engine: json.RawMessage(`{"kind":"fake"}`), SizeBytes: 4096, Languages: []string{"en", "de"}, Roles: []role{roleFinal}, Timestamps: "word", Detects: true, Recommend: true},
	{ID: "fake-stream", Name: "Fake streaming", Engine: json.RawMessage(`{"kind":"fake"}`), SizeBytes: 4096, Languages: []string{"en"}, Roles: []role{roleFinal, roleStreaming}, Timestamps: "word"},
	{ID: "fake-vad", Name: "Fake VAD", Engine: json.RawMessage(`{"kind":"fake_vad"}`), SizeBytes: 1024, Languages: []string{}, Roles: []role{roleVad}, Timestamps: "none"},
	{ID: "fake-diar", Name: "Fake diarization", Engine: json.RawMessage(`{"kind":"fake_diarization"}`), SizeBytes: 2048, Languages: []string{}, Roles: []role{roleDiarization}, Timestamps: "none"},
}

var retired = map[string]string{"fake-old": "fake-old was taken out; use fake-final"}

var allFeatures = []string{"catalog", "concurrent_streams", "detected_language", "diarization", "keep_preview", "language_model", "prepare", "segments", "store_v1", "unload", "vad"}

type model struct {
	id      string
	spec    json.RawMessage
	roles   []role
	loaded  bool
	preview bool
}

type stream struct {
	model   string
	vad     bool
	samples []float32
	last    string
	speech  bool
	start   uint64
	sent    int
}

type engine struct {
	script      script
	modelsDir   string
	store       string
	transcripts map[string]string
	features    []string

	out   io.Writer
	outMu sync.Mutex

	requests int
	models   map[string]*model
	streams  map[uint64]*stream
}

func newEngine(s script, modelsDir, store, transcripts string, out io.Writer) (*engine, error) {
	if modelsDir == "" {
		return nil, fmt.Errorf("SAYSO_MODELS_DIR is not set")
	}
	if err := os.MkdirAll(modelsDir, 0o755); err != nil {
		return nil, err
	}
	e := &engine{script: s, modelsDir: modelsDir, out: out, models: map[string]*model{}, streams: map[uint64]*stream{}, features: allFeatures}
	if s.Features != nil {
		e.features = *s.Features
	}
	if slices.Contains(e.features, "store_v1") {
		e.store = store
	}
	if transcripts != "" {
		b, err := os.ReadFile(transcripts)
		if err != nil {
			return nil, err
		}
		if err := json.Unmarshal(b, &e.transcripts); err != nil {
			return nil, fmt.Errorf("SAYSO_FAKE_TRANSCRIPTS: %v", err)
		}
	}
	if e.store != "" {
		if err := e.storeDirs(); err != nil {
			return nil, err
		}
	}
	return e, nil
}

type msg map[string]any

func (e *engine) send(id string, typ string, fields msg) {
	m := msg{"v": 1, "type": typ}
	for k, v := range fields {
		m[k] = v
	}
	if id != "" {
		m["id"] = id
	}
	b, _ := json.Marshal(m)
	e.outMu.Lock()
	e.out.Write(append(b, '\n'))
	e.outMu.Unlock()
}

type replyError struct {
	message, code, reason string
}

func (r *replyError) Error() string { return r.message }

func errorf(format string, args ...any) *replyError {
	return &replyError{message: fmt.Sprintf(format, args...)}
}

type request struct {
	V           *int             `json:"v"`
	Type        *string          `json:"type"`
	ID          string           `json:"id"`
	Model       *string          `json:"model"`
	Engine      json.RawMessage  `json:"engine"`
	Models      *[]modelRef      `json:"models"`
	SizeBytes   uint64           `json:"size_bytes"`
	KeepPreview bool             `json:"keep_preview"`
	Preview     bool             `json:"preview"`
	Vocabulary  []string         `json:"vocabulary"`
	Path        *string          `json:"path"`
	Language    string           `json:"language"`
	Segments    bool             `json:"segments"`
	Session     *json.RawMessage `json:"session"`
	PCM         *string          `json:"pcm"`
	Speakers    *uint32          `json:"speakers"`
	Prompt      string           `json:"prompt"`
}

type modelRef struct {
	ID     string          `json:"id"`
	Engine json.RawMessage `json:"engine"`
}

func (e *engine) handleLine(line []byte) (done bool, code int) {
	line = bytes.TrimSpace(line)
	if len(line) == 0 {
		return false, 0
	}
	var req request
	if err := json.Unmarshal(line, &req); err != nil || req.Type == nil {
		e.send("", "error", msg{"message": "the line is not a JSON object with a string type"})
		return false, 0
	}
	e.requests++
	if e.script.CrashAt > 0 && e.requests >= e.script.CrashAt {
		fmt.Fprintf(os.Stderr, "sayso-fake-engine: crash at request %d (%s)\n", e.requests, *req.Type)
		return true, 1
	}
	if d := e.script.DelayMs[*req.Type]; d > 0 {
		time.Sleep(time.Duration(d) * time.Millisecond)
	}
	if f, ok := e.script.Fail[*req.Type]; ok {
		if (*req.Type == "load" || *req.Type == "download") && req.Model != nil {
			e.send(req.ID, "model_state", msg{"model": *req.Model, "state": "failed", "message": f.Message, "reason": f.Reason})
		}
		e.reply(req.ID, nil, &replyError{f.Message, f.Code, f.Reason})
		return false, 0
	}
	if *req.Type == "stream_audio" {
		if err := e.streamAudio(&req); err != nil {
			e.send("", "error", msg{"message": err.Error()})
		}
		return false, 0
	}
	if req.ID == "" {
		e.send("", "error", msg{"message": "a request needs a string id"})
		return false, 0
	}
	if *req.Type == "load" && req.Model != nil && *req.Model == "fake-crash" {
		fmt.Fprintln(os.Stderr, "sayso-fake-engine: fake-crash crashes at load")
		return true, 1
	}
	fields, err := e.handle(&req)
	if *req.Type == "hello" && err == nil {
		e.send(req.ID, "hello", fields)
		return false, 0
	}
	if *req.Type == "transcribe" && err == nil {
		e.send(req.ID, "final", fields)
		return false, 0
	}
	e.reply(req.ID, fields, err)
	return *req.Type == "shutdown", 0
}

func (e *engine) reply(id string, fields msg, err error) {
	if err != nil {
		r, ok := err.(*replyError)
		if !ok {
			r = &replyError{message: err.Error()}
		}
		m := msg{"message": r.message}
		if r.code != "" {
			m["code"] = r.code
		}
		if r.reason != "" {
			m["reason"] = r.reason
		}
		e.send(id, "error", m)
		return
	}
	e.send(id, "ok", fields)
}

func (e *engine) has(feature string) bool { return slices.Contains(e.features, feature) }

func (e *engine) need(feature string) error {
	if !e.has(feature) {
		return errorf("this engine has no %s", feature)
	}
	return nil
}

func (e *engine) handle(req *request) (msg, error) {
	switch *req.Type {
	case "hello":
		v := version
		if e.script.Version != "" {
			v = e.script.Version
		}
		out := msg{"version": v, "protocol": 1}
		if e.script.Features == nil || len(*e.script.Features) > 0 {
			out["features"] = e.features
		}
		return out, nil
	case "list_models":
		return e.listModels(req)
	case "download":
		m, err := e.model(req)
		if err != nil {
			return nil, err
		}
		return nil, e.download(req.ID, m, req.SizeBytes)
	case "delete":
		m, err := e.model(req)
		if err != nil {
			return nil, err
		}
		m.loaded, m.preview = false, false
		e.deleteFiles(m)
		e.send(req.ID, "model_state", msg{"model": m.id, "state": "not_downloaded"})
		return nil, nil
	case "load":
		m, err := e.model(req)
		if err != nil {
			return nil, err
		}
		return nil, e.load(req.ID, m)
	case "unload":
		if err := e.need("unload"); err != nil {
			return nil, err
		}
		if req.Model == nil {
			return nil, errorf("missing field `model`")
		}
		m := e.models[*req.Model]
		if m != nil && (m.loaded || m.preview) {
			keep := req.KeepPreview && e.has("keep_preview") && slices.Contains(m.roles, roleStreaming)
			m.loaded, m.preview = false, keep
			for id, s := range e.streams {
				if s.model == m.id && !keep {
					delete(e.streams, id)
				}
			}
			fields := msg{"model": m.id, "state": "downloaded"}
			if keep {
				fields["preview"] = true
			}
			e.send(req.ID, "model_state", fields)
		}
		return nil, nil
	case "prepare":
		if err := e.need("prepare"); err != nil {
			return nil, err
		}
		m, err := e.loadedModel(req, roleFinal)
		if err != nil {
			return nil, err
		}
		if req.Preview {
			e.send(req.ID, "part_state", msg{"model": m.id, "part": "preview", "state": "ready"})
		}
		if len(req.Vocabulary) > 0 {
			e.send(req.ID, "part_state", msg{"model": m.id, "part": "dictionary", "state": "ready"})
		}
		return nil, nil
	case "transcribe":
		return e.transcribe(req)
	case "stream_start":
		return e.streamStart(req, false)
	case "vad_start":
		if err := e.need("vad"); err != nil {
			return nil, err
		}
		return e.streamStart(req, true)
	case "stream_end":
		return e.streamEnd(req)
	case "vad":
		if err := e.need("vad"); err != nil {
			return nil, err
		}
		if _, err := e.loadedModel(req, roleVad); err != nil {
			return nil, err
		}
		samples, err := readPath(req)
		if err != nil {
			return nil, err
		}
		parts := audio.DetectSpeech(samples)
		if parts == nil {
			parts = []audio.Part{}
		}
		return msg{"speech": parts}, nil
	case "diarize":
		return e.diarize(req)
	case "language_model":
		if err := e.need("language_model"); err != nil {
			return nil, err
		}
		return msg{"available": true, "model": "fake-lm"}, nil
	case "generate":
		if err := e.need("language_model"); err != nil {
			return nil, err
		}
		return msg{"text": strings.TrimSpace(req.Prompt), "model": "fake-lm"}, nil
	case "shutdown":
		return nil, nil
	}
	return nil, errorf("unknown request type %s", *req.Type)
}

func rolesOf(spec json.RawMessage) []role {
	var head struct{ Kind string }
	json.Unmarshal(spec, &head)
	switch {
	case strings.Contains(head.Kind, "vad"):
		return []role{roleVad}
	case strings.Contains(head.Kind, "diarization"):
		return []role{roleDiarization}
	}
	return []role{roleFinal, roleStreaming}
}

func rolesOfName(id string) []role {
	switch {
	case strings.Contains(id, "vad"):
		return []role{roleVad}
	case strings.Contains(id, "diar") || strings.Contains(id, "pyannote") || strings.Contains(id, "speaker"):
		return []role{roleDiarization}
	}
	return []role{roleFinal, roleStreaming}
}

func findCatalog(id string) *catalogModel {
	for i := range catalog {
		if catalog[i].ID == id {
			return &catalog[i]
		}
	}
	return nil
}

// modelFor is the model of an id with an engine object, or of the catalog.
func (e *engine) modelFor(id string, spec json.RawMessage) (*model, error) {
	if m, ok := e.models[id]; ok {
		if len(spec) > 0 {
			m.spec = spec
		}
		return m, nil
	}
	m := &model{id: id, spec: spec}
	switch c := findCatalog(id); {
	case len(spec) > 0 && string(spec) != "null":
		m.roles = rolesOf(spec)
		if c != nil {
			m.roles = c.Roles
		}
	case c != nil && e.has("catalog"):
		m.spec, m.roles = c.Engine, c.Roles
	case e.script.AcceptAny && retired[id] == "":
		m.roles = rolesOfName(id)
	case retired[id] != "" && e.has("catalog"):
		return nil, &replyError{message: retired[id], code: "model_retired"}
	case e.has("catalog"):
		return nil, &replyError{message: "no model " + id + " in the catalog", code: "unknown_model"}
	default:
		return nil, errorf("the request for %s has no engine object", id)
	}
	e.models[id] = m
	return m, nil
}

func (e *engine) model(req *request) (*model, error) {
	if req.Model == nil {
		return nil, errorf("missing field `model`")
	}
	return e.modelFor(*req.Model, req.Engine)
}

func (e *engine) loadedModel(req *request, need role) (*model, error) {
	if req.Model == nil {
		return nil, errorf("missing field `model`")
	}
	m := e.models[*req.Model]
	if m == nil || !(m.loaded || (need == roleStreaming && m.preview)) {
		return nil, errorf("model %s is not loaded", *req.Model)
	}
	if !slices.Contains(m.roles, need) {
		return nil, errorf("model %s cannot do %s", m.id, need)
	}
	return m, nil
}

func (e *engine) stateOf(m *model) msg {
	switch {
	case m.loaded:
		return msg{"model": m.id, "state": "ready"}
	case m.preview:
		return msg{"model": m.id, "state": "downloaded", "preview": true}
	case e.onDisk(m):
		return msg{"model": m.id, "state": "downloaded"}
	}
	return msg{"model": m.id, "state": "not_downloaded"}
}

func (e *engine) listModels(req *request) (msg, error) {
	if req.Models != nil {
		for _, ref := range *req.Models {
			m, err := e.modelFor(ref.ID, ref.Engine)
			if err != nil {
				return nil, err
			}
			e.send(req.ID, "model_state", e.stateOf(m))
		}
		return nil, nil
	}
	if err := e.need("catalog"); err != nil {
		return nil, err
	}
	for _, c := range catalog {
		m, _ := e.modelFor(c.ID, nil)
		e.send(req.ID, "model_state", e.stateOf(m))
	}
	list := slices.Clone(catalog)
	if !e.has("detected_language") {
		for i := range list {
			list[i].Detects = false
		}
	}
	var gone []msg
	for id, text := range retired {
		gone = append(gone, msg{"id": id, "message": text})
	}
	return msg{"models": list, "retired": gone}, nil
}

func (e *engine) download(id string, m *model, size uint64) error {
	if e.onDisk(m) {
		e.send(id, "model_state", msg{"model": m.id, "state": "downloaded"})
		return nil
	}
	if size == 0 {
		size = 4096
		if c := findCatalog(m.id); c != nil {
			size = c.SizeBytes
		}
	}
	e.send(id, "model_state", msg{"model": m.id, "state": "downloading"})
	for _, f := range []float64{0.25, 0.5, 1.0} {
		done := uint64(f * float64(size))
		e.send(id, "download_progress", msg{"model": m.id, "fraction": f, "bytes_done": done, "bytes_total": size})
	}
	if err := e.writeFiles(m, size); err != nil {
		e.send(id, "model_state", msg{"model": m.id, "state": "failed", "message": err.Error(), "reason": "download_failed"})
		return &replyError{message: err.Error(), reason: "download_failed"}
	}
	e.send(id, "model_state", msg{"model": m.id, "state": "downloaded"})
	return nil
}

func (e *engine) load(id string, m *model) error {
	if !e.onDisk(m) {
		if e.store != "" {
			e.send(id, "model_state", msg{"model": m.id, "state": "not_downloaded"})
		}
		return errorf("model %s is not downloaded", m.id)
	}
	if m.id == "fake-fail" {
		e.send(id, "model_state", msg{"model": m.id, "state": "failed", "message": "fake-fail fails at load", "reason": "load_failed"})
		return &replyError{message: "fake-fail fails at load", reason: "load_failed"}
	}
	if m.loaded {
		e.send(id, "model_state", msg{"model": m.id, "state": "ready", "load_ms": 0})
		return nil
	}
	e.send(id, "model_state", msg{"model": m.id, "state": "optimizing"})
	m.loaded = true
	e.send(id, "model_state", msg{"model": m.id, "state": "ready", "load_ms": 7})
	return nil
}

func readPath(req *request) ([]float32, error) {
	if req.Path == nil {
		return nil, errorf("missing field `path`")
	}
	a, err := audio.ReadFile(*req.Path)
	if err != nil {
		return nil, errorf("%v", err)
	}
	return a.AllFloats(), nil
}

func (e *engine) transcribe(req *request) (msg, error) {
	m, err := e.loadedModel(req, roleFinal)
	if err != nil {
		return nil, err
	}
	samples, err := readPath(req)
	if err != nil {
		return nil, err
	}
	started := time.Now()
	text, segs := e.words(samples, *req.Path)
	out := msg{"text": text, "elapsed_ms": max(1, time.Since(started).Milliseconds())}
	if req.Segments && e.has("segments") {
		out["segments"] = segs
	}
	if text != "" && e.has("detected_language") && (req.Language == "" || req.Language == "auto") && m.id != "fake-stream" {
		out["language"] = "en"
	}
	return out, nil
}

var lexicon = []string{"alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliett", "kilo", "lima", "mike", "november", "oscar", "papa"}

type word struct {
	StartMs uint64 `json:"start_ms"`
	EndMs   uint64 `json:"end_ms"`
	Text    string `json:"text"`
}

type segment struct {
	StartMs uint64 `json:"start_ms"`
	EndMs   uint64 `json:"end_ms"`
	Text    string `json:"text"`
	Words   []word `json:"words"`
}

// words is the transcript of samples: a fixed transcript by the hash of the
// file, else one word for each 300 ms of speech, a segment for each part.
func (e *engine) words(samples []float32, path string) (string, []segment) {
	if path != "" && e.transcripts != nil {
		if b, err := os.ReadFile(path); err == nil {
			sum := sha256.Sum256(b)
			if text, ok := e.transcripts[hex.EncodeToString(sum[:])]; ok {
				ms := uint64(len(samples) / 16)
				return text, []segment{{StartMs: 0, EndMs: ms, Text: text, Words: []word{}}}
			}
		}
	}
	segs := []segment{}
	var all []string
	n := 0
	for _, p := range audio.DetectSpeech(samples) {
		count := max(1, int((p.EndMs-p.StartMs)/300))
		step := (p.EndMs - p.StartMs) / uint64(count)
		s := segment{StartMs: p.StartMs, EndMs: p.EndMs, Words: []word{}}
		var texts []string
		for i := 0; i < count; i++ {
			w := lexicon[n%len(lexicon)]
			n++
			start := p.StartMs + uint64(i)*step
			s.Words = append(s.Words, word{StartMs: start, EndMs: start + step, Text: w})
			texts = append(texts, w)
		}
		s.Text = strings.Join(texts, " ")
		all = append(all, texts...)
		segs = append(segs, s)
	}
	return strings.Join(all, " "), segs
}

func sessionOf(req *request) (uint64, error) {
	if req.Session == nil {
		return 0, errorf("missing field `session`")
	}
	var n uint64
	if err := json.Unmarshal(*req.Session, &n); err != nil {
		return 0, errorf("session: %v", err)
	}
	return n, nil
}

func (e *engine) maxStreams() int {
	switch {
	case e.script.MaxStreams > 0:
		return e.script.MaxStreams
	case e.has("concurrent_streams"):
		return 4
	}
	return 1
}

func (e *engine) streamStart(req *request, vad bool) (msg, error) {
	need := roleStreaming
	if vad {
		need = roleVad
	}
	m, err := e.loadedModel(req, need)
	if err != nil {
		return nil, err
	}
	session, err := sessionOf(req)
	if err != nil {
		return nil, err
	}
	if !e.has("concurrent_streams") {
		for id, s := range e.streams {
			if s.model == m.id {
				delete(e.streams, id)
			}
		}
	}
	if len(e.streams) >= e.maxStreams() {
		return nil, &replyError{message: "no free stream", code: "too_many_streams"}
	}
	e.streams[session] = &stream{model: m.id, vad: vad}
	return nil, nil
}

func (e *engine) streamAudio(req *request) error {
	session, err := sessionOf(req)
	if err != nil {
		return err
	}
	s := e.streams[session]
	if s == nil {
		return errorf("session %d is not open", session)
	}
	if req.PCM == nil {
		return errorf("missing field `pcm`")
	}
	b, err := base64.StdEncoding.DecodeString(*req.PCM)
	if err != nil || len(b)%2 != 0 {
		return errorf("pcm is not base64 of 16-bit samples")
	}
	before := len(s.samples)
	for i := 0; i+1 < len(b); i += 2 {
		s.samples = append(s.samples, float32(int16(binary.LittleEndian.Uint16(b[i:])))/32768)
	}
	if s.vad {
		e.vadEvents(session, s, false)
		return nil
	}
	const every = audio.SampleRate / 2
	if before/every != len(s.samples)/every {
		e.partial(session, s)
	}
	return nil
}

func (e *engine) partial(session uint64, s *stream) {
	text, _ := e.words(s.samples, "")
	if text == s.last {
		return
	}
	s.last = text
	committed, tentative := text, ""
	if i := strings.LastIndexByte(text, ' '); i >= 0 {
		committed, tentative = text[:i], text[i+1:]
	} else {
		committed, tentative = "", text
	}
	e.send("", "partial", msg{"session": session, "committed": committed, "tentative": tentative})
}

// vadEvents sends the speech_start and speech_end of the parts of the
// stream that it did not send yet.
func (e *engine) vadEvents(session uint64, s *stream, end bool) {
	v := audio.NewBasicVad()
	parts := v.Accept(s.samples)
	var open []audio.Part
	if v.Speaking() {
		open = v.Finish()
	}
	if end {
		parts = append(parts, open...)
		open = nil
	}
	for _, p := range parts[min(s.sent, len(parts)):] {
		if !s.speech || s.start != p.StartMs {
			e.send("", "speech_start", msg{"session": session, "start_ms": p.StartMs})
		}
		e.send("", "speech_end", msg{"session": session, "start_ms": p.StartMs, "end_ms": p.EndMs})
		s.speech = false
		s.sent++
	}
	if len(open) > 0 && !s.speech {
		s.speech, s.start = true, open[0].StartMs
		e.send("", "speech_start", msg{"session": session, "start_ms": open[0].StartMs})
	}
}

func (e *engine) streamEnd(req *request) (msg, error) {
	session, err := sessionOf(req)
	if err != nil {
		return nil, err
	}
	s := e.streams[session]
	if s == nil {
		return nil, errorf("session %d is not open", session)
	}
	delete(e.streams, session)
	if s.vad {
		e.vadEvents(session, s, true)
		return msg{"session": session}, nil
	}
	e.partial(session, s)
	text, _ := e.words(s.samples, "")
	return msg{"session": session, "text": text}, nil
}

func (e *engine) diarize(req *request) (msg, error) {
	if err := e.need("diarization"); err != nil {
		return nil, err
	}
	if _, err := e.loadedModel(req, roleDiarization); err != nil {
		return nil, err
	}
	samples, err := readPath(req)
	if err != nil {
		return nil, err
	}
	parts := audio.DetectSpeech(samples)
	n := uint32(2)
	if req.Speakers != nil {
		if *req.Speakers == 0 {
			return nil, errorf("speakers must be 1 or more")
		}
		n = *req.Speakers
	}
	turns := []msg{}
	seen := map[uint32]bool{}
	for i, p := range parts {
		speaker := uint32(i) % n
		seen[speaker] = true
		turns = append(turns, msg{"start_ms": p.StartMs, "end_ms": p.EndMs, "speaker": speaker})
	}
	return msg{"speakers": len(seen), "turns": turns}, nil
}
