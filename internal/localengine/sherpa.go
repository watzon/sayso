package localengine

import (
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"os"
	"runtime"
	"strings"
	"sync"
)

const sherpaSampleRate = 16_000

// finalPassThreads: more than 8 rarely helps on the CPU.
func finalPassThreads() int32 { return int32(min(runtime.NumCPU(), 8)) }

// previewThreads: the live preview runs while the user speaks, so it stays small.
func previewThreads() int32 { return int32(min(runtime.NumCPU(), 2)) }

// sherpaLoader loads models with sherpa-onnx; a GGUF final pass goes to
// parakeet.cpp and a GGML final pass to whisper.cpp.
type sherpaLoader struct{}

func (sherpaLoader) loadFinalPass(spec *modelSpec, dirs *modelDirs) (finalPass, error) {
	switch {
	case spec.source.kind == sourceHuggingFace:
		file := dirs.main + string(os.PathSeparator) + spec.source.file
		if spec.recipe == recipeWhisperCpp {
			return loadWhisperCpp(file)
		}
		return loadParakeet(file)
	case spec.recipe.finalPass():
		o, err := loadOffline(spec.recipe, dirs.main)
		if err != nil {
			return nil, err
		}
		return o, nil
	}
	return nil, nil
}

func (l sherpaLoader) load(spec *modelSpec, dirs *modelDirs) (*loaded, error) {
	switch spec.recipe.task() {
	case taskVad:
		return loadSherpaVad(spec, dirs)
	case taskDiarization:
		return loadSherpaDiarization(spec, dirs)
	}
	fp, err := l.loadFinalPass(spec, dirs)
	if err != nil {
		return nil, err
	}
	out := &loaded{finalPass: fp}
	switch {
	case dirs.stream != "" && fp == nil:
		// A streaming model with no final pass is the preview only: load it now.
		online, err := loadOnline(dirs.stream)
		if err != nil {
			return nil, err
		}
		out.preview = online
	case dirs.stream != "":
		// A model with both loads its streaming part at the first stream, so a
		// model that is the final model only never holds it. The files are
		// checked now: a missing file shows at the load.
		if _, err := findOnlineFiles(dirs.stream); err != nil {
			releasePart(fp)
			return nil, err
		}
		dir := dirs.stream
		used := dirs.storeUse.retain()
		lazy := newLazyPreview(func() (livePreview, error) {
			o, err := loadOnline(dir)
			if err != nil {
				return nil, err
			}
			return o, nil
		})
		lazy.refs = newRefs(func() {
			if ready := lazy.readyPreview(); ready != nil {
				releasePart(ready)
			}
			used.release()
		})
		out.preview = lazy
	}
	return out, nil
}

func (p *lazyPreview) retain() {
	if p.refs != nil {
		p.refs.retain()
	}
}

func (p *lazyPreview) release() {
	if p.refs != nil {
		p.refs.release()
	}
}

// ---------------------------------------------------------------------------
// Final pass
// ---------------------------------------------------------------------------

// recognizerKey is what a recognizer is made for. Whisper with segment times
// gets its own recognizer: with them its text can differ, so only a
// transcribe with segments uses them.
type recognizerKey struct {
	language     string
	segmentTimes bool
}

// senseVoiceLanguages are the codes SenseVoice takes. It detects the language for any other.
var senseVoiceLanguages = []string{"zh", "en", "ja", "ko", "yue"}

// baseLanguage is a language code with no region, in lower case. Empty is auto.
func baseLanguage(requested string) string {
	code := strings.TrimSpace(requested)
	if i := strings.IndexAny(code, "-_"); i >= 0 {
		code = code[:i]
	}
	code = strings.ToLower(code)
	if code == "" {
		return "auto"
	}
	return code
}

// recognizerLanguage is the language the recognizer for a request is made for.
// Whisper takes the code; SenseVoice takes five codes and detects the others;
// the other models ignore the language.
func recognizerLanguage(kind offlineKind, requested string) string {
	code := baseLanguage(requested)
	switch kind {
	case offlineWhisper:
		return code
	case offlineSenseVoice:
		for _, c := range senseVoiceLanguages {
			if c == code {
				return code
			}
		}
	}
	return "auto"
}

func keyFor(kind offlineKind, requested string, times bool) recognizerKey {
	return recognizerKey{language: recognizerLanguage(kind, requested), segmentTimes: times && kind == offlineWhisper}
}

