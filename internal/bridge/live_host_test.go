package bridge

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/models"
)

// session_start, session_audio, and session_stop over a real socket (a named
// pipe on Windows), with the identity and grant checks of the host and a fake
// engine (ported from crates/sayso-host/tests/live.rs).

type liveWorldServices struct {
	*fakeServices
	mic    *fakeMic
	system *fakeSystem
	temp   string
}

func (s *liveWorldServices) Microphone() Microphone {
	if s.mic == nil {
		return nil
	}
	return s.mic
}

func (s *liveWorldServices) SystemAudio() SystemAudio {
	if s.system == nil {
		return nil
	}
	return s.system
}

func (s *liveWorldServices) LiveTempDir() string { return s.temp }

type liveWorld struct {
	host   *Host
	path   string
	eng    *liveFake
	mic    *fakeMic
	system *fakeSystem
	temp   string
}

// streamingModel is a local model that does the final pass and streams.
func streamingModel(t *testing.T) models.ModelInfo {
	t.Helper()
	for _, m := range models.LocalModels() {
		if m.FinalPass && m.LivePreview && m.Task() == models.TaskSpeech && m.Understands("en") {
			return m
		}
	}
	t.Skip("no local streaming model on this system")
	return models.ModelInfo{}
}

func appsWith(granted ...string) string {
	quoted := make([]string, len(granted))
	for i, g := range granted {
		quoted[i] = fmt.Sprintf("%q", g)
	}
	return "[[apps]]\nname = \"App\"\npath = \"/opt/App\"\nallowed = true\ngranted = [" + strings.Join(quoted, ", ") + "]\n"
}

// newLiveWorld starts a host. withSystem: this host can capture the system audio.
func newLiveWorld(t *testing.T, eng *liveFake, withSystem bool, granted ...string) *liveWorld {
	t.Helper()
	final := streamingModel(t)
	w := &liveWorld{eng: eng, mic: &fakeMic{}, temp: filepath.Join(t.TempDir(), "live-temp")}
	svc := &liveWorldServices{fakeServices: &fakeServices{eng: eng, cfg: testConfig(final.ID)}, mic: w.mic, temp: w.temp}
	if withSystem {
		w.system = &fakeSystem{}
		svc.system = w.system
	}
	w.path = socketPath(t)
	appsFile := filepath.Join(t.TempDir(), "apps.toml")
	if err := os.WriteFile(appsFile, []byte(appsWith(granted...)), 0o600); err != nil {
		t.Fatal(err)
	}
	cfg := NewHostConfig(w.path, appsFile)
	cfg.Identify = pathIdentify{"/opt/App"}
	h, err := StartHost(cfg, svc)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(h.Close)
	w.host = h
	return w
}

// liveClient is a connection that said hello.
type liveClient struct {
	*client
	// stray are the messages without an id that came, in order.
	mu    sync.Mutex
	stray []any
}

func (w *liveWorld) client(t *testing.T) *liveClient {
	t.Helper()
	c := &liveClient{client: dial(t, w.path)}
	c.send(`{"v":1,"type":"hello","id":"h","protocol":1}`)
	liveOK(t, mustRequest(c, "h", ""))
	return c
}

// request sends line (unless it is empty) and reads until the reply with id.
// It gives the reply and the messages without an id that came before it. A
// live event never has an id.
func (c *liveClient) request(id, line string) (any, []any) {
	c.t.Helper()
	if line != "" {
		c.send(line)
	}
	var before []any
	for {
		m, err := c.read()
		if err != nil {
			c.t.Fatalf("read: %v (before %v)", err, before)
		}
		msg := m.(*Message[any])
		if msg.ID != nil {
			if *msg.ID != id {
				c.t.Fatalf("a message with the id %s: %#v", *msg.ID, msg.Body)
			}
			switch msg.Body.(type) {
			case OKFields, *ErrorReply:
				return msg.Body, before
			}
			c.t.Fatalf("a live event has no id: %#v", msg.Body)
		}
		before = append(before, msg.Body)
	}
}

