package main

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/telemetry"
)

// The app's hook with the real client and a fake TelemetryDeck: nothing goes
// out during onboarding or while the switch is off.
func TestTelemetryWaitsForOnboardingAndFollowsTheSwitch(t *testing.T) {
	var mu sync.Mutex
	var sent []string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		var batch []telemetry.Signal
		if err := json.Unmarshal(body, &batch); err != nil {
			t.Errorf("body %s", body)
		}
		mu.Lock()
		for _, s := range batch {
			sent = append(sent, s.Type)
		}
		mu.Unlock()
	}))
	defer srv.Close()
	types := func() []string {
		mu.Lock()
		defer mu.Unlock()
		return append([]string(nil), sent...)
	}
	opts := telemetry.Options{AppID: "APP", Endpoint: srv.URL, TestMode: true, AppVersion: "0.16.0", Channel: telemetry.ChannelDev,
		StateFile: filepath.Join(t.TempDir(), "telemetry.json")}

	cfg := config.Default()
	tel := telemetry.New(opts, cfg.Telemetry.Enabled)
	hook := telemetrySwitch(tel, &cfg)
	time.Sleep(50 * time.Millisecond)
	if got := types(); len(got) != 0 {
		t.Fatalf("sent during onboarding: %v", got)
	}
	cfg.Onboarding.Completed = true
	hook(&cfg)
	tel.Close(5 * time.Second)
	if got := types(); len(got) != 2 || got[0] != telemetry.TypeInstalled || got[1] != telemetry.TypeLaunched {
		t.Fatalf("after onboarding: %v", got)
	}

	// The next run, turned off during onboarding's last step.
	mu.Lock()
	sent = nil
	mu.Unlock()
	cfg = config.Default()
	cfg.Telemetry.Enabled = false
	tel = telemetry.New(opts, true)
	hook = telemetrySwitch(tel, &cfg)
	cfg.Onboarding.Completed = true
	hook(&cfg)
	tel.DictationCompleted(telemetry.Dictation{ModelID: "m", ModelKind: telemetry.ModelLocal, StyleKind: telemetry.StyleRaw})
	tel.Close(5 * time.Second)
	if got := types(); len(got) != 0 {
		t.Fatalf("sent while off: %v", got)
	}
}
