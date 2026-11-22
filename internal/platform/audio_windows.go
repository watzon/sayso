package platform

import (
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"math"
	"runtime"
	"sync"
	"time"
	"unsafe"

	"golang.org/x/sys/windows"
)

// windowsAudio captures the microphone with WASAPI in shared mode, mixes it to
// mono, resamples it to 16 kHz, and hands it out in 20 ms chunks. The callback
// runs on a goroutine of its own behind a queue, so a slow callback never
// makes the capture drop audio.

const (
	targetRate   = 16_000
	frameSamples = 320 // 20 ms at 16 kHz
	// queueFrames is 20 s of audio between the capture and the callback.
	queueFrames = 1024

	eCapture           = 1
	eConsole           = 0
	deviceStateActive  = 1
	stgmRead           = 0
	vtLPWSTR           = 31
	audclntShareShared = 0
	bufferFlagsSilent  = 0x2
	// bufferDuration is the WASAPI buffer, in 100 ns units: 200 ms.
	bufferDuration = 2_000_000
	pollInterval   = 10 * time.Millisecond
)

var (
	clsidMMDeviceEnumerator = mustGUID("{BCDE0395-E52F-467C-8E3D-C4579291692E}")
	iidIMMDeviceEnumerator  = mustGUID("{A95664D2-9614-4F35-A746-DE8DB63617E6}")
	iidIAudioClient         = mustGUID("{1CB9AD4C-DBFA-4C32-B178-C2F568A703B2}")
	iidIAudioCaptureClient  = mustGUID("{C8ADBD64-E71E-48A0-A4DE-185C395CD317}")
	subtypeFloat            = mustGUID("{00000003-0000-0010-8000-00aa00389b71}")
	subtypePCM              = mustGUID("{00000001-0000-0010-8000-00aa00389b71}")
)

// propertyKey PKEY_Device_FriendlyName.
type propertyKey struct {
	fmtid windows.GUID
	pid   uint32
}

var pkeyDeviceFriendlyName = propertyKey{mustGUID("{a45c254e-df1c-4efd-8020-67d146a850e0}"), 14}

// Vtable slots, counted with IUnknown.
const (
	slotEnumAudioEndpoints      = 3
	slotGetDefaultAudioEndpoint = 4
	slotGetDevice               = 5
	slotCollectionGetCount      = 3
	slotCollectionItem          = 4
	slotDeviceActivate          = 3
	slotDeviceOpenPropertyStore = 4
	slotDeviceGetID             = 5
	slotPropertyStoreGetValue   = 5
	slotClientInitialize        = 3
	slotClientGetMixFormat      = 8
	slotClientStart             = 10
	slotClientStop              = 11
	slotClientGetService        = 14
	slotCaptureGetBuffer        = 3
	slotCaptureReleaseBuffer    = 4
	slotCaptureNextPacketSize   = 5
)

type windowsAudio struct {
	mu     sync.Mutex
	cancel context.CancelFunc
	done   chan struct{}
}

// sampleFormat is how a WASAPI mix format stores one sample.
type sampleFormat struct {
	channels int
	rate     uint32
	float    bool
	bits     int // container bits per sample
	valid    int // valid bits, for 24-in-32
}

