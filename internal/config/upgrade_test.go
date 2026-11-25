package config_test

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/platform"
)

// sections splits a TOML file into its tables: the header (with a number for
// each repeat of an array table) and the key lines under it.
func sections(text string) map[string][]string {
	out := map[string][]string{}
	seen := map[string]int{}
	head := ""
	for _, line := range strings.Split(text, "\n") {
		line = strings.TrimSpace(line)
		switch {
		case line == "" || strings.HasPrefix(line, "#"):
		case strings.HasPrefix(line, "["):
			seen[line]++
			head = line + "#" + string(rune('0'+seen[line]))
			out[head] = out[head][:0:0]
		default:
			out[head] = append(out[head], line)
		}
	}
	return out
}

func loadUpgradeFixture(t *testing.T, device string) config.Loaded {
	t.Helper()
	b, err := os.ReadFile(filepath.Join("testdata", "upgrade-0.16.toml"))
	if err != nil {
		t.Fatal(err)
	}
	text := strings.Replace(string(b), `"coreaudio:BuiltInMicrophoneDevice"`, `"`+device+`"`, 1)
	dir := t.TempDir()
	path := filepath.Join(dir, "config.toml")
	if err := os.WriteFile(path, []byte(text), 0o600); err != nil {
		t.Fatal(err)
	}
	l, err := config.Load(path)
	if err != nil {
		t.Fatal(err)
	}
	if len(l.Issues) != 0 {
		t.Fatalf("issues: %v", l.Issues)
	}
	return l
}

// A config.toml of the Rust app (0.14 to 0.16) loads with no issue, keeps
// the input device of each audio host, turns telemetry on (the file has no
// [telemetry]), and the device id it names is the id of the system.
func TestARustConfigLoadsWithItsDeviceAndTelemetryOn(t *testing.T) {
	for saved, id := range map[string]string{
		"coreaudio:BuiltInMicrophoneDevice":                    "BuiltInMicrophoneDevice",
		"wasapi:{0.0.1.00000000}.{6a1f2c3d-0000-4000-8000-1}": "{0.0.1.00000000}.{6a1f2c3d-0000-4000-8000-1}",
		"alsa:hw:CARD=USB,DEV=0":                               "hw:CARD=USB,DEV=0",
	} {
		cfg := loadUpgradeFixture(t, saved).Config
		if cfg.Audio.InputDevice == nil || *cfg.Audio.InputDevice != saved {
			t.Fatalf("%s: input_device is %v", saved, cfg.Audio.InputDevice)
		}
		if got := platform.NormalizeDeviceID(*cfg.Audio.InputDevice); got != id {
			t.Fatalf("%s: the device id is %q, want %q", saved, got, id)
		}
		if !cfg.Telemetry.Enabled {
			t.Fatalf("%s: telemetry is off for a config from before [telemetry]", saved)
		}
	}
}

// Each setting of the Rust config survives a load and a save by this app:
// every key line of the file is in the same table of the saved file.
func TestARustConfigKeepsEverySettingThroughASave(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("the file holds the macOS shape of the config")
	}
	b, err := os.ReadFile(filepath.Join("testdata", "upgrade-0.16.toml"))
	if err != nil {
		t.Fatal(err)
	}
	saved := loadUpgradeFixture(t, "coreaudio:BuiltInMicrophoneDevice").Config.ToTOML()
	got := sections(saved)
	for head, lines := range sections(string(b)) {
		have := map[string]bool{}
		for _, l := range got[head] {
			have[l] = true
		}
		for _, l := range lines {
			if !have[l] {
				t.Errorf("%s: %q is gone after a save", strings.Split(head, "#")[0], l)
			}
		}
	}
	if !strings.Contains(saved, "[telemetry]\nenabled = true\n") {
		t.Fatal("the saved file has no [telemetry] enabled = true")
	}
	again := config.Parse(saved)
	if len(again.Issues) != 0 || again.Config.ToTOML() != saved {
		t.Fatalf("a second load changes the file: issues %v", again.Issues)
	}
}
