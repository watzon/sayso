//go:build linux

package localengine

import (
	"fmt"
	"runtime"
	"strings"
	"sync"

	"github.com/ebitengine/purego"
)

// The final pass of parakeet_cpp models: a Parakeet TDT GGUF on parakeet.cpp
// (ggml), on the GPU through Vulkan, from libparakeet.so (parakeet.cpp's
// include/parakeet_capi.h). Without a discrete GPU such a model refuses to load
// with errNoGPU.

type parakeetAPI struct {
	load          func(path *byte) uintptr
	loadError     func() *byte
	free          func(ctx uintptr)
	transcribePCM func(ctx uintptr, samples *float32, n, rate, decoder int32) uintptr
	lastError     func(ctx uintptr) *byte
	freeString    func(s uintptr)
	devCount      func() uintptr
	devGet        func(i uintptr) uintptr
	devType       func(dev uintptr) int32
}

var (
	parakeetOnce   sync.Once
	parakeetLoaded *parakeetAPI
	parakeetErr    error
)

func loadParakeetAPI() (*parakeetAPI, error) {
	parakeetOnce.Do(func() {
		h, err := openLibrary([]string{"libparakeet.so"})
		if err != nil {
			parakeetErr = fmt.Errorf("cannot load parakeet.cpp: %w", err)
			return
		}
		api := &parakeetAPI{}
		for _, f := range []struct {
			fn   any
			name string
		}{
			{&api.load, "parakeet_capi_load"},
			{&api.loadError, "parakeet_capi_load_error"},
			{&api.free, "parakeet_capi_free"},
			{&api.transcribePCM, "parakeet_capi_transcribe_pcm"},
			{&api.lastError, "parakeet_capi_last_error"},
			{&api.freeString, "parakeet_capi_free_string"},
		} {
			if _, err := purego.Dlsym(h, f.name); err != nil {
				parakeetErr = fmt.Errorf("cannot load parakeet.cpp: %s: %w", f.name, err)
				return
			}
			purego.RegisterLibFunc(f.fn, h, f.name)
		}
		// ggml's device registry: in libparakeet.so when ggml is linked in, else libggml.so.
		for _, gh := range []uintptr{h, openOptional("libggml.so")} {
			if gh == 0 {
				continue
			}
			if _, err := purego.Dlsym(gh, "ggml_backend_dev_count"); err != nil {
				continue
			}
			purego.RegisterLibFunc(&api.devCount, gh, "ggml_backend_dev_count")
			purego.RegisterLibFunc(&api.devGet, gh, "ggml_backend_dev_get")
			purego.RegisterLibFunc(&api.devType, gh, "ggml_backend_dev_type")
			break
		}
		parakeetLoaded = api
	})
	return parakeetLoaded, parakeetErr
}

func openOptional(name string) uintptr {
	h, err := openLibrary([]string{name})
	if err != nil {
		return 0
	}
	return h
}

func (a *parakeetAPI) hasGPU() bool {
	if a.devCount == nil {
		return false
	}
	var types []int32
	for i := uintptr(0); i < a.devCount(); i++ {
		types = append(types, a.devType(a.devGet(i)))
	}
	return firstGPUIsDedicated(types)
}

// parakeet is a context behind a mutex: the library queues concurrent calls
// but keeps only the latest error, so the lock makes lastError this call's own.
type parakeet struct {
	api  *parakeetAPI
	mu   sync.Mutex
	ctx  uintptr
	refs *refs
}

func loadParakeet(file string) (finalPass, error) {
	api, err := loadParakeetAPI()
	if err != nil {
		return nil, err
	}
	// parakeet.cpp would fall back to an integrated GPU or the CPU, where a
	// sherpa-onnx model is the better choice.
	if !api.hasGPU() {
		return nil, errNoGPU
	}
	if strings.IndexByte(file, 0) >= 0 {
		return nil, fmt.Errorf("the model path %s has a NUL byte", file)
	}
	var held cStrings
	// The load error is per thread: load and read it on one.
	runtime.LockOSThread()
	ctx := api.load(held.add(file))
	var reason string
	if ctx == 0 {
		reason = goString(api.loadError())
	}
	runtime.UnlockOSThread()
	runtime.KeepAlive(&held)
	if ctx == 0 {
		return nil, fmt.Errorf("parakeet.cpp cannot load %s: %s", file, reason)
	}
	p := &parakeet{api: api, ctx: ctx}
	p.refs = newRefs(p.close)
	// ggml-vulkan builds its GPU pipelines on first use, per kind of work.
	// These three lengths build every pipeline the model uses, so later first
	// decodes cost what repeats do.
	for _, seconds := range []int{1, 30, 60} {
		if _, err := p.transcribe(make([]float32, SampleRate*seconds), "en", nil, false); err != nil {
			p.release()
			return nil, err
		}
	}
	return p, nil
}

func (p *parakeet) retain()  { p.refs.retain() }
func (p *parakeet) release() { p.refs.release() }

func (p *parakeet) close() {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.ctx != 0 {
		p.api.free(p.ctx)
		p.ctx = 0
	}
}

func (p *parakeet) transcribe(samples []float32, _ string, _ []string, _ bool) (timed, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.ctx == 0 {
		return timed{}, fmt.Errorf("parakeet.cpp: the model was unloaded")
	}
	if len(samples) > 1<<31-1 {
		return timed{}, fmt.Errorf("the piece of audio is too long")
	}
	// decoder 0 is the model's own head, the transducer for TDT.
	text := p.api.transcribePCM(p.ctx, samplesPtr(samples), int32(len(samples)), SampleRate, 0)
	runtime.KeepAlive(samples)
	if text == 0 {
		return timed{}, fmt.Errorf("parakeet.cpp: %s", goString(p.api.lastError(p.ctx)))
	}
	result := strings.TrimSpace(goString((*byte)(cMem(text))))
	p.api.freeString(text)
	return untimed(result), nil
}
