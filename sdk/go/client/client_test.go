package client_test

import (
	"context"
	"errors"
	"os"
	"slices"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/watzon/sayso/sdk/go/audio"
	"github.com/watzon/sayso/sdk/go/client"
	"github.com/watzon/sayso/sdk/go/internal/testbin"
	"github.com/watzon/sayso/sdk/go/protocol"
)

func TestMain(m *testing.M) {
	code := m.Run()
	testbin.Cleanup()
	os.Exit(code)
}

var all = []protocol.Capability{protocol.CapModelsRead, protocol.CapModelsDownload, protocol.CapTranscribeFile, protocol.CapSessionLive, protocol.CapAudioMic, protocol.CapAudioSystem, protocol.CapAiEnhance, protocol.CapDictationObserve}

func ctx(t *testing.T) context.Context {
	c, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	t.Cleanup(cancel)
	return c
}

func dial(t *testing.T, h *testbin.Host) *client.Client {
	t.Helper()
	c, err := client.Dial(ctx(t), h.Socket)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { c.Close() })
	return c
}

func hello(t *testing.T, c *client.Client, caps ...protocol.Capability) protocol.HelloReply {
	t.Helper()
	app := protocol.AppName{Name: "Notes"}
	r, err := c.Hello(ctx(t), protocol.Hello{App: &app, Capabilities: caps}, nil)
	if err != nil {
		t.Fatal(err)
	}
	return r
}

func code(err error) protocol.ErrorCode {
	var e *protocol.Error
	if errors.As(err, &e) {
		return e.Code
	}
	return ""
}

// ready downloads the final model of the user and gives its id.
func ready(t *testing.T, c *client.Client) string {
	t.Helper()
	final := protocol.RoleFinal
	models, err := c.ListModels(ctx(t), &final)
	if err != nil {
		t.Fatal(err)
	}
	for _, m := range models {
		if slices.Contains(m.Choice, protocol.RoleFinal) {
			if err := c.DownloadModel(ctx(t), m.ID, "tests", nil); err != nil {
				t.Fatal(err)
			}
			return m.ID
		}
	}
	t.Fatalf("no final model of the user in %+v", models)
	return ""
}

func until[T protocol.Event](t *testing.T, c *client.Client, seen *[]protocol.Event) T {
	t.Helper()
	timeout := time.After(20 * time.Second)
	for {
		select {
		case ev, ok := <-c.Events():
			if !ok {
				t.Fatalf("the events ended; seen %v", kinds(*seen))
			}
			*seen = append(*seen, ev)
			if v, ok := ev.(T); ok {
				return v
			}
		case <-timeout:
			var zero T
			t.Fatalf("no %T; seen %v", zero, kinds(*seen))
		}
	}
}

func kinds(evs []protocol.Event) []string {
	out := make([]string, len(evs))
	for i, e := range evs {
		out[i] = e.Kind()
	}
	return out
}

func TestNoHostIsHostNotRunning(t *testing.T) {
	_, err := client.Dial(ctx(t), testbin.SocketPath(t))
	if code(err) != protocol.CodeHostNotRunning || !errors.Is(err, &protocol.Error{Code: protocol.CodeHostNotRunning}) {
		t.Fatal(err)
	}
}

func TestTheHostAsksAndAllows(t *testing.T) {
	testbin.Passed(t, "H01")
	h := testbin.StartHost(t)
	c := dial(t, h)
	var waited atomic.Bool
	app := protocol.AppName{Name: "Notes"}
	r, err := c.Hello(ctx(t), protocol.Hello{App: &app, Capabilities: []protocol.Capability{protocol.CapTranscribeFile, "calendar.read"}}, func() { waited.Store(true) })
	if err != nil {
		t.Fatal(err)
	}
	q := h.Await("question", "approval")
	if q["claimed"] != "Notes" || !waited.Load() {
		t.Fatal(q, waited.Load())
	}
	if !slices.Contains(r.Capabilities, protocol.CapTranscribeFile) || !slices.Contains(r.Denied, "calendar.read") || r.HostVersion == "" || r.Protocol != 1 {
		t.Fatalf("%+v", r)
	}
	for _, f := range []string{"dictation_done", "download_question", "enhance", "record_to", "silence_limit", "segments", "vad", "diarization"} {
		if !slices.Contains(r.Features, f) {
			t.Errorf("no host feature %s in %v", f, r.Features)
		}
	}
	if slices.Contains(r.Features, "mic_capture") || slices.Contains(r.Features, "system_capture") {
		t.Fatalf("no capture without WAV files: %v", r.Features)
	}
}

