package platform

// Microphone capture on Linux through the sound server's own recorder, which
// resamples to 16 kHz mono float32: parec (PulseAudio and PipeWire's pulse
// server), else pw-record (PipeWire), else arecord (ALSA). Devices come from
// `pactl list sources`, else from /proc/asound/pcm.

import (
	"bufio"
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"math"
	"os"
	"os/exec"
	"strings"
	"sync"
	"time"
)

type linuxAudio struct {
	lookPath func(string) (string, error)
	mu       sync.Mutex
	cur      *linuxCapture
}

type linuxCapture struct {
	cmd    *exec.Cmd
	cancel context.CancelFunc
	done   chan struct{}
}

func newLinuxAudio() *linuxAudio { return &linuxAudio{lookPath: exec.LookPath} }

func (a *linuxAudio) has(tool string) bool {
	_, err := a.lookPath(tool)
	return err == nil
}

// recordCommand is the recorder for a device ("" for the default).
func (a *linuxAudio) recordCommand(deviceID string) ([]string, error) {
	switch {
	case a.has("parec"):
		args := []string{"parec", "--raw", "--format=float32le", "--rate=16000", "--channels=1", "--latency-msec=20"}
		if deviceID != "" {
			args = append(args, "--device="+deviceID)
		}
		return args, nil
	case a.has("pw-record"):
		args := []string{"pw-record", "--format", "f32", "--rate", "16000", "--channels", "1"}
		if deviceID != "" {
			args = append(args, "--target", deviceID)
		}
		return append(args, "-"), nil
	case a.has("arecord"):
		args := []string{"arecord", "-q", "-t", "raw", "-f", "FLOAT_LE", "-r", "16000", "-c", "1"}
		if deviceID != "" {
			args = append(args, "-D", deviceID)
		}
		return args, nil
	}
	return nil, errors.New("audio: no recorder: install pulseaudio-utils (parec), pipewire (pw-record), or alsa-utils (arecord)")
}

func (a *linuxAudio) ListDevices() ([]AudioDevice, error) {
	if a.has("pactl") {
		out, err := exec.Command("pactl", "list", "sources").Output()
		if err == nil {
			def, _ := exec.Command("pactl", "get-default-source").Output()
			if devices := parsePactlSources(string(out), strings.TrimSpace(string(def))); len(devices) > 0 {
				return devices, nil
			}
		}
	}
	if text, err := os.ReadFile("/proc/asound/pcm"); err == nil {
		return parseALSAPCM(string(text)), nil
	}
	return nil, nil
}

// parsePactlSources reads `pactl list sources` (the long form, stable across
// versions). Monitor sources (the sound of an output) are left out.
func parsePactlSources(text, defaultName string) []AudioDevice {
	var out []AudioDevice
	var cur *AudioDevice
	monitor := false
	flush := func() {
		if cur != nil && cur.ID != "" && !monitor {
			if cur.Name == "" {
				cur.Name = cur.ID
			}
			cur.IsDefault = cur.ID == defaultName
			out = append(out, *cur)
		}
		cur, monitor = nil, false
	}
	sc := bufio.NewScanner(strings.NewReader(text))
	for sc.Scan() {
		line := sc.Text()
		trimmed := strings.TrimSpace(line)
		switch {
		case strings.HasPrefix(line, "Source #"):
			flush()
			cur = &AudioDevice{}
		case cur == nil:
		case strings.HasPrefix(trimmed, "Name:"):
			cur.ID = strings.TrimSpace(strings.TrimPrefix(trimmed, "Name:"))
			monitor = monitor || strings.HasSuffix(cur.ID, ".monitor")
		case strings.HasPrefix(trimmed, "Description:"):
			cur.Name = strings.TrimSpace(strings.TrimPrefix(trimmed, "Description:"))
		case strings.HasPrefix(trimmed, "Monitor of Sink:"):
			monitor = monitor || strings.TrimSpace(strings.TrimPrefix(trimmed, "Monitor of Sink:")) != "n/a"
		}
	}
	flush()
	return out
}