// offlineRecognizer is a native recognizer with a reference count.
type offlineRecognizer struct {
	api  *sherpaAPI
	ptr  uintptr
	refs *refs
}

func newOfflineRecognizer(api *sherpaAPI, cfg *sherpaOfflineRecognizerConfig, keep *cStrings) (*offlineRecognizer, error) {
	ptr := api.createOfflineRecognizer(cfg)
	runtime.KeepAlive(keep)
	runtime.KeepAlive(cfg)
	if ptr == 0 {
		return nil, errors.New("sherpa-onnx cannot load the model (see the engine log)")
	}
	r := &offlineRecognizer{api: api, ptr: ptr}
	r.refs = newRefs(func() { api.destroyOfflineRecognizer(ptr) })
	return r, nil
}

// decoded is the result of one decode.
type decoded struct {
	Text              string    `json:"text"`
	Tokens            []string  `json:"tokens"`
	Timestamps        []float32 `json:"timestamps"`
	Durations         []float32 `json:"durations"`
	Lang              string    `json:"lang"`
	SegmentTimestamps []float32 `json:"segment_timestamps"`
	SegmentDurations  []float32 `json:"segment_durations"`
	SegmentTexts      []string  `json:"segment_texts"`
	hasSegmentTimes   bool
}

// parseDecoded reads the JSON of SherpaOnnxGetOfflineStreamResultAsJson.
func parseDecoded(text string) (*decoded, error) {
	var d decoded
	if err := json.Unmarshal([]byte(text), &d); err != nil {
		return nil, errors.New("the model returned a result that is not JSON")
	}
	var probe map[string]json.RawMessage
	_ = json.Unmarshal([]byte(text), &probe)
	_, starts := probe["segment_timestamps"]
	_, lengths := probe["segment_durations"]
	d.hasSegmentTimes = starts && lengths
	return &d, nil
}

func (r *offlineRecognizer) decode(samples []float32) (*decoded, error) {
	if len(samples) > math.MaxInt32 {
		return nil, errors.New("the audio is too long for one decode")
	}
	api := r.api
	stream := api.createOfflineStream(r.ptr)
	if stream == 0 {
		return nil, errors.New("sherpa-onnx cannot make a stream")
	}
	defer api.destroyOfflineStream(stream)
	api.acceptWaveformOffline(stream, sherpaSampleRate, samplesPtr(samples), int32(len(samples)))
	runtime.KeepAlive(samples)
	api.decodeOfflineStream(r.ptr, stream)
	result := api.offlineResultJSON(stream)
	if result == nil {
		return nil, errors.New("the model returned no result")
	}
	text := goString(result)
	api.destroyOfflineResultJSON(result)
	return parseDecoded(text)
}

// offline is an offline model. Whisper and SenseVoice take the language when
// the recognizer is made, so they get a new recognizer when it changes. Only
// the recognizer of the last key is kept; a failed build keeps the old one.
type offline struct {
	api   *sherpaAPI
	files *offlineFiles
	mu    sync.Mutex
	key   recognizerKey
	cur   *offlineRecognizer
	refs  *refs
}

func loadOffline(r recipe, dir string) (*offline, error) {
	api, err := loadSherpa()
	if err != nil {
		return nil, err
	}
	files, err := findOfflineFiles(r, dir)
	if err != nil {
		return nil, err
	}
	o := &offline{api: api, files: files}
	o.refs = newRefs(func() {
		o.mu.Lock()
		cur := o.cur
		o.cur = nil
		o.mu.Unlock()
		if cur != nil {
			cur.refs.release()
		}
	})
	if o.usesLanguage() {
		// The language is not known yet. The first transcribe makes the
		// recognizer; the files are checked now.
		if err := checkReadable(files); err != nil {
			return nil, err
		}
	} else {
		// The recognizer does not depend on the language: make it now, so a
		// model that sherpa-onnx cannot read fails at the load.
		rec, err := o.recognizer("auto", false)
		if err != nil {
			return nil, err
		}
		rec.refs.release()
	}
	return o, nil
}

func (o *offline) retain()  { o.refs.retain() }
func (o *offline) release() { o.refs.release() }

func (o *offline) usesLanguage() bool {
	return o.files.kind == offlineWhisper || o.files.kind == offlineSenseVoice
}

