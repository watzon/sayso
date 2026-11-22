package localengine

import (
	"errors"
	"fmt"
	"sync"
	"sync/atomic"
)

// The engine talks to models only through these interfaces. sherpa.go
// implements them with sherpa-onnx, whisper.cpp and parakeet.cpp add final
// passes, and redecode.go makes a live preview from a final pass. The tests
// use fakes.

// modelDirs are the folders of a model's extracted archives.
type modelDirs struct {
	main string
	// stream is the archive of the live preview, or "".
	stream string
	// embedding is the speaker embedding model of a diarization model, or "".
	embedding string
	// storeUse keeps the files of a model in the model store.
	storeUse *storeUse
}

// storeUse holds the use locks of a loaded model in the model store while
// something holds it. When the last holder lets go, a cleanup removes the
// artifacts of a model that another engine deleted in the meantime.
type storeUse struct {
	refs  atomic.Int64
	lease *lease
	store *modelStore
}

func newStoreUse(l *lease, s *modelStore) *storeUse {
	u := &storeUse{lease: l, store: s}
	u.refs.Store(1)
	return u
}

// retain adds a holder. Safe on nil.
func (u *storeUse) retain() *storeUse {
	if u != nil {
		u.refs.Add(1)
	}
	return u
}

// release lets one holder go. Safe on nil.
func (u *storeUse) release() {
	if u == nil || u.refs.Add(-1) != 0 {
		return
	}
	u.lease.release()
	go func() { _ = u.store.cleanup() }()
}

// loader loads models. Loading is slow, so the engine calls it on its own goroutine.
type loader interface {
	load(spec *modelSpec, dirs *modelDirs) (*loaded, error)
	// loadFinalPass loads only the final pass, for a model whose live preview
	// is in memory already. Nil when the recipe has no final pass.
	loadFinalPass(spec *modelSpec, dirs *modelDirs) (finalPass, error)
}

// loaded is a model in memory. A speech model has a final pass, a live
// preview, or both. A VAD model has vad, a diarization model diarizer.
type loaded struct {
	finalPass finalPass
	preview   livePreview
	vad       vadModel
	diarizer  diarizer
	storeUse  *storeUse
	// free releases native memory when the model leaves the engine. Can be nil.
	free func()
}

// vadModel makes one detector for each audio.
type vadModel interface {
	detector() (detector, error)
}

// diarizer finds who speaks when.
type diarizer interface {
	// diarize 16 kHz mono audio. speakers is 0 when the client does not know it.
	diarize(samples []float32, speakers uint32) ([]rawTurn, error)
}

// finalPass turns whole recordings into text.
type finalPass interface {
	// transcribe one piece of audio. language is a code or auto. A model that
	// cannot use vocabulary ignores it. times asks for a decode with times
	// when a model needs a slower one for them.
	transcribe(samples []float32, language string, vocabulary []string, times bool) (timed, error)
}

// livePreview is a streaming model.
type livePreview interface {
	start() (previewStream, error)
}

// previewStream is one stream of a live preview.
type previewStream interface {
	// accept adds audio. It must be fast: it does not decode.
	accept(samples []float32)
	// decode decodes the audio that is ready and returns the current segment.
	// After a segment with endpoint, the engine calls decode again at once.
	decode() segment
	// finish decodes the rest and returns the last segment, with endpoint set.
	finish() segment
	// close frees the stream.
	close()
}

// maxHeldSamples is the most audio a lazy stream holds while its model loads: a minute.
const maxHeldSamples = SampleRate * 60

type slotState int

const (
	slotIdle slotState = iota
	slotLoading
	slotReady
	slotFailed
)

// lazyPreview loads its streaming model when the first stream starts, on its
// own goroutine. start does not wait: the stream holds the audio and decodes
// it when the model is in memory.
type lazyPreview struct {
	loadFn func() (livePreview, error)
	// refs frees the loaded streaming model when the last holder lets go.
	refs    *refs
	mu      sync.Mutex
	state   slotState
	ready   livePreview
	failure string
}

func newLazyPreview(load func() (livePreview, error)) *lazyPreview {
	return &lazyPreview{loadFn: load}
}

func (p *lazyPreview) isLoaded() bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.state == slotReady
}

func (p *lazyPreview) spawnLoad() {
	go func() {
		var result livePreview
		var err error
		func() {
			defer func() {
				if r := recover(); r != nil {
					err = errors.New("the streaming model load stopped unexpectedly")
				}
			}()
			result, err = p.loadFn()
		}()
		p.mu.Lock()
		defer p.mu.Unlock()
		if err != nil {
			p.state, p.failure = slotFailed, err.Error()
		} else {
			p.state, p.ready = slotReady, result
		}
	}()
}

func (p *lazyPreview) start() (previewStream, error) {
	p.mu.Lock()
	switch p.state {
	case slotFailed:
		err := errors.New(p.failure)
		p.state = slotIdle
		p.mu.Unlock()
		return nil, err
	case slotIdle:
		p.state = slotLoading
		p.spawnLoad()
	}
	p.mu.Unlock()
	return &lazyStream{owner: p}, nil
}

// readyPreview is the model when it is in memory.
func (p *lazyPreview) readyPreview() livePreview {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.state == slotReady {
		return p.ready
	}
	return nil
}

// lazyStream waits for its model. It gives empty text until the model is in memory.
type lazyStream struct {
	owner *lazyPreview
	inner previewStream
	held  []float32
	lost  bool
}

func (s *lazyStream) attach() bool {
	if s.inner != nil {
		return true
	}
	if s.lost {
		return false
	}
	preview := s.owner.readyPreview()
	if preview == nil {
		return false
	}
	stream, err := preview.start()
	if err != nil {
		s.lost, s.held = true, nil
		return false
	}
	stream.accept(s.held)
	s.held, s.inner = nil, stream
	return true
}

func (s *lazyStream) accept(samples []float32) {
	if s.attach() {
		s.inner.accept(samples)
	} else if !s.lost && len(s.held)+len(samples) <= maxHeldSamples {
		s.held = append(s.held, samples...)
	}
}

func (s *lazyStream) decode() segment {
	if s.attach() {
		return s.inner.decode()
	}
	return segment{}
}

func (s *lazyStream) finish() segment {
	// A stream that ends before its model loads gives no text. The final pass
	// does not use the preview, and the stream worker must not wait.
	if s.attach() {
		return s.inner.finish()
	}
	return segment{endpoint: true}
}

func (s *lazyStream) close() {
	if s.inner != nil {
		s.inner.close()
	}
}

// errNoGPU is the reason no_gpu: the load of a GPU model found no discrete GPU.
var errNoGPU = errors.New("this model needs a discrete GPU with a Vulkan driver, and none was found")

// catchPanic turns a panic in a native call path into an error.
func catchPanic(what string, err *error) {
	if r := recover(); r != nil {
		*err = fmt.Errorf("%s stopped unexpectedly: %v", what, r)
	}
}
