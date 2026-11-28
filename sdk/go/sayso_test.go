package sayso_test

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	sayso "github.com/watzon/sayso/sdk/go"
	"github.com/watzon/sayso/sdk/go/engine"
	"github.com/watzon/sayso/sdk/go/internal/testbin"
	"github.com/watzon/sayso/sdk/go/protocol"
)

func TestMain(m *testing.M) {
	code := m.Run()
	testbin.Cleanup()
	os.Exit(code)
}

var all = []sayso.Capability{sayso.CapModelsRead, sayso.CapModelsDownload, sayso.CapTranscribeFile, sayso.CapSessionLive, sayso.CapAudioMic, sayso.CapAiEnhance, sayso.CapDictationObserve}

func ctx(t *testing.T) context.Context {
	c, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	t.Cleanup(cancel)
	return c
}

func connect(t *testing.T, opts sayso.Options) *sayso.Speech {
	t.Helper()
	if opts.Engine.Program == "" {
		opts.Engine = sayso.FromProgram(testbin.Path(t, testbin.FakeEngine))
	}
	if opts.ModelsDir == "" {
		opts.ModelsDir = filepath.Join(t.TempDir(), "models")
	}
	if opts.App == "" {
		opts.App = "Notes"
	}
	if opts.Capabilities == nil {
		opts.Capabilities = all
	}
	s, err := sayso.Connect(ctx(t), opts)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { s.Close() })
	return s
}

func next[T sayso.Event](t *testing.T, s *sayso.Speech, seen *[]string) T {
	t.Helper()
	timeout := time.After(20 * time.Second)
	for {
		select {
		case ev, ok := <-s.Events():
			if !ok {
				t.Fatalf("the events ended; seen %v", *seen)
			}
			*seen = append(*seen, ev.Kind())
			if v, ok := ev.(T); ok {
				return v
			}
		case <-timeout:
			var zero T
			t.Fatalf("no %T; seen %v", zero, *seen)
		}
	}
}

func TestLocalModeTranscribesAFile(t *testing.T) {
	testbin.Passed(t, "G01")
	testbin.Passed(t, "J04")
	s := connect(t, sayso.Options{Mode: sayso.Local})
	if f := s.Features(); f.Mode != sayso.ModeLocal || f.Enhance || f.Subscribe || !slices.Contains(f.Engine, "segments") {
		t.Fatalf("%+v", f)
	}
	models, err := s.ListModels(ctx(t), nil)
	if err != nil || len(models) != 4 || models[0].State != protocol.StateNotDownloaded {
		t.Fatal(models, err)
	}
	path := testbin.SpeechWAV(t, 900, 600)
	if _, err := s.TranscribeFile(ctx(t), path, sayso.FileOptions{}); !errors.Is(err, sayso.ErrModelNotDownloaded) {
		t.Fatal(err)
	}
	var kinds []string
	if err := s.DownloadModel(ctx(t), "fake-final", "tests", func(ev sayso.Event) { kinds = append(kinds, ev.Kind()) }); err != nil {
		t.Fatal(err)
	}
	if !slices.Contains(kinds, "download_progress") || kinds[len(kinds)-1] != "model_state" {
		t.Fatal(kinds)
	}
	if err := s.DownloadModel(ctx(t), "nothing", "tests", nil); !errors.Is(err, sayso.ErrUnknownModel) {
		t.Fatal(err)
	}
	job, err := s.TranscribeFile(ctx(t), path, sayso.FileOptions{})
	if err != nil {
		t.Fatal(err)
	}
	var seen []string
	done := next[*protocol.JobDone](t, s, &seen)
	if done.Job != job || done.Text != "alpha bravo charlie delta echo" {
		t.Fatalf("%+v", done)
	}
	if strings.Join(seen, ",") != "job_progress,segment,segment,job_progress,job_done" {
		t.Fatal(seen)
	}
	if _, err := s.TranscribeFile(ctx(t), "relative.wav", sayso.FileOptions{}); !errors.Is(err, sayso.ErrBadRequest) {
		t.Fatal(err)
	}
}

func TestLocalModeDiarizes(t *testing.T) {
	testbin.Passed(t, "J02")
	s := connect(t, sayso.Options{Mode: sayso.Local})
	for _, m := range []string{"fake-final", "fake-diar"} {
		if err := s.DownloadModel(ctx(t), m, "tests", nil); err != nil {
			t.Fatal(err)
		}
	}
	_, err := s.TranscribeFile(ctx(t), testbin.SpeechWAV(t, 600, 600), sayso.FileOptions{Diarize: true, Speakers: 2})
	if err != nil {
		t.Fatal(err)
	}
	var seen []string
	sp := next[*protocol.SpeakersEvent](t, s, &seen)
	if len(sp.Turns) != 2 || sp.Turns[0].Speaker != "1" || sp.Turns[1].Speaker != "2" {
		t.Fatalf("%+v", sp)
	}
	next[*protocol.JobDone](t, s, &seen)
}