// parseWaveFormat reads a WAVEFORMATEX or WAVEFORMATEXTENSIBLE.
func parseWaveFormat(b []byte) (sampleFormat, error) {
	if len(b) < 18 {
		return sampleFormat{}, errors.New("the mix format is too short")
	}
	le := binary.LittleEndian
	tag := le.Uint16(b[0:])
	f := sampleFormat{channels: int(le.Uint16(b[2:])), rate: le.Uint32(b[4:]), bits: int(le.Uint16(b[14:]))}
	f.valid = f.bits
	switch tag {
	case 1: // WAVE_FORMAT_PCM
	case 3: // WAVE_FORMAT_IEEE_FLOAT
		f.float = true
	case 0xFFFE: // WAVE_FORMAT_EXTENSIBLE
		if len(b) < 40 {
			return sampleFormat{}, errors.New("the extensible mix format is too short")
		}
		if v := int(le.Uint16(b[18:])); v > 0 {
			f.valid = v
		}
		var sub windows.GUID
		sub.Data1 = le.Uint32(b[24:])
		sub.Data2 = le.Uint16(b[28:])
		sub.Data3 = le.Uint16(b[30:])
		copy(sub.Data4[:], b[32:40])
		switch sub {
		case subtypeFloat:
			f.float = true
		case subtypePCM:
		default:
			return sampleFormat{}, fmt.Errorf("the mix format %v is not PCM or float", sub)
		}
	default:
		return sampleFormat{}, fmt.Errorf("the mix format tag %#x is not supported", tag)
	}
	if f.channels < 1 || f.rate == 0 || (f.float && f.bits != 32) || (!f.float && f.bits != 16 && f.bits != 24 && f.bits != 32) {
		return sampleFormat{}, fmt.Errorf("the mix format (%d channels, %d Hz, %d bits) is not supported", f.channels, f.rate, f.bits)
	}
	return f, nil
}

// toFloat converts interleaved raw samples to float32 in [-1, 1].
func (f sampleFormat) toFloat(raw []byte, out []float32) []float32 {
	out = out[:0]
	le := binary.LittleEndian
	switch {
	case f.float:
		for i := 0; i+4 <= len(raw); i += 4 {
			out = append(out, math.Float32frombits(le.Uint32(raw[i:])))
		}
	case f.bits == 16:
		for i := 0; i+2 <= len(raw); i += 2 {
			out = append(out, float32(int16(le.Uint16(raw[i:])))/32768)
		}
	case f.bits == 24:
		for i := 0; i+3 <= len(raw); i += 3 {
			v := int32(uint32(raw[i])<<8|uint32(raw[i+1])<<16|uint32(raw[i+2])<<24) >> 8
			out = append(out, float32(v)/8388608)
		}
	default:
		for i := 0; i+4 <= len(raw); i += 4 {
			out = append(out, float32(int32(le.Uint32(raw[i:])))/2147483648)
		}
	}
	return out
}

// pipeline turns raw captured audio into 20 ms 16 kHz mono chunks.
type pipeline struct {
	format    sampleFormat
	resampler *streamResampler
	interl    []float32
	mono      []float32
	resampled []float32
	pending   []float32
}

func newPipeline(f sampleFormat) *pipeline {
	return &pipeline{format: f, resampler: newStreamResampler(f.rate, targetRate)}
}

func (p *pipeline) push(raw []byte, emit func([]float32)) {
	p.interl = p.format.toFloat(raw, p.interl)
	p.mono = downmix(p.interl, p.format.channels, p.mono)
	p.resampled = p.resampler.process(p.mono, p.resampled[:0])
	p.pending = append(p.pending, p.resampled...)
	n := 0
	for ; n+frameSamples <= len(p.pending); n += frameSamples {
		emit(append([]float32(nil), p.pending[n:n+frameSamples]...))
	}
	p.pending = append(p.pending[:0], p.pending[n:]...)
}

func chunkOf(samples []float32) AudioChunk {
	return AudioChunk{Samples: samples, RMS: displayLevel(frameRMS(samples)), Time: time.Now()}
}

// levelFloorDB maps to 0, and levelFloorDB+levelRangeDB to 1, as on macOS
// and Linux. Normal speech (about -26 dBFS RMS) gives about 0.67.
const (
	levelFloorDB = -50.0
	levelRangeDB = 36.0
)

// displayLevel maps an RMS value to the 0 to 1 range of the waveform.
func displayLevel(rms float32) float32 {
	if rms <= 0 {
		return 0
	}
	l := (20*math.Log10(float64(rms)) - levelFloorDB) / levelRangeDB
	return float32(math.Max(0, math.Min(1, l)))
}

