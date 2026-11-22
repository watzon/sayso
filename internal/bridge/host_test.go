package bridge

import (
	"bufio"
	"encoding/binary"
	"fmt"
	"math"
	"net"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/models"
)

// fakeEngine is an engine with models on disk and fixed answers.
type fakeEngine struct {
	mu       sync.Mutex
	status   map[string]engine.ModelStatus
	events   chan engine.Event
	loaded   []string
	features []string
}

func newFakeEngine() *fakeEngine {
	return &fakeEngine{status: map[string]engine.ModelStatus{}, events: make(chan engine.Event, 64)}
}

func (f *fakeEngine) StartState() EngineStart { return EngineStarted }
func (f *fakeEngine) Features() []string      { return f.features }
func (f *fakeEngine) Status(m string) engine.ModelStatus {
	f.mu.Lock()
	defer f.mu.Unlock()
	if s, ok := f.status[m]; ok {
		return s
	}
	return engine.ModelStatus{State: engine.StateNotDownloaded}
}
func (f *fakeEngine) set(m, state string) {
	f.mu.Lock()
	f.status[m] = engine.ModelStatus{State: state}
	f.mu.Unlock()
}
func (f *fakeEngine) Download(m string) error {
	go func() {
		f.events <- engine.Event{Kind: engine.EventModelStatus, Model: m, Status: engine.ModelStatus{State: engine.StateDownloading, Fraction: 0.5, BytesDone: 5, BytesTotal: 10}}
		f.set(m, engine.StateDownloaded)
		f.events <- engine.Event{Kind: engine.EventModelStatus, Model: m, Status: engine.ModelStatus{State: engine.StateDownloaded}}
	}()
	return nil
}
func (f *fakeEngine) Load(m string) error {
	f.mu.Lock()
	f.loaded = append(f.loaded, m)
	f.mu.Unlock()
	return nil
}
func (f *fakeEngine) Transcribe(s []float32, m, l string) (string, error) {
	return fmt.Sprintf(" words of %d ms ", len(s)/16), nil
}
func (f *fakeEngine) TranscribeSegments(s []float32, m, l string) ([]Segment, error) {
	return []Segment{{StartMs: 0, EndMs: uint64(len(s) / 16), Text: " hello "}}, nil
}
func (f *fakeEngine) DetectSpeech(m string, s []float32) ([]SpeechPart, error) {
	return []SpeechPart{{StartMs: 0, EndMs: uint64(len(s) / 16)}}, nil
}
func (f *fakeEngine) Diarize(m string, s []float32, n *uint32) ([]RawTurn, error) {
	return []RawTurn{{0, uint64(len(s) / 16), 4}}, nil
}
func (f *fakeEngine) StartStream(uint64, string, string) error { return nil }
func (f *fakeEngine) StartVad(uint64, string) error            { return nil }
func (f *fakeEngine) PushAudio(uint64, []float32) error        { return nil }
func (f *fakeEngine) EndStream(uint64)                         {}
func (f *fakeEngine) EndVad(uint64) error                      { return nil }
func (f *fakeEngine) Subscribe() (<-chan engine.Event, func()) { return f.events, func() {} }

type fakeServices struct {
	BaseServices
	eng      Engine
	cfg      *config.Config
	allow    bool
	answer   bool
	asked    chan ApprovalQuestion
	download bool
}

func (s *fakeServices) Engine() Engine { return s.eng }
func (s *fakeServices) Config() *config.Config {
	return s.cfg
}
func (s *fakeServices) AskDownload(q DownloadQuestion, w func() bool) bool { return s.download }
func (s *fakeServices) AskApproval(q ApprovalQuestion, w func() bool) (bool, bool) {
	if s.asked != nil {
		s.asked <- q
	}
	return s.allow, s.answer
}

type pathIdentify struct{ path string }

func (p pathIdentify) Identify(Peer) (Caller, error) { return CallerFromPath(p.path), nil }

type client struct {
	t *testing.T
	c net.Conn
	r *bufio.Reader
}

func (c *client) send(line string) {
	c.t.Helper()
	if _, err := fmt.Fprintln(c.c, line); err != nil {
		c.t.Fatal(err)
	}
}

func (c *client) read() (any, error) {
	c.t.Helper()
	c.c.SetReadDeadline(time.Now().Add(10 * time.Second))
	line, err := c.r.ReadString('\n')
	if err != nil {
		return nil, err
	}
	return DecodeHost(line)
}

// until reads messages until one with this id is a reply.
func (c *client) until(id string) (any, []Event) {
	c.t.Helper()
	var events []Event
	for {
		m, err := c.read()
		if err != nil {
			c.t.Fatalf("read: %v (events %v)", err, events)
		}
		msg := m.(*Message[any])
		switch b := msg.Body.(type) {
		case OKFields, *ErrorReply:
			if msg.ID != nil && *msg.ID == id {
				return b, events
			}
		case Event:
			events = append(events, b)
		}
	}
}

