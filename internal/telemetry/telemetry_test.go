package telemetry

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"
)

// ingest is a fake TelemetryDeck: it keeps each request's raw body.
type ingest struct {
	t      *testing.T
	srv    *httptest.Server
	mu     sync.Mutex
	bodies [][]byte
	status int
	// hold blocks each request until it is closed.
	hold chan struct{}
	got  chan struct{}
}

func newIngest(t *testing.T) *ingest {
	in := &ingest{t: t, status: http.StatusOK, got: make(chan struct{}, 100)}
	in.srv = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost || r.URL.Path != "/v2/" {
			t.Errorf("request %s %s", r.Method, r.URL.Path)
		}
		if ct := r.Header.Get("Content-Type"); ct != "application/json; charset=utf-8" {
			t.Errorf("Content-Type %q", ct)
		}
		body, _ := io.ReadAll(r.Body)
		in.mu.Lock()
		in.bodies = append(in.bodies, body)
		hold, status := in.hold, in.status
		in.mu.Unlock()
		in.got <- struct{}{}
		if hold != nil {
			select {
			case <-hold:
			case <-r.Context().Done():
				return
			}
		}
		w.WriteHeader(status)
		io.WriteString(w, "OK")
	}))
	t.Cleanup(in.srv.Close)
	return in
}

func (in *ingest) requests() int {
	in.mu.Lock()
	defer in.mu.Unlock()
	return len(in.bodies)
}

func (in *ingest) signals() []map[string]any {
	in.mu.Lock()
	defer in.mu.Unlock()
	var out []map[string]any
	for _, b := range in.bodies {
		var batch []map[string]any
		if err := json.Unmarshal(b, &batch); err != nil {
			in.t.Fatalf("body is not a JSON array: %s", b)
		}
		out = append(out, batch...)
	}
	return out
}

func (in *ingest) waitRequest(t *testing.T) {
	t.Helper()
	select {
	case <-in.got:
	case <-time.After(5 * time.Second):
		t.Fatal("no request")
	}
}

func (in *ingest) options(t *testing.T) Options {
	return Options{AppID: "APP-ID", Endpoint: in.srv.URL + "/v2/", TestMode: true,
		StateFile: filepath.Join(t.TempDir(), "telemetry.json"), AppVersion: "1.2.3", Channel: ChannelDev,
		RetryAfter: 20 * time.Millisecond}
}

// settle closes the client, which sends what is queued.
func settle(c *Client) { c.Close(5 * time.Second) }

func types(sigs []map[string]any) []string {
	var out []string
	for _, s := range sigs {
		out = append(out, s["type"].(string))
	}
	return out
}

var hex64 = regexp.MustCompile(`^[0-9a-f]{64}$`)
var uuid4 = regexp.MustCompile(`^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$`)

