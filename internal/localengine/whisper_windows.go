//go:build windows

package localengine

import (
	"errors"
	"fmt"
	"os"
	"runtime"
	"sync"
	"syscall"
	"unsafe"

	"golang.org/x/sys/windows"
)

// The final pass of whisper_cpp models on Windows: one Whisper GGML file on
// whisper.dll (whisper.cpp v1.8.3), loaded at run time. whisper.cpp takes its
// param structs by value; the Windows x64 ABI passes a struct larger than 8
// bytes as a pointer to a copy, so the binding passes the structs that the
// *_by_ref functions return.

type whisperAPI struct {
	contextDefaultParams uintptr
	initFromFile         uintptr
	initState            uintptr
	free                 uintptr
	freeState            uintptr
	freeParams           uintptr
	freeContextParams    uintptr
	fullDefaultParams    uintptr
	fullWithState        uintptr
	nSegments            uintptr
	segmentT0            uintptr
	segmentT1            uintptr
	segmentText          uintptr
	langID               uintptr
	langStr              uintptr
	// ggml's device list, for the GPU choice; zero when not exported.
	devCount, devGet, devType, devMemory uintptr
}

var (
	whisperOnce   sync.Once
	whisperLoaded *whisperAPI
	whisperErr    error
)

func loadWhisperAPI() (*whisperAPI, error) {
	whisperOnce.Do(func() {
		h, err := openLibrary([]string{"whisper.dll"})
		if err != nil {
			whisperErr = fmt.Errorf("cannot load whisper.cpp: %w", err)
			return
		}
		lib := windows.Handle(h)
		api := &whisperAPI{}
		for _, f := range []struct {
			p    *uintptr
			name string
		}{
			{&api.contextDefaultParams, "whisper_context_default_params_by_ref"},
			{&api.initFromFile, "whisper_init_from_file_with_params"},
			{&api.initState, "whisper_init_state"},
			{&api.free, "whisper_free"},
			{&api.freeState, "whisper_free_state"},
			{&api.freeParams, "whisper_free_params"},
			{&api.freeContextParams, "whisper_free_context_params"},
			{&api.fullDefaultParams, "whisper_full_default_params_by_ref"},
			{&api.fullWithState, "whisper_full_with_state"},
			{&api.nSegments, "whisper_full_n_segments_from_state"},
			{&api.segmentT0, "whisper_full_get_segment_t0_from_state"},
			{&api.segmentT1, "whisper_full_get_segment_t1_from_state"},
			{&api.segmentText, "whisper_full_get_segment_text_from_state"},
			{&api.langID, "whisper_full_lang_id_from_state"},
			{&api.langStr, "whisper_lang_str"},
		} {
			p, err := windows.GetProcAddress(lib, f.name)
			if err != nil {
				whisperErr = fmt.Errorf("cannot load whisper.cpp: %s: %w", f.name, err)
				return
			}
			*f.p = p
		}
		// ggml's device registry is in ggml.dll next to whisper.dll, or in
		// whisper.dll itself when ggml is linked statically.
		for _, name := range []string{"ggml.dll", "whisper.dll"} {
			gh, err := openLibrary([]string{name})
			if err != nil {
				continue
			}
			g := windows.Handle(gh)
			count, e1 := windows.GetProcAddress(g, "ggml_backend_dev_count")
			get, e2 := windows.GetProcAddress(g, "ggml_backend_dev_get")
			typ, e3 := windows.GetProcAddress(g, "ggml_backend_dev_type")
			mem, e4 := windows.GetProcAddress(g, "ggml_backend_dev_memory")
			if e1 == nil && e2 == nil && e3 == nil && e4 == nil {
				api.devCount, api.devGet, api.devType, api.devMemory = count, get, typ, mem
				break
			}
		}
		whisperLoaded = api
	})
	return whisperLoaded, whisperErr
}

// gpuDevice is the index, among GPUs, of the best one: a dedicated GPU before
// an integrated one, then the most memory (transcribe-rs
// auto_select_gpu_device). ok is false when there is no GPU.
func (a *whisperAPI) gpuDevice() (index int32, ok bool) {
	if a.devCount == 0 {
		return 0, false
	}
	n, _, _ := syscall.SyscallN(a.devCount)
	var gpus []gpuInfo
	for i := uintptr(0); i < n; i++ {
		dev, _, _ := syscall.SyscallN(a.devGet, i)
		if dev == 0 {
			continue
		}
		t, _, _ := syscall.SyscallN(a.devType, dev)
		if int32(t) != ggmlDeviceGPU && int32(t) != ggmlDeviceIGPU {
			continue
		}
		var free, total uintptr
		syscall.SyscallN(a.devMemory, dev, uintptr(unsafe.Pointer(&free)), uintptr(unsafe.Pointer(&total)))
		gpus = append(gpus, gpuInfo{dedicated: int32(t) == ggmlDeviceGPU, vram: uint64(total)})
	}
	return bestGPU(gpus)
}