// recognizer is the recognizer for a request, with a reference for the caller.
// The lock stays during a build, so a second request for the same key waits.
func (o *offline) recognizer(language string, times bool) (*offlineRecognizer, error) {
	key := keyFor(o.files.kind, language, times)
	o.mu.Lock()
	defer o.mu.Unlock()
	if o.cur != nil && o.key == key {
		o.cur.refs.retain()
		return o.cur, nil
	}
	built, err := createOffline(o.api, o.files, key)
	if err != nil {
		return nil, err
	}
	if o.cur != nil {
		o.cur.refs.release()
	}
	o.cur, o.key = built, key
	built.refs.retain()
	return built, nil
}

// transcribe: token times give word times when the model has them (Parakeet
// TDT, SenseVoice). Whisper gives segment times with times only. Moonshine
// gives none. Whisper and SenseVoice give the language. Dictionary words need
// hotwords, which the catalog models cannot use, so they are ignored.
func (o *offline) transcribe(samples []float32, language string, _ []string, times bool) (timed, error) {
	rec, err := o.recognizer(language, times)
	if err != nil {
		return timed{}, err
	}
	defer rec.refs.release()
	result, err := rec.decode(samples)
	if err != nil {
		return timed{}, err
	}
	audioMs := uint64(len(samples)) * 1000 / sherpaSampleRate
	t := timingOf(result, audioMs)
	t.text = strings.TrimSpace(result.Text)
	t.language = normalizeLanguage(result.Lang)
	return t, nil
}

// timingOf is the times of a result: the segments of Whisper, else word times
// from the token times.
func timingOf(d *decoded, audioMs uint64) timed {
	if d.hasSegmentTimes {
		n := min(len(d.SegmentTimestamps), len(d.SegmentDurations), len(d.SegmentTexts))
		if n > 0 {
			ms := func(seconds float32) uint64 {
				return min(uint64(math.Round(float64(max(seconds, 0))*1000)), audioMs)
			}
			segments := make([]textSegment, n)
			for i := range n {
				segments[i] = textSegment{
					StartMs: ms(d.SegmentTimestamps[i]),
					EndMs:   ms(d.SegmentTimestamps[i] + max(d.SegmentDurations[i], 0)),
					Text:    strings.TrimSpace(d.SegmentTexts[i]),
				}
			}
			return timed{kind: timingSegments, segments: segments}
		}
	}
	if len(d.Timestamps) > 0 && len(d.Timestamps) == len(d.Tokens) {
		if words := wordsFromTokens(d.Tokens, d.Timestamps, d.Durations, audioMs); len(words) > 0 {
			return timed{kind: timingWords, words: words}
		}
	}
	return timed{}
}

// checkReadable fails when a model file is missing or empty.
func checkReadable(files *offlineFiles) error {
	for _, f := range files.paths() {
		if _, err := modelFile(f); err != nil {
			return err
		}
	}
	return nil
}

// baseOfflineConfig is a config with every field zero but the ones that the
// sherpa-onnx crate sets by default.
func baseOfflineConfig(s *cStrings, tokens string, threads int32) *sherpaOfflineRecognizerConfig {
	cfg := &sherpaOfflineRecognizerConfig{}
	cfg.FeatConfig = sherpaFeatureConfig{SampleRate: 16_000, FeatureDim: 80}
	cfg.LmConfig.Scale = 1
	cfg.MaxActivePaths = 4
	cfg.DecodingMethod = s.add("greedy_search")
	cfg.ModelConfig.Tokens = s.add(tokens)
	cfg.ModelConfig.NumThreads = threads
	cfg.ModelConfig.Provider = s.add("cpu")
	return cfg
}