func TestSignalsFollowTheV2Schema(t *testing.T) {
	in := newIngest(t)
	o := in.options(t)
	c := New(o, true)
	c.Launched()
	c.DictationCompleted(Dictation{ModelID: "parakeet-tdt-0.6b-v2", ModelKind: ModelLocal, StyleKind: StyleCloud})
	settle(c)

	sigs := in.signals()
	if got := types(sigs); len(got) != 3 || got[0] != TypeInstalled || got[1] != TypeLaunched || got[2] != TypeDictation {
		t.Fatalf("types %v", got)
	}
	var st state
	data, _ := os.ReadFile(o.StateFile)
	if err := json.Unmarshal(data, &st); err != nil || !uuid4.MatchString(st.InstallID) {
		t.Fatalf("state %s", data)
	}
	allowed := map[string]bool{"appID": true, "clientUser": true, "sessionID": true, "type": true, "isTestMode": true, "payload": true}
	for _, s := range sigs {
		for k := range s {
			if !allowed[k] {
				t.Errorf("unexpected field %q", k)
			}
		}
		if s["appID"] != "APP-ID" || s["isTestMode"] != true {
			t.Errorf("signal %v", s)
		}
		cu, _ := s["clientUser"].(string)
		if !hex64.MatchString(cu) || cu != ClientUser(st.InstallID) || cu == st.InstallID {
			t.Errorf("clientUser %q is not the SHA-256 of the install ID", cu)
		}
		if sid, _ := s["sessionID"].(string); !uuid4.MatchString(sid) || sid != sigs[0]["sessionID"] {
			t.Errorf("sessionID %v", s["sessionID"])
		}
		p, ok := s["payload"].(map[string]any)
		if !ok {
			t.Fatalf("payload %v", s["payload"])
		}
		for k, v := range p {
			if _, flat := v.(string); !flat {
				t.Errorf("payload %s is %T, want a flat string", k, v)
			}
		}
		if p["os"] != runtime.GOOS || p["arch"] != runtime.GOARCH || p["appVersion"] != "1.2.3" ||
			p["buildChannel"] != ChannelDev || p["platform"] != "desktop" {
			t.Errorf("payload %v", p)
		}
	}
	want := map[string]any{"os": runtime.GOOS, "arch": runtime.GOARCH, "appVersion": "1.2.3", "buildChannel": ChannelDev,
		"platform": "desktop", "modelId": "parakeet-tdt-0.6b-v2", "modelKind": "local", "styleKind": "ai_cloud"}
	p := sigs[2]["payload"].(map[string]any)
	if len(p) != len(want) {
		t.Fatalf("dictation payload %v, want exactly %v", p, want)
	}
	for k, v := range want {
		if p[k] != v {
			t.Errorf("dictation payload %s = %v, want %v", k, p[k], v)
		}
	}
}

func TestInstalledIsSentOncePerInstall(t *testing.T) {
	in := newIngest(t)
	o := in.options(t)
	c := New(o, true)
	c.Launched()
	c.Launched() // once per run
	settle(c)
	if got := types(in.signals()); len(got) != 2 || got[0] != TypeInstalled || got[1] != TypeLaunched {
		t.Fatalf("first run sent %v", got)
	}
	first := in.signals()[0]["clientUser"]

	in.mu.Lock()
	in.bodies = nil
	in.mu.Unlock()
	c = New(o, true)
	c.Launched()
	settle(c)
	sigs := in.signals()
	if got := types(sigs); len(got) != 1 || got[0] != TypeLaunched {
		t.Fatalf("second run sent %v", got)
	}
	if sigs[0]["clientUser"] != first {
		t.Fatal("the install ID changed between runs")
	}
}

func TestInstalledIsNotMarkedSentUntilTheServerTakesIt(t *testing.T) {
	in := newIngest(t)
	in.status = http.StatusServiceUnavailable
	o := in.options(t)
	o.RetryAfter = time.Hour
	c := New(o, true)
	c.Launched()
	in.waitRequest(t)
	settle(c)
	if loadState(o.StateFile).FirstLaunchSent {
		t.Fatal("a refused app.installed was marked sent")
	}

	in.mu.Lock()
	in.status, in.bodies = http.StatusOK, nil
	in.mu.Unlock()
	c = New(o, true)
	c.Launched()
	settle(c)
	if got := types(in.signals()); len(got) == 0 || got[0] != TypeInstalled {
		t.Fatalf("the next run sent %v", got)
	}
	if !loadState(o.StateFile).FirstLaunchSent {
		t.Fatal("app.installed is not marked sent")
	}
}

func TestFailedSendIsRetried(t *testing.T) {
	in := newIngest(t)
	in.status = http.StatusBadGateway
	c := New(in.options(t), true)
	c.Launched()
	in.waitRequest(t)
	in.mu.Lock()
	in.status = http.StatusOK
	in.mu.Unlock()
	in.waitRequest(t)
	settle(c)
	in.mu.Lock()
	last := in.bodies[len(in.bodies)-1]
	in.mu.Unlock()
	var batch []map[string]any
	json.Unmarshal(last, &batch)
	if len(batch) != 2 {
		t.Fatalf("retry sent %s", last)
	}
}