func socketPath(t *testing.T) string {
	if runtime.GOOS == "windows" {
		return fmt.Sprintf(`\\.\pipe\sayso-test-%d-%d`, os.Getpid(), time.Now().UnixNano())
	}
	dir, err := os.MkdirTemp("/tmp", "sb")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.RemoveAll(dir) })
	return filepath.Join(dir, "s", "host.sock")
}

func startTestHost(t *testing.T, svc *fakeServices, apps string) (*Host, string) {
	t.Helper()
	path := socketPath(t)
	appsFile := filepath.Join(t.TempDir(), "apps.toml")
	if apps != "" {
		os.WriteFile(appsFile, []byte(apps), 0o600)
	}
	cfg := NewHostConfig(path, appsFile)
	cfg.Identify = pathIdentify{"/opt/App"}
	cfg.ApprovalCooldown = 50 * time.Millisecond
	cfg.HostVersion = "9.9.9"
	h, err := StartHost(cfg, svc)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(h.Close)
	return h, path
}

func dial(t *testing.T, path string) *client {
	t.Helper()
	c, err := dialHost(path)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { c.Close() })
	return &client{t: t, c: c, r: bufio.NewReader(c)}
}

const allowedApp = "[[apps]]\nname = \"App\"\npath = \"/opt/App\"\nallowed = true\ngranted = [\"transcribe.file\", \"dictation.observe\", \"models.download\"]\ndenied = [\"ai.enhance\"]\n"

func testConfig(final models.ModelID) *config.Config {
	c := config.Default()
	c.Dictation.Model = final
	c.Dictation.Language = "en"
	return &c
}

func finalSpeechModel(t *testing.T) models.ModelInfo {
	for _, m := range models.LocalModels() {
		if m.FinalPass && m.Task() == models.TaskSpeech {
			return m
		}
	}
	t.Skip("no local speech model on this system")
	return models.ModelInfo{}
}

func TestHelloAsksOnceAndKeepsTheAnswer(t *testing.T) {
	svc := &fakeServices{allow: true, answer: true, asked: make(chan ApprovalQuestion, 4), cfg: testConfig("x")}
	h, path := startTestHost(t, svc, "")
	c := dial(t, path)
	c.send(`{"v":1,"type":"hello","id":"1","protocol":1,"app":{"name":"Notes"},"capabilities":["audio.mic","models.read","camera.read"]}`)
	reply, events := c.until("1")
	if len(events) != 1 || events[0].Kind() != "approval_waiting" {
		t.Fatal(events)
	}
	var hr HelloReply
	reply.(OKFields).Parse(&hr)
	if hr.HostVersion != "9.9.9" || hr.Protocol != 1 || strings.Join(capNames(hr.Capabilities), ",") != "models.read,audio.mic" || strings.Join(capNames(hr.Denied), ",") != "camera.read" {
		t.Fatalf("%+v", hr)
	}
	q := <-svc.asked
	if q.Name != "App" || q.ClaimedName != "Notes" || q.Approved || strings.Join(capNames(q.Capabilities), ",") != "audio.mic" {
		t.Fatalf("%+v", q)
	}
	if r := h.Apps().Current().Find(Identity{Kind: IdentityPath, Path: "/opt/App"}); r == nil || !r.Allowed {
		t.Fatal("kept")
	}
	c2 := dial(t, path)
	c2.send(`{"type":"hello","id":"2","protocol":1,"capabilities":["audio.mic"]}`)
	if _, ok := mustUntil(c2, "2").(OKFields); !ok {
		t.Fatal("second hello")
	}
	select {
	case <-svc.asked:
		t.Fatal("asked twice")
	default:
	}
	if apps := h.ConnectedApps(); len(apps) == 0 || apps[0] != "path:/opt/App" && apps[0] != "path:/opt/app" {
		t.Fatal(apps)
	}
}

func mustUntil(c *client, id string) any { r, _ := c.until(id); return r }

func capNames(c []Capability) []string {
	out := make([]string, len(c))
	for i, v := range c {
		out[i] = string(v)
	}
	return out
}

func TestARefusedAppIsClosed(t *testing.T) {
	svc := &fakeServices{allow: false, answer: true, cfg: testConfig("x")}
	_, path := startTestHost(t, svc, "")
	c := dial(t, path)
	c.send(`{"type":"hello","id":"1","protocol":1}`)
	r := mustUntil(c, "1").(*ErrorReply)
	if *r.Code != CodeNotApproved {
		t.Fatal(r)
	}
	if _, err := c.read(); err == nil {
		t.Fatal("the connection stays open")
	}
	c = dial(t, path)
	c.send(`{"type":"list_models","id":"2"}`)
	if r := mustUntil(c, "2").(*ErrorReply); *r.Code != CodeNotApproved {
		t.Fatal(r)
	}
}