func TestADeniedAppIsNotApproved(t *testing.T) {
	testbin.Passed(t, "H02")
	h := testbin.StartHost(t, "--approve", "deny")
	c := dial(t, h)
	_, err := c.Hello(ctx(t), protocol.Hello{}, nil)
	if code(err) != protocol.CodeNotApproved {
		t.Fatal(err)
	}
	select {
	case <-c.Done():
	case <-time.After(5 * time.Second):
		t.Fatal("the host keeps a refused app")
	}
}

func TestAnUnansweredQuestionWaitsForTheAnswer(t *testing.T) {
	h := testbin.StartHost(t, "--approve", "ignore")
	c := dial(t, h)
	done := make(chan error, 1)
	go func() {
		_, err := c.Hello(ctx(t), protocol.Hello{Capabilities: []protocol.Capability{protocol.CapTranscribeFile}}, nil)
		done <- err
	}()
	h.Await("question", "approval")
	select {
	case err := <-done:
		t.Fatalf("hello replied before the answer: %v", err)
	case <-time.After(300 * time.Millisecond):
	}
	h.Send(map[string]any{"approve": "allow"})
	if err := <-done; err != nil {
		t.Fatal(err)
	}
}

func TestAMissingCapabilityIsNamed(t *testing.T) {
	testbin.Passed(t, "H04")
	h := testbin.StartHost(t)
	c := dial(t, h)
	hello(t, c, protocol.CapModelsRead)
	text := "hi"
	_, err := c.Enhance(ctx(t), protocol.Enhance{Text: &text})
	var e *protocol.Error
	if !errors.As(err, &e) || e.Code != protocol.CodeCapability || e.Capability != protocol.CapAiEnhance {
		t.Fatal(err)
	}
}

func TestModelsDownloadWithTheQuestion(t *testing.T) {
	testbin.Passed(t, "M01")
	testbin.Passed(t, "M02")
	testbin.Passed(t, "M03")
	h := testbin.StartHost(t, "--download", "wait")
	c := dial(t, h)
	hello(t, c, all...)
	models, err := c.ListModels(ctx(t), nil)
	if err != nil || len(models) == 0 {
		t.Fatal(models, err)
	}
	vad := protocol.RoleVad
	vads, _ := c.ListModels(ctx(t), &vad)
	for _, m := range vads {
		if !slices.Contains(m.Roles, protocol.RoleVad) {
			t.Fatal(m)
		}
	}
	model := models[0].ID
	if _, err := c.ListModels(ctx(t), nil); err != nil {
		t.Fatal(err)
	}
	errs := make(chan error, 1)
	var events []string
	go func() {
		errs <- c.DownloadModel(ctx(t), model, "tests", func(ev protocol.Event) { events = append(events, ev.Kind()) })
	}()
	if q := h.Await("question", "download"); q["model"] != model || q["reason"] != "tests" {
		t.Fatal(q)
	}
	h.Send(map[string]any{"download_answer": false})
	if err := <-errs; code(err) != protocol.CodeDeclined {
		t.Fatal(err)
	}
	go func() {
		errs <- c.DownloadModel(ctx(t), model, "tests", func(ev protocol.Event) { events = append(events, ev.Kind()) })
	}()
	h.Await("question", "download")
	h.Send(map[string]any{"download_answer": true})
	if err := <-errs; err != nil {
		t.Fatal(err)
	}
	if !slices.Contains(events, "download_progress") || events[len(events)-1] != "model_state" {
		t.Fatal(events)
	}
	if _, err := c.ListModels(ctx(t), nil); err != nil {
		t.Fatal(err)
	}
	if err := c.DownloadModel(ctx(t), "no-such-model", "tests", nil); code(err) != protocol.CodeUnknownModel {
		t.Fatal(err)
	}
}

