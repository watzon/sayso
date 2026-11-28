package device

// Microphone capture on macOS with an AudioQueue input. The queue converts
// the device format to 16 kHz mono float32 itself. Its callbacks run on a
// locked OS thread with its own CFRunLoop. Devices come from CoreAudio, by
// their UID.

import (
	"context"
	"errors"
	"fmt"
	"math"
	"runtime"
	"sync"
	"time"
	"unsafe"

	"github.com/ebitengine/purego"
)

type audioStreamBasicDescription struct {
	SampleRate       float64
	FormatID         uint32
	FormatFlags      uint32
	BytesPerPacket   uint32
	FramesPerPacket  uint32
	BytesPerFrame    uint32
	ChannelsPerFrame uint32
	BitsPerChannel   uint32
	Reserved         uint32
}

type audioObjectPropertyAddress struct {
	Selector uint32
	Scope    uint32
	Element  uint32
}

// audioQueueBuffer is the head of AudioQueueBuffer.
type audioQueueBuffer struct {
	capacity uint32
	data     unsafe.Pointer
	size     uint32
}

const (
	kAudioFormatLinearPCM         = 0x6C70636D // 'lpcm'
	kAudioFormatFlagIsFloat       = 1
	kAudioFormatFlagIsPacked      = 8
	kAudioQueuePropertyCurrentDev = 0x61716364 // 'aqcd'

	kAudioObjectSystemObject        = 1
	kAudioHardwarePropertyDevices   = 0x64657623 // 'dev#'
	kAudioHardwarePropertyDefaultIn = 0x64496E20 // 'dIn '
	kAudioObjectPropertyScopeGlobal = 0x676C6F62 // 'glob'
	kAudioObjectPropertyScopeInput  = 0x696E7074 // 'inpt'
	kAudioObjectPropertyElementMain = 0
	kAudioDevicePropertyDeviceUID   = 0x75696420 // 'uid '
	kAudioObjectPropertyName        = 0x6C6E616D // 'lnam'
	kAudioDevicePropertyStreams     = 0x73746D23 // 'stm#'

	// queueBuffers of queueBufferFrames each: 3 x 20 ms.
	queueBuffers      = 3
	queueBufferFrames = frameSamples
)

type macAudio struct {
	mu  sync.Mutex
	cur *macCapture
}

type macCapture struct {
	queue   uintptr
	runLoop uintptr
	done    chan struct{}
	framer  *audioFramer
	cancel  context.CancelFunc
}

var (
	audioCallbackOnce sync.Once
	audioCallback     uintptr
	capturesMu        sync.Mutex
	captures          = map[uintptr]*macCapture{}
	nextCapture       uintptr
)

func getProperty[T any](object uint32, selector, scope uint32, out *T) error {
	addr := audioObjectPropertyAddress{selector, scope, kAudioObjectPropertyElementMain}
	size := uint32(unsafe.Sizeof(*out))
	if st := audioObjectGetPropertyData(object, &addr, 0, 0, &size, unsafe.Pointer(out)); st != 0 {
		return fmt.Errorf("CoreAudio property %#x: status %d", selector, st)
	}
	return nil
}

func propertySize(object uint32, selector, scope uint32) uint32 {
	addr := audioObjectPropertyAddress{selector, scope, kAudioObjectPropertyElementMain}
	var size uint32
	if audioObjectGetPropertyDataSize(object, &addr, 0, 0, &size) != 0 {
		return 0
	}
	return size
}

func deviceString(device uint32, selector uint32) string {
	var s uintptr
	if getProperty(device, selector, kAudioObjectPropertyScopeGlobal, &s) != nil || s == 0 {
		return ""
	}
	defer cfRelease(s)
	return goStringFromCF(s)
}

