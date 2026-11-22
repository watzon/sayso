package platform

import (
	"bytes"
	"encoding/binary"
	"math"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"testing/iotest"
	"time"
)

func TestLinuxKeyCodesFollowTheEvdevLayout(t *testing.T) {
	cases := map[uint16]hotkeyKey{
		57: {kind: keySpace}, 1: {kind: keyEscape}, 28: {kind: keyReturn}, 96: {kind: keyReturn},
		30: letterKey('a'), 47: letterKey('v'), 11: digitKey(0), 2: digitKey(1),
		59: fKey(1), 88: fKey(12), 190: fKey(20), 26: punctKey('['), 41: punctKey('`'), 53: punctKey('/'),
	}
	for code, want := range cases {
		if got := linuxKeyFromCode[code]; got != want {
			t.Errorf("code %d = %v, want %v", code, got, want)
		}
	}
	// Every key round-trips through its code.
	for code, key := range linuxKeyFromCode {
		back, ok := linuxCodeOfKey(key)
		if !ok || (back != code && code != evKeyKPEnter) {
			t.Errorf("%v: code %d back to %d", key, code, back)
		}
	}
	if m, ok := linuxSoloFromCode(evKeyRightAlt); !ok || m != modRightOption {
		t.Error("Right Alt is not right_option")
	}
	if _, ok := linuxSoloFromCode(evKeySpace); ok {
		t.Error("space is a modifier")
	}
}

func TestLinuxRefusesFn(t *testing.T) {
	for _, v := range []string{"fn", "fn+f5", "left_command+fn"} {
		if checkLinuxHotkey(v) == nil {
			t.Errorf("%q accepted", v)
		}
	}
	h := newLinuxHotkeys(linuxSession{})
	if h.Configure(HotkeyConfig{PushToTalk: "fn"}) == nil {
		t.Error("Configure accepted fn")
	}
	if err := h.Configure(HotkeyConfig{Toggle: "ctrl+alt+space", PushToTalk: "right_option"}); err != nil {
		t.Error(err)
	}
}

func TestPortalTriggersAndIDsMatchTheRustVersion(t *testing.T) {
	h, _ := parseHotkey("ctrl+alt+space")
	if got := portalTrigger(h); got != "CTRL+ALT+space" {
		t.Errorf("trigger %q", got)
	}
	h, _ = parseHotkey("super+shift+[")
	if got := portalTrigger(h); got != "SHIFT+LOGO+bracketleft" {
		t.Errorf("trigger %q", got)
	}
	h, _ = parseHotkey("ctrl+f12")
	if got := portalTrigger(h); got != "CTRL+F12" {
		t.Errorf("trigger %q", got)
	}
	b := portalBindingFor(h, x11ChordAction{event: HotkeyEvent{Kind: HotkeyStyle, StyleID: "My Style!"}})
	if b.id != "style-my-style-" || b.description != "Start one dictation in the My Style! style" {
		t.Errorf("style binding %+v", b)
	}
	if b := portalBindingFor(h, x11ChordAction{ptt: true}); b.id != "push-to-talk" {
		t.Errorf("ptt binding %+v", b)
	}
}

func TestChordListAndEngineSplit(t *testing.T) {
	cfg := HotkeyConfig{Toggle: "ctrl+alt+space", PushToTalk: "ctrl+alt+p", PasteLast: "ctrl+alt+v", StyleHotkeys: map[string]string{"b": "ctrl+alt+2", "a": "ctrl+alt+1"}}
	list := chordList(cfg)
	if len(list) != 5 || !list[1].action.ptt || list[3].action.event.StyleID != "a" {
		t.Fatalf("chord list %+v", list)
	}
	// With a chord backend, the engine gets no chords; the listen stream
	// still handles modifier-only push-to-talk and Esc while recording.
	b := mustBindings(t, cfg)
	ctrlAlt := setOf(modLeftControl, modLeftOption)
	for _, backend := range []chordBackend{chordsX11, chordsPortal} {
		e := newHotkeyEngine()
		e.configure(engineBindingsFor(b, backend))
		if d := e.handle(down(57, kSpace, ctrlAlt), at(0)); len(d.events) != 0 {
			t.Errorf("backend %d: engine fired a chord it does not own: %v", backend, kinds(d.events))
		}
		if d := e.handle(down(25, letterKey('p'), ctrlAlt), at(10)); len(d.events) != 0 {
			t.Errorf("backend %d: engine fired the chord push-to-talk: %v", backend, kinds(d.events))
		}
	}
	e := newHotkeyEngine()
	e.configure(engineBindingsFor(b, chordsListen))
	expectEvents(t, "listen backend owns chords", e.handle(down(57, kSpace, ctrlAlt), at(0)), HotkeyToggle)
	solo := mustBindings(t, HotkeyConfig{Toggle: "ctrl+alt+space", PushToTalk: "right_option"})
	e = newHotkeyEngine()
	e.configure(engineBindingsFor(solo, chordsX11))
	expectEvents(t, "solo ptt stays with the listen stream", e.handle(mod(modRightOption, setOf(modRightOption)), at(0)), HotkeyPushToTalkDown)
}

