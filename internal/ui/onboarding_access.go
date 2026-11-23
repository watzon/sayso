package ui

import (
	"context"
	"fmt"
	"log"
	"runtime"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/platform"
	"github.com/watzon/sayso/internal/theme"
)

// Step 4: the microphone and Accessibility on one screen, with live
// detection and the microphone test inline. Step 5: the toggle key and an
// optional push-to-talk key (onboarding/permissions.rs, hotkeys.rs).

// permPoll is how often the permissions are read while a step shows them:
// the page updates by itself when the user allows one in System Settings.
const permPoll = time.Second

type accessState struct {
	// clock is the time of the frame being built (Tester time in tests).
	clock    time.Time
	perm     platform.PermissionStatus
	permRead time.Time
	// permStale: a request ended (on its own goroutine), so the next frame
	// reads the permissions again without waiting for the poll.
	permStale atomic.Bool
	showWhy   bool
	meter     micMeter
}

// hasInputPermissions is true where Sayso needs a permission to paste and
// to hear keys in other apps: macOS, and the Linux desktops. Windows needs
// none.
func hasInputPermissions() bool { return runtime.GOOS != "windows" }

// permissions are the permissions as last read; a read older than permPoll
// reads them again.
func (a *App) permissions() platform.PermissionStatus {
	s := &a.onb.access
	pm := a.platformPermissions()
	if pm == nil {
		return s.perm
	}
	now := s.clock
	if now.IsZero() {
		now = time.Now()
	}
	if s.permStale.Swap(false) || s.permRead.IsZero() || now.Sub(s.permRead) >= permPoll || now.Before(s.permRead) {
		s.perm, s.permRead = pm.Status(), now
		// The microphones too: one plugged in while the step shows
		// appears in the microphone test (the next read lists them).
		a.set.devicesRead = false
	}
	return s.perm
}

// pollPermissions reads the permissions again at the next poll, and asks
// for a frame then, so the page follows System Settings.
func (a *App) pollPermissions(c *mu.Context) {
	if a.platformPermissions() != nil {
		c.After(permPoll)
	}
}

// requestPermission asks for a permission off the UI thread (a system
// prompt blocks), then marks the grants stale so the next frame reads them.
// It runs through App.background: on its own goroutine in the app, and at
// once without a UI loop (Deps.Post nil, as in tests), so a request always
// ends, and its read happens, before anything after the click.
func (a *App) requestPermission(kind string) {
	pm := a.platformPermissions()
	if pm == nil {
		return
	}
	a.background(func() {
		var err error
		switch kind {
		case "microphone":
			err = pm.RequestMicrophone()
		case "accessibility":
			err = pm.RequestAccessibility()
		case "input_monitoring":
			err = pm.RequestInputMonitoring()
		}
		if err != nil {
			log.Printf("permission %s: %v", kind, err)
		}
		a.onb.access.permStale.Store(true)
		a.post(a.invalidate)
	})
}

func (a *App) openPermissionSettings(kind string) {
	pm := a.platformPermissions()
	if pm == nil {
		return
	}
	go func() {
		if err := pm.OpenSettings(kind); err != nil {
			log.Printf("permission settings %s: %v", kind, err)
		}
	}()
}

// ---- The microphone test ----

// micMeter is the level meter of the permissions step. It records with a
// capture of its own (platform.Services.Meter), never the dictation's.
type micMeter struct {
	mu      sync.Mutex
	on      bool
	gen     int
	err     string
	levels  [48]float32
	next    int
	running platform.AudioCapture
	// ops are the starts and stops of the capture, run in the order asked
	// by one goroutine at a time: a Stop asked while the device still opens
	// runs after the open, never before it (where it would find nothing to
	// stop and leave the microphone on).
	ops     []func()
	working bool
}

// queue runs op after every op asked before it, off the main goroutine.
func (m *micMeter) queue(op func()) {
	m.mu.Lock()
	m.ops = append(m.ops, op)
	if m.working {
		m.mu.Unlock()
		return
	}
	m.working = true
	m.mu.Unlock()
	go func() {
		for {
			m.mu.Lock()
			if len(m.ops) == 0 {
				m.working = false
				m.mu.Unlock()
				return
			}
			op := m.ops[0]
			m.ops = m.ops[1:]
			m.mu.Unlock()
			op()
		}
	}()
}

// current reports whether gen is still the test that runs.
func (m *micMeter) current(gen int) bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.gen == gen && m.on
}