// createOffline makes a recognizer for key.
func createOffline(api *sherpaAPI, f *offlineFiles, key recognizerKey) (*offlineRecognizer, error) {
	s := &cStrings{}
	cfg := baseOfflineConfig(s, f.tokens, finalPassThreads())
	m := &cfg.ModelConfig
	switch f.kind {
	case offlineTransducer:
		m.Transducer = sherpaOfflineTransducerModelConfig{Encoder: s.add(f.encoder), Decoder: s.add(f.decoder), Joiner: s.add(f.joiner)}
		m.ModelType = s.add("nemo_transducer")
	case offlineNemoCtc:
		m.NemoCtc.Model = s.add(f.model)
	case offlineWhisper:
		// An empty language lets Whisper detect it. Token times need
		// cross-attention outputs, which the exports do not have.
		language := key.language
		if language == "auto" {
			language = ""
		}
		m.Whisper = sherpaOfflineWhisperModelConfig{Encoder: s.add(f.encoder), Decoder: s.add(f.decoder), Language: s.add(language), Task: s.add("transcribe")}
		if key.segmentTimes {
			m.Whisper.EnableSegmentTimestamps = 1
		}
	case offlineSenseVoice:
		// With inverse text normalization: punctuation, capitals, and digits.
		m.SenseVoice = sherpaOfflineSenseVoiceModelConfig{Model: s.add(f.model), Language: s.add(key.language), UseItn: 1}
	case offlineMoonshineV2:
		m.Moonshine = sherpaOfflineMoonshineModelConfig{Encoder: s.add(f.encoder), MergedDecoder: s.add(f.mergedDecoder)}
	case offlineMoonshineV1:
		m.Moonshine = sherpaOfflineMoonshineModelConfig{Preprocessor: s.add(f.preprocessor), Encoder: s.add(f.encoder), UncachedDecoder: s.add(f.uncachedDecoder), CachedDecoder: s.add(f.cachedDecoder)}
	}
	return newOfflineRecognizer(api, cfg, s)
}

// ---------------------------------------------------------------------------
// Live preview
// ---------------------------------------------------------------------------

// online is a streaming transducer.
type online struct {
	api  *sherpaAPI
	ptr  uintptr
	refs *refs
}

func loadOnline(dir string) (*online, error) {
	api, err := loadSherpa()
	if err != nil {
		return nil, err
	}
	files, err := findOnlineFiles(dir)
	if err != nil {
		return nil, err
	}
	s := &cStrings{}
	cfg := &sherpaOnlineRecognizerConfig{
		FeatConfig: sherpaFeatureConfig{SampleRate: 16_000, FeatureDim: 80},
		ModelConfig: sherpaOnlineModelConfig{
			Transducer: sherpaOnlineTransducerModelConfig{Encoder: s.add(files.encoder), Decoder: s.add(files.decoder), Joiner: s.add(files.joiner)},
			Tokens:     s.add(files.tokens),
			NumThreads: previewThreads(),
			Provider:   s.add("cpu"),
		},
		DecodingMethod: s.add("greedy_search"),
		// The sherpa-onnx defaults: an endpoint after 2.4 s of silence with no
		// words, 1.2 s after words, or 20 s of speech.
		EnableEndpoint:          1,
		Rule1MinTrailingSilence: 2.4,
		Rule2MinTrailingSilence: 1.2,
		Rule3MinUtteranceLength: 20,
	}
	ptr := api.createOnlineRecognizer(cfg)
	runtime.KeepAlive(s)
	runtime.KeepAlive(cfg)
	if ptr == 0 {
		return nil, errors.New("sherpa-onnx cannot load the streaming model (see the engine log)")
	}
	o := &online{api: api, ptr: ptr}
	o.refs = newRefs(func() { api.destroyOnlineRecognizer(ptr) })
	return o, nil
}

func (o *online) retain()  { o.refs.retain() }
func (o *online) release() { o.refs.release() }

func (o *online) start() (previewStream, error) {
	stream := o.api.createOnlineStream(o.ptr)
	if stream == 0 {
		return nil, errors.New("sherpa-onnx cannot make a stream")
	}
	o.refs.retain()
	return &onlineSession{owner: o, stream: stream}, nil
}

// onlineSession is one stream. It holds a reference to its recognizer.
type onlineSession struct {
	owner  *online
	stream uintptr
}

func (s *onlineSession) decodeReady() {
	api := s.owner.api
	for api.isOnlineStreamReady(s.owner.ptr, s.stream) != 0 {
		api.decodeOnlineStream(s.owner.ptr, s.stream)
	}
}

func (s *onlineSession) text() string {
	api := s.owner.api
	result := api.onlineResultJSON(s.owner.ptr, s.stream)
	if result == nil {
		return ""
	}
	text := goString(result)
	api.destroyOnlineResultJSON(result)
	var d struct {
		Text string `json:"text"`
	}
	if json.Unmarshal([]byte(text), &d) != nil {
		return ""
	}
	return strings.TrimSpace(d.Text)
}

func (s *onlineSession) accept(samples []float32) {
	if len(samples) == 0 {
		return
	}
	s.owner.api.onlineAcceptWaveform(s.stream, sherpaSampleRate, samplesPtr(samples), int32(len(samples)))
	runtime.KeepAlive(samples)
}