// parseALSAPCM reads /proc/asound/pcm lines such as
// "00-00: ALC892 Analog : ALC892 Analog : playback 1 : capture 1".
// The id is the ALSA name for arecord ("hw:0,0").
func parseALSAPCM(text string) []AudioDevice {
	var out []AudioDevice
	for _, line := range strings.Split(text, "\n") {
		if !strings.Contains(line, "capture") {
			continue
		}
		fields := strings.Split(line, ":")
		if len(fields) < 2 {
			continue
		}
		var card, dev int
		if _, err := fmt.Sscanf(strings.TrimSpace(fields[0]), "%d-%d", &card, &dev); err != nil {
			continue
		}
		out = append(out, AudioDevice{
			ID:        fmt.Sprintf("hw:%d,%d", card, dev),
			Name:      strings.TrimSpace(fields[1]),
			IsDefault: len(out) == 0,
		})
	}
	return out
}

func (a *linuxAudio) Start(ctx context.Context, deviceID string, onChunk func(AudioChunk)) error {
	if onChunk == nil {
		return errors.New("audio: no chunk handler")
	}
	args, err := a.recordCommand(NormalizeDeviceID(deviceID))
	if err != nil {
		return err
	}
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.cur != nil {
		return errors.New("audio: capture already running")
	}
	runCtx, cancel := context.WithCancel(ctx)
	cmd := exec.CommandContext(runCtx, args[0], args[1:]...)
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		cancel()
		return err
	}
	var stderr strings.Builder
	cmd.Stderr = &stderr
	if err := cmd.Start(); err != nil {
		cancel()
		return fmt.Errorf("audio: %s: %w", args[0], err)
	}
	c := &linuxCapture{cmd: cmd, cancel: cancel, done: make(chan struct{})}
	a.cur = c
	framer := newAudioFramer(onChunk)
	go func() {
		defer close(c.done)
		readFloat32(stdout, framer.push)
		_ = cmd.Wait()
		framer.close()
		a.mu.Lock()
		if a.cur == c {
			a.cur = nil
		}
		a.mu.Unlock()
	}()
	return nil
}

// readFloat32 reads little-endian float32 samples until the stream ends.
func readFloat32(r io.Reader, push func([]float32)) {
	buf := make([]byte, frameSamples*4)
	samples := make([]float32, frameSamples)
	pending := 0
	for {
		n, err := r.Read(buf[pending:])
		pending += n
		whole := pending / 4
		for i := 0; i < whole; i++ {
			samples[i] = math.Float32frombits(binary.LittleEndian.Uint32(buf[i*4:]))
		}
		if whole > 0 {
			push(samples[:whole])
		}
		// Keep a partial sample for the next read.
		copy(buf, buf[whole*4:pending])
		pending -= whole * 4
		if err != nil {
			return
		}
	}
}

func (a *linuxAudio) Stop() error {
	a.mu.Lock()
	c := a.cur
	a.mu.Unlock()
	if c == nil {
		return nil
	}
	c.cancel()
	select {
	case <-c.done:
	case <-time.After(3 * time.Second):
		return errors.New("audio: the recorder did not stop")
	}
	return nil
}

// ---------------------------------------------------------------------------
// Framing and level, as sayso-capture/src/mic.rs
// ---------------------------------------------------------------------------

const (
	frameSamples = 320 // 20 ms at 16 kHz
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
// Normal speech (about -26 dBFS RMS) gives about 0.67.
func displayLevel(rms float32) float32 {
	if rms <= 0 {
		return 0
	}
	l := (20*math.Log10(float64(rms)) - levelFloorDB) / levelRangeDB
	return float32(math.Max(0, math.Min(1, l)))
}

// audioFramer cuts samples into 20 ms chunks and hands them to a worker
// goroutine, so a slow handler never stalls the reader.
type audioFramer struct {
	pending []float32
	out     chan AudioChunk
	done    chan struct{}
}

func newAudioFramer(onChunk func(AudioChunk)) *audioFramer {
	f := &audioFramer{out: make(chan AudioChunk, 1024), done: make(chan struct{})}
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
		chunk := AudioChunk{Samples: frame, RMS: displayLevel(rmsOf(frame)), Time: time.Now()}
		select {
		case f.out <- chunk:
		default: // The handler is 20 s behind: drop rather than block.
		}
	}
	if cap(f.pending) > 4*frameSamples && len(f.pending) < frameSamples {
		f.pending = append([]float32(nil), f.pending...)
	}
}

func (f *audioFramer) close() {
	close(f.out)
	<-f.done
}