func TestLocalModeCancelsAJob(t *testing.T) {
	testbin.Passed(t, "J03")
	s := connect(t, sayso.Options{Mode: sayso.Local, Engine: sayso.EngineSpec{Program: testbin.Path(t, testbin.FakeEngine), Env: []string{`SAYSO_FAKE_SCRIPT={"delay_ms":{"transcribe":1500}}`}}})
	if err := s.DownloadModel(ctx(t), "fake-final", "tests", nil); err != nil {
		t.Fatal(err)
	}
	job, err := s.TranscribeFile(ctx(t), testbin.SpeechWAV(t, 600), sayso.FileOptions{})
	if err != nil {
		t.Fatal(err)
	}
	if err := s.CancelJob(ctx(t), job); err != nil {
		t.Fatal(err)
	}
	var seen []string
	if f := next[*protocol.JobFailed](t, s, &seen); f.Message != "The job was cancelled." {
		t.Fatal(f)
	}
	time.Sleep(1800 * time.Millisecond)
	select {
	case ev := <-s.Events():
		t.Fatalf("an event after the end of the job: %v", ev.Kind())
	default:
	}
}

func TestLocalModeRunsALiveSessionOfTheClient(t *testing.T) {
	testbin.Passed(t, "G02")
	s := connect(t, sayso.Options{Mode: sayso.Local, Capture: sayso.CaptureSources{Off: true}})
	if err := s.DownloadModel(ctx(t), "fake-stream", "tests", nil); err != nil {
		t.Fatal(err)
	}
	if _, err := s.StartSession(ctx(t), sayso.SessionOptions{Sources: []sayso.Source{sayso.SourceMic}}); !errors.Is(err, sayso.ErrUnsupported) {
		t.Fatal(err)
	}
	rec := filepath.Join(t.TempDir(), "rec")
	sess, err := s.StartSession(ctx(t), sayso.SessionOptions{Model: "fake-stream", RecordTo: rec})
	if err != nil {
		t.Fatal(err)
	}
	samples := testbin.Speech(900)
	for i := 0; i < len(samples); i += 2560 {
		if err := s.SendAudio(sess.ID, samples[i:min(i+2560, len(samples))]); err != nil {
			t.Fatal(err)
		}
	}
	var seen []string
	p := next[*protocol.PartialEvent](t, s, &seen)
	if p.Session != sess.ID || p.Source == nil || *p.Source != sayso.SourceClient {
		t.Fatalf("%+v", p)
	}
	text, err := s.StopSession(ctx(t), sess.ID)
	if err != nil || text != "alpha bravo charlie" {
		t.Fatal(text, err)
	}
	if _, err := os.Stat(filepath.Join(rec, "client.wav")); err != nil {
		t.Fatal(err)
	}
}

func TestLocalModeNeedsSaysoForEnhanceAndSubscribe(t *testing.T) {
	s := connect(t, sayso.Options{Mode: sayso.Local})
	if _, err := s.Enhance(ctx(t), "hi", sayso.EnhanceOptions{}); !errors.Is(err, sayso.ErrUnsupported) {
		t.Fatal(err)
	}
	if _, err := s.Subscribe(ctx(t), []string{"dictation_done"}); !errors.Is(err, sayso.ErrUnsupported) {
		t.Fatal(err)
	}
}

func TestAnEngineThatDoesNotStartSaysWhy(t *testing.T) {
	testbin.Passed(t, "G04")
	_, err := sayso.Connect(ctx(t), sayso.Options{Mode: sayso.Local, ModelsDir: t.TempDir(), Engine: sayso.EngineSpec{Program: testbin.Path(t, testbin.FakeEngine), Env: []string{`SAYSO_FAKE_SCRIPT={"stderr":["cannot load libwhisper"],"exit_at_start":4}`}}})
	var se *engine.StartError
	if !errors.As(err, &se) || !strings.Contains(err.Error(), "cannot load libwhisper") {
		t.Fatal(err)
	}
	if _, err := sayso.Connect(ctx(t), sayso.Options{Mode: sayso.Local}); !errors.Is(err, sayso.ErrUnsupported) {
		t.Fatalf("Local mode without an engine: %v", err)
	}
}

