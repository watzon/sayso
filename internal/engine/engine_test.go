package engine

import (
	"bufio"
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

// The test binary is also the fake engine and the fake cleanup runtime.
func TestMain(m *testing.M) {
	switch os.Getenv("SAYSO_FAKE") {
	case "engine":
		fakeEngine()
		os.Exit(0)
	case "cleanup":
		fakeCleanup()
		os.Exit(0)
	}
	os.Exit(m.Run())
}

func reply(w *bufio.Writer, mu *sync.Mutex, m map[string]any) {
	m["v"] = 1
	b, _ := json.Marshal(m)
	mu.Lock()
	w.Write(append(b, '\n'))
	w.Flush()
	mu.Unlock()
}

func fakeEngine() {
	out := bufio.NewWriter(os.Stdout)
	var mu sync.Mutex
	in := bufio.NewScanner(os.Stdin)
	in.Buffer(make([]byte, 1<<20), 1<<24)
	streams := map[float64]int{}
	marker := filepath.Join(os.Getenv("SAYSO_MODELS_DIR"), "crashed-once")
	for in.Scan() {
		var req map[string]any
		if json.Unmarshal(in.Bytes(), &req) != nil {
			reply(out, &mu, map[string]any{"type": "error", "message": "not a request"})
			continue
		}
		id, _ := req["id"].(string)
		switch req["type"] {
		case "hello":
			reply(out, &mu, map[string]any{"type": "hello", "id": id, "version": "0.0.1", "protocol": 1,
				"features": []string{"unload", "catalog", "language_model", "segments", "store_v1"}})
			reply(out, &mu, map[string]any{"type": "log", "level": "info",
				"message": "env " + os.Getenv("SAYSO_MODELS_DIR") + "|" + os.Getenv("SAYSO_MODEL_STORE") + "|" + os.Getenv("EXTRA")})
		case "list_models":
			models, _ := req["models"].([]any)
			for _, m := range models {
				reply(out, &mu, map[string]any{"type": "model_state", "id": id, "model": m.(map[string]any)["id"], "state": "downloaded"})
			}
			if req["models"] == nil {
				reply(out, &mu, map[string]any{"type": "ok", "id": id, "models": []any{map[string]any{"id": "m1", "name": "M1", "roles": []string{"final"}}}, "retired": []any{}})
			} else {
				reply(out, &mu, map[string]any{"type": "ok", "id": id})
			}
		case "download":
			reply(out, &mu, map[string]any{"type": "model_state", "id": id, "model": req["model"], "state": "downloading"})
			reply(out, &mu, map[string]any{"type": "download_progress", "id": id, "model": req["model"], "fraction": 0.5, "bytes_done": 50, "bytes_total": 100})
			reply(out, &mu, map[string]any{"type": "model_state", "id": id, "model": req["model"], "state": "downloaded"})
			reply(out, &mu, map[string]any{"type": "ok", "id": id})
		case "load":
			if req["model"] == "missing" {
				reply(out, &mu, map[string]any{"type": "model_state", "id": id, "model": "missing", "state": "failed", "message": "no files", "reason": "load_failed"})
				reply(out, &mu, map[string]any{"type": "error", "id": id, "message": "model missing is not downloaded"})
				continue
			}
			reply(out, &mu, map[string]any{"type": "model_state", "id": id, "model": req["model"], "state": "optimizing"})
			reply(out, &mu, map[string]any{"type": "model_state", "id": id, "model": req["model"], "state": "ready", "load_ms": 42})
			reply(out, &mu, map[string]any{"type": "ok", "id": id})
		case "unload":
			reply(out, &mu, map[string]any{"type": "model_state", "id": id, "model": req["model"], "state": "downloaded"})
			reply(out, &mu, map[string]any{"type": "ok", "id": id})
		case "transcribe":
			path, _ := req["path"].(string)
			if strings.HasSuffix(path, "crash.wav") {
				if _, err := os.Stat(marker); err != nil {
					os.WriteFile(marker, nil, 0o644)
					os.Exit(3)
				}
			}
			b, _ := os.ReadFile(path)
			text := fmt.Sprintf("bytes=%d lang=%v vocab=%v", len(b), req["language"], req["vocabulary"])
			m := map[string]any{"type": "final", "id": id, "text": text, "elapsed_ms": 7}
			if req["segments"] == true {
				m["segments"] = []any{map[string]any{"start_ms": 0, "end_ms": 10, "text": "x", "words": []any{}}}
			}
			reply(out, &mu, m)
		case "stream_start":
			streams[req["session"].(float64)] = 0
			reply(out, &mu, map[string]any{"type": "ok", "id": id})
		case "stream_audio":
			s := req["session"].(float64)
			pcm, _ := base64.StdEncoding.DecodeString(req["pcm"].(string))
			streams[s] += len(pcm) / 2
			reply(out, &mu, map[string]any{"type": "partial", "session": s, "committed": "", "tentative": fmt.Sprintf("samples=%d", streams[s])})
		case "stream_end":
			s := req["session"].(float64)
			reply(out, &mu, map[string]any{"type": "ok", "id": id, "session": s, "text": fmt.Sprintf("samples=%d", streams[s])})
		case "language_model":
			reply(out, &mu, map[string]any{"type": "ok", "id": id, "available": true, "model": "apple-fm"})
		case "generate":
			if req["prompt"] == "slow" {
				continue
			}
			reply(out, &mu, map[string]any{"type": "ok", "id": id, "text": strings.ToUpper(req["prompt"].(string)), "model": "apple-fm"})
		case "shutdown":
			reply(out, &mu, map[string]any{"type": "ok", "id": id})
			return
		default:
			reply(out, &mu, map[string]any{"type": "error", "id": id, "message": "unknown type"})
		}
	}
}

func fakeCleanup() {
	out := bufio.NewWriter(os.Stdout)
	var mu sync.Mutex
	in := bufio.NewScanner(os.Stdin)
	loaded := ""
	for in.Scan() {
		var req map[string]any
		json.Unmarshal(in.Bytes(), &req)
		id := req["id"]
		switch req["type"] {
		case "hello":
			hello := map[string]any{"id": id, "type": "hello", "version": 1, "llama_cpp": "b1", "backend": "cpu"}
			if libs := os.Getenv("SAYSO_FAKE_CLEANUP_LIBS"); libs != "" {
				hello["libraries"] = libs
			}
			reply(out, &mu, hello)
		case "load":
			if req["model_id"] == "hang" {
				time.Sleep(time.Hour)
			}
			loaded = req["model_id"].(string)
			reply(out, &mu, map[string]any{"id": id, "type": "loaded", "model_id": loaded, "load_ms": 3})
		case "count":
			if req["model_id"] != loaded {
				reply(out, &mu, map[string]any{"id": id, "type": "error", "code": "wrong_model", "message": "load again"})
				continue
			}
			reply(out, &mu, map[string]any{"id": id, "type": "count", "tokens": len(strings.Fields(req["text"].(string)))})
		case "complete":
			if req["prompt"] == "hang" {
				time.Sleep(time.Hour)
			}
			if req["prompt"] == "crash" {
				fmt.Fprintln(os.Stderr, "panic: cannot load libffi.so.8")
				os.Exit(3)
			}
			reply(out, &mu, map[string]any{"id": id, "type": "completion", "text": " " + req["prompt"].(string) + ".", "stop": "end", "tokens_in": 3, "tokens_out": 2, "ms": 5})
		}
	}
	// The end of stdin: a graceful stop.
	if marker := os.Getenv("SAYSO_FAKE_CLEANUP_EXIT"); marker != "" {
		f, _ := os.OpenFile(marker, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
		fmt.Fprintln(f, "graceful")
		f.Close()
	}
}

func startFake(t *testing.T, extra func(*Options)) (*Client, <-chan Event) {
	t.Helper()
	exe, _ := os.Executable()
	dir := t.TempDir()
	opts := Options{
		EnginePath: exe, ModelsDir: filepath.Join(dir, "models"), CacheDir: filepath.Join(dir, "cache"),
		StoreDir: filepath.Join(dir, "models", "v1"), Env: []string{"SAYSO_FAKE=engine"},
		BackoffInitial: 10 * time.Millisecond, BackoffMax: 50 * time.Millisecond, HelloTimeout: 5 * time.Second,
	}
	if extra != nil {
		extra(&opts)
	}
	c, events, err := Start(opts)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { c.Close() })
	return c, events
}

func waitFor(t *testing.T, events <-chan Event, kind EventKind, match func(Event) bool) Event {
	t.Helper()
	deadline := time.After(5 * time.Second)
	for {
		select {
		case e := <-events:
			if e.Kind == kind && (match == nil || match(e)) {
				return e
			}
		case <-deadline:
			t.Fatalf("no event %d", kind)
		}
	}
}

func TestHelloEnvironmentAndFeatures(t *testing.T) {
	c, events := startFake(t, func(o *Options) { o.BeforeLaunch = func() []string { return []string{"EXTRA=yes"} } })
	var log Event
	started := false
	for deadline := time.After(5 * time.Second); !started || log.Message == ""; {
		select {
		case e := <-events:
			if e.Kind == EventStarted {
				started = true
			}
			if e.Kind == EventLog && strings.HasPrefix(e.Message, "env ") {
				log = e
			}
		case <-deadline:
			t.Fatal("no start or log event")
		}
	}
	parts := strings.Split(strings.TrimPrefix(log.Message, "env "), "|")
	if !strings.HasSuffix(parts[0], "models") || !strings.HasSuffix(parts[1], filepath.Join("models", "v1")) || parts[2] != "yes" {
		t.Fatal(log.Message)
	}
	if !c.Features().Has(FeatureStoreV1) || c.Features().Version != "0.0.1" {
		t.Fatal(c.Features())
	}
}

func TestModelLifecycle(t *testing.T) {
	c, events := startFake(t, nil)
	ctx := context.Background()
	eng := json.RawMessage(`{"kind":"parakeet_tdt","version":"v3"}`)
	if err := c.ListModels(ctx, []ModelRef{{ID: "m1", Engine: eng}}); err != nil {
		t.Fatal(err)
	}
	if s, _ := c.Status("m1"); s.State != StateDownloaded {
		t.Fatal(s)
	}
	if err := c.Download(ctx, "m1", eng, 100); err != nil {
		t.Fatal(err)
	}
	waitFor(t, events, EventModelStatus, func(e Event) bool { return e.Status.State == StateDownloading && e.Status.Fraction == 0.5 })
	ms, err := c.Load(ctx, "m1", eng)
	if err != nil || ms != 42 {
		t.Fatal(ms, err)
	}
	if s, _ := c.Status("m1"); !s.IsUsable() {
		t.Fatal(s)
	}
	_, err = c.Load(ctx, "missing", nil)
	var ee *Error
	if !errors.As(err, &ee) || !strings.Contains(ee.Message, "not downloaded") {
		t.Fatal(err)
	}
	if s, _ := c.Status("missing"); s.State != StateFailed || s.Reason != ReasonLoadFailed {
		t.Fatal(s)
	}
	cat, retired, err := c.Catalog(ctx)
	if err != nil || len(cat) != 1 || cat[0].ID != "m1" || retired == nil {
		t.Fatal(cat, retired, err)
	}
	if err := c.Unload(ctx, "m1", false); err != nil {
		t.Fatal(err)
	}
}

func TestTranscribeStreamAndGenerate(t *testing.T) {
	c, events := startFake(t, nil)
	ctx := context.Background()
	final, err := c.TranscribeSamples(ctx, "m1", make([]float32, 160), "en", nil)
	if err != nil || final.Text != "bytes=364 lang=en vocab=[]" || final.ElapsedMs != 7 {
		t.Fatal(final, err)
	}
	entries, _ := os.ReadDir(c.opts.CacheDir)
	if len(entries) != 0 {
		t.Fatal("temporary WAV left behind")
	}
	if err := c.StreamStart(ctx, 1, "m1", "en"); err != nil {
		t.Fatal(err)
	}
	c.StreamAudio(1, make([]float32, 2560))
	waitFor(t, events, EventPartial, func(e Event) bool { return e.Session == 1 && e.Tentative == "samples=2560" })
	text, err := c.StreamEnd(ctx, 1)
	if err != nil || text != "samples=2560" {
		t.Fatal(text, err)
	}
	lm, _ := c.LanguageModel(ctx)
	if !lm.Available || lm.Model != "apple-fm" {
		t.Fatal(lm)
	}
	g, err := c.Generate(ctx, "be nice", "hi", time.Second, nil)
	if err != nil || g.Text != "HI" {
		t.Fatal(g, err)
	}
	_, err = c.Generate(ctx, "x", "slow", 50*time.Millisecond, nil)
	var ee *Error
	if !errors.As(err, &ee) || ee.Code != ReasonTimeout {
		t.Fatal(err)
	}
	if _, err := c.Call(ctx, "nope", nil, nil); err == nil {
		t.Fatal("unknown request accepted")
	}
}

func TestRestartAfterACrashReloadsModels(t *testing.T) {
	c, events := startFake(t, nil)
	ctx := context.Background()
	if _, err := c.Load(ctx, "m1", json.RawMessage(`{"kind":"x"}`)); err != nil {
		t.Fatal(err)
	}
	wav := filepath.Join(t.TempDir(), "crash.wav")
	os.WriteFile(wav, EncodeWAV(nil), 0o644)
	if _, err := c.Transcribe(ctx, "m1", wav, "en", nil, false); !errors.Is(err, ErrCrashed) {
		t.Fatal("want crash error, got", err)
	}
	waitFor(t, events, EventCrashed, nil)
	waitFor(t, events, EventModelStatus, func(e Event) bool { return e.Model == "m1" && e.Status.State == StateReady })
	waitFor(t, events, EventRestarted, nil)
	final, err := c.Transcribe(ctx, "m1", wav, "en", []string{"Sayso"}, true)
	if err != nil || len(final.Segments) != 1 {
		t.Fatal(final, err)
	}
}

func TestStartFailsForAMissingProgram(t *testing.T) {
	_, events, err := Start(Options{EnginePath: filepath.Join(t.TempDir(), "nope"), ModelsDir: t.TempDir()})
	if err == nil {
		t.Fatal("started a missing program")
	}
	if e := <-events; e.Kind != EventStartFailed {
		t.Fatal(e)
	}
}

func TestWAVAndPCM(t *testing.T) {
	pcm := ToPCM16([]float32{1, -1, 2, 0})
	if pcm[0] != 32767 || pcm[1] != -32767 || pcm[2] != 32767 || pcm[3] != 0 {
		t.Fatal(pcm)
	}
	w := EncodeWAV(pcm)
	if len(w) != 44+8 || string(w[:4]) != "RIFF" || string(w[36:40]) != "data" {
		t.Fatal(len(w))
	}
}

func TestCleanupClient(t *testing.T) {
	exe, _ := os.Executable()
	t.Setenv("SAYSO_FAKE", "cleanup")
	c := NewCleanupClient(exe)
	defer c.Stop()
	err := c.With("s1", "/m.gguf", 2048, func(s *CleanupSession) error {
		n, err := s.Count("three small words", time.Second)
		if err != nil || n != 3 {
			return fmt.Errorf("count %d %v", n, err)
		}
		r, err := s.Complete("hi", []string{"<|im_end|>"}, 40, time.Second)
		if err != nil || r.Text != " hi." || r.Stop != StopEnd {
			return fmt.Errorf("complete %+v %v", r, err)
		}
		_, err = s.Complete("hang", nil, 40, 100*time.Millisecond)
		if !errors.Is(err, ErrCleanupTimeout) {
			return fmt.Errorf("want timeout, got %v", err)
		}
		// The next request starts the runtime again and loads the model.
		n, err = s.Count("one", time.Second)
		if err != nil || n != 1 {
			return fmt.Errorf("after timeout %d %v", n, err)
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	if h, ok := c.Hello(); !ok || h.Backend != "cpu" {
		t.Fatal(h)
	}
	c.SetEnabled(false)
	if c.IsRunning() {
		t.Fatal("still running after disable")
	}
	if err := c.EnsureLoaded("s1", "/m.gguf", 2048); !errors.Is(err, ErrCleanupDisabled) {
		t.Fatal(err)
	}
}
