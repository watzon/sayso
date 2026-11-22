package platform

// Pause and resume the now-playing app through the MediaRemote adapter of
// the app bundle (native/macos/MediaRemoteAdapter). Ported from
// sayso-platform-macos/src/media.rs.
//
// Since macOS 15.4 the private MediaRemote framework answers only Apple
// processes. /usr/bin/perl is one, so the adapter script runs under perl and
// loads the adapter framework. macOS has one now-playing app, so this pauses
// one player at most. A run outside the app bundle has no adapter and pauses
// nothing.

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	mediaPlay  = "0"
	mediaPause = "1"
	// mediaRunLimit stops a perl that hangs. The adapter itself stops a read after 2 s.
	mediaRunLimit = 4 * time.Second
	// The app can report "playing" for a moment after the pause, so the
	// resume reads the state again for a while before it gives up.
	mediaResumeTries = 5
	mediaResumeWait  = 200 * time.Millisecond
)

type macMedia struct {
	adapter *mediaAdapter
	mu      sync.Mutex
	paused  string // the player that Sayso paused, empty when none
}

type mediaAdapter struct {
	script, framework string
}

func newMacMedia() *macMedia {
	m := &macMedia{}
	if exe, err := os.Executable(); err == nil {
		m.adapter = adapterInBundle(exe)
	}
	return m
}

// adapterInBundle finds the adapter in Sayso.app/Contents for the executable
// Sayso.app/Contents/MacOS/Sayso.
func adapterInBundle(exe string) *mediaAdapter {
	contents := filepath.Dir(filepath.Dir(exe))
	a := &mediaAdapter{
		script:    filepath.Join(contents, "Resources", "mediaremote-adapter.pl"),
		framework: filepath.Join(contents, "Frameworks", "MediaRemoteAdapter.framework"),
	}
	if st, err := os.Stat(a.script); err != nil || !st.Mode().IsRegular() {
		return nil
	}
	if st, err := os.Stat(a.framework); err != nil || !st.IsDir() {
		return nil
	}
	return a
}

func (m *macMedia) Pause() (bool, error) {
	if m.adapter == nil {
		return false, nil
	}
	now, err := m.adapter.nowPlaying()
	if err != nil || now == nil || !now.playing {
		return false, err
	}
	if _, err := m.adapter.run("send", mediaPause); err != nil {
		return false, err
	}
	m.mu.Lock()
	m.paused = now.player
	m.mu.Unlock()
	return true, nil
}

func (m *macMedia) Resume() error {
	m.mu.Lock()
	player := m.paused
	m.paused = ""
	m.mu.Unlock()
	if m.adapter == nil || player == "" {
		return nil
	}
	for attempt := 0; attempt < mediaResumeTries; attempt++ {
		if attempt > 0 {
			time.Sleep(mediaResumeWait)
		}
		now, err := m.adapter.nowPlaying()
		if err != nil {
			return err
		}
		// Another app plays now, or none: play nothing.
		if now == nil || now.player != player {
			return nil
		}
		if !now.playing {
			_, err := m.adapter.run("send", mediaPlay)
			return err
		}
	}
	return nil
}

type nowPlaying struct {
	player  string
	playing bool
}

func (a *mediaAdapter) nowPlaying() (*nowPlaying, error) {
	out, err := a.run("get", "--no-artwork", "--allow-missing-title")
	if err != nil {
		return nil, err
	}
	return parseNowPlaying(out), nil
}

func (a *mediaAdapter) run(args ...string) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), mediaRunLimit)
	defer cancel()
	cmd := exec.CommandContext(ctx, "/usr/bin/perl", append([]string{a.script, a.framework}, args...)...)
	var stdout, stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("media: the adapter failed %q: %v: %s", args[0], err, strings.TrimSpace(stderr.String()))
	}
	return stdout.String(), nil
}

// parseNowPlaying reads the output of "get": a JSON object, or null when no
// app plays media. The player is the bundle ID and the process: the bundle
// ID alone would also match a new process of the same app.
func parseNowPlaying(out string) *nowPlaying {
	var v struct {
		BundleID       *string `json:"bundleIdentifier"`
		ParentBundleID *string `json:"parentApplicationBundleIdentifier"`
		PID            *int64  `json:"processIdentifier"`
		Playing        bool    `json:"playing"`
	}
	if err := json.Unmarshal([]byte(strings.TrimSpace(out)), &v); err != nil {
		return nil
	}
	bundle := v.BundleID
	if bundle == nil {
		bundle = v.ParentBundleID
	}
	var player string
	switch {
	case bundle != nil && v.PID != nil:
		player = *bundle + " " + strconv.FormatInt(*v.PID, 10)
	case bundle != nil:
		player = *bundle
	case v.PID != nil:
		player = strconv.FormatInt(*v.PID, 10)
	default:
		return nil
	}
	return &nowPlaying{player: player, playing: v.Playing}
}