func (*macAudio) ListDevices() ([]Device, error) {
	if err := loadDarwin(); err != nil {
		return nil, err
	}
	var defaultID uint32
	_ = getProperty(kAudioObjectSystemObject, kAudioHardwarePropertyDefaultIn, kAudioObjectPropertyScopeGlobal, &defaultID)
	size := propertySize(kAudioObjectSystemObject, kAudioHardwarePropertyDevices, kAudioObjectPropertyScopeGlobal)
	if size == 0 {
		return nil, nil
	}
	ids := make([]uint32, size/4)
	addr := audioObjectPropertyAddress{kAudioHardwarePropertyDevices, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain}
	if st := audioObjectGetPropertyData(kAudioObjectSystemObject, &addr, 0, 0, &size, unsafe.Pointer(&ids[0])); st != 0 {
		return nil, fmt.Errorf("CoreAudio devices: status %d", st)
	}
	ids = ids[:size/4]
	var out []Device
	for _, id := range ids {
		// Input devices have at least one input stream.
		if propertySize(id, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput) == 0 {
			continue
		}
		uid := deviceString(id, kAudioDevicePropertyDeviceUID)
		if uid == "" {
			continue
		}
		name := deviceString(id, kAudioObjectPropertyName)
		if name == "" {
			name = uid
		}
		out = append(out, Device{ID: uid, Name: name, IsDefault: id == defaultID})
	}
	return out, nil
}

func (a *macAudio) Start(ctx context.Context, deviceID string, onChunk func(Chunk)) error {
	if onChunk == nil {
		return errors.New("audio: no chunk handler")
	}
	if err := loadDarwin(); err != nil {
		return err
	}
	// A denied microphone gives silence instead of an error, so check first.
	switch microphoneAuthorization() {
	case micDenied, micRestricted:
		return fmt.Errorf("audio: %w", ErrDenied)
	}
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.cur != nil {
		return errors.New("audio: capture already running")
	}
	audioCallbackOnce.Do(func() { audioCallback = purego.NewCallback(audioTrampoline) })

	c := &macCapture{done: make(chan struct{}), framer: newAudioFramer(onChunk)}
	capturesMu.Lock()
	nextCapture++
	handle := nextCapture
	captures[handle] = c
	capturesMu.Unlock()
	forget := func() {
		capturesMu.Lock()
		delete(captures, handle)
		capturesMu.Unlock()
	}

	ready := make(chan error, 1)
	go func() {
		runtime.LockOSThread()
		defer runtime.UnlockOSThread()
		defer close(c.done)
		if err := c.open(handle, NormalizeDeviceID(deviceID)); err != nil {
			ready <- err
			return
		}
		ready <- nil
		cfRunLoopRun()
		c.framer.close()
	}()
	if err := <-ready; err != nil {
		<-c.done
		forget()
		return err
	}
	runCtx, cancel := context.WithCancel(ctx)
	c.cancel = cancel
	a.cur = c
	go func() {
		<-runCtx.Done()
		// A synchronous stop or dispose waits for the queue's run loop; once
		// that loop has ended, each one waited out a 500 ms timeout.
		audioQueueStop(c.queue, true)
		audioQueueDispose(c.queue, true)
		stopRunLoop(c.runLoop, c.done)
		forget()
		a.mu.Lock()
		if a.cur == c {
			a.cur = nil
		}
		a.mu.Unlock()
	}()
	return nil
}

// open makes, configures, and starts the queue on the calling thread.
func (c *macCapture) open(handle uintptr, deviceID string) error {
	format := audioStreamBasicDescription{
		SampleRate:       targetRate,
		FormatID:         kAudioFormatLinearPCM,
		FormatFlags:      kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked,
		BytesPerPacket:   4,
		FramesPerPacket:  1,
		BytesPerFrame:    4,
		ChannelsPerFrame: 1,
		BitsPerChannel:   32,
	}
	c.runLoop = cfRunLoopGetCurrent()
	if st := audioQueueNewInput(&format, audioCallback, handle, c.runLoop, kCFRunLoopCommonModes, 0, &c.queue); st != 0 {
		return fmt.Errorf("audio: AudioQueueNewInput: status %d", st)
	}
	fail := func(what string, st int32) error {
		audioQueueDispose(c.queue, true)
		return fmt.Errorf("audio: %s: status %d", what, st)
	}
	if deviceID != "" {
		uid := cfString(deviceID)
		st := audioQueueSetProperty(c.queue, kAudioQueuePropertyCurrentDev, uintptr(unsafe.Pointer(&uid)), uint32(unsafe.Sizeof(uid)))
		cfRelease(uid)
		if st != 0 {
			return fail("select device "+deviceID, st)
		}
	}
	for i := 0; i < queueBuffers; i++ {
		var buf uintptr
		if st := audioQueueAllocateBuffer(c.queue, queueBufferFrames*4, &buf); st != 0 {
			return fail("AudioQueueAllocateBuffer", st)
		}
		if st := audioQueueEnqueueBuffer(c.queue, buf, 0, 0); st != 0 {
			return fail("AudioQueueEnqueueBuffer", st)
		}
	}
	if st := audioQueueStart(c.queue, 0); st != 0 {
		return fail("AudioQueueStart", st)
	}
	return nil
}