func (s *onlineSession) decode() segment {
	s.decodeReady()
	text := s.text()
	endpoint := s.owner.api.onlineIsEndpoint(s.owner.ptr, s.stream) != 0
	if endpoint {
		s.owner.api.onlineStreamReset(s.owner.ptr, s.stream)
	}
	return segment{text: text, endpoint: endpoint}
}

func (s *onlineSession) finish() segment {
	// Silence at the end lets the model emit the last words. The Kroko model
	// decodes in chunks of about 1.4 s, and 0.8 s loses the last words.
	s.accept(make([]float32, sherpaSampleRate*2))
	s.owner.api.onlineInputFinished(s.stream)
	s.decodeReady()
	return segment{text: s.text(), endpoint: true}
}

func (s *onlineSession) close() {
	if s.stream != 0 {
		s.owner.api.destroyOnlineStream(s.stream)
		s.stream = 0
		s.owner.refs.release()
	}
}

// ---------------------------------------------------------------------------
// VAD
// ---------------------------------------------------------------------------

const (
	// vadThreshold is the speech probability above which a window is speech.
	vadThreshold = 0.5
	// vadMaxSpeechSeconds is long on purpose: the tracker cuts speech at 30 s
	// (see vad.go), and the detector keeps its other rules.
	vadMaxSpeechSeconds = 3600.0
	// vadBufferSeconds is the audio the buffer of a detector holds at first.
	vadBufferSeconds = 60.0
)

// sherpaVad is a VAD model: the config of sherpa-onnx. Each audio gets its
// own detector, because a detector keeps the state of the audio it hears.
type sherpaVad struct {
	api    *sherpaAPI
	model  string
	recipe recipe
	win    int
}

func (v *sherpaVad) config(s *cStrings) *sherpaVadModelConfig {
	cfg := &sherpaVadModelConfig{SampleRate: sherpaSampleRate, NumThreads: 1, Provider: s.add("cpu")}
	detector := sherpaVadDetectorConfig{
		Model:              s.add(v.model),
		Threshold:          vadThreshold,
		MinSilenceDuration: float32(minSilenceMs) / 1000,
		MinSpeechDuration:  float32(minSpeechMs) / 1000,
		WindowSize:         int32(v.win),
		MaxSpeechDuration:  vadMaxSpeechSeconds,
	}
	if v.recipe == recipeTenVad {
		cfg.TenVad = detector
	} else {
		cfg.SileroVad = detector
	}
	return cfg
}

func loadSherpaVad(spec *modelSpec, dirs *modelDirs) (*loaded, error) {
	if spec.source.kind != sourceSherpaFile {
		return nil, errors.New("a VAD model is one file")
	}
	api, err := loadSherpa()
	if err != nil {
		return nil, err
	}
	file, err := vadFile(dirs.main, spec.source.file)
	if err != nil {
		return nil, err
	}
	v := &sherpaVad{api: api, model: file, recipe: spec.recipe, win: 512}
	if spec.recipe == recipeTenVad {
		v.win = 256
	}
	// A model that sherpa-onnx cannot read fails at the load.
	d, err := v.detector()
	if err != nil {
		return nil, err
	}
	d.close()
	return &loaded{vad: v}, nil
}

func (v *sherpaVad) detector() (detector, error) {
	s := &cStrings{}
	cfg := v.config(s)
	ptr := v.api.createVad(cfg, vadBufferSeconds)
	runtime.KeepAlive(s)
	runtime.KeepAlive(cfg)
	if ptr == 0 {
		return nil, errors.New("sherpa-onnx cannot load the VAD model (see the engine log)")
	}
	return &sherpaDetector{api: v.api, ptr: ptr, win: v.win}, nil
}

type sherpaDetector struct {
	api *sherpaAPI
	ptr uintptr
	win int
}

func (d *sherpaDetector) window() int { return d.win }

func (d *sherpaDetector) accept(w []float32) {
	d.api.vadAcceptWaveform(d.ptr, samplesPtr(w), int32(len(w)))
	runtime.KeepAlive(w)
}

func (d *sherpaDetector) detected() bool { return d.api.vadDetected(d.ptr) != 0 }