func TestNoAnswerCoolsDown(t *testing.T) {
	svc := &fakeServices{answer: false, cfg: testConfig("x")}
	_, path := startTestHost(t, svc, "")
	c := dial(t, path)
	c.send(`{"type":"hello","id":"1","protocol":1}`)
	if r := mustUntil(c, "1").(*ErrorReply); *r.Code != CodeNotApproved {
		t.Fatal(r)
	}
}

func TestRequestsOfAnApprovedApp(t *testing.T) {
	eng := newFakeEngine()
	final := finalSpeechModel(t)
	eng.set(string(final.ID), engine.StateReady)
	svc := &fakeServices{eng: eng, cfg: testConfig(final.ID)}
	h, path := startTestHost(t, svc, allowedApp)
	c := dial(t, path)
	c.send(`{"type":"hello","id":"h","protocol":1,"capabilities":["transcribe.file"]}`)
	mustUntil(c, "h")

	c.send(`{"type":"list_models","id":"1","role":"final"}`)
	var list ListModelsReply
	mustUntil(c, "1").(OKFields).Parse(&list)
	found := false
	for _, m := range list.Models {
		if m.ID == string(final.ID) {
			found = m.State == StateReady && len(m.Choice) > 0 && m.Choice[0] == RoleFinal
		}
	}
	if !found {
		t.Fatalf("%+v", list.Models)
	}

	c.send(`{"type":"enhance","id":"2","text":"x"}`)
	if r := mustUntil(c, "2").(*ErrorReply); *r.Code != CodeCapability || *r.Capability != CapAiEnhance {
		t.Fatal(r)
	}
	c.send(`{"type":"translate","id":"3"}`)
	if r := mustUntil(c, "3").(*ErrorReply); r.Code != nil || r.Message != "unknown request type translate" {
		t.Fatal(r)
	}
	c.send(`{"type":"cancel_job","id":"4"}`)
	if r := mustUntil(c, "4").(*ErrorReply); *r.Code != CodeBadRequest || !strings.Contains(r.Message, "bad cancel_job request") {
		t.Fatal(r)
	}
	c.send(`{"type":"cancel_job","id":"5","job":99}`)
	if r := mustUntil(c, "5").(*ErrorReply); r.Message != "this connection has no job 99" {
		t.Fatal(r)
	}
	c.send(`{"type":"transcribe_file","id":"6","path":"relative.wav"}`)
	if r := mustUntil(c, "6").(*ErrorReply); *r.Code != CodeBadRequest {
		t.Fatal(r)
	}

	wav := filepath.Join(t.TempDir(), "a.wav")
	writeTone(t, wav, 44100, 2, 3)
	c.send(fmt.Sprintf(`{"type":"transcribe_file","id":"7","path":%q}`, wav))
	var job JobReply
	reply, events := c.until("7")
	reply.(OKFields).Parse(&job)
	if job.Job == 0 || len(events) != 0 {
		t.Fatal(job, events)
	}
	var done *JobDone
	var segments int
	for done == nil {
		m, err := c.read()
		if err != nil {
			t.Fatal(err)
		}
		switch e := m.(*Message[any]).Body.(type) {
		case *JobDone:
			done = e
		case *SegmentEvent:
			segments++
		case *JobFailed:
			t.Fatal(e.Message)
		}
	}
	if done.Job != job.Job || segments == 0 || !strings.Contains(done.Text, "hello") {
		t.Fatalf("%+v %d", done, segments)
	}

	c.send(`{"type":"subscribe","id":"8","events":["dictation_done","nope"]}`)
	var sub SubscribeReply
	mustUntil(c, "8").(OKFields).Parse(&sub)
	if len(sub.Events) != 1 || sub.Events[0] != "dictation_done" {
		t.Fatal(sub)
	}
	h.DictationDone(DictationDone{Text: "t", Transcript: "r", Time: "now"}, true, false)
	h.DictationDone(DictationDone{Text: "Hello.", Transcript: "hello", Time: "2026-11-22T00:00:00Z"}, false, false)
	m, _ := c.read()
	if d, ok := m.(*Message[any]).Body.(*DictationDone); !ok || d.Text != "Hello." {
		t.Fatalf("%#v", m)
	}

	other := final.ID
	for _, m := range models.LocalModels() {
		if m.ID != final.ID {
			other = m.ID
			break
		}
	}
	svc.download = true
	c.send(fmt.Sprintf(`{"type":"download_model","id":"9","model":%q,"reason":"notes"}`, other))
	reply, events = c.until("9")
	if _, ok := reply.(OKFields); !ok {
		t.Fatal(reply, events)
	}
	var sawProgress bool
	for _, e := range events {
		if p, ok := e.(*DownloadProgress); ok && p.Fraction == 0.5 {
			sawProgress = true
		}
	}
	if !sawProgress {
		t.Fatal(events)
	}
	c.send(`{"type":"download_model","id":"10","model":"no-such-model","reason":"x"}`)
	if r := mustUntil(c, "10").(*ErrorReply); *r.Code != CodeUnknownModel {
		t.Fatal(r)
	}

	c.send(`{"type":"bye","id":"11"}`)
	if _, ok := mustUntil(c, "11").(OKFields); !ok {
		t.Fatal("bye")
	}
	if _, err := c.read(); err == nil {
		t.Fatal("open after bye")
	}
}

