// Command sayso-audio captures audio and decodes audio files for an SDK
// that cannot do it itself, over NDJSON on stdout
// (sdk/protocol/audio-helper.md).
package main

import (
	"bufio"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"os/signal"
	"strconv"
	"sync"
	"syscall"
	"time"

	"github.com/watzon/sayso/sdk/go/audio"
	"github.com/watzon/sayso/sdk/go/capture/mic"
	"github.com/watzon/sayso/sdk/go/capture/system"
	"github.com/watzon/sayso/sdk/go/protocol"
)

const frame = 320

// out writes one message per line; captures write from their goroutines.
type out struct {
	mu sync.Mutex
	w  *bufio.Writer
}

func (o *out) send(kind string, fields map[string]any) {
	m := map[string]any{"v": 1, "type": kind}
	for k, v := range fields {
		m[k] = v
	}
	b, _ := json.Marshal(m)
	o.mu.Lock()
	defer o.mu.Unlock()
	o.w.Write(append(b, '\n'))
	o.w.Flush()
}

func (o *out) fail(code, message string) int {
	o.send("error", map[string]any{"code": code, "message": message})
	return 1
}

func main() {
	o := &out{w: bufio.NewWriter(os.Stdout)}
	os.Exit(run(os.Args[1:], os.Stdin, o))
}

func run(args []string, stdin io.Reader, o *out) int {
	if len(args) == 0 {
		return o.fail("bad_request", "usage: sayso-audio devices|permission|capture|decode")
	}
	switch args[0] {
	case "devices":
		return devices(o)
	case "permission":
		return permission(args[1:], o)
	case "capture":
		return capture(args[1:], stdin, o)
	case "decode":
		return decode(args[1:], o)
	}
	return o.fail("bad_request", "unknown command "+args[0])
}

func devices(o *out) int {
	list := []map[string]any{}
	if os.Getenv("SAYSO_AUDIO_FAKE_WAV") != "" {
		list = append(list, map[string]any{"id": "fake", "name": "fake", "default": true})
	} else {
		ds, err := mic.Devices()
		if err != nil {
			return o.fail("failed", err.Error())
		}
		for _, d := range ds {
			list = append(list, map[string]any{"id": d.ID, "name": d.Name, "default": d.IsDefault})
		}
	}
	o.send("devices", map[string]any{"devices": list})
	return 0
}