type whisperCpp struct {
	api     *whisperAPI
	mu      sync.Mutex
	ctx     uintptr
	state   uintptr
	threads int32
	refs    *refs
}

func loadWhisperCpp(file string) (finalPass, error) {
	if _, err := os.Stat(file); err != nil {
		return nil, fmt.Errorf("cannot load Whisper from %s: the file is missing", file)
	}
	api, err := loadWhisperAPI()
	if err != nil {
		return nil, err
	}
	path, err := windows.BytePtrFromString(file)
	if err != nil {
		return nil, fmt.Errorf("the model path %s has a NUL byte", file)
	}
	cp, _, _ := syscall.SyscallN(api.contextDefaultParams)
	if cp == 0 {
		return nil, errors.New("cannot load Whisper: whisper.cpp gave no context params")
	}
	params := (*whisperContextParams)(cMem(cp))
	device, gpu := api.gpuDevice()
	params.UseGpu = gpu
	params.FlashAttn = true
	params.GpuDevice = device
	ctx, _, _ := syscall.SyscallN(api.initFromFile, uintptr(unsafe.Pointer(path)), cp)
	runtime.KeepAlive(path)
	syscall.SyscallN(api.freeContextParams, cp)
	if ctx == 0 {
		return nil, fmt.Errorf("cannot load Whisper from %s: whisper.cpp could not read it", file)
	}
	state, _, _ := syscall.SyscallN(api.initState, ctx)
	if state == 0 {
		syscall.SyscallN(api.free, ctx)
		return nil, fmt.Errorf("cannot load Whisper from %s: whisper.cpp could not make a state", file)
	}
	w := &whisperCpp{api: api, ctx: ctx, state: state, threads: int32(min(max(runtime.NumCPU(), 1), 8))}
	w.refs = newRefs(w.close)
	return w, nil
}

func (w *whisperCpp) retain()  { w.refs.retain() }
func (w *whisperCpp) release() { w.refs.release() }

func (w *whisperCpp) close() {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.state != 0 {
		syscall.SyscallN(w.api.freeState, w.state)
		w.state = 0
	}
	if w.ctx != 0 {
		syscall.SyscallN(w.api.free, w.ctx)
		w.ctx = 0
	}
}

// transcribe gives segments with times (10 ms steps) and the language that
// whisper.cpp found, at no extra cost.
func (w *whisperCpp) transcribe(samples []float32, language string, vocabulary []string, _ bool) (timed, error) {
	if whisperIsSilence(samples) {
		return untimed(""), nil
	}
	input := whisperPadded(samples)
	var held cStrings
	var lang, prompt *byte
	if code := whisperLanguage(language); code != "" {
		lang = held.add(code)
	}
	if p := whisperPrompt(vocabulary); p != "" {
		prompt = held.add(p)
	}

	w.mu.Lock()
	defer w.mu.Unlock()
	if w.state == 0 {
		return timed{}, errors.New("Whisper failed: the model was unloaded")
	}
	pp, _, _ := syscall.SyscallN(w.api.fullDefaultParams, whisperSamplingBeamSearch)
	if pp == 0 {
		return timed{}, errors.New("Whisper failed: whisper.cpp gave no params")
	}
	defer syscall.SyscallN(w.api.freeParams, pp)
	setWhisperOptions((*whisperFullParams)(cMem(pp)), w.threads, lang, prompt)
	rc, _, _ := syscall.SyscallN(w.api.fullWithState, w.ctx, w.state, pp,
		uintptr(unsafe.Pointer(&input[0])), uintptr(len(input)))
	runtime.KeepAlive(input)
	runtime.KeepAlive(&held)
	if int32(rc) != 0 {
		return timed{}, fmt.Errorf("Whisper failed: whisper_full returned %d", int32(rc))
	}
	n, _, _ := syscall.SyscallN(w.api.nSegments, w.state)
	var raw []byte
	var segments []textSegment
	for i := uintptr(0); i < uintptr(int32(n)); i++ {
		p, _, _ := syscall.SyscallN(w.api.segmentText, w.state, i)
		if p == 0 {
			return timed{}, fmt.Errorf("Whisper failed: segment %d is missing", i)
		}
		text := goString((*byte)(cMem(p)))
		t0, _, _ := syscall.SyscallN(w.api.segmentT0, w.state, i)
		t1, _, _ := syscall.SyscallN(w.api.segmentT1, w.state, i)
		raw = append(raw, text...)
		segments = append(segments, textSegment{
			StartMs: uint64(max(int64(t0), 0)) * 10,
			EndMs:   uint64(max(int64(t1), 0)) * 10,
			Text:    cleanWhisperText(text),
		})
	}
	out := timed{text: cleanWhisperText(string(raw))}
	if len(segments) > 0 {
		out.kind = timingSegments
		out.segments = segments
	}
	id, _, _ := syscall.SyscallN(w.api.langID, w.state)
	if ls, _, _ := syscall.SyscallN(w.api.langStr, uintptr(int32(id))); ls != 0 {
		out.language = normalizeLanguage(goString((*byte)(cMem(ls))))
	}
	return out, nil
}
