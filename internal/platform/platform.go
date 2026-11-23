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

// HotkeyConflict is another program that reacts to a hotkey: an app, or a
// shortcut of the system.
type HotkeyConflict struct {
	// Hotkey is the hotkey, as config.toml writes it.
	Hotkey string
	// App: Name is an app; else Name is the English name of a system shortcut.
	App  bool
	Name string
	// Confirmed (App): Sayso read the app's setting; else it is the default.
	Confirmed bool
	// Enabled (system): the shortcut is on.
	Enabled bool
}

// ConflictFinder finds the other programs that use a hotkey. A
// HotkeyManager implements it where the system lets Sayso look (macOS).
type ConflictFinder interface {
	Conflicts(hotkey string) []HotkeyConflict
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
	Microphone bool
	// MicrophoneDenied: the user said no (macOS), or microphone access is
	// off (Windows). False while the system has not asked yet.
	MicrophoneDenied bool
	Accessibility    bool
	InputMonitoring  bool
	NeedsInputPerms  bool
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

// Rect is a rectangle of the screen. X and Y are its top-left corner.
type Rect struct {
	X, Y, Width, Height int
}

// Empty is true for a rectangle with no area.
func (r Rect) Empty() bool { return r.Width <= 0 || r.Height <= 0 }

// FocusFrame is where the keyboard focus is: the frame of the focused input,
// else of the focused window, of the app in front.
type FocusFrame struct {
	// Rect is in the screen space of the UI toolkit's displays: points (or
	// DIPs) with y growing down from the top of the primary display, so a
	// display above the primary one has negative y. Empty when only
	// Display is known.
	Rect Rect
	// Pixels is true when Rect is in device pixels: divide it by a
	// display's scale factor to compare it with the display (X11 with a
	// GDK scale).
	Pixels bool
	// Display is the toolkit's ID of the display that holds the focus, or
	// 0 when only Rect is known (Windows: the HMONITOR).
	Display int64
}

// FocusTracker follows the keyboard focus across displays.
type FocusTracker interface {
	// Focused is where the focus is. ok is false when nothing has it that
	// a dictation could type into: the desktop, or no window. Sayso's own
	// overlay and tray popover never count; its hub does. It may wait for
	// the app in front (Accessibility IPC), so call it off the main thread.
	Focused() (f FocusFrame, ok bool)
	// Watch calls changed, on any goroutine and without blocking it, when
	// the focus may have moved to another app, window or input. stop
	// stops it and releases what it holds; it may be called once.
	Watch(changed func()) (stop func(), err error)
}

// Services bundles all platform services for the current OS.
type Services struct {
	Audio AudioCapture
	// Meter is a second capture for the microphone test of onboarding, so
	// the test never holds the capture a dictation needs; nil has no test.
	Meter       AudioCapture
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
	// Focus follows the keyboard focus, for the overlay to show on the
	// display the user types on; nil where it is not known.
	Focus FocusTracker
}

// AppIconSource renders the icon of an app.
type AppIconSource interface {
	// AppIconPNG is the icon of the app with bundleID as a PNG at least
	// size px wide, or nil when the app is not found.
	AppIconPNG(bundleID string, size int) []byte
	// AppName is the name of the installed app with bundleID as Finder
	// shows it, or "" when the app is not installed.
	AppName(bundleID string) string
}
