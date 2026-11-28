package engine_test

import (
	"context"
	"encoding/json"
	"errors"
	"io/fs"
	"math"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/watzon/sayso/sdk/go/audio"
	"github.com/watzon/sayso/sdk/go/engine"
	"github.com/watzon/sayso/sdk/go/internal/testbin"
	"github.com/watzon/sayso/sdk/go/protocol"
	"github.com/watzon/sayso/sdk/go/protocol/corpus"
)

func TestMain(m *testing.M) {
	code := m.Run()
	testbin.Cleanup()
	os.Exit(code)
}

func start(t *testing.T, env ...string) (*engine.Engine, string) {
	t.Helper()
	models := filepath.Join(t.TempDir(), "models")
	e, err := engine.Start(context.Background(), engine.Options{Program: testbin.Path(t, testbin.FakeEngine), ModelsDir: models, Env: env})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { e.Close() })
	return e, models
}

func ctx(t *testing.T) context.Context {
	c, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	t.Cleanup(cancel)
	return c
}

// speech is a 16 kHz mono WAV file: silence, then a tone for each length in
// ms, with 1 s of silence after each.
func speech(t *testing.T, lengths ...int) (string, []int16) {
	t.Helper()
	samples := make([]int16, audio.SampleRate/2)
	for _, ms := range lengths {
		for i := 0; i < ms*16; i++ {
			samples = append(samples, int16(9000*math.Sin(2*math.Pi*220*float64(i)/audio.SampleRate)))
		}
		samples = append(samples, make([]int16, audio.SampleRate)...)
	}
	path := filepath.Join(t.TempDir(), "speech.wav")
	if err := audio.WriteWAV(path, samples); err != nil {
		t.Fatal(err)
	}
	return path, samples
}

func ready(t *testing.T, e *engine.Engine, model string) {
	t.Helper()
	if err := e.Download(ctx(t), model, nil, 0, nil, nil); err != nil {
		t.Fatal(err)
	}
	if _, err := e.Load(ctx(t), model, nil); err != nil {
		t.Fatal(err)
	}
}

func TestHelloListsEveryFeature(t *testing.T) {
	e, _ := start(t)
	h := e.Hello()
	if h.Protocol != 1 || h.Version == "" {
		t.Fatal(h)
	}
	for _, f := range engine.KnownFeatures {
		if !e.Has(f) {
			t.Errorf("no %s", f)
		}
	}
}

func TestADownloadWritesTheModelStore(t *testing.T) {
	e, models := start(t)
	var progress []engine.DownloadProgress
	var states []string
	if err := e.Download(ctx(t), "fake-final", nil, 0, func(p engine.DownloadProgress) { progress = append(progress, p) }, func(s engine.ModelState) { states = append(states, s.State) }); err != nil {
		t.Fatal(err)
	}
	if len(progress) == 0 || progress[len(progress)-1].Fraction != 1 || strings.Join(states, ",") != "downloading,downloaded" {
		t.Fatal(progress, states)
	}
	manifest := filepath.Join(models, "v1", "models", "fake-final", "fake-1.json")
	b, err := os.ReadFile(manifest)
	if err != nil {
		t.Fatal(err)
	}
	var m struct{ Artifacts []string }
	json.Unmarshal(b, &m)
	if len(m.Artifacts) != 1 {
		t.Fatal(string(b))
	}
	for _, p := range []string{"artifacts/" + m.Artifacts[0] + "/model.bin", "locks/meta", "locks/" + m.Artifacts[0] + ".use", "staging", "trash"} {
		if _, err := os.Stat(filepath.Join(models, "v1", filepath.FromSlash(p))); err != nil {
			t.Error(err)
		}
	}
	list, _, st, err := e.Catalog(ctx(t))
	if err != nil || len(list) != 4 {
		t.Fatal(list, err)
	}
	if st[0].Model != "fake-final" || st[0].State != "downloaded" {
		t.Fatal(st)
	}
	if err := e.Delete(ctx(t), "fake-final", nil); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(manifest); !errors.Is(err, fs.ErrNotExist) {
		t.Fatal("the manifest stays after delete")
	}
}

func TestTranscribeGivesWordsAndSegments(t *testing.T) {
	e, _ := start(t)
	ready(t, e, "fake-final")
	path, _ := speech(t, 900, 600)
	f, err := e.Transcribe(ctx(t), engine.TranscribeRequest{Model: "fake-final", Path: path, Language: "auto", Segments: true})
	if err != nil {
		t.Fatal(err)
	}
	if f.Text != "alpha bravo charlie delta echo" || len(f.Segments) != 2 || f.Segments[0].Words[0].Text != "alpha" || f.Language != "en" {
		t.Fatalf("%+v", f)
	}
	again, _ := e.Transcribe(ctx(t), engine.TranscribeRequest{Model: "fake-final", Path: path, Language: "en"})
	if again.Text != f.Text || again.Language != "" || again.Segments != nil {
		t.Fatalf("%+v", again)
	}
}