// syncMeter runs the test while the permissions step shows and the
// microphone is allowed.
func (a *App) syncMeter(micOK bool) {
	m := &a.onb.access.meter
	m.mu.Lock()
	on, failed := m.on, m.err != ""
	m.mu.Unlock()
	if on || failed || !micOK || a.d.Platform == nil || a.d.Platform.Meter == nil {
		return
	}
	device := ""
	if d := a.cfg.Audio.InputDevice; d != nil {
		device = platform.NormalizeDeviceID(*d)
	}
	capture := a.d.Platform.Meter
	m.mu.Lock()
	m.on, m.running, m.levels, m.next = true, capture, [48]float32{}, 0
	m.gen++
	gen := m.gen
	m.mu.Unlock()
	m.queue(func() {
		// A stop asked after this start already ran its course.
		if !m.current(gen) {
			return
		}
		err := capture.Start(context.Background(), device, func(ch platform.AudioChunk) {
			m.mu.Lock()
			if m.gen == gen {
				m.levels[m.next%len(m.levels)] = ch.RMS
				m.next++
			}
			m.mu.Unlock()
		})
		if err != nil {
			m.mu.Lock()
			if m.gen == gen {
				m.on, m.running, m.err = false, nil, err.Error()
			}
			m.mu.Unlock()
			a.post(a.invalidate)
		}
	})
}

// stopMeter ends the test and clears its error, so the next visit of the
// step tries again.
func (a *App) stopMeter() {
	m := &a.onb.access.meter
	m.mu.Lock()
	capture := m.running
	m.on, m.running, m.err = false, nil, ""
	m.gen++
	m.mu.Unlock()
	if capture != nil {
		m.queue(func() {
			if err := capture.Stop(); err != nil {
				log.Printf("mic test: %v", err)
			}
		})
	}
}

// meterLevels are the test's last levels, oldest first, and whether it runs.
func (m *micMeter) snapshot() ([]float32, bool, string) {
	m.mu.Lock()
	defer m.mu.Unlock()
	n := len(m.levels)
	out := make([]float32, n)
	for i := range n {
		out[i] = m.levels[(m.next+i)%n]
	}
	return out, m.on, m.err
}

// inputDeviceName is the name of the microphone in use.
func (a *App) inputDeviceName() string {
	devs := a.audioDevices()
	want := ""
	if d := a.cfg.Audio.InputDevice; d != nil {
		want = platform.NormalizeDeviceID(*d)
	}
	for _, d := range devs {
		if (want != "" && d.ID == want) || (want == "" && d.IsDefault) {
			return d.Name
		}
	}
	return i18n.T("onboarding.permissions.default_microphone")
}

// ---- Step 4: Permissions ----

type permState int

const (
	permWaiting permState = iota
	permAllowed
	permBlocked
)

