package platform

// Pause and resume the now-playing app. Ported from
// sayso-platform-macos/src/media.rs.
//
// Since macOS 15.4 the private MediaRemote framework answers only Apple
// processes. /usr/bin/osascript is one, so a JavaScript for Automation script
// (media_darwin.js, embedded) reads the now-playing app and sends the
// commands. Before macOS 15.4 MediaRemote has no MRNowPlayingRequest, but it
// answers any process, so Sayso then calls it directly
// (media_fallback_darwin.go). macOS has one now-playing app, so this pauses
// one player at most.

import (
	"bytes"
	"context"
	_ "embed"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"os/exec"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	mediaPlay  = 0
	mediaPause = 1
	// mediaRunLimit stops an osascript that hangs. A "get" takes about 0.05 s
	// and a "send" about 0.35 s.
	mediaRunLimit = 4 * time.Second
	// The app can report "playing" for a moment after the pause, so the
	// resume reads the state again for a while before it gives up.
	mediaResumeTries = 5
	mediaResumeWait  = 200 * time.Millisecond
)

//go:embed media_darwin.js
var mediaScript string

// errMediaUnsupported means that this macOS has no MRNowPlayingRequest.
var errMediaUnsupported = errors.New("media: MRNowPlayingRequest is missing")

// mediaBackend reads the now-playing app and sends it commands. nowPlaying
// gives nil when no app plays media.
type mediaBackend interface {
	nowPlaying() (*nowPlaying, error)
	send(command int) error
}

type macMedia struct {
	mu      sync.Mutex
	backend mediaBackend
	// fallback is used once the backend reports errMediaUnsupported.
	fallback func() mediaBackend
	paused   string // the player that Sayso paused, empty when none
}

func newMacMedia() *macMedia {
	return &macMedia{
		backend:  &jxaMedia{run: runOsascript},
		fallback: newDirectMedia,
	}
}

func (m *macMedia) current() (mediaBackend, *nowPlaying, error) {
	m.mu.Lock()
	b := m.backend
	m.mu.Unlock()
	now, err := b.nowPlaying()
	if !errors.Is(err, errMediaUnsupported) {
		return b, now, err
	}
	slog.Info("media: MRNowPlayingRequest is missing, calling MediaRemote directly")
	b = m.fallback()
	m.mu.Lock()
	m.backend = b
	m.mu.Unlock()
	now, err = b.nowPlaying()
	return b, now, err
}

func (m *macMedia) Pause() (bool, error) {
	b, now, err := m.current()
	if err != nil || now == nil || !now.playing {
		return false, err
	}
	if err := b.send(mediaPause); err != nil {
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
	if player == "" {
		return nil
	}
	for attempt := 0; attempt < mediaResumeTries; attempt++ {
		if attempt > 0 {
			time.Sleep(mediaResumeWait)
		}
		b, now, err := m.current()
		if err != nil {
			return err
		}
		// Another app plays now, or none: play nothing.
		if now == nil || now.player != player {
			return nil
		}
		if !now.playing {
			return b.send(mediaPlay)
		}
	}
	return nil
}

type nowPlaying struct {
	player  string
	playing bool
}

// jxaMedia runs media_darwin.js. run gets the arguments of the script and
// gives its output.
type jxaMedia struct {
	run func(args ...string) (string, error)
}

func (j *jxaMedia) nowPlaying() (*nowPlaying, error) {
	out, err := j.run("get")
	if err != nil {
		return nil, err
	}
	var probe struct {
		Unsupported bool `json:"unsupported"`
	}
	if json.Unmarshal([]byte(strings.TrimSpace(out)), &probe) == nil && probe.Unsupported {
		return nil, errMediaUnsupported
	}
	return parseNowPlaying(out), nil
}

func (j *jxaMedia) send(command int) error {
	_, err := j.run("send", strconv.Itoa(command))
	return err
}

func runOsascript(args ...string) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), mediaRunLimit)
	defer cancel()
	// osascript reads the script from stdin when the first argument is "-".
	cmd := exec.CommandContext(ctx, "/usr/bin/osascript", append([]string{"-l", "JavaScript", "-"}, args...)...)
	cmd.Stdin = strings.NewReader(mediaScript)
	var stdout, stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("media: osascript %q failed: %v: %s", args[0], err, strings.TrimSpace(stderr.String()))
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