func TestAStreamGivesPartialsAndItsText(t *testing.T) {
	e, _ := start(t)
	ready(t, e, "fake-stream")
	_, samples := speech(t, 900)
	const session = 1 << 40
	if err := e.StreamStart(ctx(t), session, "fake-stream", "en"); err != nil {
		t.Fatal(err)
	}
	for i := 0; i < len(samples); i += 2560 {
		if err := e.StreamAudio(session, samples[i:min(i+2560, len(samples))]); err != nil {
			t.Fatal(err)
		}
	}
	text, err := e.StreamEnd(ctx(t), session)
	if err != nil || text != "alpha bravo charlie" {
		t.Fatal(text, err)
	}
	var last engine.Partial
	timeout := time.After(5 * time.Second)
	for last.Tentative != "charlie" {
		select {
		case m := <-e.Events():
			if m.Type == "partial" {
				m.Decode(&last)
			}
		case <-timeout:
			t.Fatalf("last partial %+v", last)
		}
	}
	if last.Session != session || last.Committed != "alpha bravo" {
		t.Fatalf("%+v", last)
	}
}

func TestAVadSessionGivesItsSpeech(t *testing.T) {
	e, _ := start(t)
	ready(t, e, "fake-vad")
	path, samples := speech(t, 800, 800)
	parts, err := e.Vad(ctx(t), "fake-vad", path)
	if err != nil || len(parts) != 2 {
		t.Fatal(parts, err)
	}
	if err := e.VadStart(ctx(t), 7, "fake-vad"); err != nil {
		t.Fatal(err)
	}
	for i := 0; i < len(samples); i += 3200 {
		e.StreamAudio(7, samples[i:min(i+3200, len(samples))])
	}
	if _, err := e.StreamEnd(ctx(t), 7); err != nil {
		t.Fatal(err)
	}
	var kinds []string
	for len(kinds) < 4 {
		m := <-e.Events()
		if m.Type == "speech_start" || m.Type == "speech_end" {
			kinds = append(kinds, m.Type)
		}
	}
	if strings.Join(kinds, ",") != "speech_start,speech_end,speech_start,speech_end" {
		t.Fatal(kinds)
	}
}

func TestDiarizeNumbersTheSpeakers(t *testing.T) {
	e, _ := start(t)
	ready(t, e, "fake-diar")
	path, _ := speech(t, 600, 600, 600)
	d, err := e.Diarize(ctx(t), "fake-diar", path, 2)
	if err != nil || d.Speakers != 2 || len(d.Turns) != 3 || d.Turns[2].Speaker != 0 {
		t.Fatal(d, err)
	}
}

func TestErrorRepliesHaveTheirCodes(t *testing.T) {
	e, _ := start(t, `SAYSO_FAKE_SCRIPT={"max_streams":1}`)
	ready(t, e, "fake-stream")
	var ee *engine.Error
	if _, err := e.Load(ctx(t), "fake-old", nil); !errors.As(err, &ee) || ee.Code != "model_retired" {
		t.Fatal(err)
	}
	if _, err := e.Load(ctx(t), "nothing", nil); !errors.As(err, &ee) || ee.Code != "unknown_model" {
		t.Fatal(err)
	}
	if err := e.StreamStart(ctx(t), 1, "fake-stream", ""); err != nil {
		t.Fatal(err)
	}
	if err := e.StreamStart(ctx(t), 2, "fake-stream", ""); !errors.As(err, &ee) || ee.Code != "too_many_streams" {
		t.Fatal(err)
	}
	if _, err := e.Transcribe(ctx(t), engine.TranscribeRequest{Model: "fake-vad", Path: "/x.wav"}); !errors.As(err, &ee) || !strings.Contains(ee.Message, "not loaded") {
		t.Fatal(err)
	}
}

func TestAStartFailureCarriesTheTailOfStderr(t *testing.T) {
	_, err := engine.Start(context.Background(), engine.Options{
		Program:   testbin.Path(t, testbin.FakeEngine),
		ModelsDir: t.TempDir(),
		Env:       []string{`SAYSO_FAKE_SCRIPT={"stderr":["libsherpa-onnx: not found"],"exit_at_start":3}`},
	})
	var se *engine.StartError
	if !errors.As(err, &se) || !slices.Contains(se.Stderr, "libsherpa-onnx: not found") || !strings.Contains(err.Error(), "libsherpa-onnx: not found") {
		t.Fatalf("%v", err)
	}
	if _, err := engine.Start(context.Background(), engine.Options{Program: filepath.Join(t.TempDir(), "none"), ModelsDir: t.TempDir()}); !errors.As(err, &se) {
		t.Fatal(err)
	}
}