func (a *App) onbPermissions(c *mu.Context, p theme.Colors) {
	st := a.permissions()
	a.pollPermissions(c)
	a.syncMeter(st.Microphone)
	mic := permWaiting
	switch {
	case st.Microphone:
		mic = permAllowed
	case st.MicrophoneDenied:
		mic = permBlocked
	}
	focusMic := mic != permAllowed
	focusAX := !focusMic && !st.Accessibility
	title, lead := i18n.T("onboarding.permissions.title"), i18n.T("onboarding.permissions.lead")
	if !hasInputPermissions() {
		title, lead = i18n.T("onboarding.permissions.title_mic_only"), i18n.T("onboarding.permissions.lead_mic_only")
	}
	stepColumn(c, p, title, lead, func() {
		mu.Row(c).Gap(14).AlignItems(mu.Start).Children(func() {
			card := onbCard(c, p).Label(i18n.T("os.permission.microphone"))
			if focusMic {
				ring(card, p)
			}
			card.Children(func() {
				permHead(c, p, i18n.T("os.permission.microphone"), mic)
				text := i18n.TOS("onboarding.permissions.mic_undetermined", nil)
				switch mic {
				case permAllowed:
					text = i18n.T("onboarding.permissions.mic_granted")
				case permBlocked:
					text = i18n.T("onboarding.permissions.mic_denied")
					if runtime.GOOS == "windows" {
						text = i18n.T("onboarding.permissions.mic_denied_windows")
					}
				}
				uiText(c, text, 13, 400, p.Graphite).FixedLineHeight(19)
				switch mic {
				case permAllowed:
					a.micWell(c, p)
				case permWaiting:
					mu.Row(c).Padding(6, 0, 0, 0).Children(func() {
						if a.btn(c, p, "onboarding.permissions.allow_mic", i18n.T("onboarding.permissions.allow_microphone"), btnPrimary) {
							a.requestPermission("microphone")
						}
					})
				case permBlocked:
					mu.Row(c).Padding(6, 0, 0, 0).Children(func() {
						if a.btn(c, p, "onboarding.permissions.open_mic", i18n.TOS("os.open_permission_settings", nil), btnPrimary) {
							a.openPermissionSettings("microphone")
						}
					})
				}
			})
			if !hasInputPermissions() {
				return
			}
			axState := permWaiting
			if st.Accessibility {
				axState = permAllowed
			}
			ax := onbCard(c, p).Label(i18n.TOS("os.permission.accessibility", nil))
			if focusAX {
				ring(ax, p)
			}
			ax.Children(func() {
				// macOS has no "denied" for Accessibility: until it is on, Sayso waits.
				permHead(c, p, i18n.TOS("os.permission.accessibility", nil), axState)
				text := i18n.TOS("onboarding.permissions.ax_needed", nil)
				if st.Accessibility {
					text = i18n.T("onboarding.permissions.ax_granted")
				}
				uiText(c, text, 13, 400, p.Graphite).FixedLineHeight(19)
				if st.Accessibility {
					return
				}
				mu.Row(c).Gap(10).Padding(6, 0, 0, 0).AlignItems(mu.Center).Children(func() {
					if a.btn(c, p, "onboarding.permissions.open_ax", i18n.TOS("onboarding.permissions.grant_ax", nil), btnPrimary) {
						// The request shows the macOS prompt, which adds Sayso to
						// the list and opens System Settings.
						a.requestPermission("accessibility")
					}
					b := a.button(c, p, "onboarding.permissions.why_ax", i18n.T("onboarding.permissions.why_ax"), bopt{kind: btnGhost})
					if b {
						a.onb.access.showWhy = !a.onb.access.showWhy
					}
				})
				if a.onb.access.showWhy {
					uiText(c, i18n.TOS("onboarding.permissions.why_ax_text", nil), 12, 400, p.Graphite).FixedLineHeight(17)
				}
			})
		})
		a.permissionsDownload(c, p)
		if hasInputPermissions() {
			banner(c, p, bannerInfo, i18n.TOS("onboarding.permissions.im_note", nil))
		}
	})
}

// permHead is a card's title and its state: Allowed, Waiting or Blocked.
func permHead(c *mu.Context, p theme.Colors, title string, s permState) {
	mu.Row(c).AlignItems(mu.Center).Children(func() {
		cardTitle(c, p, title, 20, 24).Grow(1).Basis(0).Role(mu.RoleHeading)
		word, col := i18n.T("onboarding.permissions.waiting"), p.Accent
		switch s {
		case permAllowed:
			checkLine(c, p, i18n.T("onboarding.permissions.allowed"), 600).Shrink(0).Role(mu.RoleStatus).Label(title + ": " + i18n.T("onboarding.permissions.allowed"))
			return
		case permBlocked:
			word, col = i18n.T("onboarding.permissions.blocked"), p.Danger
		}
		mu.Row(c).Gap(6).Shrink(0).AlignItems(mu.Center).Role(mu.RoleStatus).Label(title + ": " + word).Children(func() {
			statusDot(c, col)
			uiText(c, word, 13, 600, col)
		})
	})
}

// micWell is the test: the ink moves with the voice, beside the device.
func (a *App) micWell(c *mu.Context, p theme.Colors) {
	m := &a.onb.access.meter
	reduce := reduceMotion(c)
	started := a.onb.started
	debossed(mu.Row(a.k(c, "onboarding.permissions.meter")).Gap(12).Padding(12, 14).Radius(12).AlignItems(mu.Center), p).Children(func() {
		if _, _, err := m.snapshot(); err != "" {
			uiText(c, i18n.T("onboarding.permissions.mic_failed", "error", err), 12, 400, p.Danger).Role(mu.RoleAlert).Grow(1).Basis(0)
			return
		}
		mu.Box(c).Size(200, 34).Shrink(0).AccessHidden(true).Draw(mirrored(rtl(c), func(pt *mu.Painter, r mu.Rect) {
			levels, on, _ := m.snapshot()
			t := float32(0)
			if !reduce {
				t = float32(time.Since(started).Seconds())
				if on {
					pt.AnimationFrame()
				}
			}
			paintInkLayers(pt, r, smoothedLevels(levels, 28), t, p.Ink, inkLayers)
		}))
		uiText(c, a.inputDeviceName(), 12, 400, p.Graphite).Grow(1).Basis(0).MinWidth(0).TextAlign(mu.End).SingleLine().Ellipsis("…")
	})
}

