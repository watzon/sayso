package bridge

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/update"
)

type captureServices struct {
	*fakeServices
	mic Microphone
	sys SystemAudio
}

func (c captureServices) Microphone() Microphone   { return c.mic }
func (c captureServices) SystemAudio() SystemAudio { return c.sys }

type nopMic struct{}

func (nopMic) Open(Feed) (func(), error) { return func() {}, nil }

type nopSystem struct{}

func (nopSystem) Open(*uint32, Feed) (func(), []string, error) { return func() {}, nil, nil }

func startHostWith(t *testing.T, svc Services, version string) string {
	t.Helper()
	path := socketPath(t)
	appsFile := filepath.Join(t.TempDir(), "apps.toml")
	os.WriteFile(appsFile, []byte(allowedApp), 0o600)
	cfg := NewHostConfig(path, appsFile)
	cfg.Identify = pathIdentify{"/opt/App"}
	cfg.ApprovalCooldown = 50 * time.Millisecond
	cfg.HostVersion = version
	h, err := StartHost(cfg, svc)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(h.Close)
	return path
}

func helloOf(t *testing.T, path string) HelloReply {
	t.Helper()
	c := dial(t, path)
	c.send(`{"type":"hello","id":"h","protocol":1}`)
	var hr HelloReply
	ok, isOK := mustUntil(c, "h").(OKFields)
	if !isOK {
		t.Fatal("hello was refused")
	}
	if err := ok.Parse(&hr); err != nil {
		t.Fatal(err)
	}
	return hr
}

func TestHelloNamesTheFeaturesOfTheHost(t *testing.T) {
	base := "dictation_done,download_question,enhance,record_to,silence_limit"
	eng := newFakeEngine()
	eng.features = []string{"catalog", "diarization", "segments", "store_v1", "vad"}
	cases := []struct {
		name string
		svc  Services
		want string
	}{
		{"no engine, no capture", captureServices{fakeServices: &fakeServices{cfg: testConfig("x")}}, base},
		{"the microphone", captureServices{fakeServices: &fakeServices{cfg: testConfig("x")}, mic: nopMic{}}, "dictation_done,download_question,enhance,mic_capture,record_to,silence_limit"},
		{"the microphone and the system", captureServices{fakeServices: &fakeServices{cfg: testConfig("x")}, mic: nopMic{}, sys: nopSystem{}}, "dictation_done,download_question,echo_rule,enhance,mic_capture,record_to,silence_limit,system_capture"},
		{"an engine with diarization", captureServices{fakeServices: &fakeServices{cfg: testConfig("x"), eng: eng}}, "diarization,dictation_done,download_question,enhance,record_to,segments,silence_limit,vad"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			hr := helloOf(t, startHostWith(t, c.svc, "9.9.9"))
			if got := strings.Join(hr.Features, ","); got != c.want {
				t.Fatalf("got %s\nwant %s", got, c.want)
			}
		})
	}
}

func TestHelloGivesTheVersionOfTheApp(t *testing.T) {
	path := socketPath(t)
	appsFile := filepath.Join(t.TempDir(), "apps.toml")
	os.WriteFile(appsFile, []byte(allowedApp), 0o600)
	cfg := NewHostConfig(path, appsFile)
	cfg.Identify = pathIdentify{"/opt/App"}
	h, err := StartHost(cfg, &fakeServices{cfg: testConfig("x")})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(h.Close)
	if hr := helloOf(t, path); hr.HostVersion != update.Version {
		t.Fatalf("host_version %s, want %s", hr.HostVersion, update.Version)
	}
}