func liveOK(t *testing.T, reply any) OKFields {
	t.Helper()
	ok, is := reply.(OKFields)
	if !is {
		t.Fatalf("not ok: %#v", reply)
	}
	return ok
}

func liveRefused(t *testing.T, reply any) *ErrorReply {
	t.Helper()
	e, is := reply.(*ErrorReply)
	if !is {
		t.Fatalf("not an error: %#v", reply)
	}
	return e
}

func errCode(e *ErrorReply) ErrorCode {
	if e.Code == nil {
		return ""
	}
	return *e.Code
}

func (c *liveClient) start(id, fields string) any {
	c.t.Helper()
	r, _ := c.request(id, fmt.Sprintf(`{"type":"session_start","id":%q,%s}`, id, fields))
	return r
}

func (c *liveClient) startOK(id, fields string) SessionReply {
	c.t.Helper()
	var s SessionReply
	liveOK(c.t, c.start(id, fields)).Parse(&s)
	if s.Session == 0 {
		c.t.Fatal("no session")
	}
	return s
}

// stop sends session_stop and gives its text and the messages before it.
func (c *liveClient) stop(id string, session uint64) (string, []any) {
	c.t.Helper()
	r, before := c.request(id, fmt.Sprintf(`{"type":"session_stop","id":%q,"session":%d}`, id, session))
	var text TextReply
	liveOK(c.t, r).Parse(&text)
	return text.Text, before
}

// sendAudio sends the audio in frames of 160 ms, without an id.
func (c *liveClient) sendAudio(session uint64, samples []int16) {
	step := SampleRate * 160 / 1000
	for i := 0; i < len(samples); i += step {
		c.send(fmt.Sprintf(`{"type":"session_audio","session":%d,"pcm":%q}`, session, EncodeI16(samples[i:min(i+step, len(samples))])))
	}
}

// waitFor reads messages without an id until match is true.
func (c *liveClient) waitFor(what string, match func(any) bool) {
	c.t.Helper()
	for {
		m, err := c.read()
		if err != nil {
			c.t.Fatalf("%s: %v", what, err)
		}
		if match(m.(*Message[any]).Body) {
			return
		}
	}
}

func liveSegments(msgs []any) []*SegmentEvent {
	var out []*SegmentEvent
	for _, m := range msgs {
		if s, ok := m.(*SegmentEvent); ok {
			out = append(out, s)
		}
	}
	return out
}