// permissionsDownload is the model's download, which runs on in the
// background: its name, a thin track and the state.
func (a *App) permissionsDownload(c *mu.Context, p theme.Colors) {
	id := a.FinalModel()
	m, _ := models.Find(id)
	st := a.modelStatus(id)
	frac, label := float32(0), i18n.T("onboarding.permissions.model_not_started")
	switch st.State {
	case engine.StateDownloading:
		frac, label = st.Fraction, fmt.Sprintf("%.0f%%", st.Fraction*100)
	case engine.StateReady:
		frac, label = 1, i18n.T("onboarding.permissions.model_ready")
	case engine.StateOptimizing:
		frac, label = 1, i18n.T("onboarding.permissions.model_optimizing")
	case engine.StateDownloaded:
		frac, label = 1, i18n.T("onboarding.permissions.model_downloaded")
	case engine.StateFailed:
		label = i18n.T("onboarding.permissions.model_stopped")
	}
	mu.Row(c).Gap(12).PaddingX(4).AlignItems(mu.Center).Children(func() {
		uiText(c, m.Name, 13, 400, p.Graphite).Shrink(0).SingleLine()
		f := max(0, min(frac, 1))
		mu.Box(a.k(c, "onboarding.permissions.download")).Grow(1).Basis(0).Height(4).Radius(99).Background(p.Deboss).
			InsetShadow(0, 1, 1, 0, p.ShadowA(0.25)).Role(mu.RoleProgress).Label(i18n.T("onboarding.permissions.download_label", "name", m.Name)).
			Range(0, 100, float64(int(f*100+0.5))).Draw(mirrored(rtl(c), func(pt *mu.Painter, r mu.Rect) {
			if f > 0 {
				pt.Fill(mu.Rect{X: r.X, Y: r.Y, W: max(r.H, r.W*f), H: r.H}, p.InkFill, r.H/2)
			}
		}))
		mu.Text(c, label).Font(monoFont(label)).FontSize(12).TextColor(p.Graphite).Shrink(0).SingleLine()
	})
}

// ---- Step 5: Hotkeys ----

// The recorder slots of the step.
const (
	slotToggle = "toggle"
	slotPTT    = "push_to_talk"
)

type hotkeyRecorder struct {
	// slot is the hotkey that listens for keys, or "".
	slot string
	// note says why the last key was not saved.
	note string
	// paused is true while the global hotkeys are off for the recording.
	paused bool
	// conflicts are the other programs found per hotkey (recorder.rs).
	conflicts conflictCache
}

// conflictTTL is how long a lookup of other programs counts: running apps
// change.
const conflictTTL = 4 * time.Second

// conflictCache holds the conflicts of each hotkey. A lookup runs off the
// main goroutine (it starts processes), and the step redraws when it ends.
type conflictCache struct {
	mu      sync.Mutex
	entries map[string]conflictEntry
}

type conflictEntry struct {
	at   time.Time
	busy bool
	list []platform.HotkeyConflict
}

// conflictFinder is the platform's finder of other programs, or nil where
// the system gives no way to look (Windows and Linux for now).
func (a *App) conflictFinder() platform.ConflictFinder {
	if a.d.Platform == nil || a.d.Platform.Hotkeys == nil {
		return nil
	}
	f, _ := a.d.Platform.Hotkeys.(platform.ConflictFinder)
	return f
}

