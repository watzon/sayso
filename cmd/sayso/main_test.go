package main

import (
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/watzon/sayso/internal/paths"
	"github.com/watzon/sayso/internal/ui"
)

func TestParseArgs(t *testing.T) {
	cases := []struct {
		args []string
		want options
	}{
		{nil, options{}},
		{[]string{"--version"}, options{version: true}},
		{[]string{"--pseudo"}, options{pseudo: true}},
		{[]string{"--toggle"}, options{command: ui.Command{Kind: ui.CmdToggle}}},
		{[]string{"--style=email"}, options{command: ui.Command{Kind: ui.CmdStyle, Style: "email"}}},
		{[]string{"--push-to-talk-up-submit"}, options{command: ui.Command{Kind: ui.CmdPushToTalkUpSubmit}}},
		{[]string{"--open"}, options{command: ui.Command{Kind: ui.CmdOpen}}},
	}
	for _, c := range cases {
		got, err := parseArgs(c.args, io.Discard)
		if err != nil || got != c.want {
			t.Errorf("%v: %+v, %v; want %+v", c.args, got, err, c.want)
		}
	}
	if _, err := parseArgs([]string{"--bogus"}, io.Discard); err == nil {
		t.Error("an unknown flag parsed")
	}
}

// The embedded copy must match the repository's file; run go generate
// ./cmd/sayso after editing release-notes.toml.
func TestReleaseNotesCopyIsCurrent(t *testing.T) {
	want, err := os.ReadFile("../../release-notes.toml")
	if err != nil {
		t.Fatal(err)
	}
	if strings.ReplaceAll(string(want), "\r\n", "\n") != strings.ReplaceAll(releaseNotes, "\r\n", "\n") {
		t.Error("cmd/sayso/release-notes.toml is out of date: run go generate ./cmd/sayso")
	}
}

// The smoke run renders every surface and writes only under the
// temporary folders it is given.
func TestSmokeRunStaysInItsFolders(t *testing.T) {
	dir := t.TempDir()
	for _, v := range []string{"XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"} {
		t.Setenv(v, dir)
	}
	p := paths.Resolve(paths.SystemEnv{})
	for _, d := range []string{p.ConfigDir, p.DataDir, p.CacheDir} {
		if !strings.HasPrefix(d, dir) {
			t.Fatalf("folder %s is outside %s", d, dir)
		}
	}
	out := captureStdout(t, func() {
		if err := run(options{}, true); err != nil {
			t.Fatal(err)
		}
	})
	if !strings.Contains(out, "renders ok") {
		t.Errorf("smoke output: %s", out)
	}
	if _, err := os.Stat(filepath.Join(p.DataDir, "sayso.db")); err != nil {
		t.Errorf("the smoke run did not use its data folder: %v", err)
	}
}

func captureStdout(t *testing.T, fn func()) string {
	t.Helper()
	r, w, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	old := os.Stdout
	os.Stdout = w
	done := make(chan string)
	go func() {
		b, _ := io.ReadAll(r)
		done <- string(b)
	}()
	fn()
	os.Stdout = old
	w.Close()
	return <-done
}
