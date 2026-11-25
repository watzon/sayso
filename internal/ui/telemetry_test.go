package ui

import (
	"testing"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/i18n"
)

// telemetrySwitch is the telemetry switch a tester shows: found, and on.
func telemetrySwitch(t *testing.T, tt *mu.Tester) bool {
	t.Helper()
	tt.Frame()
	for _, a := range tt.AccessAll() {
		if a.Name == i18n.T("settings.general.telemetry") && a.Role == mu.RoleSwitch {
			return a.Checked
		}
	}
	t.Fatalf("no telemetry switch; texts %q", tt.Texts())
	return false
}

func savedTelemetry(t *testing.T, e *testEnv) bool {
	t.Helper()
	l, err := config.Load(e.paths.ConfigFile())
	if err != nil {
		t.Fatal(err)
	}
	return l.Config.Telemetry.Enabled
}

// Onboarding's last step and Settings › General show one setting: a change
// in either is saved, published to the telemetry client, and shown by the
// other.
func TestTelemetrySwitchesShareOneSetting(t *testing.T) {
	e := newEnv(t)
	var heard []bool
	e.a.OnConfig(func(c *config.Config) { heard = append(heard, c.Telemetry.Enabled) })
	e.a.onb.step = stepDone
	onb := mu.NewTesterWith(e.a.OnboardingView, OnboardingW, OnboardingH, mu.TesterOptions{Preferences: mu.Preferences{ReduceMotion: true}})
	if !telemetrySwitch(t, onb) {
		t.Fatal("telemetry is off by default")
	}
	e.a.OpenSettings(SecGeneral)
	hub := e.hub(t)
	if !telemetrySwitch(t, hub) {
		t.Fatal("Settings › General shows telemetry off by default")
	}
	if miss := missingKeys(e.a.Keys(), []string{"onboarding.telemetry", "general.telemetry"}); len(miss) > 0 {
		t.Fatalf("no key on %v", miss)
	}

	press(t, onb, i18n.T("settings.general.telemetry"))
	if telemetrySwitch(t, onb) || e.a.cfg.Telemetry.Enabled || savedTelemetry(t, e) || len(heard) == 0 || heard[len(heard)-1] {
		t.Fatalf("onboarding did not turn it off: config %v, heard %v", e.a.cfg.Telemetry.Enabled, heard)
	}
	if telemetrySwitch(t, hub) {
		t.Fatal("Settings › General still shows telemetry on")
	}

	press(t, hub, i18n.T("settings.general.telemetry"))
	if !telemetrySwitch(t, hub) || !savedTelemetry(t, e) || !heard[len(heard)-1] {
		t.Fatalf("Settings did not turn it on: heard %v", heard)
	}
	if !telemetrySwitch(t, onb) {
		t.Fatal("onboarding still shows telemetry off")
	}
}