func TestEvdevTranslatorTracksModifiersAcrossEvents(t *testing.T) {
	var tr evdevTranslator
	ev, ok := tr.translate(evdevInput{typ: evKey, code: evKeyRightAlt, value: 1})
	if !ok || ev.kind != rawModifiers || ev.changed != modRightOption || ev.held != setOf(modRightOption) || !ev.mods.option {
		t.Fatalf("right alt down: %+v", ev)
	}
	if _, ok := tr.translate(evdevInput{typ: evKey, code: evKeyRightAlt, value: 2}); ok {
		t.Error("modifier repeat produced an event")
	}
	ev, _ = tr.translate(evdevInput{typ: evKey, code: 57, value: 1})
	if ev.kind != rawKeyDown || ev.key.kind != keySpace || ev.held != setOf(modRightOption) {
		t.Errorf("space: %+v", ev)
	}
	ev, _ = tr.translate(evdevInput{typ: evKey, code: 57, value: 2})
	if !ev.repeat {
		t.Errorf("repeat: %+v", ev)
	}
	ev, _ = tr.translate(evdevInput{typ: evKey, code: evKeyRightAlt, value: 0})
	if ev.held != 0 {
		t.Errorf("after release: %+v", ev)
	}
	if ev, ok := tr.translate(evdevInput{typ: evKey, code: btnMouseFirst, value: 1}); !ok || ev.kind != rawPointer {
		t.Error("mouse button")
	}
	if _, ok := tr.translate(evdevInput{typ: evKey, code: btnMouseFirst, value: 0}); ok {
		t.Error("mouse button release produced an event")
	}
	if ev, ok := tr.translate(evdevInput{typ: evRel, code: relWheel, value: -1}); !ok || ev.kind != rawPointer {
		t.Error("wheel")
	}
	if _, ok := tr.translate(evdevInput{typ: evRel, code: 0 /* REL_X */, value: 5}); ok {
		t.Error("mouse motion produced an event")
	}
}

// evdevSequence feeds evdev input through the translator and an engine.
func evdevSequence(t *testing.T, cfg HotkeyConfig, steps []struct {
	in evdevInput
	ms int
}) []HotkeyEventKind {
	t.Helper()
	e := engineWith(t, cfg, false)
	var tr evdevTranslator
	var got []HotkeyEventKind
	for _, s := range steps {
		if raw, ok := tr.translate(s.in); ok {
			got = append(got, kinds(e.handle(raw, at(s.ms)).events)...)
		}
	}
	return got
}

type step = struct {
	in evdevInput
	ms int
}

func key(code uint16, value int32, ms int) step {
	return step{evdevInput{typ: evKey, code: code, value: value}, ms}
}