// hotkeyConflicts are the other programs that react to h, as last found;
// a system shortcut that is off does not count.
func (a *App) hotkeyConflicts(h *config.Hotkey) []platform.HotkeyConflict {
	f := a.conflictFinder()
	if h == nil || f == nil {
		return nil
	}
	key := h.String()
	cc := &a.onb.keys.conflicts
	cc.mu.Lock()
	defer cc.mu.Unlock()
	if cc.entries == nil {
		cc.entries = map[string]conflictEntry{}
	}
	e, ok := cc.entries[key]
	if (!ok || time.Since(e.at) >= conflictTTL) && !e.busy {
		e.busy = true
		cc.entries[key] = e
		go func() {
			var list []platform.HotkeyConflict
			for _, x := range f.Conflicts(key) {
				if x.App || x.Enabled {
					list = append(list, x)
				}
			}
			cc.mu.Lock()
			cc.entries[key] = conflictEntry{at: time.Now(), list: list}
			cc.mu.Unlock()
			a.post(a.invalidate)
		}()
	}
	return e.list
}

// hotkeyWords is a hotkey in words, for sentences: "Option+Space".
func hotkeyWords(h config.Hotkey) string {
	caps := h.Keycaps()
	for i, c := range caps {
		switch c {
		case "⌃":
			c = "Control"
		case "⌥":
			c = "Option"
		case "⇧":
			c = "Shift"
		case "⌘":
			c = "Command"
		case "↵":
			c = "Return"
		case "⇥":
			c = "Tab"
		case "⌫":
			c = "Delete"
		case "fn":
			c = "Fn"
		default:
			c = strings.NewReplacer("⌥", "Option", "⌘", "Command", "⌃", "Control", "⇧", "Shift").Replace(c)
		}
		caps[i] = c
	}
	return strings.Join(caps, "+")
}

// shortcutName is a system shortcut's name in the interface language; a
// name Sayso has no text for (a product name) stays as it is.
func shortcutName(name string) string {
	var b strings.Builder
	under := false
	for _, r := range strings.ToLower(name) {
		if (r >= 'a' && r <= 'z') || (r >= '0' && r <= '9') {
			if under && b.Len() > 0 {
				b.WriteByte('_')
			}
			b.WriteRune(r)
			under = false
		} else {
			under = true
		}
	}
	if text, ok := i18n.Translate("settings.recorder.shortcut." + b.String()); ok {
		return text
	}
	return name
}

// conflictText is the banner of one conflict (recorder.rs conflict_text,
// with the app open).
func conflictText(x platform.HotkeyConflict) string {
	keys := x.Hotkey
	if h, err := config.ParseHotkey(x.Hotkey); err == nil {
		keys = hotkeyWords(h)
	}
	switch {
	case x.App && x.Confirmed:
		return i18n.T("settings.recorder.conflict_app", "name", x.Name, "keys", keys)
	case x.App:
		return i18n.T("settings.recorder.conflict_app_open_default", "name", x.Name, "keys", keys)
	}
	return i18n.TOS("settings.recorder.conflict_system", map[string]any{"keys": keys, "name": shortcutName(x.Name)})
}

// startRecording listens for the next chord pressed in this window. The
// global hotkeys pause meanwhile, so the old key does not start a dictation.
func (a *App) startRecording(slot string) {
	k := &a.onb.keys
	k.slot, k.note = slot, ""
	if a.d.Platform != nil && a.d.Platform.Hotkeys != nil && !k.paused {
		k.paused = true
		if err := a.d.Platform.Hotkeys.Configure(platform.HotkeyConfig{StyleHotkeys: map[string]string{}}); err != nil {
			log.Printf("hotkeys: %v", err)
		}
	}
	a.invalidate()
}

// cancelRecording stops listening and gives the hotkeys back.
func (a *App) cancelRecording() {
	k := &a.onb.keys
	k.slot = ""
	if k.paused {
		k.paused = false
		a.applyHotkeys()
	}
}

// recordKeys reads the chord pressed while a slot listens.
func (a *App) recordKeys(c *mu.Context) {
	h, done := captureChord(c)
	if !done {
		return
	}
	slot := a.onb.keys.slot
	a.cancelRecording()
	if h == nil {
		a.invalidate()
		return
	}
	a.setRecorded(slot, h)
}

// setRecorded saves a recorded key into slot, unless another Sayso action
// uses it.
func (a *App) setRecorded(slot string, h *config.Hotkey) {
	if other := hotkeyClash(&a.cfg, slot, *h); other != "" {
		a.onb.keys.note = i18n.T("settings.recorder.used_by", "keys", keysText(h.Keycaps()), "action", other)
		a.invalidate()
		return
	}
	a.onb.keys.note = ""
	switch slot {
	case slotToggle:
		a.cfg.Hotkeys.Toggle = h
	case slotPTT:
		a.cfg.Hotkeys.PushToTalk = h
	}
	a.hotkeysChanged()
	a.invalidate()
}