func audioTrampoline(user, queue uintptr, buffer *audioQueueBuffer, startTime uintptr, packets uint32, desc uintptr) uintptr {
	capturesMu.Lock()
	c := captures[user]
	capturesMu.Unlock()
	if c != nil && buffer.size > 0 {
		samples := unsafe.Slice((*float32)(buffer.data), buffer.size/4)
		c.framer.push(samples)
	}
	audioQueueEnqueueBuffer(queue, uintptr(unsafe.Pointer(buffer)), 0, 0)
	return 0
}

func (a *macAudio) Stop() error {
	a.mu.Lock()
	c := a.cur
	a.mu.Unlock()
	if c == nil {
		return nil
	}
	c.cancel()
	<-c.done
	a.mu.Lock()
	if a.cur == c {
		a.cur = nil
	}
	a.mu.Unlock()
	return nil
}

// ---------------------------------------------------------------------------
// Framing and level, as sayso-capture/src/mic.rs
// ---------------------------------------------------------------------------

const (
	targetRate = 16000
	// frameSamples is one 20 ms frame at 16 kHz.
	frameSamples = 320
	// levelFloorDB maps to 0, and levelFloorDB+levelRangeDB to 1. Normal
	// speech (about -26 dBFS RMS) gives about 0.67.
	levelFloorDB = -50.0
	levelRangeDB = 36.0
)

func rmsOf(samples []float32) float32 {
	if len(samples) == 0 {
		return 0
	}
	var sum float64
	for _, s := range samples {
		sum += float64(s) * float64(s)
	}
	return float32(math.Sqrt(sum / float64(len(samples))))
}

// displayLevel maps an RMS value to the 0 to 1 range of the waveform.
func displayLevel(rms float32) float32 {
	if rms <= 0 {
		return 0
	}
	l := (20*math.Log10(float64(rms)) - levelFloorDB) / levelRangeDB
	return float32(math.Max(0, math.Min(1, l)))
}

// audioFramer cuts samples into 20 ms chunks and hands them to a worker
// goroutine, so a slow handler never stalls the audio thread.
type audioFramer struct {
	pending []float32
	out     chan Chunk
	done    chan struct{}
}

func newAudioFramer(onChunk func(Chunk)) *audioFramer {
	f := &audioFramer{out: make(chan Chunk, 1024), done: make(chan struct{})}
	go func() {
		defer close(f.done)
		for c := range f.out {
			onChunk(c)
		}
	}()
	return f
}

// push copies samples; the caller may reuse its buffer after.
func (f *audioFramer) push(samples []float32) {
	f.pending = append(f.pending, samples...)
	for len(f.pending) >= frameSamples {
		frame := make([]float32, frameSamples)
		copy(frame, f.pending[:frameSamples])
		f.pending = f.pending[frameSamples:]
		chunk := Chunk{Samples: frame, RMS: displayLevel(rmsOf(frame)), Time: time.Now()}
		select {
		case f.out <- chunk:
		default: // The handler is 20 s behind: drop rather than block audio.
		}
	}
	if cap(f.pending) > 4*frameSamples && len(f.pending) < frameSamples {
		f.pending = append([]float32(nil), f.pending...)
	}
}

// close ends the worker after it handled the queued chunks.
func (f *audioFramer) close() {
	close(f.out)
	<-f.done
}