func TestACrashEndsTheRequestsAndTheEvents(t *testing.T) {
	e, _ := start(t, `SAYSO_FAKE_SCRIPT={"crash_at":3}`)
	if _, err := e.ListModels(ctx(t), []engine.ModelRef{{ID: "fake-final"}}); err != nil {
		t.Fatal(err)
	}
	if _, err := e.Load(ctx(t), "fake-final", nil); !errors.Is(err, engine.ErrStopped) {
		t.Fatal(err)
	}
	select {
	case <-e.Done():
	case <-time.After(5 * time.Second):
		t.Fatal("Done did not close")
	}
	if err := e.StreamAudio(1, []int16{1}); !errors.Is(err, engine.ErrStopped) {
		t.Fatal(err)
	}
}

func TestTheEngineCorpusDecodes(t *testing.T) {
	cases, err := corpus.Read("engine", "")
	if err != nil {
		t.Fatal(err)
	}
	seen := map[string]bool{}
	for _, c := range cases {
		var m map[string]any
		if err := json.Unmarshal([]byte(c.Message), &m); err != nil {
			t.Fatalf("%s: %v", c, err)
		}
		kind, _ := m["type"].(string)
		seen[kind] = true
		known := slices.Contains(protocol.EngineRequestTypes, kind) || slices.Contains(protocol.EngineReplyTypes, kind) || slices.Contains(protocol.EngineEventTypes, kind)
		if !known && !c.DecodeOnly {
			t.Errorf("%s: unknown type %s", c, kind)
		}
		if f, ok := m["features"].([]any); ok {
			for _, x := range f {
				if !slices.Contains(engine.KnownFeatures, x.(string)) {
					t.Errorf("%s: unknown feature %v", c, x)
				}
			}
		}
		if kind == "final" {
			var f engine.Final
			if err := json.Unmarshal([]byte(c.Message), &f); err != nil {
				t.Errorf("%s: %v", c, err)
			}
		}
	}
	for _, kind := range slices.Concat(protocol.EngineRequestTypes, protocol.EngineReplyTypes, protocol.EngineEventTypes) {
		if !seen[kind] {
			t.Errorf("no case of %s", kind)
		}
	}
}

func touch(t *testing.T, path string) {
	t.Helper()
	os.MkdirAll(filepath.Dir(path), 0o755)
	if err := os.WriteFile(path, nil, 0o755); err != nil {
		t.Fatal(err)
	}
}

func TestFindTakesTheLastVersionFolder(t *testing.T) {
	dir := t.TempDir()
	touch(t, filepath.Join(dir, "sayso-engine-0.11.0", engine.Program()))
	touch(t, filepath.Join(dir, "sayso-engine-0.12.0", engine.Program()))
	touch(t, filepath.Join(dir, "sayso-engine-0.13.0", "README.md"))
	touch(t, filepath.Join(dir, "other", engine.Program()))
	got, err := engine.Find(dir)
	if err != nil || got != filepath.Join(dir, "sayso-engine-0.12.0", engine.Program()) {
		t.Fatal(got, err)
	}
	direct := filepath.Join(dir, "sayso-engine-0.11.0")
	if got, _ := engine.Find(direct); got != filepath.Join(direct, engine.Program()) {
		t.Fatal(got)
	}
	if _, err := engine.Find(filepath.Join(dir, "other", "x")); err == nil {
		t.Fatal("a missing folder is an error")
	}
	empty := t.TempDir()
	if _, err := engine.Find(empty); !errors.Is(err, fs.ErrNotExist) || !strings.Contains(err.Error(), "Unpack a release file") {
		t.Fatal(err)
	}
}

func TestFindTakesABuildThatThisProcessorRuns(t *testing.T) {
	dir := t.TempDir()
	exe := ""
	if runtime.GOOS == "windows" {
		exe = ".exe"
	}
	for _, name := range []string{"sayso-engine", "sayso-engine-avx", "sayso-engine-baseline"} {
		touch(t, filepath.Join(dir, name+exe))
	}
	got, err := engine.Find(dir)
	if err != nil {
		t.Fatal(err)
	}
	if runtime.GOARCH != "amd64" && got != filepath.Join(dir, "sayso-engine"+exe) {
		t.Fatal(got)
	}
	if !strings.HasPrefix(filepath.Base(got), "sayso-engine") {
		t.Fatal(got)
	}
}