func TestHostLiveClientSessionGivesPartialsSegmentsSpeakersAndItsText(t *testing.T) {
	w := newLiveWorld(t, newLiveFake(), false, "session.live")
	var obsMu sync.Mutex
	var seen []*LiveInfo
	w.host.SetLiveObserver(func(i *LiveInfo) { obsMu.Lock(); seen = append(seen, i); obsMu.Unlock() })
	c := w.client(t)
	recordTo := filepath.Join(t.TempDir(), "meeting")
	session := c.startOK("1", fmt.Sprintf(`"sources":["client"],"diarize":true,"record_to":%q`, recordTo)).Session
	info, ok := w.host.LiveSession()
	if !ok || info.Session != session || len(info.Sources) != 1 || info.Sources[0] != SourceClient || info.CapturesMic || info.Stopping {
		t.Fatal(info, ok)
	}
	if w.eng.openNow() != 2 {
		t.Fatal("a preview stream and a VAD session", w.eng.openNow())
	}

	audio := toPCM(concat(silenceMs(1000), speechMs(2000), silenceMs(1000), speechMs(1500), silenceMs(1000)))
	c.sendAudio(session, audio)
	// With an id, session_audio gets ok.
	reply, early := c.request("2", fmt.Sprintf(`{"type":"session_audio","id":"2","session":%d,"pcm":%q}`, session, EncodeI16(make([]int16, 160))))
	liveOK(t, reply)
	text, before := c.stop("3", session)
	if len(before) == 0 {
		t.Fatal("no event between the audio and the reply of the stop")
	}
	before = append(early, before...)
	if text != "part1 part2" {
		t.Fatal(text)
	}
	if _, ok := w.host.LiveSession(); ok || w.eng.openNow() != 0 {
		t.Fatal("the session is gone")
	}

	// The reply of session_stop comes after the last events of the session.
	partials := 0
	for _, m := range before {
		if p, ok := m.(*PartialEvent); ok && p.Session == session && p.Source != nil && *p.Source == SourceClient {
			partials++
		}
	}
	if partials == 0 {
		t.Fatal("no partials", before)
	}
	segments := liveSegments(before)
	if len(segments) != 2 || !near(segments[0].StartMs, 1000, 60) || !near(segments[1].StartMs, 4000, 60) {
		t.Fatalf("%+v", segments)
	}
	for _, s := range segments {
		if s.Job != nil || s.Session == nil || *s.Session != session || s.Source == nil || *s.Source != SourceClient {
			t.Fatalf("%+v", s)
		}
	}
	speakers, ok := before[len(before)-1].(*SpeakersEvent)
	if !ok || *speakers.Session != session || *speakers.Source != SourceClient || len(speakers.Turns) != 2 || speakers.Turns[0].Speaker != "1" || speakers.Turns[1].Speaker != "2" {
		t.Fatalf("the speakers come last, before the reply: %#v", before[len(before)-1])
	}

	// The recording has every sample, also the last request with an id.
	got, err := readLiveWav(filepath.Join(recordTo, "client.wav"))
	if err != nil || len(got) != len(audio)+160 {
		t.Fatal(len(got), err)
	}
	// The models stayed held while the session ran; now they wait for the idle time.
	wanted := w.host.ModelUse().Wanted(time.Minute)
	if !strings.Contains(strings.Join(wanted, ","), string(streamingModel(t).ID)) {
		t.Fatal(wanted)
	}
	obsMu.Lock()
	defer obsMu.Unlock()
	if len(seen) < 2 || seen[0] == nil || seen[0].Session != session || seen[len(seen)-1] != nil {
		t.Fatal(seen)
	}
}

func mustRequest(c *liveClient, id, line string) any {
	c.t.Helper()
	r, _ := c.request(id, line)
	return r
}

func TestHostLiveASessionWithoutAudioStopsWithNoText(t *testing.T) {
	w := newLiveWorld(t, newLiveFake(), false, "session.live")
	c := w.client(t)
	session := c.startOK("1", `"sources":["client"],"vad_model":"basic"`).Session
	if w.eng.openNow() != 1 {
		t.Fatal("only the preview stream", w.eng.openNow())
	}
	text, before := c.stop("2", session)
	if text != "" || len(liveSegments(before)) != 0 {
		t.Fatal(text, before)
	}
	// A second stop, and audio after the stop, name no session that runs.
	if e := liveRefused(t, mustRequest(c, "3", fmt.Sprintf(`{"type":"session_stop","id":"3","session":%d}`, session))); errCode(e) != CodeBadRequest {
		t.Fatal(e)
	}
	pcm := EncodeI16(make([]int16, 160))
	if e := liveRefused(t, mustRequest(c, "4", fmt.Sprintf(`{"type":"session_audio","id":"4","session":%d,"pcm":%q}`, session, pcm))); errCode(e) != CodeBadRequest {
		t.Fatal(e)
	}
	// Without an id, the error has none either.
	c.send(fmt.Sprintf(`{"type":"session_audio","session":%d,"pcm":%q}`, session, pcm))
	m, err := c.read()
	if err != nil {
		t.Fatal(err)
	}
	msg := m.(*Message[any])
	if e, ok := msg.Body.(*ErrorReply); !ok || msg.ID != nil || errCode(e) != CodeBadRequest {
		t.Fatalf("expected an error without an id, got %#v", msg)
	}
}

