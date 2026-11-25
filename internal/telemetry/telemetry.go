// Package telemetry sends anonymous usage signals to TelemetryDeck (Ingest
// API v2): that Sayso was installed and launched, and that a dictation
// finished, with the model and the kind of style it used. A signal never
// holds text, audio, prompts, dictionary words, app names, or anything else
// the user said or typed; the payload types below have no field for them.
//
// The user turns it off in Onboarding or Settings › General. Off stops every
// signal at once, including queued ones and a send in flight.
package telemetry

import (
	"bytes"
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"log/slog"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"time"
)

// DefaultAppID is the TelemetryDeck App ID of the official app. Empty sends
// nothing. Set it here, with -ldflags "-X
// github.com/watzon/sayso/internal/telemetry.DefaultAppID=<id>", or with
// SAYSO_TELEMETRY_APP_ID.
var DefaultAppID = ""

// DefaultEndpoint is TelemetryDeck's Ingest API v2.
const DefaultEndpoint = "https://nom.telemetrydeck.com/v2/"

// The signal types.
const (
	TypeInstalled = "app.installed"
	TypeLaunched  = "app.launched"
	TypeDictation = "dictation.completed"
)

// The values of Dictation.ModelKind and Dictation.StyleKind.
const (
	ModelLocal = "local"
	ModelCloud = "cloud"
	StyleRaw   = "raw"
	StyleLocal = "ai_local"
	StyleCloud = "ai_cloud"
)

// The build channels: an official release, or any other build.
const (
	ChannelRelease = "release"
	ChannelDev     = "dev"
)

const platformTag = "desktop"

// maxQueued bounds the signals kept while TelemetryDeck cannot be reached.
const maxQueued = 50

// Options set up a Client.
type Options struct {
	AppID    string
	Endpoint string
	TestMode bool
	// StateFile keeps the install ID and whether app.installed was sent.
	StateFile  string
	AppVersion string
	Channel    string
	HTTP       *http.Client
	// RetryAfter is the wait after a failed send; 0 is one minute.
	RetryAfter time.Duration
}

// OptionsForThisBuild are the options of this build: the App ID, the
// endpoint, and test mode from the build, overridden by
// SAYSO_TELEMETRY_APP_ID, SAYSO_TELEMETRY_ENDPOINT and
// SAYSO_TELEMETRY_TEST_MODE. A build that is not official is in test mode.
func OptionsForThisBuild(dataDir, version string, official bool) Options {
	o := Options{AppID: DefaultAppID, Endpoint: DefaultEndpoint, TestMode: !official,
		StateFile: filepath.Join(dataDir, "telemetry.json"), AppVersion: version, Channel: ChannelDev}
	if official {
		o.Channel = ChannelRelease
	}
	if v := strings.TrimSpace(os.Getenv("SAYSO_TELEMETRY_APP_ID")); v != "" {
		o.AppID = v
	}
	if v := strings.TrimSpace(os.Getenv("SAYSO_TELEMETRY_ENDPOINT")); v != "" {
		o.Endpoint = v
	}
	switch strings.ToLower(strings.TrimSpace(os.Getenv("SAYSO_TELEMETRY_TEST_MODE"))) {
	case "1", "true":
		o.TestMode = true
	case "0", "false":
		o.TestMode = false
	}
	return o
}

// Dictation is what dictation.completed tells: nothing about the words.
type Dictation struct {
	// ModelID is a catalog model id, or "<provider kind>:<model>" for cloud
	// transcription ("open_ai_compatible" alone for a custom endpoint).
	ModelID   string
	ModelKind string // ModelLocal or ModelCloud
	StyleKind string // StyleRaw, StyleLocal, or StyleCloud
}

// Signal is one TelemetryDeck v2 event.
type Signal struct {
	AppID      string            `json:"appID"`
	ClientUser string            `json:"clientUser"`
	SessionID  string            `json:"sessionID"`
	Type       string            `json:"type"`
	IsTestMode bool              `json:"isTestMode"`
	Payload    map[string]string `json:"payload"`
}

// state is <data>/telemetry.json.
type state struct {
	InstallID       string `json:"install_id"`
	FirstLaunchSent bool   `json:"first_launch_sent"`
}

// Client queues signals and sends them on its own goroutine.
type Client struct {
	o         Options
	sessionID string

	mu       sync.Mutex
	enabled  bool
	launched bool
	st       state
	queue    []Signal
	// gen changes when telemetry is turned off, so a send that started
	// before does not mark app.installed as sent or put its signals back.
	gen    int
	cancel context.CancelFunc

	wake chan struct{}
	stop chan struct{}
	done chan struct{}
}

// New starts a client; enabled is the user's setting.
func New(o Options, enabled bool) *Client {
	if o.HTTP == nil {
		o.HTTP = &http.Client{Timeout: 15 * time.Second}
	}
	if o.RetryAfter == 0 {
		o.RetryAfter = time.Minute
	}
	c := &Client{o: o, sessionID: newUUID(), enabled: enabled,
		wake: make(chan struct{}, 1), stop: make(chan struct{}), done: make(chan struct{})}
	c.st = loadState(o.StateFile)
	go c.loop()
	return c
}

// Active is true when signals can leave: an App ID is set and the user has
// telemetry on.
func (c *Client) Active() bool {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.activeLocked()
}

func (c *Client) activeLocked() bool { return c.enabled && strings.TrimSpace(c.o.AppID) != "" }

// SetEnabled follows the user's setting. Off drops the queue and cancels a
// send in flight.
func (c *Client) SetEnabled(on bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.enabled == on {
		return
	}
	c.enabled = on
	if !on {
		c.queue = nil
		c.gen++
		if c.cancel != nil {
			c.cancel()
		}
	}
}