func (d *sherpaDetector) nextSegment() (start, end uint64, ok bool) {
	if d.api.vadEmpty(d.ptr) != 0 {
		return 0, 0, false
	}
	seg := d.api.vadFront(d.ptr)
	if seg == nil {
		return 0, 0, false
	}
	start = uint64(max(seg.Start, 0))
	end = start + uint64(max(seg.N, 0))
	d.api.destroySegment(seg)
	d.api.vadPop(d.ptr)
	return start, end, true
}

func (d *sherpaDetector) flush() { d.api.vadFlush(d.ptr) }

func (d *sherpaDetector) close() {
	if d.ptr != 0 {
		d.api.destroyVad(d.ptr)
		d.ptr = 0
	}
}

// ---------------------------------------------------------------------------
// Diarization
// ---------------------------------------------------------------------------

// sherpaDiarizer is pyannote segmentation, a speaker embedding model, and
// fast clustering. The clustering changes with each request, so a request
// sets it and runs under the lock.
type sherpaDiarizer struct {
	api     *sherpaAPI
	mu      sync.Mutex
	ptr     uintptr
	strings *cStrings
	cfg     *sherpaDiarizationConfig
	refs    *refs
}

func clustering(speakers uint32) sherpaFastClusteringConfig {
	c := sherpaFastClusteringConfig{NumClusters: -1, Threshold: clusterThreshold}
	if speakers > 0 {
		c.NumClusters = int32(min(speakers, math.MaxInt32))
	}
	return c
}

func loadSherpaDiarization(spec *modelSpec, dirs *modelDirs) (*loaded, error) {
	if spec.source.kind != sourceDiarization || dirs.embedding == "" {
		return nil, errors.New("a diarization model has a segmentation model and an embedding model")
	}
	files, err := findDiarizationFiles(dirs.main, dirs.embedding, spec.source.file)
	if err != nil {
		return nil, err
	}
	api, err := loadSherpa()
	if err != nil {
		return nil, err
	}
	s := &cStrings{}
	threads := finalPassThreads()
	cfg := &sherpaDiarizationConfig{
		Segmentation: sherpaSegmentationModelConfig{
			Pyannote:   sherpaPyannoteModelConfig{Model: s.add(files.segmentation), WindowShiftRatio: 0.1},
			NumThreads: threads,
			Provider:   s.add("cpu"),
		},
		Embedding:      sherpaSpeakerEmbeddingExtractorConfig{Model: s.add(files.embedding), NumThreads: threads, Provider: s.add("cpu")},
		Clustering:     clustering(0),
		MinDurationOn:  0.3,
		MinDurationOff: 0.5,
	}
	ptr := api.createDiarization(cfg)
	runtime.KeepAlive(cfg)
	if ptr == 0 {
		return nil, errors.New("sherpa-onnx cannot load the diarization model (see the engine log)")
	}
	if rate := api.diarizationSampleRate(ptr); rate != sherpaSampleRate {
		api.destroyDiarization(ptr)
		return nil, fmt.Errorf("the diarization model needs %d Hz audio, not %d Hz", rate, sherpaSampleRate)
	}
	d := &sherpaDiarizer{api: api, ptr: ptr, strings: s, cfg: cfg}
	d.refs = newRefs(func() { api.destroyDiarization(ptr) })
	return &loaded{diarizer: d}, nil
}

func (d *sherpaDiarizer) retain()  { d.refs.retain() }
func (d *sherpaDiarizer) release() { d.refs.release() }

func (d *sherpaDiarizer) diarize(samples []float32, speakers uint32) ([]rawTurn, error) {
	d.mu.Lock()
	defer d.mu.Unlock()
	cfg := *d.cfg
	cfg.Clustering = clustering(speakers)
	d.api.diarizationSetConfig(d.ptr, &cfg)
	result := d.api.diarizationProcess(d.ptr, samplesPtr(samples), int32(len(samples)))
	runtime.KeepAlive(samples)
	runtime.KeepAlive(d.strings)
	if result == 0 {
		return nil, errors.New("sherpa-onnx cannot diarize the audio (see the engine log)")
	}
	defer d.api.destroyResult(result)
	n := int(d.api.resultNumSegments(result))
	if n <= 0 {
		return nil, nil
	}
	first := d.api.resultSortByStart(result)
	if first == nil {
		return nil, nil
	}
	defer d.api.destroyDiarSegment(first)
	segments := unsafeSlice(first, n)
	turns := make([]rawTurn, n)
	for i, s := range segments {
		turns[i] = rawTurn{start: s.Start, end: s.End, speaker: s.Speaker}
	}
	return turns, nil
}