func TestHostLiveOneSessionRunsAtATimeAcrossConnections(t *testing.T) {
	w := newLiveWorld(t, newLiveFake(), false, "session.live")
	first := w.client(t)
	session := first.startOK("1", `"sources":["client"]`).Session
	second := w.client(t)
	if e := liveRefused(t, second.start("1", `"sources":["client"]`)); errCode(e) != CodeBusy {
		t.Fatal(e)
	}
	// The other connection cannot stop or feed a session that it did not start.
	if e := liveRefused(t, mustRequest(second, "2", fmt.Sprintf(`{"type":"session_stop","id":"2","session":%d}`, session))); errCode(e) != CodeBadRequest {
		t.Fatal(e)
	}
	first.stop("2", session)
	session = second.startOK("3", `"sources":["client"]`).Session
	second.stop("4", session)
}

func TestHostLiveASessionNeedsItsCapabilities(t *testing.T) {
	w := newLiveWorld(t, newLiveFake(), false)
	c := w.client(t)
	e := liveRefused(t, c.start("1", `"sources":["client"]`))
	if errCode(e) != CodeCapability || e.Capability == nil || *e.Capability != CapSessionLive {
		t.Fatal(e)
	}
	e = liveRefused(t, mustRequest(c, "2", `{"type":"session_audio","id":"2","session":1,"pcm":""}`))
	if e.Capability == nil || *e.Capability != CapSessionLive {
		t.Fatal(e)
	}
	// An app can always stop its own session, so this needs no grant.
	if e := liveRefused(t, mustRequest(c, "3", `{"type":"session_stop","id":"3","session":1}`)); errCode(e) != CodeBadRequest {
		t.Fatal(e)
	}

	w = newLiveWorld(t, newLiveFake(), false, "session.live")
	c = w.client(t)
	if e := liveRefused(t, c.start("1", `"sources":["client","mic"]`)); errCode(e) != CodeCapability || *e.Capability != CapAudioMic {
		t.Fatal(e)
	}
	if e := liveRefused(t, c.start("2", `"sources":["system"]`)); *e.Capability != CapAudioSystem {
		t.Fatal(e)
	}
	if _, ok := w.host.LiveSession(); ok {
		t.Fatal("a session runs")
	}
}

func TestHostLiveTheSourceSystemIsUnsupportedWithoutACapture(t *testing.T) {
	w := newLiveWorld(t, newLiveFake(), false, "session.live", "audio.system")
	c := w.client(t)
	if e := liveRefused(t, c.start("1", `"sources":["system"]`)); errCode(e) != CodeUnsupported {
		t.Fatal(e)
	}
	if _, ok := w.host.LiveSession(); ok || w.eng.openNow() != 0 {
		t.Fatal("something runs")
	}
}

func TestHostLiveTheSourceSystemRepliesWithWhatItLeavesOutAndGivesItsEvents(t *testing.T) {
	w := newLiveWorld(t, newLiveFake(), true, "session.live", "audio.system", "audio.mic")
	c := w.client(t)
	reply := c.startOK("1", `"sources":["mic","system"]`)
	if strings.Join(reply.Excludes, ",") != "sayso,app" {
		t.Fatal(reply.Excludes)
	}
	w.system.current().Gap(240)
	w.system.current().Ended(SourceEnd{Reason: EndPermission})
	var gap, ended bool
	c.waitFor("the events arrive", func(m any) bool {
		switch e := m.(type) {
		case *SourceGap:
			gap = gap || (e.Session == reply.Session && e.Source == SourceSystem && e.Ms == 240)
		case *SourceEnded:
			ended = ended || (e.Source == SourceSystem && e.Reason == EndPermission)
		}
		return gap && ended
	})
	if _, ok := w.host.LiveSession(); !ok || !w.mic.isOpen() {
		t.Fatal("the session goes on with the microphone")
	}
	c.stop("2", reply.Session)
}