// hotkeyClash is the name of another Sayso action that uses or overlaps h,
// or "" (recorder.rs Slot::clash). The toggle key overlaps; the others
// must be equal.
func hotkeyClash(cfg *config.Config, slot string, h config.Hotkey) string {
	hk := cfg.Hotkeys
	slots := []struct {
		slot, label string
		key         *config.Hotkey
	}{
		{slotToggle, i18n.T("settings.recorder.action_toggle"), hk.Toggle},
		{slotPTT, i18n.T("settings.recorder.action_push_to_talk"), hk.PushToTalk},
		{"paste_last", i18n.T("settings.recorder.action_paste_last"), hk.PasteLast},
		{"cycle_style", i18n.T("settings.recorder.action_cycle_style"), hk.CycleStyle},
		{"incognito", i18n.T("settings.recorder.action_incognito"), hk.Incognito},
	}
	for _, s := range slots {
		if s.slot == slot || s.key == nil {
			continue
		}
		if slot == slotToggle || s.slot == slotToggle {
			if s.key.Overlaps(h) {
				return s.label
			}
		} else if *s.key == h {
			return s.label
		}
	}
	for _, sh := range cfg.StyleHotkeys.All() {
		if sh.Hotkey == h || (slot == slotToggle && sh.Hotkey.Overlaps(h)) {
			return i18n.T("settings.recorder.action_style", "style", sh.Style)
		}
	}
	return ""
}

// heldCaps are the caps of h that are held down in this window now.
func heldCaps(c *mu.Context, h config.Hotkey) []bool {
	caps := h.Keycaps()
	held := make([]bool, len(caps))
	if h.Kind != config.HotkeyChord {
		return held
	}
	mods := c.Modifiers()
	m := h.Modifiers
	var on []bool
	if m.Function {
		on = append(on, false) // fn is not a key MyGo reports
	}
	type mod struct {
		set  bool
		down bool
	}
	order := []mod{{m.Control, mods&mu.Ctrl != 0}, {m.Option, mods&mu.Alt != 0}, {m.Shift, mods&mu.Shift != 0}, {m.Command, mods&mu.Super != 0}}
	if runtime.GOOS != "darwin" {
		order = []mod{{m.Command, mods&mu.Super != 0}, {m.Control, mods&mu.Ctrl != 0}, {m.Option, mods&mu.Alt != 0}, {m.Shift, mods&mu.Shift != 0}}
	}
	for _, o := range order {
		if o.set {
			on = append(on, o.down)
		}
	}
	parts := strings.Split(h.String(), "+")
	want := parts[len(parts)-1]
	keyDown := false
	for _, k := range c.KeysDown() {
		if n, ok := keyName(k); ok && n == want {
			keyDown = true
		}
	}
	on = append(on, keyDown)
	copy(held, on)
	return held
}

// bigKeycaps are the 52 px keys of the onboarding wells, with "+" between
// them; a key held down now is pressed ink. A chord reads left to right in
// every language.
func bigKeycaps(c *mu.Context, p theme.Colors, caps []string, held []bool) {
	ltr(mu.Row(c)).Gap(10).AlignItems(mu.Center).Shrink(0).Role(mu.RoleText).Label(strings.Join(caps, " ")).Children(func() {
		for i, k := range caps {
			if i > 0 {
				uiText(c, "+", 16, 400, p.Pencil).AccessHidden(true)
			}
			key := mu.Box(c).Height(52).MinWidth(52).PaddingX(18).Radius(10).Justify(mu.Center).AlignItems(mu.Center).AccessHidden(true)
			if len([]rune(k)) > 1 {
				key.PaddingX(20).MinWidth(150)
			}
			fg := p.Ink
			if i < len(held) && held[i] {
				fg = p.OnInk
				key.Background(p.InkFill).InsetShadow(0, 2, 4, 0, black.Alpha(0.5)).Shadow(0, 1, 0, 0, p.Highlight(0.7))
			} else {
				keycapStack(key, p)
			}
			key.Children(func() { mono(c, k, 18, fg) })
		}
	})
}