func TestOffSendsNothing(t *testing.T) {
	in := newIngest(t)
	o := in.options(t)
	c := New(o, false)
	c.Launched()
	c.DictationCompleted(Dictation{ModelID: "m", ModelKind: ModelLocal, StyleKind: StyleRaw})
	settle(c)
	if n := in.requests(); n != 0 {
		t.Fatalf("%d requests while off", n)
	}
	if _, err := os.Stat(o.StateFile); !os.IsNotExist(err) {
		t.Fatal("off wrote an install ID")
	}
}

func TestTurningOffStopsQueuedRetriesAndTheSendInFlight(t *testing.T) {
	in := newIngest(t)
	in.hold = make(chan struct{})
	o := in.options(t)
	c := New(o, true)
	c.Launched()
	in.waitRequest(t) // the send is in flight
	c.SetEnabled(false)
	c.DictationCompleted(Dictation{ModelID: "m", ModelKind: ModelLocal, StyleKind: StyleRaw})
	close(in.hold)
	time.Sleep(100 * time.Millisecond) // more than RetryAfter
	settle(c)
	if n := in.requests(); n != 1 {
		t.Fatalf("%d requests, want only the one in flight when it was turned off", n)
	}
	if loadState(o.StateFile).FirstLaunchSent {
		t.Fatal("the cancelled app.installed was marked sent")
	}

	// Off with a failed batch waiting for its retry.
	in2 := newIngest(t)
	in2.status = http.StatusInternalServerError
	o2 := in2.options(t)
	c = New(o2, true)
	c.Launched()
	in2.waitRequest(t)
	c.SetEnabled(false)
	in2.mu.Lock()
	in2.status = http.StatusOK
	in2.mu.Unlock()
	time.Sleep(100 * time.Millisecond)
	settle(c)
	if n := in2.requests(); n != 1 {
		t.Fatalf("%d requests: the retry ran after telemetry was turned off", n)
	}
}

// acceptThenOff is a TelemetryDeck that takes each batch with 200, but the
// user turns telemetry off before the client reads the answer.
type acceptThenOff struct {
	mu    sync.Mutex
	c     *Client
	types []string
}

func (a *acceptThenOff) RoundTrip(r *http.Request) (*http.Response, error) {
	body, _ := io.ReadAll(r.Body)
	var batch []Signal
	json.Unmarshal(body, &batch)
	a.mu.Lock()
	for _, s := range batch {
		a.types = append(a.types, s.Type)
	}
	c := a.c
	a.mu.Unlock()
	if c != nil {
		c.SetEnabled(false)
	}
	return &http.Response{StatusCode: http.StatusOK, Body: io.NopCloser(strings.NewReader("OK")), Request: r}, nil
}

func TestInstalledTakenByTheServerIsMarkedSentEvenIfTurnedOffMeanwhile(t *testing.T) {
	rt := &acceptThenOff{}
	o := Options{AppID: "APP-ID", Endpoint: "https://ingest.test/v2/", TestMode: true,
		StateFile: filepath.Join(t.TempDir(), "telemetry.json"), HTTP: &http.Client{Transport: rt}, RetryAfter: time.Hour}
	c := New(o, true)
	rt.mu.Lock()
	rt.c = c
	rt.mu.Unlock()
	c.Launched()
	settle(c)
	if len(rt.types) == 0 || rt.types[0] != TypeInstalled {
		t.Fatalf("first run sent %v", rt.types)
	}

	rt.mu.Lock()
	rt.c, rt.types = nil, nil
	rt.mu.Unlock()
	c = New(o, false)
	c.SetEnabled(true)
	c.Launched()
	settle(c)
	if len(rt.types) != 1 || rt.types[0] != TypeLaunched {
		t.Fatalf("after on again and a relaunch: %v, want only %s", rt.types, TypeLaunched)
	}
}

func TestTurningOnAgainSendsLaunched(t *testing.T) {
	in := newIngest(t)
	c := New(in.options(t), false)
	c.Launched()
	c.SetEnabled(true)
	c.Launched()
	settle(c)
	if got := types(in.signals()); len(got) != 2 || got[1] != TypeLaunched {
		t.Fatalf("sent %v", got)
	}
}