func TestHostLiveARefusedSystemAudioPermissionGetsTheCodePermission(t *testing.T) {
	w := newLiveWorld(t, newLiveFake(), true, "session.live", "audio.system", "audio.mic")
	w.system.set(func(s *fakeSystem) {
		s.refuse = &OpenError{Kind: OpenPermission, Message: "the system audio permission is denied"}
	})
	c := w.client(t)
	if e := liveRefused(t, c.start("1", `"sources":["mic","system"]`)); errCode(e) != CodePermission {
		t.Fatal(e)
	}
	if _, ok := w.host.LiveSession(); ok || w.eng.openNow() != 0 || w.mic.isOpen() {
		t.Fatal("the failed start closes what it opened")
	}
}

func TestHostLiveAMicSessionTakesTheSharedMicrophone(t *testing.T) {
	w := newLiveWorld(t, newLiveFake(), false, "session.live", "audio.mic")
	c := w.client(t)
	session := c.startOK("1", `"sources":["mic"]`).Session
	if info, _ := w.host.LiveSession(); !info.CapturesMic || !w.mic.isOpen() {
		t.Fatal(info)
	}
	// session_audio is only for the source client.
	if e := liveRefused(t, mustRequest(c, "2", fmt.Sprintf(`{"type":"session_audio","id":"2","session":%d,"pcm":%q}`, session, EncodeI16(make([]int16, 160))))); errCode(e) != CodeBadRequest {
		t.Fatal(e)
	}
	w.mic.frames(concat(silenceMs(1000), speechMs(2000), silenceMs(1000), speechMs(1500), silenceMs(1000)))
	text, before := c.stop("3", session)
	if text != "part1 part2" || w.mic.isOpen() {
		t.Fatal("the session gave the microphone back", text)
	}
	if s := liveSegments(before); len(s) != 2 || *s[0].Source != SourceMic {
		t.Fatal(s)
	}
}

func TestHostLiveAClosedConnectionStopsItsSession(t *testing.T) {
	w := newLiveWorld(t, newLiveFake(), false, "session.live", "audio.mic")
	first := w.client(t)
	first.startOK("1", `"sources":["client","mic"]`)
	if w.eng.openNow() != 4 {
		t.Fatal(w.eng.openNow())
	}
	first.c.Close()
	eventually(t, "the session stops", func() bool { _, ok := w.host.LiveSession(); return !ok })
	eventually(t, "the streams close", func() bool { return w.eng.openNow() == 0 })
	if w.mic.isOpen() {
		t.Fatal("the microphone is on")
	}
	second := w.client(t)
	session := second.startOK("1", `"sources":["client"]`).Session
	second.stop("2", session)
}

func TestHostLiveAClosingHostStopsTheSession(t *testing.T) {
	w := newLiveWorld(t, newLiveFake(), false, "session.live")
	c := w.client(t)
	c.startOK("1", `"sources":["client"]`)
	w.host.Close()
	c.waitFor("host_closing", func(m any) bool { e, ok := m.(Event); return ok && e.Kind() == "host_closing" })
	eventually(t, "the session stops", func() bool { _, ok := w.host.LiveSession(); return !ok })
	eventually(t, "the streams close", func() bool { return w.eng.openNow() == 0 })
}

func TestHostLiveAFullEngineGivesTooManyStreams(t *testing.T) {
	eng := newLiveFake()
	eng.maxStreams = 1
	w := newLiveWorld(t, eng, false, "session.live")
	c := w.client(t)
	if e := liveRefused(t, c.start("1", `"sources":["client"]`)); errCode(e) != CodeTooManyStreams {
		t.Fatal(e)
	}
	if w.eng.openNow() != 0 {
		t.Fatal("the host closed the stream that it opened")
	}
	// One stream is enough with Basic VAD: the preview stream.
	session := c.startOK("2", `"sources":["client"],"vad_model":"basic"`).Session
	c.stop("3", session)
}