func TestSaysoModeWithoutSaysoIsHostNotRunning(t *testing.T) {
	testbin.Passed(t, "A04")
	_, err := sayso.Connect(ctx(t), sayso.Options{Mode: sayso.Sayso, Socket: testbin.SocketPath(t)})
	if !errors.Is(err, sayso.ErrHostNotRunning) {
		t.Fatal(err)
	}
}

func TestSaysoModeUsesTheHost(t *testing.T) {
	h := testbin.StartHost(t)
	s := connect(t, sayso.Options{Mode: sayso.Sayso, Socket: h.Socket})
	f := s.Features()
	if f.Mode != sayso.ModeSayso || !f.Enhance || !f.Subscribe || !f.DownloadQuestion || f.MicCapture || !slices.Contains(f.Host, "record_to") {
		t.Fatalf("%+v", f)
	}
	if _, err := s.StartSession(ctx(t), sayso.SessionOptions{Sources: []sayso.Source{sayso.SourceMic}}); !errors.Is(err, sayso.ErrUnsupported) || !strings.Contains(err.Error(), "mic_capture") {
		t.Fatalf("the SDK gates the microphone on the host features: %v", err)
	}
	r, err := s.Enhance(ctx(t), "um hello", sayso.EnhanceOptions{Prompt: "Fix."})
	if err != nil || r.Text != "[app-prompt] um hello" {
		t.Fatal(r, err)
	}
	got, err := s.Subscribe(ctx(t), []string{"dictation_done"})
	if err != nil || !slices.Equal(got, []string{"dictation_done"}) {
		t.Fatal(got, err)
	}
	h.Send(map[string]any{"dictation_done": map[string]any{"text": "Hi.", "transcript": "hi"}})
	var seen []string
	if d := next[*protocol.DictationDone](t, s, &seen); d.Text != "Hi." {
		t.Fatal(d)
	}
}

func TestADeniedAppIsNotApproved(t *testing.T) {
	testbin.Passed(t, "H02")
	h := testbin.StartHost(t, "--approve", "deny")
	_, err := sayso.Connect(ctx(t), sayso.Options{Mode: sayso.Auto, Socket: h.Socket, Engine: sayso.FromProgram(testbin.Path(t, testbin.FakeEngine)), ModelsDir: t.TempDir()})
	if !errors.Is(err, sayso.ErrNotApproved) {
		t.Fatal(err)
	}
}

func TestAutoFollowsSaysoWhileIdle(t *testing.T) {
	testbin.Passed(t, "A01")
	testbin.Passed(t, "A03")
	socket := testbin.SocketPath(t)
	s := connect(t, sayso.Options{Mode: sayso.Auto, Socket: socket})
	if s.Mode() != sayso.ModeLocal {
		t.Fatal("no Sayso: Local")
	}
	if err := s.DownloadModel(ctx(t), "fake-stream", "tests", nil); err != nil {
		t.Fatal(err)
	}
	sess, err := s.StartSession(ctx(t), sayso.SessionOptions{Model: "fake-stream"})
	if err != nil {
		t.Fatal(err)
	}
	h := testbin.StartHostAt(t, socket)
	if _, err := s.ListModels(ctx(t), nil); err != nil || s.Mode() != sayso.ModeLocal {
		t.Fatal("a running session keeps Local", s.Mode(), err)
	}
	if _, err := s.StopSession(ctx(t), sess.ID); err != nil {
		t.Fatal(err)
	}
	if _, err := s.ListModels(ctx(t), nil); err != nil {
		t.Fatal(err)
	}
	var seen []string
	if mc := next[sayso.ModeChanged](t, s, &seen); mc.Mode != sayso.ModeSayso || len(mc.Features.Host) == 0 || s.Mode() != sayso.ModeSayso {
		t.Fatalf("%+v", mc)
	}
	h.Stop()
	if _, err := s.ListModels(ctx(t), nil); err != nil {
		t.Fatal(err)
	}
	if mc := next[sayso.ModeChanged](t, s, &seen); mc.Mode != sayso.ModeLocal || s.Mode() != sayso.ModeLocal {
		t.Fatalf("%+v %v", mc, seen)
	}
	if !slices.Contains(seen, "host_closing") {
		t.Fatal(seen)
	}
}

