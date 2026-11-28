package sayso_test

import (
	"bufio"
	"encoding/json"
	"errors"
	"net"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	sayso "github.com/watzon/sayso/sdk/go"
	"github.com/watzon/sayso/sdk/go/audio"
	"github.com/watzon/sayso/sdk/go/internal/testbin"
	"github.com/watzon/sayso/sdk/go/protocol"
)

func TestAnUnansweredQuestionWaitsAndTheCallbackRunsOnce(t *testing.T) {
	testbin.Passed(t, "H03")
	h := testbin.StartHost(t, "--approve", "ignore")
	var calls atomic.Int32
	type result struct {
		s   *sayso.Speech
		err error
	}
	done := make(chan result, 1)
	go func() {
		s, err := sayso.Connect(ctx(t), sayso.Options{Mode: sayso.Sayso, App: "Notes", Capabilities: all, Socket: h.Socket, OnApprovalWaiting: func() { calls.Add(1) }})
		done <- result{s, err}
	}()
	h.Await("question", "approval")
	select {
	case r := <-done:
		t.Fatalf("connect returned before the answer: %v", r.err)
	case <-time.After(300 * time.Millisecond):
	}
	h.Send(map[string]any{"approve": "allow"})
	r := <-done
	if r.err != nil {
		t.Fatal(r.err)
	}
	t.Cleanup(func() { r.s.Close() })
	if calls.Load() != 1 || r.s.Mode() != sayso.ModeSayso {
		t.Fatal(calls.Load(), r.s.Mode())
	}
	select {
	case ev := <-r.s.Events():
		t.Fatalf("the wait is for the callback only: %v", ev.Kind())
	default:
	}
}

func TestTheSDKRefusesTheSourcesThatTheHostLacks(t *testing.T) {
	testbin.Passed(t, "H06")
	h := testbin.StartHost(t)
	s := connect(t, sayso.Options{Mode: sayso.Sayso, Socket: h.Socket})
	for source, feature := range map[sayso.Source]string{sayso.SourceMic: "mic_capture", sayso.SourceSystem: "system_capture"} {
		_, err := s.StartSession(ctx(t), sayso.SessionOptions{Sources: []sayso.Source{source}})
		var e *protocol.Error
		if !errors.As(err, &e) || e.Code != protocol.CodeUnsupported || !strings.Contains(e.Message, feature) || e.Capability != "" {
			t.Fatalf("%s: %v", source, err)
		}
	}
}

// oldHost answers hello with no features, gives every other request the
// error unsupported, and sends the type of each request to seen.
func oldHost(t *testing.T, socket string, seen chan<- string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(socket), 0o700); err != nil {
		t.Fatal(err)
	}
	ln, err := net.Listen("unix", socket)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ln.Close() })
	go func() {
		conn, err := ln.Accept()
		if err != nil {
			return
		}
		defer conn.Close()
		sc := bufio.NewScanner(conn)
		for sc.Scan() {
			var req map[string]any
			if json.Unmarshal(sc.Bytes(), &req) != nil {
				return
			}
			reply := map[string]any{"v": 1, "id": req["id"]}
			if req["type"] == "hello" {
				reply["type"], reply["host_version"], reply["protocol"], reply["features"], reply["capabilities"] = "ok", "0.17.0", 1, []string{}, []string{"session.live", "audio.mic"}
			} else {
				reply["type"], reply["code"], reply["message"] = "error", "unsupported", "the host said so"
				seen <- req["type"].(string)
			}
			line, _ := json.Marshal(reply)
			if _, err := conn.Write(append(line, '\n')); err != nil {
				return
			}
		}
	}()
}