func TestEvdevModifierTapAndPushToTalk(t *testing.T) {
	got := evdevSequence(t, HotkeyConfig{Toggle: "right_control"}, []step{key(evKeyRightCtrl, 1, 0), key(evKeyRightCtrl, 0, 300)})
	if !reflect.DeepEqual(got, []HotkeyEventKind{HotkeyToggle}) {
		t.Errorf("tap: %v", got)
	}
	got = evdevSequence(t, HotkeyConfig{Toggle: "right_control"}, []step{key(evKeyRightCtrl, 1, 0), key(evKeyRightCtrl, 0, 401)})
	if len(got) != 0 {
		t.Errorf("slow tap: %v", got)
	}
	got = evdevSequence(t, HotkeyConfig{Toggle: "right_control"}, []step{
		key(evKeyRightCtrl, 1, 0), key(46 /* c */, 1, 50), key(46, 0, 60), key(evKeyRightCtrl, 0, 100)})
	if len(got) != 0 {
		t.Errorf("ctrl+c fired: %v", got)
	}
	got = evdevSequence(t, HotkeyConfig{Toggle: "right_control"}, []step{
		key(evKeyRightCtrl, 1, 0), {evdevInput{typ: evRel, code: relWheel, value: 1}, 50}, key(evKeyRightCtrl, 0, 100)})
	if len(got) != 0 {
		t.Errorf("scroll did not cancel: %v", got)
	}
	got = evdevSequence(t, HotkeyConfig{PushToTalk: "left_control+left_option"}, []step{
		key(evKeyLeftCtrl, 1, 0), key(evKeyLeftAlt, 1, 10), key(evKeyLeftAlt, 2, 500), key(evKeyLeftCtrl, 0, 3000)})
	if !reflect.DeepEqual(got, []HotkeyEventKind{HotkeyPushToTalkDown, HotkeyPushToTalkUp}) {
		t.Errorf("ptt: %v", got)
	}
}

func TestCommandWords(t *testing.T) {
	for word, want := range map[string][]HotkeyEventKind{
		"toggle":                 {HotkeyToggle},
		"cancel":                 {HotkeyCancel},
		"paste-last":             {HotkeyPasteLast},
		"cycle-style":            {HotkeyCycleStyle},
		"incognito":              {HotkeyToggleIncognito},
		"push-to-talk-down":      {HotkeyPushToTalkDown},
		"push-to-talk-up":        {HotkeyPushToTalkUp},
		"push-to-talk-up-submit": {HotkeyEnterSend, HotkeyPushToTalkUp},
		" toggle\n":              {HotkeyToggle},
	} {
		got, ok := commandEvents(word)
		if !ok || !reflect.DeepEqual(kinds(got), want) {
			t.Errorf("%q: %v %v", word, kinds(got), ok)
		}
	}
	if got, ok := commandEvents("style=email"); !ok || got[0].Kind != HotkeyStyle || got[0].StyleID != "email" {
		t.Errorf("style: %+v", got)
	}
	for _, bad := range []string{"style=", "explode", ""} {
		if _, ok := commandEvents(bad); ok {
			t.Errorf("%q accepted", bad)
		}
	}
}

func TestCommandSocketDeliversCommands(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("XDG_RUNTIME_DIR", dir)
	t.Setenv("FLATPAK_ID", "")
	got := make(chan HotkeyEvent, 4)
	l, err := listenCommands(func(e HotkeyEvent) { got <- e })
	if err != nil {
		t.Fatal(err)
	}
	defer l.close()
	if st, err := os.Stat(filepath.Join(dir, "sayso.sock")); err != nil || st.Mode().Perm() != 0o600 {
		t.Errorf("socket: %v %v", st, err)
	}
	if _, err := listenCommands(func(HotkeyEvent) {}); err == nil {
		t.Error("a second listener took the socket of a live one")
	}
	if err := SendCommand("style=email"); err != nil {
		t.Fatal(err)
	}
	select {
	case e := <-got:
		if e.Kind != HotkeyStyle || e.StyleID != "email" {
			t.Errorf("event %+v", e)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("no event")
	}
	if SendCommand("explode") == nil {
		t.Error("unknown command sent")
	}
}

func TestPrivateDirRefusesASharedFolder(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "x")
	if err := privateDir(dir); err != nil {
		t.Fatal(err)
	}
	if err := privateDir(dir); err != nil {
		t.Errorf("own private folder: %v", err)
	}
	os.Chmod(dir, 0o777)
	if privateDir(dir) == nil {
		t.Error("accepted a folder others can open")
	}
}

func TestParsePactlSources(t *testing.T) {
	text := `Source #0
	State: SUSPENDED
	Name: alsa_output.pci-0000_00_1f.3.analog-stereo.monitor
	Description: Monitor of Built-in Audio Analog Stereo
	Monitor of Sink: alsa_output.pci-0000_00_1f.3.analog-stereo
Source #1
	State: RUNNING
	Name: alsa_input.usb-TONOR_G11-00.mono-fallback
	Description: TONOR G11 USB mic Mono
	Monitor of Sink: n/a
Source #2
	Name: alsa_input.pci-0000_00_1f.3.analog-stereo
	Description: Built-in Audio Analog Stereo
	Monitor of Sink: n/a
`
	got := parsePactlSources(text, "alsa_input.usb-TONOR_G11-00.mono-fallback")
	want := []AudioDevice{
		{ID: "alsa_input.usb-TONOR_G11-00.mono-fallback", Name: "TONOR G11 USB mic Mono", IsDefault: true},
		{ID: "alsa_input.pci-0000_00_1f.3.analog-stereo", Name: "Built-in Audio Analog Stereo"},
	}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("got %+v", got)
	}
}

