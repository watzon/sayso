package client_test

import (
	"os"
	"path/filepath"
	"slices"
	"testing"
	"time"

	sayso "github.com/watzon/sayso/sdk/go"
	"github.com/watzon/sayso/sdk/go/audio"
	"github.com/watzon/sayso/sdk/go/client"
	"github.com/watzon/sayso/sdk/go/internal/testbin"
	"github.com/watzon/sayso/sdk/go/protocol"
)

func TestTheHostSaysItsVersionAndTheSDKItsOwn(t *testing.T) {
	testbin.Passed(t, "H08")
	h := testbin.StartHost(t)
	c := dial(t, h)
	r := hello(t, c, protocol.CapModelsRead)
	if r.HostVersion != sayso.Version || r.Protocol != protocol.Protocol || len(r.Features) == 0 {
		t.Fatalf("%+v, SDK %s", r, sayso.Version)
	}
}

func TestAFifthWaitingConnectionOfOneAppIsBusy(t *testing.T) {
	testbin.Passed(t, "H05")
	h := testbin.StartHost(t, "--approve", "ignore")
	waiting := make(chan error, 4)
	for range 4 {
		c := dial(t, h)
		go func() {
			_, err := c.Hello(ctx(t), protocol.Hello{}, nil)
			waiting <- err
		}()
	}
	h.Await("question", "approval")
	time.Sleep(800 * time.Millisecond)
	fifth := dial(t, h)
	done := make(chan error, 1)
	go func() {
		_, err := fifth.Hello(ctx(t), protocol.Hello{}, nil)
		done <- err
	}()
	select {
	case err := <-done:
		if code(err) != protocol.CodeBusy {
			t.Fatal(err)
		}
	case <-time.After(10 * time.Second):
		t.Fatal("the fifth connection waits")
	}
	h.Send(map[string]any{"approve": "deny"})
	for range 4 {
		<-waiting
	}
}

func TestALiveSessionOfTheClientSourceHasPartials(t *testing.T) {
	testbin.Passed(t, "L01")
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
	var seen []protocol.Event
	if p := until[*protocol.PartialEvent](t, c, &seen); p.Session != s.Session {
		t.Fatalf("%+v", p)
	}
	if text, err := c.SessionStop(ctx(t), s.Session); err != nil || text == "" {
		t.Fatal(text, err)
	}
}

func TestTheEchoRuleDropsTheMicrophoneSegmentsThatRepeatTheSystemAudio(t *testing.T) {
	testbin.Passed(t, "L03")
	wav := testbin.SpeechWAV(t, 900, 900)
	h := testbin.StartHost(t, "--mic-wav", wav, "--system-wav", wav, "--speed", "4")
	c := dial(t, h)
	r := hello(t, c, all...)
	if !slices.Contains(r.Features, "system_capture") || !slices.Contains(r.Features, "echo_rule") {
		t.Fatal(r.Features)
	}
	ready(t, c)
	s, err := c.SessionStart(ctx(t), protocol.SessionStart{Sources: []protocol.Source{protocol.SourceMic, protocol.SourceSystem}})
	if err != nil {
		t.Fatal(err)
	}
	var seen []protocol.Event
	for {
		seg := until[*protocol.SegmentEvent](t, c, &seen)
		if seg.Source != nil && *seg.Source == protocol.SourceSystem {
			break
		}
	}
	time.Sleep(1500 * time.Millisecond)
	if _, err := c.SessionStop(ctx(t), s.Session); err != nil {
		t.Fatal(err)
	}
	seen = drain(c, seen)
	for _, ev := range seen {
		if seg, ok := ev.(*protocol.SegmentEvent); ok && seg.Source != nil && *seg.Source == protocol.SourceMic {
			t.Fatalf("the microphone heard the system audio, and the echo rule kept it: %v", kinds(seen))
		}
	}
}

func TestTheSilenceLimitGivesOneEvent(t *testing.T) {
	testbin.Passed(t, "L04")
	h := testbin.StartHost(t, "--mic-wav", testbin.SpeechWAV(t, 600), "--speed", "8")
	c := dial(t, h)
	hello(t, c, all...)
	ready(t, c)
	limit := uint64(1000)
	s, err := c.SessionStart(ctx(t), protocol.SessionStart{Sources: []protocol.Source{protocol.SourceMic}, SilenceLimitMs: &limit})
	if err != nil {
		t.Fatal(err)
	}
	var seen []protocol.Event
	if e := until[*protocol.SilenceLimit](t, c, &seen); e.Session != s.Session || e.SilenceMs != 1000 {
		t.Fatalf("%+v", e)
	}
	time.Sleep(500 * time.Millisecond)
	if _, err := c.SessionStop(ctx(t), s.Session); err != nil {
		t.Fatal(err)
	}
	seen = drain(c, seen)
	count := 0
	for _, ev := range seen {
		if ev.Kind() == "silence_limit" {
			count++
		}
	}
	if count != 1 {
		t.Fatalf("%d silence_limit events: %v", count, kinds(seen))
	}
}

func TestRecordToWritesAWAVFileForEachSource(t *testing.T) {
	testbin.Passed(t, "L05")
	h := testbin.StartHost(t, "--mic-wav", testbin.SpeechWAV(t, 600), "--system-wav", testbin.SpeechWAV(t, 900), "--speed", "8")
	c := dial(t, h)
	hello(t, c, all...)
	ready(t, c)
	folder := filepath.Join(t.TempDir(), "rec")
	if err := os.Mkdir(folder, 0o755); err != nil {
		t.Fatal(err)
	}
	s, err := c.SessionStart(ctx(t), protocol.SessionStart{Sources: []protocol.Source{protocol.SourceMic, protocol.SourceSystem}, RecordTo: &folder})
	if err != nil {
		t.Fatal(err)
	}
	time.Sleep(1500 * time.Millisecond)
	if _, err := c.SessionStop(ctx(t), s.Session); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"mic.wav", "system.wav"} {
		a, err := audio.ReadFile(filepath.Join(folder, name))
		if err != nil {
			entries, _ := os.ReadDir(folder)
			t.Fatal(err, entries)
		}
		if !slices.ContainsFunc(a.Samples(), func(v int16) bool { return v != 0 }) {
			t.Fatalf("%s has %d samples and no sound", name, len(a.Samples()))
		}
	}
}

func TestASecondLiveSessionWhileOneRunsIsRefused(t *testing.T) {
	testbin.Passed(t, "L06")
	h := testbin.StartHost(t)
	c := dial(t, h)
	hello(t, c, all...)
	ready(t, c)
	client := []protocol.Source{protocol.SourceClient}
	first, err := c.SessionStart(ctx(t), protocol.SessionStart{Sources: client})
	if err != nil {
		t.Fatal(err)
	}
	_, err = c.SessionStart(ctx(t), protocol.SessionStart{Sources: client})
	if got := code(err); got != protocol.CodeBusy && got != protocol.CodeTooManyStreams {
		t.Fatal(err)
	}
	if _, err := c.SessionStop(ctx(t), first.Session); err != nil {
		t.Fatal(err)
	}
}

func drain(c *client.Client, seen []protocol.Event) []protocol.Event {
	_ = c.Close()
	for ev := range c.Events() {
		seen = append(seen, ev)
	}
	return seen
}

