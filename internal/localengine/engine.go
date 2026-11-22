package localengine

import (
	"bufio"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

// Goroutines, as in the Rust and Swift engines:
//   - The stdin loop reads requests. It never runs a model.
//   - Each request with one reply (hello, list_models, download, delete, load,
//     transcribe, vad, diarize) runs on its own goroutine.
//   - unload runs on the stdin loop, so a later load of the same model finds
//     the model gone.
//   - The streaming requests (stream_start, vad_start, stream_audio,
//     stream_end) run in arrival order on one stream worker.

// config is the settings of one engine process.
type config struct {
	modelsDir      string
	baseURL        string
	hfEndpoint     string
	decodeInterval time.Duration
	store          *modelStore
	descriptors    *descriptors
}

func configFromEnv(modelsDir string) (*config, error) {
	d, err := parseDescriptors(builtInDescriptors)
	if err != nil {
		return nil, fmt.Errorf("the built-in descriptors are invalid: %w", err)
	}
	c := &config{modelsDir: modelsDir, baseURL: defaultBaseURL, hfEndpoint: defaultHFEndpoint, decodeInterval: 150 * time.Millisecond, descriptors: d}
	if v := os.Getenv("SAYSO_MODELS_URL"); v != "" {
		c.baseURL = v
	}
	if v := os.Getenv("SAYSO_HF_ENDPOINT"); v != "" {
		c.hfEndpoint = v
	}
	return c, nil
}

// refs is a reference count for a native model part. It starts at one, the
// reference of the engine. free runs when the last reference goes.
type refs struct {
	n    atomic.Int64
	free func()
}

func newRefs(free func()) *refs {
	r := &refs{free: free}
	r.n.Store(1)
	return r
}

func (r *refs) retain() { r.n.Add(1) }

func (r *refs) release() {
	if r.n.Add(-1) == 0 && r.free != nil {
		r.free()
	}
}

// counted is a model part with a reference count. The fakes of the tests have none.
type counted interface {
	retain()
	release()
}

func retainPart(part any) {
	if c, ok := part.(counted); ok && part != nil {
		c.retain()
	}
}

func releasePart(part any) {
	if c, ok := part.(counted); ok && part != nil {
		c.release()
	}
}

type loadedEntry struct {
	spec   *modelSpec
	loaded *loaded
}

// Engine runs the requests of one engine process.
type Engine struct {
	config  *config
	catalog *catalog
	folders *modelFolders
	out     *Output
	loader  loader
	client  *http.Client

	mu        sync.Mutex
	loadedMap map[string]*loadedEntry
	busy      map[string]bool
}

func newEngine(c *config, out *Output, l loader) (*Engine, error) {
	cat, err := currentCatalog()
	if err != nil {
		return nil, fmt.Errorf("the built-in catalog is invalid: %w", err)
	}
	return &Engine{
		config: c, catalog: cat, folders: &modelFolders{root: c.modelsDir}, out: out, loader: l,
		client: newHTTPClient(), loadedMap: map[string]*loadedEntry{}, busy: map[string]bool{},
	}, nil
}

// acquire marks a model busy until the returned function runs.
func (e *Engine) acquire(model string) (func(), error) {
	e.mu.Lock()
	defer e.mu.Unlock()
	if e.busy[model] {
		return nil, fmt.Errorf("model %s is busy", model)
	}
	e.busy[model] = true
	return func() {
		e.mu.Lock()
		delete(e.busy, model)
		e.mu.Unlock()
	}, nil
}

func (e *Engine) modelState(id *string, model, state string, extra fields) {
	f := fields{"model": model, "state": state}
	for k, v := range extra {
		f[k] = v
	}
	e.out.Send("model_state", id, f)
}

// handle runs one request that has a single reply. Errors become error replies.
func (e *Engine) handle(r *Request) {
	var err error
	switch r.Kind {
	case "hello":
		e.out.Send("hello", r.ID, fields{"version": Version, "protocol": ProtocolVersion, "features": Features})
	case "list_models":
		err = e.listModels(r)
	case "download":
		err = e.download(r)
	case "delete":
		err = e.deleteModel(r)
	case "prepare":
		e.out.OK(r.ID, nil)
	case "load":
		err = e.load(r)
	case "unload":
		err = e.unload(r)
	case "transcribe":
		err = e.transcribe(r)
	case "vad":
		err = e.vad(r)
	case "diarize":
		err = e.diarize(r)
	default:
		err = fmt.Errorf("unknown request type %s", r.Kind)
	}
	if err != nil {
		e.out.Failure(err, r.ID)
	}
}

// finalPassIsOut is true when the model has a final pass that is not in
// memory now: an unload with keep_preview took it out.
func finalPassIsOut(spec *modelSpec, l *loaded) bool {
	return spec.recipe.finalPass() && l.finalPass == nil
}

// status is the state of a model and the extra fields of its model_state.
func (e *Engine) status(model string, spec *modelSpec) (string, fields) {
	e.mu.Lock()
	entry := e.loadedMap[model]
	e.mu.Unlock()
	if entry != nil {
		if finalPassIsOut(entry.spec, entry.loaded) {
			onDisk := true
			if store, desc := e.stored(model, entry.spec); store != nil {
				if on, err := store.onDisk(desc); err == nil {
					onDisk = on
				}
			}
			if onDisk {
				return "downloaded", fields{"preview": true}
			}
			return "not_downloaded", fields{"preview": true}
		}
		return "ready", nil
	}
	if store, desc := e.stored(model, spec); store != nil {
		onDisk, err := store.onDisk(desc)
		if err != nil {
			e.out.Log("warning", fmt.Sprintf("%s: cannot read the model store: %v", model, err))
		}
		if onDisk {
			return "downloaded", nil
		}
		return "not_downloaded", nil
	}
	if e.adoptLegacy(model, spec) == legacyFormatChanged {
		return "failed", fields{"message": "the files of this model are in an old format; download the model again", "reason": "format_changed"}
	}
	if e.folders.isDownloaded(model, spec) {
		return "downloaded", nil
	}
	return "not_downloaded", nil
}

// stored is the store and descriptor of a model in the model store, or nils.
func (e *Engine) stored(model string, spec *modelSpec) (*modelStore, *descriptor) {
	if e.config.store == nil {
		return nil, nil
	}
	desc := e.config.descriptors.find(model, spec.format())
	if desc == nil {
		return nil, nil
	}
	return e.config.store, desc
}

// adoptLegacy moves the files of the Windows engine before 0.12 into place. A
// model with a download or a delete in progress is left alone.
func (e *Engine) adoptLegacy(model string, spec *modelSpec) legacyState {
	done, err := e.acquire(model)
	if err != nil {
		return legacyNone
	}
	defer done()
	state, err := e.folders.adoptLegacy(model, spec)
	if err != nil {
		e.out.Log("warning", fmt.Sprintf("%s: cannot move the old files into place: %v", model, err))
		return legacyFormatChanged
	}
	return state
}

// specOf is the engine of a model: the engine object of the request, else the
// one of the built-in catalog.
func (e *Engine) specOf(model string, engine json.RawMessage) (*modelSpec, error) {
	if engine != nil {
		spec, err := parseSpec(engine)
		if err != nil {
			return nil, fmt.Errorf("model %s: %w", model, err)
		}
		return spec, nil
	}
	if entry := e.catalog.model(model); entry != nil {
		spec, err := parseSpec(entry.Engine)
		if err != nil {
			return nil, fmt.Errorf("model %s: %w", model, err)
		}
		return spec, nil
	}
	if retired := e.catalog.retiredModel(model); retired != nil {
		return nil, &codedError{"model_retired", fmt.Sprintf("model %s was taken out of the catalog. %s", model, retired.Message)}
	}
	return nil, &codedError{"unknown_model", fmt.Sprintf("model %s is not in the catalog; send its engine object", model)}
}

func (e *Engine) listModels(r *Request) error {
	type item struct {
		id   string
		spec *modelSpec
	}
	var items []item
	raw, explicit := r.OptionalValue("models")
	if explicit {
		var list []map[string]json.RawMessage
		if json.Unmarshal(raw, &list) != nil {
			return errors.New("models is not an array")
		}
		for _, entry := range list {
			var id string
			if v, ok := entry["id"]; !ok || json.Unmarshal(v, &id) != nil {
				return errors.New("models[].id is missing")
			}
			var engine json.RawMessage
			if v, ok := entry["engine"]; ok && string(v) != "null" {
				engine = v
			}
			spec, err := e.specOf(id, engine)
			if err != nil {
				return err
			}
			items = append(items, item{id, spec})
		}
	} else {
		for _, m := range e.catalog.models {
			spec, err := e.specOf(m.ID, nil)
			if err != nil {
				return err
			}
			items = append(items, item{m.ID, spec})
		}
	}
	for _, it := range items {
		state, extra := e.status(it.id, it.spec)
		e.modelState(r.ID, it.id, state, extra)
	}
	if explicit {
		e.out.OK(r.ID, nil)
	} else {
		e.out.OK(r.ID, e.catalog.reply())
	}
	return nil
}

// incompleteError is the reason incomplete_download: a download ended with
// files that are not complete.
type incompleteError struct{ message string }

func (e *incompleteError) Error() string { return e.message }

func (e *Engine) report(id *string, model string, p *progress, fraction float64) {
	_, done, total := p.snapshot()
	f := fields{"model": model, "fraction": fraction, "bytes_done": done, "bytes_total": total}
	e.out.Send("download_progress", id, f)
	e.modelState(id, model, "downloading", f)
}

func (e *Engine) download(r *Request) error {
	model, err := r.String("model")
	if err != nil {
		return err
	}
	engine, _ := r.OptionalValue("engine")
	spec, err := e.specOf(model, engine)
	if err != nil {
		return err
	}
	if store, desc := e.stored(model, spec); store != nil {
		return e.downloadStored(r, store, desc)
	}
	if _, err := e.folders.folder(model); err != nil {
		return err
	}
	if e.adoptLegacy(model, spec) != legacyFormatChanged && e.folders.isDownloaded(model, spec) {
		e.modelState(r.ID, model, "downloaded", nil)
		e.out.OK(r.ID, nil)
		return nil
	}
	done, err := e.acquire(model)
	if err != nil {
		return err
	}
	defer done()
	size, _ := r.OptionalUint("size_bytes")
	p := newProgress(int64(size))
	p.due(time.Now())
	e.report(r.ID, model, p, 0)
	if err := e.fetch(r.ID, model, spec, p); err != nil {
		reason := "download_failed"
		var incomplete *incompleteError
		if errors.As(err, &incomplete) {
			reason = "incomplete_download"
		}
		e.modelState(r.ID, model, "failed", fields{"message": err.Error(), "reason": reason})
		return err
	}
	e.report(r.ID, model, p, 1)
	e.modelState(r.ID, model, "downloaded", nil)
	e.out.OK(r.ID, nil)
	return nil
}

// downloadStored is download of a model in the model store.
func (e *Engine) downloadStored(r *Request, store *modelStore, desc *descriptor) error {
	model := desc.Model
	on, err := store.onDisk(desc)
	if err != nil {
		return err
	}
	if on {
		e.modelState(r.ID, model, "downloaded", nil)
		e.out.OK(r.ID, nil)
		return nil
	}
	done, err := e.acquire(model)
	if err != nil {
		return err
	}
	defer done()
	data := e.config.descriptors
	var total uint64
	for _, part := range data.parts(desc) {
		total += part.artifact.size()
	}
	if size, ok := r.OptionalUint("size_bytes"); ok {
		total = size
	}
	p := newProgress(int64(total))
	p.due(time.Now())
	e.report(r.ID, model, p, 0)
	onBytes := func(n int64) {
		p.add(n)
		if p.due(time.Now()) {
			fraction, _, _ := p.snapshot()
			e.report(r.ID, model, p, fraction)
		}
	}
	err = store.download(data, desc, func(a *artifact, work, files string) error {
		return fetchStoreArtifact(e.client, a, e.storeURL, work, files, onBytes)
	}, func() {
		// Another engine downloads a part of this model.
		fraction, _, _ := p.snapshot()
		e.report(r.ID, model, p, fraction)
	})
	if err != nil {
		reason := "download_failed"
		var verify *verifyError
		if errors.As(err, &verify) {
			reason = "incomplete_download"
		}
		e.modelState(r.ID, model, "failed", fields{"message": err.Error(), "reason": reason})
		return err
	}
	e.report(r.ID, model, p, 1)
	e.modelState(r.ID, model, "downloaded", nil)
	e.out.OK(r.ID, nil)
	return nil
}

// storeURL is the URL to use for a URL of a descriptor: SAYSO_MODELS_URL and
// SAYSO_HF_ENDPOINT also move the files of a descriptor.
func (e *Engine) storeURL(url string) string {
	if rest, ok := strings.CutPrefix(url, hfPrefix); ok {
		return strings.TrimRight(e.config.hfEndpoint, "/") + "/" + rest
	}
	if rest, ok := strings.CutPrefix(url, releasesPrefix); ok {
		return siblingReleaseURL(e.config.baseURL, rest)
	}
	return url
}

// fetch downloads the archives that are missing, checks the files, and marks
// the model complete.
func (e *Engine) fetch(id *string, model string, spec *modelSpec, p *progress) error {
	if err := e.folders.prepareDownload(model, spec); err != nil {
		return err
	}
	for _, archive := range spec.archives() {
		dest, err := e.folders.archiveDir(model, archive)
		if err != nil {
			return err
		}
		if isDir(dest) {
			continue // A complete archive from a download that stopped later.
		}
		staging, err := e.folders.stagingDir(model, archive)
		if err != nil {
			return err
		}
		url, file := e.sourceOf(spec, archive)
		err = fetchArchive(e.client, url, file, staging, dest, func(n int64) {
			p.add(n)
			if p.due(time.Now()) {
				fraction, _, _ := p.snapshot()
				e.report(id, model, p, fraction)
			}
		})
		if err != nil {
			return err
		}
	}
	dirs, err := e.dirs(model, spec)
	if err != nil {
		return err
	}
	if err := checkFiles(spec, dirs); err != nil {
		return &incompleteError{err.Error()}
	}
	if err := e.folders.markDownloaded(model, spec); err != nil {
		return err
	}
	if !e.folders.isDownloaded(model, spec) {
		return &incompleteError{"download finished but the files are incomplete"}
	}
	return nil
}

// sourceOf is where one archive of a model comes from: its URL, and its file
// name when it is one file and not a .tar.bz2 archive.
func (e *Engine) sourceOf(spec *modelSpec, archive string) (url, file string) {
	base := e.config.baseURL
	switch {
	case spec.source.kind == sourceHuggingFace && archive == spec.archive:
		return hfFileURL(e.config.hfEndpoint, spec.source.repo, archive, spec.source.file), spec.source.file
	case spec.source.kind == sourceSherpaFile && archive == spec.archive:
		return fileURL(base, spec.source.file), spec.source.file
	case spec.source.kind == sourceDiarization && spec.embeddingArchive() == archive:
		return fileURL(siblingReleaseURL(base, embeddingRelease), spec.source.file), spec.source.file
	case spec.source.kind == sourceDiarization:
		return archiveURL(siblingReleaseURL(base, segmentationRelease), archive), ""
	}
	return archiveURL(base, archive), ""
}

func (e *Engine) deleteModel(r *Request) error {
	model, err := r.String("model")
	if err != nil {
		return err
	}
	done, err := e.acquire(model)
	if err != nil {
		return err
	}
	defer done()
	e.evict(model)
	var stored []*descriptor
	for _, d := range e.config.descriptors.Descriptors {
		if d.Model == model && (d.Format == formatSherpaOnnx || d.Format == formatWhisperCpp || d.Format == formatParakeetCpp) {
			stored = append(stored, d)
		}
	}
	if e.config.store != nil && len(stored) > 0 {
		for _, d := range stored {
			if err := e.config.store.delete(model, d.Format); err != nil {
				return err
			}
		}
	} else if err := e.folders.delete(model); err != nil {
		return err
	}
	e.modelState(r.ID, model, "not_downloaded", nil)
	e.out.OK(r.ID, nil)
	return nil
}

func (e *Engine) dirs(model string, spec *modelSpec) (*modelDirs, error) {
	main, err := e.folders.archiveDir(model, spec.archive)
	if err != nil {
		return nil, err
	}
	d := &modelDirs{main: main}
	if a := spec.streamArchive(); a != "" {
		if d.stream, err = e.folders.archiveDir(model, a); err != nil {
			return nil, err
		}
	}
	if a := spec.embeddingArchive(); a != "" {
		if d.embedding, err = e.folders.archiveDir(model, a); err != nil {
			return nil, err
		}
	}
	return d, nil
}

// storedDirs are the folders of a model in the model store, by the roles of
// the descriptors.
func storedDirs(spec *modelSpec, used *storeUse) (*modelDirs, error) {
	main, ok := used.lease.folder("main")
	if !ok {
		if main, ok = used.lease.folder("segmentation"); !ok {
			return nil, fmt.Errorf("the manifest of %s has no main artifact", spec.archive)
		}
	}
	d := &modelDirs{main: main, storeUse: used}
	if spec.streamArchive() != "" {
		if preview, ok := used.lease.folder("preview"); ok {
			d.stream = preview
		} else {
			d.stream = main
		}
	}
	d.embedding, _ = used.lease.folder("embedding")
	return d, nil
}

func (e *Engine) load(r *Request) error {
	model, err := r.String("model")
	if err != nil {
		return err
	}
	engine, _ := r.OptionalValue("engine")
	spec, err := e.specOf(model, engine)
	if err != nil {
		return err
	}
	// The live preview that an unload with keep_preview left in memory.
	var keptPreview livePreview
	e.mu.Lock()
	if entry := e.loadedMap[model]; entry != nil && entry.spec.equal(spec) {
		if !finalPassIsOut(entry.spec, entry.loaded) {
			e.mu.Unlock()
			e.modelState(r.ID, model, "ready", fields{"load_ms": 0})
			e.out.OK(r.ID, nil)
			return nil
		}
		keptPreview = entry.loaded.preview
		retainPart(keptPreview)
	}
	e.mu.Unlock()
	defer releasePart(keptPreview)
	// The locks of "Load" in the model store. A load never writes in the store.
	var used *storeUse
	if store, desc := e.stored(model, spec); store != nil {
		l, err := store.load(desc)
		if err != nil {
			return err
		}
		if l == nil {
			e.modelState(r.ID, model, "not_downloaded", nil)
			return fmt.Errorf("model %s is not downloaded", model)
		}
		used = newStoreUse(l, store)
	} else {
		e.adoptLegacy(model, spec)
		if !e.folders.isDownloaded(model, spec) {
			return fmt.Errorf("model %s is not downloaded", model)
		}
	}
	done, err := e.acquire(model)
	if err != nil {
		used.release()
		return err
	}
	e.modelState(r.ID, model, "optimizing", nil)
	start := time.Now()
	var dirs *modelDirs
	if used != nil {
		dirs, err = storedDirs(spec, used)
	} else {
		dirs, err = e.dirs(model, spec)
	}
	var l *loaded
	if err == nil {
		l, err = e.loadParts(model, spec, dirs, keptPreview)
	}
	if err != nil {
		reason := "load_failed"
		if errors.Is(err, errNoGPU) {
			reason = "no_gpu"
		}
		e.modelState(r.ID, model, "failed", fields{"message": err.Error(), "reason": reason})
		used.release()
		done()
		return err
	}
	if used != nil {
		used.store.step("load.loaded")
	}
	l.storeUse = used
	e.mu.Lock()
	old := e.loadedMap[model]
	e.loadedMap[model] = &loadedEntry{spec: spec, loaded: l}
	e.mu.Unlock()
	if old != nil {
		freeLoaded(old.loaded)
	}
	e.modelState(r.ID, model, "ready", fields{"load_ms": time.Since(start).Milliseconds()})
	e.out.OK(r.ID, nil)
	// The model is free before the reply ends: an unload or delete that follows
	// must not find it busy.
	done()
	return nil
}

// loadParts loads the parts of a model. With a kept live preview, only the
// final pass loads.
func (e *Engine) loadParts(model string, spec *modelSpec, dirs *modelDirs, keptPreview livePreview) (l *loaded, err error) {
	defer catchPanic("the load", &err)
	if keptPreview != nil {
		fp, err := e.loader.loadFinalPass(spec, dirs)
		if err != nil {
			return nil, err
		}
		retainPart(keptPreview)
		return &loaded{finalPass: fp, preview: keptPreview}, nil
	}
	l, err = e.loader.load(spec, dirs)
	if err != nil {
		return nil, err
	}
	if !spec.redecodePreview {
		return l, nil
	}
	if l.finalPass == nil {
		freeLoaded(l)
		return nil, fmt.Errorf("model %s has no final pass for its live preview", model)
	}
	return attachRedecode(l.finalPass), nil
}

// freeLoaded lets go of the engine's references to the parts of a model.
func freeLoaded(l *loaded) {
	releasePart(l.finalPass)
	releasePart(l.preview)
	releasePart(l.vad)
	releasePart(l.diarizer)
	if l.free != nil {
		l.free()
	}
	l.storeUse.release()
}

// unload takes a model out of memory. With keep_preview, a model whose live
// preview is a model of its own keeps it, and only its final pass leaves.
func (e *Engine) unload(r *Request) error {
	model, err := r.String("model")
	if err != nil {
		return err
	}
	keepPreview := r.OptionalBool("keep_preview")
	done, err := e.acquire(model)
	if err != nil {
		return err
	}
	defer done()
	e.mu.Lock()
	entry := e.loadedMap[model]
	e.mu.Unlock()
	var spec *modelSpec
	if entry != nil {
		spec = entry.spec
	}
	hasPreview := entry != nil && entry.loaded.preview != nil && entry.spec.streamArchive() != ""
	if keepPreview && hasPreview {
		if e.evictFinalPass(model) {
			e.modelState(r.ID, model, e.stateAfterUnload(model, spec), fields{"preview": true})
		}
	} else if e.evict(model) {
		e.modelState(r.ID, model, e.stateAfterUnload(model, spec), nil)
	}
	e.out.OK(r.ID, nil)
	return nil
}

// stateAfterUnload is the state of a model after unload. In the model store,
// another engine can have deleted it in the meantime; this does not wait for
// a lock, and says downloaded when another engine holds it.
func (e *Engine) stateAfterUnload(model string, spec *modelSpec) string {
	if spec == nil {
		return "downloaded"
	}
	if store, desc := e.stored(model, spec); store != nil {
		if on, known, err := store.onDiskNow(desc); err == nil && known && !on {
			return "not_downloaded"
		}
	}
	return "downloaded"
}

// evictFinalPass drops the final pass of a loaded model and keeps its live
// preview. False when the model had no final pass in memory.
func (e *Engine) evictFinalPass(model string) bool {
	e.mu.Lock()
	entry := e.loadedMap[model]
	var removed finalPass
	if entry != nil && entry.loaded.finalPass != nil {
		removed = entry.loaded.finalPass
		next := *entry.loaded
		next.finalPass = nil
		e.loadedMap[model] = &loadedEntry{spec: entry.spec, loaded: &next}
	}
	e.mu.Unlock()
	releasePart(removed)
	return removed != nil
}

// evict drops a loaded model. A final pass or a stream that runs now keeps
// its part of the model until it ends.
func (e *Engine) evict(model string) bool {
	e.mu.Lock()
	entry := e.loadedMap[model]
	delete(e.loadedMap, model)
	e.mu.Unlock()
	if entry == nil {
		return false
	}
	freeLoaded(entry.loaded)
	return true
}

func (e *Engine) transcribe(r *Request) error {
	model, err := r.String("model")
	if err != nil {
		return err
	}
	path, err := r.String("path")
	if err != nil {
		return err
	}
	language, ok := r.OptionalString("language")
	if !ok {
		language = "en"
	}
	vocabulary := r.Strings("vocabulary")
	e.mu.Lock()
	entry := e.loadedMap[model]
	var fp finalPass
	if entry != nil && entry.loaded.finalPass != nil && entry.spec.recipe.task() == taskSpeech {
		fp = entry.loaded.finalPass
		retainPart(fp)
	}
	e.mu.Unlock()
	if entry == nil {
		return fmt.Errorf("model %s is not loaded", model)
	}
	if err := checkTask(model, entry.spec, taskSpeech); err != nil {
		return err
	}
	if fp == nil {
		if finalPassIsOut(entry.spec, entry.loaded) {
			return fmt.Errorf("model %s is not loaded", model)
		}
		return fmt.Errorf("model %s has no final pass", model)
	}
	defer releasePart(fp)
	samples, err := readWAV(path)
	if err != nil {
		return err
	}
	if len(samples) == 0 {
		return errors.New("audio file has no samples")
	}
	withSegments := r.OptionalBool("segments")
	start := time.Now()
	var parts []string
	var pieces []piece
	var languages languageTally
	offset := 0
	for _, chunk := range splitAudio(samples, entry.spec.recipe.maxChunkSeconds()) {
		startMs := uint64(offset * 1000 / SampleRate)
		offset += len(chunk)
		endMs := uint64(offset * 1000 / SampleRate)
		t, err := safeTranscribe(fp, chunk, language, vocabulary, withSegments)
		if err != nil {
			return err
		}
		languages.add(len(chunk), t.text, t.language)
		parts = append(parts, t.text)
		if withSegments {
			pieces = append(pieces, piece{startMs: startMs, endMs: endMs, timed: t})
		}
	}
	text := strings.TrimSpace(joinTexts(parts))
	f := fields{"text": text, "elapsed_ms": time.Since(start).Milliseconds()}
	if withSegments {
		f["segments"] = allSegments(pieces)
	}
	// Only for auto: with a named language, a model gives that language back.
	if isAutoLanguage(language) && text != "" {
		if found := languages.mostAudio(); found != "" && e.reportsLanguage(model, found) {
			f["language"] = found
		}
	}
	e.out.Send("final", r.ID, f)
	return nil
}

func safeTranscribe(fp finalPass, samples []float32, language string, vocabulary []string, times bool) (t timed, err error) {
	defer catchPanic("the final pass", &err)
	return fp.transcribe(samples, language, vocabulary, times)
}

// reportsLanguage is true when final.language can be code for model: the
// catalog says that the model finds the language, and code is one of its
// languages. A model that is not in the catalog reports what it finds.
func (e *Engine) reportsLanguage(model, code string) bool {
	entry := e.catalog.model(model)
	if entry == nil {
		return true
	}
	if !entry.DetectsLanguage {
		return false
	}
	for _, c := range entry.Languages {
		if c == code {
			return true
		}
	}
	return false
}

// part is a loaded part of a model with a reference for the caller. The
// caller calls done when it no longer uses the part.
type part[T any] struct {
	value T
	used  *storeUse
}

func (p part[T]) done() {
	releasePart(p.value)
	p.used.release()
}

// takePart gets one part of a loaded model of task t, in one step with its
// files, because an unload can run at the same time.
func takePart[T any](e *Engine, model string, t task, pick func(*loaded) (T, bool), missing string) (part[T], error) {
	e.mu.Lock()
	defer e.mu.Unlock()
	var zero part[T]
	entry := e.loadedMap[model]
	if entry == nil {
		return zero, fmt.Errorf("model %s is not loaded", model)
	}
	if err := checkTask(model, entry.spec, t); err != nil {
		return zero, err
	}
	value, ok := pick(entry.loaded)
	if !ok {
		return zero, fmt.Errorf("model %s has no %s", model, missing)
	}
	retainPart(value)
	return part[T]{value: value, used: entry.loaded.storeUse.retain()}, nil
}

func (e *Engine) vadPart(model string) (part[vadModel], error) {
	return takePart(e, model, taskVad, func(l *loaded) (vadModel, bool) { return l.vad, l.vad != nil }, "VAD")
}

func (e *Engine) diarizerPart(model string) (part[diarizer], error) {
	return takePart(e, model, taskDiarization, func(l *loaded) (diarizer, bool) { return l.diarizer, l.diarizer != nil }, "diarizer")
}

func (e *Engine) previewPart(model string) (part[livePreview], error) {
	return takePart(e, model, taskSpeech, func(l *loaded) (livePreview, bool) { return l.preview, l.preview != nil }, "live preview")
}

// vad finds the parts of a WAV file with speech, with the tracker of a VAD session.
func (e *Engine) vad(r *Request) error {
	model, err := r.String("model")
	if err != nil {
		return err
	}
	path, err := r.String("path")
	if err != nil {
		return err
	}
	p, err := e.vadPart(model)
	if err != nil {
		return err
	}
	defer p.done()
	samples, err := readWAV(path)
	if err != nil {
		return err
	}
	det, err := p.value.detector()
	if err != nil {
		return err
	}
	defer det.close()
	t := newTracker(det)
	events := append(t.accept(samples), t.finish()...)
	speech := []fields{}
	for _, part := range speechParts(events) {
		speech = append(speech, fields{"start_ms": part[0], "end_ms": part[1]})
	}
	e.out.OK(r.ID, fields{"speech": speech})
	return nil
}

// diarize finds who speaks when in a WAV file.
func (e *Engine) diarize(r *Request) error {
	model, err := r.String("model")
	if err != nil {
		return err
	}
	path, err := r.String("path")
	if err != nil {
		return err
	}
	var speakers uint64
	if raw, ok := r.OptionalValue("speakers"); ok {
		n, err := parseUint(raw)
		if err != nil || n < 1 {
			return errors.New("speakers must be an integer of 1 or more")
		}
		speakers = min(n, 1<<32-1)
	}
	p, err := e.diarizerPart(model)
	if err != nil {
		return err
	}
	defer p.done()
	samples, err := readWAV(path)
	if err != nil {
		return err
	}
	audioMs := uint64(len(samples) * 1000 / SampleRate)
	var raw []rawTurn
	if len(samples) > 0 {
		if raw, err = p.value.diarize(samples, uint32(speakers)); err != nil {
			return err
		}
	}
	count, turns := finishDiarization(raw, audioMs)
	e.out.OK(r.ID, fields{"speakers": count, "turns": turns})
	return nil
}

func parseUint(raw json.RawMessage) (uint64, error) {
	return strconv.ParseUint(strings.TrimSpace(string(raw)), 10, 64)
}

// checkTask fails when the model does another task than t.
func checkTask(model string, spec *modelSpec, t task) error {
	if actual := spec.recipe.task(); actual != t {
		return fmt.Errorf("model %s is a %s model, not a %s model", model, actual.name(), t.name())
	}
	return nil
}

// checkFiles checks that each archive holds the files its recipe needs.
func checkFiles(spec *modelSpec, dirs *modelDirs) error {
	// A GGUF or GGML model is one file, checked by its size as it downloads.
	if spec.recipe.finalPass() && !spec.recipe.singleFile() {
		if _, err := findOfflineFiles(spec.recipe, dirs.main); err != nil {
			return err
		}
	}
	if dirs.stream != "" {
		if _, err := findOnlineFiles(dirs.stream); err != nil {
			return err
		}
	}
	switch spec.source.kind {
	case sourceSherpaFile:
		if _, err := vadFile(dirs.main, spec.source.file); err != nil {
			return err
		}
	case sourceDiarization:
		if dirs.embedding == "" {
			return errors.New("a diarization model has an embedding folder")
		}
		if _, err := findDiarizationFiles(dirs.main, dirs.embedding, spec.source.file); err != nil {
			return err
		}
	}
	return nil
}

// maxStreams is the most streams open at the same time, on all models
// together. The protocol asks for at least 3.
const maxStreams = 8

// session is a stream: a live preview or a VAD session.
type session struct {
	// Preview.
	stream     previewStream
	partials   partials
	lastDecode time.Time
	preview    part[livePreview]
	// VAD.
	tracker  *tracker
	detector detector
	vad      part[vadModel]
	isVad    bool
}

func (s *session) close() {
	if s.isVad {
		s.detector.close()
		s.vad.done()
		return
	}
	s.stream.close()
	s.preview.done()
}

// streamWorker runs the streaming requests in arrival order.
type streamWorker struct {
	engine   *Engine
	sessions map[uint64]*session
}

func (w *streamWorker) handle(r *Request) {
	if err := w.run(r); err != nil {
		w.engine.out.Failure(err, r.ID)
	}
}

// reserve makes room for a session. A session id that is open already starts
// again. The limit counts preview streams and VAD sessions together.
func (w *streamWorker) reserve(id uint64) error {
	if old := w.sessions[id]; old != nil {
		old.close()
		delete(w.sessions, id)
	}
	if len(w.sessions) >= maxStreams {
		return &codedError{"too_many_streams", fmt.Sprintf("%d streams are open; end one first", maxStreams)}
	}
	return nil
}

func (w *streamWorker) run(r *Request) (err error) {
	defer catchPanic("the stream worker", &err)
	id, err := r.Uint("session")
	if err != nil {
		return err
	}
	out := w.engine.out
	switch r.Kind {
	case "stream_start":
		model, err := r.String("model")
		if err != nil {
			return err
		}
		p, err := w.engine.previewPart(model)
		if err != nil {
			return err
		}
		if err := w.reserve(id); err != nil {
			p.done()
			return err
		}
		stream, err := p.value.start()
		if err != nil {
			p.done()
			return err
		}
		w.sessions[id] = &session{stream: stream, lastDecode: time.Now(), preview: p}
		out.OK(r.ID, nil)
	case "vad_start":
		model, err := r.String("model")
		if err != nil {
			return err
		}
		p, err := w.engine.vadPart(model)
		if err != nil {
			return err
		}
		if err := w.reserve(id); err != nil {
			p.done()
			return err
		}
		det, err := p.value.detector()
		if err != nil {
			p.done()
			return err
		}
		w.sessions[id] = &session{isVad: true, tracker: newTracker(det), detector: det, vad: p}
		out.OK(r.ID, nil)
	case "stream_audio":
		s := w.sessions[id]
		if s == nil {
			return fmt.Errorf("unknown stream session %d", id)
		}
		pcm, err := r.String("pcm")
		if err != nil {
			return err
		}
		samples, err := decodePCM(pcm)
		if err != nil {
			return err
		}
		if s.isVad {
			sendSpeech(out, id, s.tracker.accept(samples))
			return nil
		}
		s.stream.accept(samples)
		if time.Since(s.lastDecode) >= w.engine.config.decodeInterval {
			s.lastDecode = time.Now()
			// After an ended segment, ask again at once: a stream can end a
			// segment and start the next one in the same decode.
			for {
				seg := s.stream.decode()
				s.partials.update(seg)
				if !seg.endpoint {
					break
				}
			}
			sendPartial(out, id, &s.partials)
		}
	case "stream_end":
		s := w.sessions[id]
		if s == nil {
			return fmt.Errorf("unknown stream session %d", id)
		}
		delete(w.sessions, id)
		defer s.close()
		if s.isVad {
			sendSpeech(out, id, s.tracker.finish())
			out.OK(r.ID, fields{"session": id})
			return nil
		}
		s.partials.update(s.stream.finish())
		sendPartial(out, id, &s.partials)
		out.OK(r.ID, fields{"session": id, "text": s.partials.text()})
	default:
		return fmt.Errorf("unknown request type %s", r.Kind)
	}
	return nil
}

func sendPartial(out *Output, id uint64, p *partials) {
	if committed, tentative, ok := p.changed(); ok {
		out.Send("partial", nil, fields{"session": id, "committed": committed, "tentative": tentative})
	}
}

func sendSpeech(out *Output, id uint64, events []vadEvent) {
	for _, e := range events {
		if e.isEnd {
			out.Send("speech_end", nil, fields{"session": id, "start_ms": e.startMs, "end_ms": e.endMs})
		} else {
			out.Send("speech_start", nil, fields{"session": id, "start_ms": e.startMs})
		}
	}
}

// serve reads requests until the input closes or a shutdown request arrives.
func (e *Engine) serve(input io.Reader) {
	streams := make(chan *Request, 256)
	go func() {
		w := &streamWorker{engine: e, sessions: map[uint64]*session{}}
		for r := range streams {
			w.handle(r)
		}
	}()
	// The process ends after serve; a decode that runs now ends unanswered.
	defer close(streams)
	reader := bufio.NewReaderSize(input, 1<<20)
	for {
		line, err := reader.ReadString('\n')
		if strings.TrimSpace(line) != "" {
			if e.serveLine(line, streams) {
				return
			}
		}
		if err != nil {
			return
		}
	}
}

// serveLine handles one line. True after shutdown.
func (e *Engine) serveLine(line string, streams chan<- *Request) bool {
	r, err := ParseRequest(strings.TrimRight(line, "\r\n"))
	if err != nil {
		e.out.Error(err.Error(), nil)
		return false
	}
	switch r.Kind {
	case "stream_start", "vad_start", "stream_audio", "stream_end":
		streams <- r
	case "shutdown":
		e.out.OK(r.ID, nil)
		return true
	case "hello", "unload":
		// hello replies immediately so piped one-shot checks don't exit on EOF
		// before the reply writes; unload runs in arrival order so a load that
		// follows does not find the model.
		e.handle(r)
	default:
		go e.handle(r)
	}
	return false
}