func TestParseALSAPCM(t *testing.T) {
	got := parseALSAPCM("00-00: ALC892 Analog : ALC892 Analog : playback 1 : capture 1\n00-01: ALC892 Digital : ALC892 Digital : playback 1\n01-00: USB Audio : USB Audio : capture 1\n")
	want := []AudioDevice{{ID: "hw:0,0", Name: "ALC892 Analog", IsDefault: true}, {ID: "hw:1,0", Name: "USB Audio"}}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("got %+v", got)
	}
}

func TestReadFloat32KeepsSamplesSplitAcrossReads(t *testing.T) {
	var raw bytes.Buffer
	var want []float32
	for i := 0; i < 1000; i++ {
		v := float32(math.Sin(float64(i) / 10))
		want = append(want, v)
		binary.Write(&raw, binary.LittleEndian, v)
	}
	var got []float32
	readFloat32(iotest.OneByteReader(&raw), func(s []float32) { got = append(got, s...) })
	if !reflect.DeepEqual(got, want) {
		t.Errorf("got %d samples, want %d", len(got), len(want))
	}
}

func TestLinuxRecorderChoice(t *testing.T) {
	only := func(tools ...string) func(string) (string, error) {
		return func(name string) (string, error) {
			for _, t := range tools {
				if t == name {
					return "/usr/bin/" + name, nil
				}
			}
			return "", exec.ErrNotFound
		}
	}
	a := &linuxAudio{lookPath: only("pw-record", "arecord")}
	args, err := a.recordCommand("mic")
	if err != nil || args[0] != "pw-record" || !strings.Contains(strings.Join(args, " "), "--target mic") {
		t.Errorf("pw-record: %v %v", args, err)
	}
	a = &linuxAudio{lookPath: only()}
	if _, err := a.recordCommand(""); err == nil {
		t.Error("no recorder accepted")
	}
}

func TestScaleWAV(t *testing.T) {
	wav := silentWAVLinux(4)
	binary.LittleEndian.PutUint16(wav[44:], uint16(10000))
	out, err := scaleWAV(wav, 0.5)
	if err != nil {
		t.Fatal(err)
	}
	if v := int16(binary.LittleEndian.Uint16(out[44:])); v != 5000 {
		t.Errorf("scaled sample %d", v)
	}
	if v := int16(binary.LittleEndian.Uint16(wav[44:])); v != 10000 {
		t.Error("input changed")
	}
	if _, err := scaleWAV([]byte("nope"), 0.5); err == nil {
		t.Error("garbage accepted")
	}
}

func silentWAVLinux(n int) []byte {
	var b bytes.Buffer
	b.WriteString("RIFF")
	binary.Write(&b, binary.LittleEndian, uint32(36+n*2))
	b.WriteString("WAVEfmt ")
	for _, v := range []any{uint32(16), uint16(1), uint16(1), uint32(16000), uint32(32000), uint16(2), uint16(16)} {
		binary.Write(&b, binary.LittleEndian, v)
	}
	b.WriteString("data")
	binary.Write(&b, binary.LittleEndian, uint32(n*2))
	b.Write(make([]byte, n*2))
	return b.Bytes()
}