func TestSaysoModeStartsSaysoWithALauncher(t *testing.T) {
	testbin.Passed(t, "S01")
	socket := testbin.SocketPath(t)
	host := testbin.Path(t, testbin.TestHost)
	fake := testbin.Path(t, testbin.FakeEngine)
	var started *exec.Cmd
	launch := func(context.Context) error {
		started = exec.Command(host, "--socket", socket, "--engine", fake, "--data", t.TempDir())
		stdin, err := started.StdinPipe()
		if err != nil {
			return err
		}
		t.Cleanup(func() { stdin.Close(); started.Wait() })
		return started.Start()
	}
	failing := func(context.Context) error { return errors.New("not installed here") }
	s := connect(t, sayso.Options{Mode: sayso.Sayso, Start: true, StartWait: 15 * time.Second, Socket: socket, Launchers: []sayso.Launcher{failing, launch}})
	if s.Mode() != sayso.ModeSayso || started == nil {
		t.Fatal(s.Mode())
	}
	if err := sayso.StartSayso(ctx(t), []sayso.Launcher{failing}); !errors.Is(err, sayso.ErrHostNotRunning) || !strings.Contains(err.Error(), "not installed here") {
		t.Fatal(err)
	}
}

func TestTheSaysoModeReconnectsAfterTheHostCameBack(t *testing.T) {
	testbin.Passed(t, "X03")
	socket := testbin.SocketPath(t)
	h := testbin.StartHostAt(t, socket)
	s := connect(t, sayso.Options{Mode: sayso.Sayso, Socket: socket})
	h.Stop()
	_, err := s.ListModels(ctx(t), nil)
	if errors.Is(err, sayso.ErrConnectionLost) {
		_, err = s.ListModels(ctx(t), nil)
	}
	if !errors.Is(err, sayso.ErrHostNotRunning) {
		t.Fatal(err)
	}
	testbin.StartHostAt(t, socket)
	if _, err := s.ListModels(ctx(t), nil); err != nil {
		t.Fatal(err)
	}
}

func TestTheModelStoreIsInTheModelsFolder(t *testing.T) {
	if filepath.Dir(sayso.ModelStore()) != sayso.ModelsDir() || filepath.Base(sayso.ModelStore()) != "v1" {
		t.Fatal(sayso.ModelStore())
	}
	if sayso.Version != "0.18.0" {
		t.Fatal(sayso.Version)
	}
}

func TestAnAppOutsideTheRepositoryBuildsWithTheSDK(t *testing.T) {
	if testing.Short() {
		t.Skip("builds a module")
	}
	dir := t.TempDir()
	module := testbin.ModuleDir()
	gomod := "module example.com/notes\n\ngo 1.26.0\n\nrequire github.com/watzon/sayso/sdk/go v0.0.0\n\nreplace github.com/watzon/sayso/sdk/go => " + filepath.ToSlash(module) + "\n"
	main := `package main

import (
	"context"
	"errors"
	"fmt"

	sayso "github.com/watzon/sayso/sdk/go"
	"github.com/watzon/sayso/sdk/go/client"
	"github.com/watzon/sayso/sdk/go/conformance"
)

func main() {
	_, err := sayso.Connect(context.Background(), sayso.Options{Mode: sayso.Sayso})
	fmt.Println(errors.Is(err, sayso.ErrHostNotRunning), client.MaxLine > 0, conformance.Core)
}
`
	sum, err := os.ReadFile(filepath.Join(module, "go.sum"))
	if err != nil {
		t.Fatal(err)
	}
	for name, text := range map[string]string{"go.mod": gomod, "main.go": main, "go.sum": string(sum)} {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(text), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	run := func(args ...string) string {
		cmd := exec.Command("go", args...)
		cmd.Dir = dir
		cmd.Env = append(os.Environ(), "CGO_ENABLED=0", "GOFLAGS=-mod=mod", "GOWORK=off")
		out, err := cmd.CombinedOutput()
		if err != nil {
			t.Fatalf("go %v: %v\n%s", args, err, out)
		}
		return string(out)
	}
	run("build", "-o", filepath.Join(dir, "notes"), ".")
	for _, goos := range []string{"darwin", "linux", "windows"} {
		cmd := exec.Command("go", "list", "-deps", ".")
		cmd.Dir = dir
		cmd.Env = append(os.Environ(), "CGO_ENABLED=0", "GOFLAGS=-mod=mod", "GOWORK=off", "GOOS="+goos)
		out, err := cmd.CombinedOutput()
		if err != nil {
			t.Fatalf("%s: %v\n%s", goos, err, out)
		}
		for _, p := range strings.Fields(string(out)) {
			if strings.HasPrefix(p, "github.com/watzon/sayso/") && !strings.HasPrefix(p, "github.com/watzon/sayso/sdk/go") {
				t.Fatalf("%s: the app imports %s", goos, p)
			}
		}
	}
}