func pwstr(p uintptr) string {
	if p == 0 {
		return ""
	}
	defer procCoTaskMemFree.Call(p)
	return windows.UTF16PtrToString((*uint16)(cPtr(p)))
}

func deviceID(dev comObject) string {
	var p uintptr
	if dev.hr("GetId", slotDeviceGetID, uintptr(unsafe.Pointer(&p))) != nil {
		return ""
	}
	return pwstr(p)
}

func deviceName(dev comObject) string {
	var store comObject
	if dev.hr("OpenPropertyStore", slotDeviceOpenPropertyStore, stgmRead, uintptr(unsafe.Pointer(&store))) != nil {
		return ""
	}
	defer store.release()
	var pv [24]byte // PROPVARIANT
	if store.hr("GetValue", slotPropertyStoreGetValue, uintptr(unsafe.Pointer(&pkeyDeviceFriendlyName)), uintptr(unsafe.Pointer(&pv))) != nil {
		return ""
	}
	defer procPropVariantClear.Call(uintptr(unsafe.Pointer(&pv)))
	if binary.LittleEndian.Uint16(pv[0:]) != vtLPWSTR {
		return ""
	}
	p := *(*uintptr)(unsafe.Pointer(&pv[8]))
	return windows.UTF16PtrToString((*uint16)(cPtr(p)))
}

// onCOMThread runs f with COM on a locked OS thread.
func onCOMThread(f func(enum comObject) error) error {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	done, err := comInit()
	if err != nil {
		return err
	}
	defer done()
	enum, err := createInstance(&clsidMMDeviceEnumerator, &iidIMMDeviceEnumerator, clsctxAll)
	if err != nil {
		return err
	}
	defer enum.release()
	return f(enum)
}

func (a *windowsAudio) ListDevices() ([]AudioDevice, error) {
	var out []AudioDevice
	err := onCOMThread(func(enum comObject) error {
		defaultID := ""
		var def comObject
		if enum.hr("GetDefaultAudioEndpoint", slotGetDefaultAudioEndpoint, eCapture, eConsole, uintptr(unsafe.Pointer(&def))) == nil {
			defaultID = deviceID(def)
			def.release()
		}
		var coll comObject
		if err := enum.hr("EnumAudioEndpoints", slotEnumAudioEndpoints, eCapture, deviceStateActive, uintptr(unsafe.Pointer(&coll))); err != nil {
			return err
		}
		defer coll.release()
		var n uint32
		if err := coll.hr("GetCount", slotCollectionGetCount, uintptr(unsafe.Pointer(&n))); err != nil {
			return err
		}
		for i := uint32(0); i < n; i++ {
			var dev comObject
			if coll.hr("Item", slotCollectionItem, uintptr(i), uintptr(unsafe.Pointer(&dev))) != nil {
				continue
			}
			id := deviceID(dev)
			out = append(out, AudioDevice{ID: id, Name: deviceName(dev), IsDefault: id != "" && id == defaultID})
			dev.release()
		}
		return nil
	})
	return out, err
}

func (a *windowsAudio) Start(ctx context.Context, deviceID string, onChunk func(AudioChunk)) error {
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.cancel != nil {
		return errors.New("the microphone is recording already")
	}
	ctx, cancel := context.WithCancel(ctx)
	queue := make(chan []float32, queueFrames)
	started := make(chan error, 1)
	done := make(chan struct{})
	worker := make(chan struct{})
	go func() {
		defer close(worker)
		for s := range queue {
			onChunk(chunkOf(s))
		}
	}()
	go func() {
		defer close(done)
		defer func() { close(queue); <-worker }()
		err := onCOMThread(func(enum comObject) error {
			return capture(ctx, enum, NormalizeDeviceID(deviceID), queue, started)
		})
		if err != nil {
			select {
			case started <- err:
			default:
			}
		}
	}()
	if err := <-started; err != nil {
		cancel()
		<-done
		return err
	}
	a.cancel, a.done = cancel, done
	return nil
}