func TestDesktopEntriesAndWMClass(t *testing.T) {
	e, ok := parseDesktopEntry("org.gnome.Nautilus", "[Desktop Entry]\nName=Files\nIcon=org.gnome.Nautilus\n[Desktop Action new]\nName=New Window\n")
	if !ok || e.name != "Files" || !e.listed {
		t.Errorf("entry %+v", e)
	}
	if e, _ := parseDesktopEntry("x", "[Desktop Entry]\nName=X\nNoDisplay=true\n"); e.listed {
		t.Error("NoDisplay entry listed")
	}
	if _, ok := parseDesktopEntry("x", "[Other]\nName=X\n"); ok {
		t.Error("entry without the group")
	}
	entries := map[string]desktopEntry{
		"org.gnome.Nautilus": {id: "org.gnome.Nautilus", name: "Files"},
		"code":               {id: "code", name: "Visual Studio Code", wmClass: "Code"},
		"firefox":            {id: "firefox", name: "Firefox"},
	}
	for class, want := range map[string]string{"Nautilus": "Files", "Code": "Visual Studio Code", "Firefox": "Firefox"} {
		if e, ok := entryForClass(entries, class); !ok || e.name != want {
			t.Errorf("%s: %+v", class, e)
		}
	}
	if parseWMClass([]byte("navigator\x00Firefox\x00")) != "Firefox" || parseWMClass([]byte("xterm\x00")) != "xterm" {
		t.Error("WM_CLASS")
	}
	dir := t.TempDir()
	os.MkdirAll(filepath.Join(dir, "a", "applications"), 0o755)
	os.MkdirAll(filepath.Join(dir, "b", "applications"), 0o755)
	os.WriteFile(filepath.Join(dir, "a", "applications", "app.desktop"), []byte("[Desktop Entry]\nName=First\n"), 0o644)
	os.WriteFile(filepath.Join(dir, "b", "applications", "app.desktop"), []byte("[Desktop Entry]\nName=Second\n"), 0o644)
	if got := loadDesktopEntries([]string{filepath.Join(dir, "a"), filepath.Join(dir, "b")}); got["app"].name != "First" {
		t.Errorf("first folder must win: %+v", got)
	}
}

func TestLoginItemWritesAndRemovesTheAutostartEntry(t *testing.T) {
	home := t.TempDir()
	env := map[string]string{"HOME": home}
	l := linuxLoginItem{getenv: func(k string) string { return env[k] }, executable: func() (string, error) { return "/opt/my apps/sayso", nil }}
	if on, err := l.Enabled(); on || err != nil {
		t.Fatalf("initial: %v %v", on, err)
	}
	if err := l.SetEnabled(true); err != nil {
		t.Fatal(err)
	}
	text, _ := os.ReadFile(filepath.Join(home, ".config/autostart", autostartFileName))
	if !strings.Contains(string(text), "\nExec=\"/opt/my apps/sayso\"\n") {
		t.Errorf("entry:\n%s", text)
	}
	if on, _ := l.Enabled(); !on {
		t.Error("not enabled after SetEnabled(true)")
	}
	if err := l.SetEnabled(false); err != nil {
		t.Fatal(err)
	}
	if on, _ := l.Enabled(); on {
		t.Error("still enabled")
	}
	if err := l.SetEnabled(false); err != nil {
		t.Errorf("second disable: %v", err)
	}
	env["FLATPAK_ID"] = "dev.sayso.Sayso"
	if words, _ := l.launchCommand(); strings.Join(words, " ") != "flatpak run dev.sayso.Sayso" {
		t.Errorf("flatpak command %v", words)
	}
	if autostartEnabled("[Desktop Entry]\nHidden=true\n") || autostartEnabled("[Desktop Entry]\nX-GNOME-Autostart-enabled=false\n") {
		t.Error("disabled entries read as enabled")
	}
}

func TestPasteKeys(t *testing.T) {
	for in, want := range map[string]pasteKey{"shift+insert": pasteShiftInsert, "Insert+Shift": pasteShiftInsert, "CTRL + V": pasteCtrlV, "shift+ctrl+v": pasteCtrlShiftV} {
		if got, ok := parsePasteKey(in); !ok || got != want {
			t.Errorf("%q: %v %v", in, got, ok)
		}
	}
	for _, bad := range []string{"alt+v", ""} {
		if _, ok := parsePasteKey(bad); ok {
			t.Errorf("%q accepted", bad)
		}
	}
	if got := strings.Join(ydotoolArgs(pasteShiftInsert.evdevCodes()), " "); got != "key 42:1 110:1 110:0 42:0" {
		t.Errorf("ydotool %q", got)
	}
}

