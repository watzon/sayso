package ui

import (
	"bufio"
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/dictation"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/platform"
)

// The test binary is also a fake engine: every model it is asked about is
// on disk, and load makes it ready.
func TestMain(m *testing.M) {
	if os.Getenv("SAYSO_FAKE") == "engine" {
		fakeEngineProcess()
		os.Exit(0)
	}
	os.Exit(m.Run())
}

func fakeEngineProcess() {
	out := bufio.NewWriter(os.Stdout)
	send := func(m map[string]any) {
		m["v"] = 1
		b, _ := json.Marshal(m)
		out.Write(append(b, '\n'))
		out.Flush()
	}
	in := bufio.NewScanner(os.Stdin)
	in.Buffer(make([]byte, 1<<20), 1<<24)
	for in.Scan() {
		var req map[string]any
		if json.Unmarshal(in.Bytes(), &req) != nil {
			continue
		}
		id, _ := req["id"].(string)
		switch req["type"] {
		case "hello":
			send(map[string]any{"type": "hello", "id": id, "version": "0.0.1", "protocol": engine.ProtocolVersion, "features": []string{}})
		case "list_models":
			list, _ := req["models"].([]any)
			for _, m := range list {
				send(map[string]any{"type": "model_state", "id": id, "model": m.(map[string]any)["id"], "state": "downloaded"})
			}
			send(map[string]any{"type": "ok", "id": id})
		case "load":
			send(map[string]any{"type": "model_state", "id": id, "model": req["model"], "state": "ready", "load_ms": 1})
			send(map[string]any{"type": "ok", "id": id})
		case "shutdown":
			send(map[string]any{"type": "ok", "id": id})
			return
		default:
			send(map[string]any{"type": "ok", "id": id})
		}
	}
}

func firstLocalSpeechModel(t *testing.T) models.ModelID {
	t.Helper()
	for _, m := range models.LocalModels() {
		if m.FinalPass && m.Task() == models.TaskSpeech {
			return m.ID
		}
	}
	t.Skip("no local speech model on this system")
	return ""
}

func startFakeEngine(t *testing.T, preload func() string) *EngineAdapter {
	t.Helper()
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	e, err := StartEngine(engine.Options{
		EnginePath: exe, ModelsDir: filepath.Join(dir, "models"), CacheDir: filepath.Join(dir, "cache"),
		Env: []string{"SAYSO_FAKE=engine"}, HelloTimeout: 5 * time.Second,
	}, preload, nil)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { e.Close() })
	return e
}

func waitState(t *testing.T, e *EngineAdapter, model, state string) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if e.Status(model).State == state {
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatalf("%s is %q, want %q", model, e.Status(model).State, state)
}

func TestEngineStartListsTheModelsOnDisk(t *testing.T) {
	model := string(firstLocalSpeechModel(t))
	e := startFakeEngine(t, func() string { return "" })
	waitState(t, e, model, engine.StateDownloaded)
	for _, m := range models.LocalModels() {
		if !e.Status(string(m.ID)).IsOnDisk() {
			t.Errorf("%s was not listed at start: %+v", m.ID, e.Status(string(m.ID)))
		}
	}
}

func TestEngineStartPreloadsTheDictationModel(t *testing.T) {
	model := firstLocalSpeechModel(t)
	cfg := config.Default()
	cfg.Dictation.Model = model
	cfg.Dictation.PreloadModel = true
	e := startFakeEngine(t, func() string { return PreloadModel(&cfg) })
	waitState(t, e, string(model), engine.StateReady)
}

func TestPreloadModelIsOffWhenTheSettingIsOff(t *testing.T) {
	cfg := config.Default()
	cfg.Dictation.Model = "some-model"
	cfg.Dictation.PreloadModel = false
	if got := PreloadModel(&cfg); got != "" {
		t.Fatalf("PreloadModel = %q, want none", got)
	}
	cfg.Dictation.PreloadModel = true
	if got := PreloadModel(&cfg); got != "some-model" {
		t.Fatalf("PreloadModel = %q", got)
	}
}

// micStub is a microphone that records nothing.
type micStub struct{}

func (micStub) ListDevices() ([]platform.AudioDevice, error)                   { return nil, nil }
func (micStub) Start(context.Context, string, func(platform.AudioChunk)) error { return nil }
func (micStub) Stop() error                                                    { return nil }

type seenStates struct {
	mu      sync.Mutex
	phases  []string
	notices []string
}

func (s *seenStates) StateChanged(st dictation.State) {
	s.mu.Lock()
	s.phases = append(s.phases, st.Phase())
	s.mu.Unlock()
}
func (s *seenStates) Notice(m dictation.Message) {
	s.mu.Lock()
	s.notices = append(s.notices, m.Key)
	s.mu.Unlock()
}
func (s *seenStates) Level(float32)          {}
func (s *seenStates) Preview(string, string) {}

func (s *seenStates) saw(phase string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, p := range s.phases {
		if p == phase {
			return true
		}
	}
	return false
}

// The Go app never asked the engine for its models, so a downloaded model
// stayed "Not downloaded" and dictation refused to start.
func TestADownloadedModelLetsDictationStartAfterEngineStart(t *testing.T) {
	model := firstLocalSpeechModel(t)
	cfg := config.Default()
	cfg.Dictation.Model = model
	cfg.Dictation.PreloadModel = false
	cfg.Dictation.LivePreview = false
	e := startFakeEngine(t, func() string { return PreloadModel(&cfg) })
	waitState(t, e, string(model), engine.StateDownloaded)
	obs := &seenStates{}
	ctl := dictation.New(dictation.Deps{
		Platform: platform.Services{Audio: micStub{}},
		Engine:   e,
		Config:   func() *config.Config { return &cfg },
		Observer: obs,
	})
	t.Cleanup(ctl.Close)
	ctl.Toggle()
	deadline := time.Now().Add(5 * time.Second)
	for !obs.saw("recording") && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	started := obs.saw("recording")
	obs.mu.Lock()
	defer obs.mu.Unlock()
	if !started {
		t.Fatalf("dictation did not start: phases %v, notices %v", obs.phases, obs.notices)
	}
	for _, n := range obs.notices {
		if n == "overlay.notice.download_model" {
			t.Fatalf("a downloaded model asked to be downloaded: %v", obs.notices)
		}
	}
}