func (a *windowsAudio) Stop() error {
	a.mu.Lock()
	cancel, done := a.cancel, a.done
	a.cancel, a.done = nil, nil
	a.mu.Unlock()
	if cancel == nil {
		return nil
	}
	cancel()
	select {
	case <-done:
	case <-time.After(2 * time.Second):
		return errors.New("the microphone did not stop in time")
	}
	return nil
}

func capture(ctx context.Context, enum comObject, id string, queue chan<- []float32, started chan<- error) error {
	var dev comObject
	if id != "" {
		if err := enum.hr("GetDevice", slotGetDevice, uintptr(unsafe.Pointer(utf16Ptr(id))), uintptr(unsafe.Pointer(&dev))); err != nil {
			return fmt.Errorf("the microphone %s is not available: %w", id, err)
		}
	} else if err := enum.hr("GetDefaultAudioEndpoint", slotGetDefaultAudioEndpoint, eCapture, eConsole, uintptr(unsafe.Pointer(&dev))); err != nil {
		return fmt.Errorf("there is no microphone: %w", err)
	}
	defer dev.release()
	var client comObject
	if err := dev.hr("Activate", slotDeviceActivate, uintptr(unsafe.Pointer(&iidIAudioClient)), clsctxAll, 0, uintptr(unsafe.Pointer(&client))); err != nil {
		return err
	}
	defer client.release()
	var fmtPtr uintptr
	if err := client.hr("GetMixFormat", slotClientGetMixFormat, uintptr(unsafe.Pointer(&fmtPtr))); err != nil {
		return err
	}
	defer procCoTaskMemFree.Call(fmtPtr)
	cb := binary.LittleEndian.Uint16(unsafe.Slice((*byte)(cPtr(fmtPtr)), 18)[16:])
	format, err := parseWaveFormat(unsafe.Slice((*byte)(cPtr(fmtPtr)), 18+int(cb)))
	if err != nil {
		return err
	}
	if err := client.hr("Initialize", slotClientInitialize, audclntShareShared, 0, bufferDuration, 0, fmtPtr, 0); err != nil {
		return err
	}
	var cc comObject
	if err := client.hr("GetService", slotClientGetService, uintptr(unsafe.Pointer(&iidIAudioCaptureClient)), uintptr(unsafe.Pointer(&cc))); err != nil {
		return err
	}
	defer cc.release()
	if err := client.hr("Start", slotClientStart); err != nil {
		return err
	}
	defer client.call(slotClientStop)
	started <- nil
	p := newPipeline(format)
	blockAlign := format.channels * format.bits / 8
	emit := func(s []float32) {
		// The queue holds 20 s; past that the callback waits, and so does the capture.
		select {
		case queue <- s:
		case <-ctx.Done():
		}
	}
	ticker := time.NewTicker(pollInterval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return nil
		case <-ticker.C:
		}
		for {
			var packet uint32
			if err := cc.hr("GetNextPacketSize", slotCaptureNextPacketSize, uintptr(unsafe.Pointer(&packet))); err != nil {
				return err
			}
			if packet == 0 {
				break
			}
			var data uintptr
			var frames, flags uint32
			if err := cc.hr("GetBuffer", slotCaptureGetBuffer, uintptr(unsafe.Pointer(&data)), uintptr(unsafe.Pointer(&frames)), uintptr(unsafe.Pointer(&flags)), 0, 0); err != nil {
				return err
			}
			n := int(frames) * blockAlign
			var raw []byte
			if flags&bufferFlagsSilent != 0 || data == 0 {
				raw = make([]byte, n)
			} else {
				raw = append([]byte(nil), unsafe.Slice((*byte)(cPtr(data)), n)...)
			}
			cc.call(slotCaptureReleaseBuffer, uintptr(frames))
			p.push(raw, emit)
		}
	}
}