func TestAHostWithNoFeaturesGetsTheRequestAndAnswersForItself(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("the fake old host listens on a Unix socket")
	}
	testbin.Passed(t, "H07")
	socket := testbin.SocketPath(t)
	seen := make(chan string, 4)
	oldHost(t, socket, seen)
	s := connect(t, sayso.Options{Mode: sayso.Sayso, Socket: socket, Capabilities: []sayso.Capability{sayso.CapSessionLive, sayso.CapAudioMic}})
	wait := func(want string) {
		t.Helper()
		select {
		case got := <-seen:
			if got != want {
				t.Fatalf("the host got %s, not %s", got, want)
			}
		case <-time.After(5 * time.Second):
			t.Fatalf("the host got no %s", want)
		}
	}
	_, err := s.StartSession(ctx(t), sayso.SessionOptions{Sources: []sayso.Source{sayso.SourceMic}, SilenceLimit: time.Second, RecordTo: t.TempDir()})
	var e *protocol.Error
	if !errors.As(err, &e) || e.Code != protocol.CodeUnsupported || e.Message != "the host said so" {
		t.Fatalf("the host answers, not the SDK: %v", err)
	}
	wait("session_start")
	if _, err := s.Enhance(ctx(t), "a", sayso.EnhanceOptions{}); err == nil {
		t.Fatal("enhance worked")
	}
	wait("enhance")
	if _, err := s.Subscribe(ctx(t), []string{"dictation_done"}); err == nil {
		t.Fatal("subscribe worked")
	}
	wait("subscribe")
}

func TestAutoEndsTheJobOfAKilledHostOnceAndGoesLocal(t *testing.T) {
	testbin.Passed(t, "A02")
	socket := testbin.SocketPath(t)
	h := testbin.StartHostAt(t, socket, "--speed", "0.5")
	s := connect(t, sayso.Options{Mode: sayso.Auto, Socket: socket})
	if s.Mode() != sayso.ModeSayso {
		t.Fatal("Sayso runs, so Auto uses it")
	}
	final := protocol.RoleFinal
	models, err := s.ListModels(ctx(t), &final)
	if err != nil {
		t.Fatal(err)
	}
	i := slices.IndexFunc(models, func(m protocol.ModelInfo) bool { return slices.Contains(m.Choice, protocol.RoleFinal) })
	if i < 0 {
		t.Fatal(models)
	}
	if err := s.DownloadModel(ctx(t), models[i].ID, "tests", nil); err != nil {
		t.Fatal(err)
	}
	long := make([]int, 60)
	for i := range long {
		long[i] = 900
	}
	job, err := s.TranscribeFile(ctx(t), testbin.SpeechWAV(t, long...), sayso.FileOptions{})
	if err != nil {
		t.Fatal(err)
	}
	h.Send(map[string]any{"kill": true})
	<-h.Exited()
	var seen []string
	if f := next[*protocol.JobFailed](t, s, &seen); f.Job != job || f.Code == nil || *f.Code != protocol.CodeConnectionLost {
		t.Fatalf("%+v", f)
	}
	if _, err := s.ListModels(ctx(t), nil); err != nil {
		t.Fatal("the next request runs in Local mode:", err)
	}
	if mc := next[sayso.ModeChanged](t, s, &seen); mc.Mode != sayso.ModeLocal || s.Mode() != sayso.ModeLocal {
		t.Fatalf("%+v", mc)
	}
	ends := 0
	for _, k := range seen {
		if k == "job_done" || k == "job_failed" {
			ends++
		}
	}
	if ends != 1 {
		t.Fatal(seen)
	}
}

func TestLocalModeCutsAFileWhereTheVoiceStopsForLong(t *testing.T) {
	testbin.Passed(t, "G03")
	s := connect(t, sayso.Options{Mode: sayso.Local})
	if err := s.DownloadModel(ctx(t), "fake-final", "tests", nil); err != nil {
		t.Fatal(err)
	}
	// Two tones of 3 words with 5 s of silence between them: longer than the
	// longest pause inside one chunk, so the final pass runs on two chunks,
	// and the fake engine counts its words from alpha in each.
	samples := make([]int16, 8000)
	tone := testbin.Speech(900)[8000 : 8000+900*16]
	for range 2 {
		samples = append(samples, tone...)
		samples = append(samples, make([]int16, 5*audio.SampleRate)...)
	}
	path := filepath.Join(t.TempDir(), "cut.wav")
	if err := audio.WriteWAV(path, samples); err != nil {
		t.Fatal(err)
	}
	job, err := s.TranscribeFile(ctx(t), path, sayso.FileOptions{Model: "fake-final", VadModel: "basic"})
	if err != nil {
		t.Fatal(err)
	}
	var seen []string
	if done := next[*protocol.JobDone](t, s, &seen); done.Job != job || done.Text != "alpha bravo charlie alpha bravo charlie" {
		t.Fatalf("%+v", done)
	}
}