func TestHostLiveASessionThatCannotRunIsRefusedWithACode(t *testing.T) {
	eng := newLiveFake()
	eng.notOnDisk = map[string]bool{}
	for _, m := range models.ModelsFor(models.TaskDiarization) {
		eng.notOnDisk[string(m.ID)] = true
	}
	w := newLiveWorld(t, eng, false, "session.live")
	c := w.client(t)
	streaming := streamingModel(t).ID
	for i, want := range []struct {
		fields string
		code   ErrorCode
	}{
		{`"sources":["client"],"model":"no-such-model"`, CodeUnknownModel},
		{fmt.Sprintf(`"sources":["client"],"vad_model":%q`, streaming), CodeWrongRole},
		{`"sources":["client"],"diarize":true`, CodeModelNotDownloaded},
		{`"sources":["client"],"record_to":"relative"`, CodeBadRequest},
		{`"sources":[]`, CodeBadRequest},
	} {
		if e := liveRefused(t, c.start(fmt.Sprint(i), want.fields)); errCode(e) != want.code {
			t.Fatal(want, e)
		}
	}
	if _, ok := w.host.LiveSession(); ok {
		t.Fatal("a session runs")
	}
}

func TestHostLiveALiveSessionHoldsSaysoOpenUntilItEnds(t *testing.T) {
	w := newLiveWorld(t, newLiveFake(), false, "session.live", "audio.mic")
	var mu sync.Mutex
	changes := 0
	w.host.SetHoldsObserver(func() { mu.Lock(); changes++; mu.Unlock() })
	var last *LiveInfo
	w.host.SetLiveObserver(func(i *LiveInfo) { mu.Lock(); last = i; mu.Unlock() })
	c := w.client(t)
	session := c.startOK("1", `"sources":["mic"]`).Session
	if h := w.host.Holds(); len(h) != 1 || h[0].Work != WorkLiveSession {
		t.Fatal(h)
	}
	c.stop("2", session)
	if len(w.host.Holds()) != 0 {
		t.Fatal(w.host.Holds())
	}
	mu.Lock()
	if changes < 2 || last != nil {
		t.Fatal("the stop and the end", changes, last)
	}
	mu.Unlock()
	// A connection that closes ends the hold too.
	c.startOK("3", `"sources":["client"]`)
	if len(w.host.Holds()) != 1 {
		t.Fatal(w.host.Holds())
	}
	c.c.Close()
	eventually(t, "the session of a closed connection ends", func() bool { return len(w.host.Holds()) == 0 })
}

func (c *liveClient) waitEnded(session uint64) string {
	c.t.Helper()
	var text string
	c.waitFor("session_ended", func(m any) bool {
		if e, ok := m.(*SessionEnded); ok && e.Session == session {
			text = e.Text
			return true
		}
		return false
	})
	return text
}

func TestHostLiveStopInSaysoEndsTheSessionAndTellsTheApp(t *testing.T) {
	w := newLiveWorld(t, newLiveFake(), false, "session.live")
	c := w.client(t)
	session := c.startOK("1", `"sources":["client"]`).Session
	if !w.host.StopLiveSession(session) {
		t.Fatal("it stops")
	}
	if w.host.StopLiveSession(session) {
		t.Fatal("it stops once")
	}
	c.waitEnded(session)
	eventually(t, "the slot is free", func() bool { _, ok := w.host.LiveSession(); return !ok })
	if e := liveRefused(t, mustRequest(c, "2", fmt.Sprintf(`{"type":"session_stop","id":"2","session":%d}`, session))); errCode(e) != CodeBadRequest {
		t.Fatal("the session is gone", e)
	}
	next := c.startOK("3", `"sources":["client"]`).Session
	if next == session {
		t.Fatal("the same id")
	}
	c.stop("4", next)
}