func permission(args []string, o *out) int {
	fs := flag.NewFlagSet("permission", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	request := fs.Bool("request", false, "")
	if fs.Parse(args) != nil || fs.NArg() != 1 {
		return o.fail("bad_request", "usage: sayso-audio permission [--request] mic|system")
	}
	var state mic.State
	switch fs.Arg(0) {
	case "mic":
		state = mic.Permission()
		if *request {
			state = mic.RequestPermission()
		}
	case "system":
		state = system.Access()
		if *request {
			state = system.Request()
		}
	default:
		return o.fail("bad_request", "the source is mic or system, not "+fs.Arg(0))
	}
	o.send("permission", map[string]any{"state": string(state)})
	return 0
}

// pcm sends 16 kHz mono audio in frames of 20 ms, with the time of each.
type pcm struct {
	o       *out
	pending []float32
	at      uint64 // samples sent so far
}

func (p *pcm) push(samples []float32) {
	p.pending = append(p.pending, samples...)
	for len(p.pending) >= frame {
		p.frame(p.pending[:frame])
		p.pending = p.pending[frame:]
	}
}

func (p *pcm) frame(f []float32) {
	p.o.send("audio", map[string]any{"t_ms": p.at * 1000 / audio.SampleRate, "pcm": protocol.EncodeI16(audio.FloatsToI16(f))})
	p.at += uint64(len(f))
}

func (p *pcm) flush() {
	if len(p.pending) > 0 {
		p.frame(p.pending)
		p.pending = nil
	}
}

func decode(args []string, o *out) int {
	if len(args) != 1 {
		return o.fail("bad_request", "usage: sayso-audio decode <absolute path>")
	}
	a, err := audio.ReadFile(args[0])
	if err != nil {
		code := "failed"
		if errors.Is(err, audio.ErrUnsupported) {
			code = "unsupported"
		}
		return o.fail(code, err.Error())
	}
	p := &pcm{o: o}
	p.push(a.AllFloats())
	p.flush()
	o.send("done", map[string]any{"ms": a.LenMs()})
	return 0
}

// stopper is closed when stdin closes or a signal comes.
func stopper(stdin io.Reader) <-chan struct{} {
	stop := make(chan struct{})
	var once sync.Once
	end := func() { once.Do(func() { close(stop) }) }
	go func() {
		io.Copy(io.Discard, stdin)
		end()
	}()
	sig := make(chan os.Signal, 1)
	signal.Notify(sig, os.Interrupt, syscall.SIGTERM)
	go func() {
		<-sig
		end()
	}()
	return stop
}

func capture(args []string, stdin io.Reader, o *out) int {
	fs := flag.NewFlagSet("capture", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	source := fs.String("source", "", "")
	device := fs.String("device", "", "")
	exclude := fs.String("exclude-pid", "", "")
	if fs.Parse(args) != nil || fs.NArg() != 0 || (*source != "mic" && *source != "system") {
		return o.fail("bad_request", "usage: sayso-audio capture --source mic|system [--device ID] [--exclude-pid N]")
	}
	var pid *uint32
	if *exclude != "" {
		n, err := strconv.ParseUint(*exclude, 10, 32)
		if err != nil {
			return o.fail("bad_request", "--exclude-pid is a process id")
		}
		v := uint32(n)
		pid = &v
	}
	stop := stopper(stdin)
	start := time.Now()
	p := &pcm{o: o}
	var mu sync.Mutex
	ended := make(chan struct{})
	var endOnce sync.Once
	onChunk := func(samples []float32, at time.Time) {
		mu.Lock()
		defer mu.Unlock()
		if !at.IsZero() {
			// A capture that started late gets silence to its time.
			want := uint64(max(0, at.Sub(start)).Seconds() * audio.SampleRate)
			if want > p.at+uint64(len(p.pending))+frame {
				p.push(make([]float32, want-p.at-uint64(len(p.pending))))
			}
		}
		p.push(samples)
	}
	onGap := func(ms uint64) { o.send("gap", map[string]any{"ms": ms}) }
	onEnd := func(err error) {
		reason := "failed"
		if errors.Is(err, mic.ErrDenied) {
			reason = "permission"
		}
		o.send("ended", map[string]any{"reason": reason, "message": err.Error()})
		endOnce.Do(func() { close(ended) })
	}
	var halt func()
	var excludes []string
	// started goes before the first audio: the callbacks wait for mu.
	mu.Lock()
	defer func() {
		if halt == nil {
			mu.Unlock()
		}
	}()
	switch {
	case os.Getenv("SAYSO_AUDIO_FAKE_WAV") != "":
		a, err := audio.ReadFile(os.Getenv("SAYSO_AUDIO_FAKE_WAV"))
		if err != nil {
			return o.fail("failed", err.Error())
		}
		halt = playFake(a.AllFloats(), func(s []float32) { onChunk(s, time.Time{}) })
		excludes = []string{}
		if *source == "system" {
			excludes = []string{"sayso"}
			if pid != nil {
				excludes = append(excludes, "app")
			}
		}
	case *source == "mic":
		s, err := mic.Open(mic.Options{Device: *device, OnChunk: func(c mic.Chunk) { onChunk(c.Samples, c.Time) }, OnGap: onGap, OnEnd: onEnd})
		if err != nil {
			if errors.Is(err, mic.ErrDenied) {
				return o.fail("permission", err.Error())
			}
			return o.fail("failed", err.Error())
		}
		halt, excludes = s.Stop, []string{}
	default:
		c, err := system.Open(system.Options{ExcludePID: pid, OnChunk: func(c system.Chunk) { onChunk(c.Samples, c.Time) }, OnGap: onGap, OnEnd: onEnd})
		switch {
		case errors.Is(err, system.ErrUnsupported):
			return o.fail("unsupported", err.Error())
		case errors.Is(err, mic.ErrDenied):
			return o.fail("permission", err.Error())
		case err != nil:
			return o.fail("failed", err.Error())
		}
		halt, excludes = c.Stop, c.Excludes()
	}
	o.send("started", map[string]any{"excludes": excludes})
	mu.Unlock()
	code := 0
	select {
	case <-stop:
	case <-ended:
		code = 1
	}
	halt()
	mu.Lock()
	p.flush()
	mu.Unlock()
	return code
}

// playFake plays samples at the speed of real time, then silence.
func playFake(samples []float32, on func([]float32)) func() {
	done := make(chan struct{})
	stopped := make(chan struct{})
	go func() {
		defer close(stopped)
		tick := time.NewTicker(20 * time.Millisecond)
		defer tick.Stop()
		for i := 0; ; i += frame {
			select {
			case <-done:
				return
			case <-tick.C:
			}
			if i < len(samples) {
				on(samples[i:min(i+frame, len(samples))])
			} else {
				on(make([]float32, frame))
			}
		}
	}()
	var once sync.Once
	return func() { once.Do(func() { close(done); <-stopped }) }
}

func init() {
	flag.Usage = func() { fmt.Fprintln(os.Stderr, "usage: sayso-audio devices|permission|capture|decode") }
}