func (a *App) onbHotkeys(c *mu.Context, p theme.Colors) {
	h := a.cfg.Hotkeys
	k := &a.onb.keys
	recToggle, recPTT := k.slot == slotToggle, k.slot == slotPTT
	st := a.permissions()
	a.pollPermissions(c)
	// Other programs: Sayso can look on macOS. Elsewhere the step promises
	// only the check of Sayso's own actions.
	finder := a.conflictFinder() != nil
	toggleConflicts := a.hotkeyConflicts(h.Toggle)
	conflicts := append(append([]platform.HotkeyConflict(nil), toggleConflicts...), a.hotkeyConflicts(h.PushToTalk)...)
	if finder {
		c.After(conflictTTL) // running apps change
	}
	lead := i18n.T("onboarding.hotkeys.lead")
	if !finder {
		lead = i18n.T("onboarding.hotkeys.lead_local")
	}
	stepColumn(c, p, i18n.T("onboarding.hotkeys.title"), lead, func() {
		mu.Row(c).Gap(14).AlignItems(mu.Stretch).Children(func() {
			// Start and stop.
			onbCard(c, p).Gap(14).Children(func() {
				mu.Row(c).AlignItems(mu.Center).Children(func() {
					cardTitle(c, p, i18n.T("onboarding.hotkeys.start_stop"), 20, 24).Grow(1).Basis(0).Role(mu.RoleHeading)
					label, name := i18n.T("onboarding.hotkeys.change"), i18n.T("onboarding.hotkeys.change_toggle")
					if recToggle {
						label, name = i18n.T("onboarding.hotkeys.cancel"), i18n.T("onboarding.hotkeys.cancel_label")
					}
					if a.textAction(c, p, "onboarding.hotkeys.change_toggle", label, name, p.Accent) {
						if recToggle {
							a.cancelRecording()
						} else {
							a.startRecording(slotToggle)
						}
					}
				})
				shallowWell(mu.Row(a.k(c, "onboarding.hotkeys.toggle")).Height(88).Radius(12).Justify(mu.Center).AlignItems(mu.Center), p).Children(func() {
					switch {
					case recToggle:
						listening(c, p)
					case h.Toggle != nil:
						bigKeycaps(c, p, h.Toggle.Keycaps(), heldCaps(c, *h.Toggle))
					default:
						uiText(c, i18n.T("onboarding.hotkeys.no_key"), 14, 400, p.Graphite)
					}
				})
				switch {
				case recToggle:
					uiText(c, i18n.T("onboarding.hotkeys.press_new_keys"), 13, 400, p.Graphite).Role(mu.RoleStatus)
				case h.Toggle != nil && len(toggleConflicts) > 0:
					uiText(c, i18n.T("onboarding.hotkeys.conflict"), 13, 400, p.Danger).Role(mu.RoleStatus)
				case h.Toggle != nil:
					held := heldCaps(c, *h.Toggle)
					all := len(held) > 0
					for _, x := range held {
						all = all && x
					}
					text := i18n.T("onboarding.hotkeys.no_clash")
					if finder {
						text = i18n.T("onboarding.hotkeys.no_conflict")
					}
					if all {
						text = i18n.T("onboarding.hotkeys.pressed_now", "keys", keysText(h.Toggle.Keycaps()))
					}
					checkLine(c, p, text, 400).Role(mu.RoleStatus)
				default:
					uiText(c, i18n.T("onboarding.hotkeys.set_a_key"), 13, 400, p.Graphite).Role(mu.RoleStatus)
				}
			})
			// Push to talk.
			onbCard(c, p).Gap(14).Children(func() {
				mu.Row(c).Gap(14).AlignItems(mu.Center).Children(func() {
					cardTitle(c, p, i18n.T("onboarding.hotkeys.push_to_talk"), 20, 24).Grow(1).Basis(0).Role(mu.RoleHeading)
					switch {
					case recPTT:
						if a.textAction(c, p, "onboarding.hotkeys.ptt_cancel", i18n.T("onboarding.hotkeys.cancel"), i18n.T("onboarding.hotkeys.cancel_label"), p.Accent) {
							a.cancelRecording()
						}
					case h.PushToTalk != nil:
						if a.textAction(c, p, "onboarding.hotkeys.ptt_change", i18n.T("onboarding.hotkeys.change"), i18n.T("onboarding.hotkeys.change_ptt"), p.Accent) {
							a.startRecording(slotPTT)
						}
						if a.textAction(c, p, "onboarding.hotkeys.ptt_clear", i18n.T("onboarding.hotkeys.clear"), i18n.T("onboarding.hotkeys.clear_ptt"), p.Graphite) {
							a.cfg.Hotkeys.PushToTalk = nil
							a.hotkeysChanged()
						}
					default:
						uiText(c, i18n.T("onboarding.hotkeys.optional"), 13, 400, p.Graphite)
					}
				})
				switch {
				case recPTT:
					dashedBox(c, p, func() { listening(c, p) })
				case h.PushToTalk != nil:
					shallowWell(mu.Row(a.k(c, "onboarding.hotkeys.ptt")).Height(88).Radius(12).Justify(mu.Center).AlignItems(mu.Center), p).Children(func() {
						bigKeycaps(c, p, h.PushToTalk.Keycaps(), heldCaps(c, *h.PushToTalk))
					})
				default:
					dashedBox(c, p, func() {
						uiText(c, i18n.T("onboarding.hotkeys.hold_hint"), 14, 400, p.Graphite).TextAlign(mu.Center).PaddingX(12)
						mu.Row(c).Gap(8).Children(func() {
							if runtime.GOOS == "darwin" && a.button(c, p, "onboarding.hotkeys.ptt_right_option", i18n.T("onboarding.hotkeys.use_right_option"), bopt{size: sizeSmall}) {
								a.setRecorded(slotPTT, ptr(config.SoloKey(config.RightOption)))
							}
							if a.button(c, p, "onboarding.hotkeys.ptt_record", i18n.T("onboarding.hotkeys.record_key"), bopt{size: sizeSmall}) {
								a.startRecording(slotPTT)
							}
						})
					})
				}
				a.inputMonitoringLine(c, p, st)
			})
		})
		for _, x := range conflicts {
			banner(c, p, bannerWarning, conflictText(x))
		}
		if k.note != "" {
			banner(c, p, bannerWarning, k.note)
		}
		if k.slot != "" {
			banner(c, p, bannerInfo, i18n.T("onboarding.hotkeys.window_only"))
		}
	})
}