// Launched sends app.launched, after app.installed the first time on this
// install. It sends once per run; while telemetry is off it waits for the
// next call.
func (c *Client) Launched() {
	c.mu.Lock()
	if c.launched || !c.activeLocked() {
		c.mu.Unlock()
		return
	}
	c.launched = true
	if !c.st.FirstLaunchSent {
		c.enqueueLocked(TypeInstalled, nil)
	}
	c.enqueueLocked(TypeLaunched, nil)
	c.mu.Unlock()
	c.poke()
}

// DictationCompleted sends dictation.completed.
func (c *Client) DictationCompleted(d Dictation) {
	c.mu.Lock()
	if !c.activeLocked() {
		c.mu.Unlock()
		return
	}
	c.enqueueLocked(TypeDictation, map[string]string{"modelId": d.ModelID, "modelKind": d.ModelKind, "styleKind": d.StyleKind})
	c.mu.Unlock()
	c.poke()
}

// Close stops the client, giving a queued send up to wait to finish.
func (c *Client) Close(wait time.Duration) {
	select {
	case <-c.stop:
		return
	default:
	}
	close(c.stop)
	select {
	case <-c.done:
	case <-time.After(wait):
		c.mu.Lock()
		if c.cancel != nil {
			c.cancel()
		}
		c.mu.Unlock()
	}
}

func (c *Client) enqueueLocked(typ string, extra map[string]string) {
	if c.st.InstallID == "" {
		c.st.InstallID = newUUID()
		c.saveStateLocked()
	}
	payload := map[string]string{"os": runtime.GOOS, "arch": runtime.GOARCH, "appVersion": c.o.AppVersion,
		"buildChannel": c.o.Channel, "platform": platformTag}
	for k, v := range extra {
		payload[k] = v
	}
	c.queue = append(c.queue, Signal{AppID: c.o.AppID, ClientUser: ClientUser(c.st.InstallID), SessionID: c.sessionID,
		Type: typ, IsTestMode: c.o.TestMode, Payload: payload})
	if n := len(c.queue); n > maxQueued {
		c.queue = c.queue[n-maxQueued:]
	}
}

func (c *Client) poke() {
	select {
	case c.wake <- struct{}{}:
	default:
	}
}

func (c *Client) loop() {
	defer close(c.done)
	var retry <-chan time.Time
	for {
		select {
		case <-c.stop:
			c.flush()
			return
		case <-c.wake:
		case <-retry:
		}
		retry = nil
		if !c.flush() {
			retry = time.After(c.o.RetryAfter)
		}
	}
}

// flush sends the queue in one request. False: it failed and the signals
// wait for a retry.
func (c *Client) flush() bool {
	c.mu.Lock()
	if len(c.queue) == 0 || !c.activeLocked() {
		c.queue = nil
		c.mu.Unlock()
		return true
	}
	batch, gen := c.queue, c.gen
	c.queue = nil
	ctx, cancel := context.WithCancel(context.Background())
	c.cancel = cancel
	c.mu.Unlock()

	err := c.post(ctx, batch)
	cancel()

	c.mu.Lock()
	defer c.mu.Unlock()
	c.cancel = nil
	if gen != c.gen || !c.activeLocked() {
		return true // turned off meanwhile: nothing goes back
	}
	if err != nil {
		slog.Debug("telemetry: send failed", "err", err)
		c.queue = append(batch, c.queue...)
		if n := len(c.queue); n > maxQueued {
			c.queue = c.queue[n-maxQueued:]
		}
		return false
	}
	for _, s := range batch {
		if s.Type == TypeInstalled && !c.st.FirstLaunchSent {
			c.st.FirstLaunchSent = true
			c.saveStateLocked()
		}
	}
	return true
}

func (c *Client) post(ctx context.Context, batch []Signal) error {
	body, err := json.Marshal(batch)
	if err != nil {
		return err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, c.o.Endpoint, bytes.NewReader(body))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json; charset=utf-8")
	resp, err := c.o.HTTP.Do(req)
	if err != nil {
		return err
	}
	resp.Body.Close()
	if resp.StatusCode/100 != 2 {
		return fmt.Errorf("status %d", resp.StatusCode)
	}
	return nil
}

func (c *Client) saveStateLocked() {
	if c.o.StateFile == "" {
		return
	}
	data, err := json.MarshalIndent(c.st, "", "  ")
	if err == nil {
		err = os.MkdirAll(filepath.Dir(c.o.StateFile), 0o755)
	}
	if err == nil {
		tmp := c.o.StateFile + ".tmp"
		if err = os.WriteFile(tmp, data, 0o600); err == nil {
			err = os.Rename(tmp, c.o.StateFile)
		}
	}
	if err != nil {
		slog.Warn("telemetry: could not save state", "err", err)
	}
}

func loadState(path string) state {
	var s state
	if path == "" {
		return s
	}
	if data, err := os.ReadFile(path); err == nil {
		_ = json.Unmarshal(data, &s)
	}
	return s
}

// ClientUser is the clientUser of an install: the lowercase hex SHA-256 of
// its random ID. TelemetryDeck hashes it again, with a salt.
func ClientUser(installID string) string {
	sum := sha256.Sum256([]byte(installID))
	return hex.EncodeToString(sum[:])
}

// newUUID is a random (version 4) UUID.
func newUUID() string {
	var b [16]byte
	if _, err := rand.Read(b[:]); err != nil {
		panic(err)
	}
	b[6] = b[6]&0x0f | 0x40
	b[8] = b[8]&0x3f | 0x80
	return fmt.Sprintf("%x-%x-%x-%x-%x", b[0:4], b[4:6], b[6:8], b[8:10], b[10:16])
}