func TestEmptyAppIDSendsNothing(t *testing.T) {
	in := newIngest(t)
	o := in.options(t)
	o.AppID = "  "
	c := New(o, true)
	if c.Active() {
		t.Fatal("active without an App ID")
	}
	c.Launched()
	c.DictationCompleted(Dictation{ModelID: "m", ModelKind: ModelLocal, StyleKind: StyleRaw})
	settle(c)
	if n := in.requests(); n != 0 {
		t.Fatalf("%d requests without an App ID", n)
	}
}

func TestOptionsForThisBuild(t *testing.T) {
	for _, k := range []string{"SAYSO_TELEMETRY_APP_ID", "SAYSO_TELEMETRY_ENDPOINT", "SAYSO_TELEMETRY_TEST_MODE"} {
		t.Setenv(k, "")
	}
	dev := OptionsForThisBuild("/data", "0.16.0", false)
	if !dev.TestMode || dev.Channel != ChannelDev || dev.AppID != DefaultAppID || dev.Endpoint != DefaultEndpoint ||
		dev.StateFile != filepath.Join("/data", "telemetry.json") || dev.AppVersion != "0.16.0" {
		t.Fatalf("dev build %+v", dev)
	}
	rel := OptionsForThisBuild("/data", "0.16.0", true)
	if rel.TestMode || rel.Channel != ChannelRelease {
		t.Fatalf("official build %+v", rel)
	}
	t.Setenv("SAYSO_TELEMETRY_APP_ID", "QA-ID")
	t.Setenv("SAYSO_TELEMETRY_ENDPOINT", "http://127.0.0.1:9/v2/")
	t.Setenv("SAYSO_TELEMETRY_TEST_MODE", "1")
	qa := OptionsForThisBuild("/data", "0.16.0", true)
	if qa.AppID != "QA-ID" || qa.Endpoint != "http://127.0.0.1:9/v2/" || !qa.TestMode || qa.Channel != ChannelRelease {
		t.Fatalf("overrides %+v", qa)
	}
	t.Setenv("SAYSO_TELEMETRY_TEST_MODE", "false")
	if OptionsForThisBuild("/data", "0.16.0", false).TestMode {
		t.Fatal("SAYSO_TELEMETRY_TEST_MODE=false kept test mode")
	}
}

func TestNoAppIDByDefaultMeansInactive(t *testing.T) {
	if DefaultAppID != "" {
		t.Skip("this build has an App ID")
	}
	t.Setenv("SAYSO_TELEMETRY_APP_ID", "")
	c := New(OptionsForThisBuild(t.TempDir(), "0.16.0", true), true)
	defer settle(c)
	if c.Active() {
		t.Fatal("active with the empty default App ID")
	}
}

func TestSignalsQueuedBeforeOffNeverGoOut(t *testing.T) {
	in := newIngest(t)
	in.hold = make(chan struct{})
	c := New(in.options(t), true)
	c.Launched()
	in.waitRequest(t)
	c.DictationCompleted(Dictation{ModelID: "m", ModelKind: ModelLocal, StyleKind: StyleRaw}) // queued behind the send
	c.SetEnabled(false)
	c.SetEnabled(true)
	close(in.hold)
	settle(c)
	for _, typ := range types(in.signals()) {
		if typ == TypeDictation {
			t.Fatal("a dictation queued before telemetry was turned off was sent")
		}
	}
}

func TestASendCancelledByOffIsNotRetriedAfterOnAgain(t *testing.T) {
	in := newIngest(t)
	in.hold = make(chan struct{})
	c := New(in.options(t), true)
	c.Launched()
	in.waitRequest(t)
	c.SetEnabled(false) // cancels the send
	c.SetEnabled(true)
	close(in.hold)
	time.Sleep(100 * time.Millisecond) // more than RetryAfter
	settle(c)
	if n := in.requests(); n != 1 {
		t.Fatalf("%d requests: the cancelled batch was sent again", n)
	}
}