// inputMonitoringLine says what the keys need of Input Monitoring.
func (a *App) inputMonitoringLine(c *mu.Context, p theme.Colors, st platform.PermissionStatus) {
	switch {
	case !hasInputPermissions():
		uiText(c, i18n.T("onboarding.hotkeys.works_everywhere"), 13, 400, p.Graphite)
	case !a.cfg.Hotkeys.NeedsInputMonitoring():
		uiText(c, i18n.TOS("onboarding.hotkeys.im_needed", nil), 13, 400, p.Graphite)
	case st.InputMonitoring:
		checkLine(c, p, i18n.TOS("onboarding.hotkeys.im_on", nil), 400)
	default:
		mu.Row(c).Gap(10).AlignItems(mu.Center).Children(func() {
			uiText(c, i18n.TOS("onboarding.hotkeys.im_off", nil), 13, 400, p.Danger).Grow(1).Basis(0).MinWidth(0)
			if a.button(c, p, "onboarding.hotkeys.open_im", i18n.TOS("os.open_permission_settings", nil), bopt{size: sizeSmall}) {
				a.openPermissionSettings("input_monitoring")
			}
		})
	}
}

// textAction is a text button: "Change", "Clear". name is its name for a
// screen reader.
func (a *App) textAction(c *mu.Context, p theme.Colors, key, label, name string, col mu.Color) bool {
	b := mu.ButtonBase(a.k(c, key)).Label(name).Shrink(0).Cursor(mu.CursorPointer)
	if b.Hovered() {
		b.Opacity(0.75)
	}
	b.Children(func() { uiText(c, label, 13, 500, col).SingleLine() })
	return clicked(c, b)
}

// dashedBox is the empty push-to-talk well: a dashed outline.
func dashedBox(c *mu.Context, p theme.Colors, fn func()) {
	mu.Column(c).Height(88).Radius(12).Gap(10).Justify(mu.Center).AlignItems(mu.Center).
		Border(1.5, p.DebossShade).BorderStyle(mu.BorderDashed).Children(fn)
}

// listening is the "Press the keys" line with its accent dot.
func listening(c *mu.Context, p theme.Colors) {
	mu.Row(c).Gap(10).AlignItems(mu.Center).Role(mu.RoleStatus).Children(func() {
		mu.Box(c).Size(8, 8).Radius(99).Background(p.Accent)
		uiText(c, i18n.T("onboarding.hotkeys.listening_window"), 14, 500, p.Graphite)
	})
}
