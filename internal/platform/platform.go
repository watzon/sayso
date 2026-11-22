package platform

import (
	"context"
	"strings"
	"time"
)

// AudioDevice describes an available microphone input device.
type AudioDevice struct {
	ID        string
	Name      string
	IsDefault bool
}

// cpalHosts are the host names cpal writes before a device id
// ("<host>:<device>"). The Rust app saved input_device in that form.
var cpalHosts = []string{"coreaudio", "wasapi", "asio", "alsa", "jack", "pulse", "pulseaudio", "pipewire"}

// NormalizeDeviceID strips the cpal host prefix from a device id saved by
// the Rust app ("coreaudio:<UID>" becomes "<UID>"). Only the first part is
// a host: an ALSA id such as "alsa:hw:0,0" becomes "hw:0,0".
func NormalizeDeviceID(id string) string {
	host, rest, ok := strings.Cut(id, ":")
	if !ok {
		return id
	}
	for _, h := range cpalHosts {
		if strings.EqualFold(host, h) {
			return rest
		}
	}
	return id
}

// AudioChunk carries 16 kHz mono float32 PCM samples and an RMS level in [0, 1].
type AudioChunk struct {
	Samples []float32
	RMS     float32
	Time    time.Time
}

// AudioCapture captures 16 kHz mono microphone audio.
type AudioCapture interface {
	ListDevices() ([]AudioDevice, error)
	Start(ctx context.Context, deviceID string, onChunk func(AudioChunk)) error
	Stop() error
}

// HotkeyEventKind identifies a global hotkey action.
type HotkeyEventKind int

const (
	HotkeyToggle HotkeyEventKind = iota
	HotkeyPushToTalkDown
	HotkeyPushToTalkUp
	HotkeyCancel
	HotkeyEnterSend
	HotkeyPasteLast
	HotkeyCycleStyle
	HotkeyToggleIncognito
	HotkeyStyle
)

// HotkeyEvent is emitted when a configured hotkey fires.
type HotkeyEvent struct {
	Kind    HotkeyEventKind
	StyleID string
}

// HotkeyConfig holds the serialized hotkey strings and recording-time interception flags.
type HotkeyConfig struct {
	Toggle         string
	PushToTalk     string
	PasteLast      string
	CycleStyle     string
	Incognito      string
	StyleHotkeys   map[string]string
	CancelMode     string // "double_escape", "single_escape", "off"
	DoubleEscapeMs int
	EnterSends     bool
	TakeEscape     bool
}

// HotkeyManager registers and listens for global hotkeys.
type HotkeyManager interface {
	Configure(cfg HotkeyConfig) error
	SetRecording(recording bool)
	Start(onEvent func(HotkeyEvent)) error
	Stop() error
}

// InsertOptions configures how text is inserted into the focused app.
type InsertOptions struct {
	RestoreClipboard bool
	RestoreDelay     time.Duration
	TrailingSpace    bool
	PressEnterAfter  bool
	FallbackToType   bool
}

// InsertResult reports how insertion completed.
type InsertResult struct {
	Method            string // "pasted", "typed", "failed", "not_inserted"
	ClipboardRestored bool
	Backend           string
	Elapsed           time.Duration
	Err               error
}

// Inserter inserts text into the currently focused application.
type Inserter interface {
	Insert(text string, opts InsertOptions) InsertResult
	ReadClipboard() (string, error)
	WriteClipboard(text string) error
}

// ActiveApp describes the focused application and optional browser page host.
type ActiveApp struct {
	BundleID string
	Name     string
	Site     string
}

// ContextProvider queries the currently focused application and browser host.
type ContextProvider interface {
	ActiveApp() (ActiveApp, error)
}

// PermissionStatus reports OS permission states.
type PermissionStatus struct {
	Microphone      bool
	Accessibility   bool
	InputMonitoring bool
	NeedsInputPerms bool
}

// Permissions checks and requests OS permissions.
type Permissions interface {
	Status() PermissionStatus
	RequestMicrophone() error
	RequestAccessibility() error
	RequestInputMonitoring() error
	OpenSettings(kind string) error
}

// SoundCue identifies an audio feedback cue.
type SoundCue string

const (
	CueStart  SoundCue = "start"
	CueStop   SoundCue = "stop"
	CueCancel SoundCue = "cancel"
	CueInsert SoundCue = "insert"
)

// SoundPlayer plays short WAV audio cues.
type SoundPlayer interface {
	Play(cue SoundCue, wavData []byte, volume float32) error
}

// MediaController pauses and resumes system media playback during dictation.
type MediaController interface {
	Pause() (wasPlaying bool, err error)
	Resume() error
}

// LoginItem manages launch-at-login registration.
type LoginItem interface {
	Enabled() (bool, error)
	SetEnabled(enabled bool) error
}

// DockPolicy controls whether the app appears in the Dock / taskbar.
type DockPolicy interface {
	SetShowInDock(show bool) error
}

// Services bundles all platform services for the current OS.
type Services struct {
	Audio       AudioCapture
	Hotkeys     HotkeyManager
	Inserter    Inserter
	Context     ContextProvider
	Permissions Permissions
	Sounds      SoundPlayer
	Media       MediaController
	LoginItem   LoginItem
	Dock        DockPolicy
	// AppIcons renders app icons for badges; nil where the platform has
	// none, and the badges show the app's first letter.
	AppIcons AppIconSource
}

// AppIconSource renders the icon of an app.
type AppIconSource interface {
	// AppIconPNG is the icon of the app with bundleID as a PNG at least
	// size px wide, or nil when the app is not found.
	AppIconPNG(bundleID string, size int) []byte
}