func TestAJobHasItsEventsAndOneEnd(t *testing.T) {
	testbin.Passed(t, "J01")
	h := testbin.StartHost(t)
	c := dial(t, h)
	hello(t, c, all...)
	ready(t, c)
	path := testbin.SpeechWAV(t, 900, 600)
	job, err := c.TranscribeFile(ctx(t), protocol.TranscribeFile{Path: path})
	if err != nil {
		t.Fatal(err)
	}
	var seen []protocol.Event
	done := until[*protocol.JobDone](t, c, &seen)
	if done.Job != job || !strings.Contains(done.Text, "alpha") {
		t.Fatalf("%+v %v", done, kinds(seen))
	}
	if kinds(seen)[0] != "job_progress" || !slices.Contains(kinds(seen), "segment") {
		t.Fatal(kinds(seen))
	}
	if _, err := c.TranscribeFile(ctx(t), protocol.TranscribeFile{Path: "relative.wav"}); code(err) != protocol.CodeBadRequest {
		t.Fatal(err)
	}
	missing := "no-such-model"
	if _, err := c.TranscribeFile(ctx(t), protocol.TranscribeFile{Path: path, Model: &missing}); code(err) != protocol.CodeUnknownModel {
		t.Fatal(err)
	}
}

func TestCancelEndsAJob(t *testing.T) {
	testbin.Passed(t, "J03")
	h := testbin.StartHost(t)
	c := dial(t, h)
	hello(t, c, all...)
	ready(t, c)
	long := make([]int, 40)
	for i := range long {
		long[i] = 900
	}
	job, err := c.TranscribeFile(ctx(t), protocol.TranscribeFile{Path: testbin.SpeechWAV(t, long...)})
	if err != nil {
		t.Fatal(err)
	}
	if err := c.CancelJob(ctx(t), job); err != nil {
		t.Fatal(err)
	}
	var seen []protocol.Event
	failed := until[*protocol.JobFailed](t, c, &seen)
	if failed.Job != job || failed.Message != "The job was cancelled." {
		t.Fatalf("%+v", failed)
	}
	if err := c.CancelJob(ctx(t), 999); code(err) != protocol.CodeBadRequest {
		t.Fatal(err)
	}
}

func TestALiveSessionOfTheClientSource(t *testing.T) {
	h := testbin.StartHost(t)
	c := dial(t, h)
	hello(t, c, all...)
	ready(t, c)
	s, err := c.SessionStart(ctx(t), protocol.SessionStart{Sources: []protocol.Source{protocol.SourceClient}})
	if err != nil {
		t.Fatal(err)
	}
	samples := testbin.Speech(900, 900)
	for i := 0; i < len(samples); i += 3200 {
		if err := c.SessionAudio(s.Session, samples[i:min(i+3200, len(samples))]); err != nil {
			t.Fatal(err)
		}
	}
	time.Sleep(200 * time.Millisecond)
	text, err := c.SessionStop(ctx(t), s.Session)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(text, "alpha") {
		t.Fatalf("%q", text)
	}
	if _, err := c.SessionStart(ctx(t), protocol.SessionStart{Sources: []protocol.Source{protocol.SourceMic}}); code(err) != protocol.CodeUnsupported {
		t.Fatalf("a host without a microphone: %v", err)
	}
}

func TestALiveSessionOfTheMicrophone(t *testing.T) {
	testbin.Passed(t, "L02")
	h := testbin.StartHost(t, "--mic-wav", testbin.SpeechWAV(t, 900, 900), "--speed", "4")
	c := dial(t, h)
	r := hello(t, c, all...)
	if !slices.Contains(r.Features, "mic_capture") {
		t.Fatal(r.Features)
	}
	ready(t, c)
	s, err := c.SessionStart(ctx(t), protocol.SessionStart{Sources: []protocol.Source{protocol.SourceMic}})
	if err != nil {
		t.Fatal(err)
	}
	var seen []protocol.Event
	seg := until[*protocol.SegmentEvent](t, c, &seen)
	if seg.Session == nil || *seg.Session != s.Session || seg.Source == nil || *seg.Source != protocol.SourceMic {
		t.Fatalf("%+v", seg)
	}
	text, err := c.SessionStop(ctx(t), s.Session)
	if err != nil || !strings.Contains(text, "alpha") {
		t.Fatal(text, err)
	}
}

func TestEnhanceRunsAStyle(t *testing.T) {
	testbin.Passed(t, "E01")
	h := testbin.StartHost(t)
	c := dial(t, h)
	hello(t, c, all...)
	text, prompt := "um so hello", "Translate."
	r, err := c.Enhance(ctx(t), protocol.Enhance{Text: &text, Prompt: &prompt})
	if err != nil {
		t.Fatal(err)
	}
	if r.Text != "[app-prompt] um so hello" || r.Provider != "testhost" || r.Model != "echo" {
		t.Fatalf("%+v", r)
	}
	empty := " "
	if _, err := c.Enhance(ctx(t), protocol.Enhance{Text: &empty}); code(err) != protocol.CodeBadRequest {
		t.Fatal(err)
	}
}