func TestCloseSaysHostClosing(t *testing.T) {
	svc := &fakeServices{cfg: testConfig("x")}
	h, path := startTestHost(t, svc, allowedApp)
	c := dial(t, path)
	c.send(`{"type":"hello","id":"h","protocol":1}`)
	mustUntil(c, "h")
	h.Close()
	m, err := c.read()
	if err != nil || m.(*Message[any]).Body.(Event).Kind() != "host_closing" {
		t.Fatal(m, err)
	}
	if runtime.GOOS != "windows" {
		if _, err := os.Stat(path); !os.IsNotExist(err) {
			t.Fatal("the socket is left")
		}
	}
}

func TestASecondHostDoesNotTakeTheSocket(t *testing.T) {
	_, path := startTestHost(t, &fakeServices{cfg: testConfig("x")}, "")
	cfg := NewHostConfig(path, filepath.Join(t.TempDir(), "apps.toml"))
	if _, err := StartHost(cfg, &fakeServices{cfg: testConfig("x")}); err == nil {
		t.Fatal("two hosts")
	}
}

func TestASilentProgramIsClosed(t *testing.T) {
	if testing.Short() {
		t.Skip("waits 5 s")
	}
	_, path := startTestHost(t, &fakeServices{cfg: testConfig("x")}, "")
	c := dial(t, path)
	start := time.Now()
	if _, err := c.read(); err == nil || time.Since(start) < 4*time.Second {
		t.Fatal(err, time.Since(start))
	}
}

// writeTone writes a WAV file of a 440 Hz tone with pauses.
func writeTone(t *testing.T, path string, rate, channels, seconds int) {
	n := rate * seconds
	data := make([]byte, 0, n*channels*2)
	for i := 0; i < n; i++ {
		v := 0.0
		if (i/(rate/2))%2 == 0 {
			v = 0.5 * math.Sin(2*math.Pi*440*float64(i)/float64(rate))
		}
		for c := 0; c < channels; c++ {
			data = binary.LittleEndian.AppendUint16(data, uint16(int16(v*32767)))
		}
	}
	var b []byte
	b = append(b, "RIFF"...)
	b = binary.LittleEndian.AppendUint32(b, uint32(36+len(data)))
	b = append(b, "WAVEfmt "...)
	b = binary.LittleEndian.AppendUint32(b, 16)
	b = binary.LittleEndian.AppendUint16(b, 1)
	b = binary.LittleEndian.AppendUint16(b, uint16(channels))
	b = binary.LittleEndian.AppendUint32(b, uint32(rate))
	b = binary.LittleEndian.AppendUint32(b, uint32(rate*channels*2))
	b = binary.LittleEndian.AppendUint16(b, uint16(channels*2))
	b = binary.LittleEndian.AppendUint16(b, 16)
	b = append(b, "data"...)
	b = binary.LittleEndian.AppendUint32(b, uint32(len(data)))
	b = append(b, data...)
	if err := os.WriteFile(path, b, 0o600); err != nil {
		t.Fatal(err)
	}
}

func TestLiveSessionsAreUnsupportedForNow(t *testing.T) {
	eng := newFakeEngine()
	svc := &fakeServices{eng: eng, cfg: testConfig("x")}
	_, path := startTestHost(t, svc, "[[apps]]\nname = \"App\"\npath = \"/opt/App\"\nallowed = true\ngranted = [\"session.live\"]\n")
	c := dial(t, path)
	c.send(`{"type":"hello","id":"h","protocol":1}`)
	mustUntil(c, "h")
	c.send(`{"type":"session_start","id":"1","sources":["client"]}`)
	if r := mustUntil(c, "1").(*ErrorReply); *r.Code != CodeUnsupported || !strings.Contains(r.Message, "live sessions") {
		t.Fatal(r)
	}
	c.send(`{"type":"session_start","id":"2","sources":["mic"]}`)
	if r := mustUntil(c, "2").(*ErrorReply); *r.Code != CodeCapability || *r.Capability != CapAudioMic {
		t.Fatal(r)
	}
}