func TestHostLiveASessionEndsWhenTheUserTakesItsGrantBack(t *testing.T) {
	w := newLiveWorld(t, newLiveFake(), false, "session.live", "audio.mic")
	c := w.client(t)
	session := c.startOK("1", `"sources":["mic"]`).Session
	w.host.StopWorkWithoutGrant()
	info, ok := w.host.LiveSession()
	if !ok || info.Stopping {
		t.Fatal("the grant is still there")
	}
	if err := w.host.Apps().SetGrant(info.AppID, CapSessionLive, false); err != nil {
		t.Fatal(err)
	}
	w.host.StopWorkWithoutGrant()
	c.waitEnded(session)
	eventually(t, "the slot is free", func() bool { _, ok := w.host.LiveSession(); return !ok })
	eventually(t, "the microphone is off", func() bool { return !w.mic.isOpen() })
}

func TestHostLiveAGrantThatGoesDuringTheLoadStopsTheSessionWhenItRuns(t *testing.T) {
	eng := newLiveFake()
	eng.loadStarted, eng.loadRelease = make(chan struct{}), make(chan struct{})
	started, release := eng.loadStarted, eng.loadRelease
	w := newLiveWorld(t, eng, false, "session.live", "audio.mic")
	c := w.client(t)
	c.send(`{"type":"session_start","id":"1","sources":["mic"]}`)
	select {
	case <-started:
	case <-time.After(5 * time.Second):
		t.Fatal("no load")
	}
	appID := w.host.ConnectedApps()[0]
	if err := w.host.Apps().SetGrant(appID, CapAudioMic, false); err != nil {
		t.Fatal(err)
	}
	w.host.StopWorkWithoutGrant()
	if w.mic.isOpen() {
		t.Fatal("the session does not run yet")
	}
	close(release)
	var s SessionReply
	liveOK(t, mustRequest(c, "1", "")).Parse(&s)
	c.waitEnded(s.Session)
	eventually(t, "the slot is free", func() bool { _, ok := w.host.LiveSession(); return !ok })
	eventually(t, "the microphone is off", func() bool { return !w.mic.isOpen() })
}

func TestHostLiveAnAppCanStopItsOwnSessionAfterAGrantGoes(t *testing.T) {
	w := newLiveWorld(t, newLiveFake(), false, "session.live")
	c := w.client(t)
	session := c.startOK("1", `"sources":["client"]`).Session
	info, _ := w.host.LiveSession()
	if err := w.host.Apps().SetGrant(info.AppID, CapSessionLive, false); err != nil {
		t.Fatal(err)
	}
	if e := liveRefused(t, mustRequest(c, "2", fmt.Sprintf(`{"type":"session_audio","id":"2","session":%d,"pcm":""}`, session))); errCode(e) != CodeCapability {
		t.Fatal("no new audio", e)
	}
	c.stop("3", session)
	eventually(t, "the slot is free", func() bool { _, ok := w.host.LiveSession(); return !ok })
}

func TestHostLiveAnAppThatQuitsDuringTheQuestionOfTheSystemLeavesNoSession(t *testing.T) {
	w := newLiveWorld(t, newLiveFake(), true, "session.live", "audio.system")
	w.system.set(func(s *fakeSystem) { s.hold = true })
	app := w.client(t)
	app.send(`{"type":"session_start","id":"s","sources":["system"]}`)
	eventually(t, "the capture waits", func() bool { w.system.mu.Lock(); defer w.system.mu.Unlock(); return w.system.waiting })
	app.c.Close()
	c := w.client(t)
	eventually(t, "the host saw the end of the connection", func() bool { return len(w.host.ConnectedApps()) == 1 })
	if e := liveRefused(t, c.start("1", `"sources":["client"]`)); errCode(e) != CodeBusy {
		t.Fatal("the question is still open", e)
	}
	w.system.set(func(s *fakeSystem) { s.hold = false })
	n := 0
	eventually(t, "the slot is free", func() bool {
		n++
		_, isErr := c.start(fmt.Sprint("r", n), `"sources":["client"]`).(*ErrorReply)
		return !isErr
	})
	if info, ok := w.host.LiveSession(); !ok || len(info.Sources) != 1 || info.Sources[0] != SourceClient {
		t.Fatal("the session of the app that quit is gone", info)
	}
}