func TestSessionDetection(t *testing.T) {
	env := func(m map[string]string) func(string) string { return func(k string) string { return m[k] } }
	if s := detectSession(env(map[string]string{"WAYLAND_DISPLAY": "wayland-0", "DISPLAY": ":0", "XDG_CURRENT_DESKTOP": "ubuntu:GNOME"})); s.kind != sessionWayland || !s.hasX11 || s.desktop != desktopGNOME {
		t.Errorf("GNOME Wayland: %+v", s)
	}
	if s := detectSession(env(map[string]string{"DISPLAY": ":0", "XDG_CURRENT_DESKTOP": "KDE"})); s.kind != sessionX11 || s.desktop != desktopKDE {
		t.Errorf("KDE X11: %+v", s)
	}
	if s := detectSession(env(map[string]string{"XDG_SESSION_TYPE": "wayland", "XDG_CURRENT_DESKTOP": "sway"})); s.kind != sessionWayland || s.hasX11 || s.desktop != desktopWlroots {
		t.Errorf("sway: %+v", s)
	}
	if s := detectSession(env(nil)); s.kind != sessionNone {
		t.Errorf("no display: %+v", s)
	}
}

func TestLinuxNewAndPermissions(t *testing.T) {
	s, err := New()
	if err != nil || s.Dock.SetShowInDock(true) != nil {
		t.Fatal(err)
	}
	st := s.Permissions.Status()
	if !st.Microphone || st.NeedsInputPerms {
		t.Errorf("status %+v", st)
	}
}

// TestLinuxClipboardRoundTrip runs where a clipboard tool and a display exist,
// and puts the old clipboard back.
func TestLinuxClipboardRoundTrip(t *testing.T) {
	ins := newLinuxInserter(currentSession())
	if ins.clipboardCommand(selClipboard, false) == nil {
		t.Skip("no display or no clipboard tool")
	}
	saved, err := ins.ReadClipboard()
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := ins.WriteClipboard(saved); err != nil {
			t.Errorf("restore: %v", err)
		}
	})
	text := "sayso clipboard test ✓"
	if err := ins.WriteClipboard(text); err != nil {
		t.Fatal(err)
	}
	if got, err := ins.ReadClipboard(); err != nil || got != text {
		t.Errorf("read %q %v", got, err)
	}
}

// TestX11GrabSeesXTestKeys grabs a chord and presses it with XTest. It runs
// only with an X server (for example Xvfb in CI).
func TestX11GrabSeesXTestKeys(t *testing.T) {
	if os.Getenv("DISPLAY") == "" {
		t.Skip("no X server")
	}
	got := make(chan HotkeyEvent, 8)
	g, err := startX11Grabs(func(e HotkeyEvent) { got <- e })
	if err != nil {
		t.Fatal(err)
	}
	defer g.close()
	// Not Ctrl+Alt+F keys: XKB turns those into a VT switch before any grab.
	toggle, _ := parseHotkey("ctrl+alt+k")
	ptt, _ := parseHotkey("ctrl+alt+j")
	codeOf := func(h hotkey) x11ChordKey {
		c, _ := linuxCodeOfKey(h.key)
		return x11ChordKey{keycodeX11(c), x11ModMask(h.mods)}
	}
	if err := g.bind(map[x11ChordKey]x11ChordAction{
		codeOf(toggle): {event: HotkeyEvent{Kind: HotkeyToggle}},
		codeOf(ptt):    {ptt: true},
	}); err != nil {
		t.Fatal(err)
	}
	expect := func(want HotkeyEventKind) {
		t.Helper()
		select {
		case e := <-got:
			if e.Kind != want {
				t.Errorf("event %v, want %v", e.Kind, want)
			}
		case <-time.After(2 * time.Second):
			t.Errorf("no %v event", want)
		}
	}
	codeK, codeJ := linuxLetterCodes['k'-'a'], linuxLetterCodes['j'-'a']
	if err := x11PressKeys(evKeyLeftCtrl, evKeyLeftAlt, codeK); err != nil {
		t.Fatal(err)
	}
	expect(HotkeyToggle)
	if err := x11PressKeys(evKeyLeftCtrl, evKeyLeftAlt, codeJ); err != nil {
		t.Fatal(err)
	}
	expect(HotkeyPushToTalkDown)
	expect(HotkeyPushToTalkUp)
	// Without the modifiers the key is not grabbed.
	if err := x11PressKeys(codeK); err != nil {
		t.Fatal(err)
	}
	select {
	case e := <-got:
		t.Errorf("bare K fired %v", e.Kind)
	case <-time.After(200 * time.Millisecond):
	}
}