func TestSubscribeGetsDictationDone(t *testing.T) {
	testbin.Passed(t, "E02")
	h := testbin.StartHost(t)
	c := dial(t, h)
	hello(t, c, all...)
	got, err := c.Subscribe(ctx(t), []string{"dictation_done", "future_event"})
	if err != nil || !slices.Equal(got, []string{"dictation_done"}) {
		t.Fatal(got, err)
	}
	h.Send(map[string]any{"dictation_done": map[string]any{"text": "Hello.", "transcript": "hello", "app": "Mail"}})
	var seen []protocol.Event
	d := until[*protocol.DictationDone](t, c, &seen)
	if d.Text != "Hello." || d.App == nil || *d.App != "Mail" || d.Time == "" {
		t.Fatalf("%+v", d)
	}
	h.Send(map[string]any{"dictation_done": map[string]any{"text": "secret", "incognito": true}})
	h.Await("sent", "dictation_done")
	h.Send(map[string]any{"dictation_done": map[string]any{"text": "after"}})
	if d := until[*protocol.DictationDone](t, c, &seen); d.Text != "after" {
		t.Fatalf("an incognito dictation was sent: %+v", d)
	}
}

func TestByeEndsTheConnection(t *testing.T) {
	h := testbin.StartHost(t)
	c := dial(t, h)
	hello(t, c)
	if err := c.Bye(ctx(t)); err != nil {
		t.Fatal(err)
	}
	if c.Err() != nil {
		t.Fatal(c.Err())
	}
	if _, err := c.ListModels(ctx(t), nil); code(err) != protocol.CodeConnectionLost {
		t.Fatal(err)
	}
}

func TestHostClosingEndsTheOpenJob(t *testing.T) {
	testbin.Passed(t, "X01")
	h := testbin.StartHost(t)
	c := dial(t, h)
	hello(t, c, all...)
	ready(t, c)
	long := make([]int, 60)
	for i := range long {
		long[i] = 900
	}
	job, err := c.TranscribeFile(ctx(t), protocol.TranscribeFile{Path: testbin.SpeechWAV(t, long...)})
	if err != nil {
		t.Fatal(err)
	}
	h.Send(map[string]any{"close": true})
	var seen []protocol.Event
	failed := until[*protocol.JobFailed](t, c, &seen)
	if failed.Job != job {
		t.Fatal(failed)
	}
	for range c.Events() {
	}
	if code(c.Err()) != protocol.CodeHostClosing || !slices.Contains(kinds(seen), "host_closing") && code(c.Err()) != protocol.CodeHostClosing {
		t.Fatal(c.Err(), kinds(seen))
	}
}

func TestAKilledHostIsConnectionLostWithOneEndPerJob(t *testing.T) {
	testbin.Passed(t, "X02")
	h := testbin.StartHost(t, "--speed", "0.5")
	c := dial(t, h)
	hello(t, c, all...)
	ready(t, c)
	long := make([]int, 60)
	for i := range long {
		long[i] = 900
	}
	job, err := c.TranscribeFile(ctx(t), protocol.TranscribeFile{Path: testbin.SpeechWAV(t, long...)})
	if err != nil {
		t.Fatal(err)
	}
	h.Send(map[string]any{"kill": true})
	<-h.Exited()
	ends := 0
	for ev := range c.Events() {
		switch e := ev.(type) {
		case *protocol.JobDone:
			ends++
		case *protocol.JobFailed:
			ends++
			if e.Job != job || e.Code == nil || *e.Code != protocol.CodeConnectionLost {
				t.Fatalf("%+v", e)
			}
		}
	}
	if ends != 1 || code(c.Err()) != protocol.CodeConnectionLost {
		t.Fatal(ends, c.Err())
	}
}

func TestTheClientEncodesPCMAsTheProtocol(t *testing.T) {
	in := []int16{0, 1, -1, 32767, -32768}
	out, err := protocol.DecodeI16(protocol.EncodeI16(in))
	if err != nil || !slices.Equal(in, out) {
		t.Fatal(out, err)
	}
	if audio.SampleRate != protocol.SampleRate {
		t.Fatal("the rates differ")
	}
}
