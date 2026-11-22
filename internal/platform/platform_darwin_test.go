package platform

import (
	"bytes"
	"context"
	"encoding/binary"
	"os"
	"strings"
	"testing"
	"time"
)

func TestMacKeyCodesFollowTheANSILayout(t *testing.T) {
	cases := map[uint16]hotkeyKey{
		49: {kind: keySpace}, 53: {kind: keyEscape}, 36: {kind: keyReturn}, 76: {kind: keyReturn},
		9: letterKey('v'), 2: letterKey('d'), 6: letterKey('z'), 18: digitKey(1), 29: digitKey(0),
		122: fKey(1), 96: fKey(5), 111: fKey(12), 90: fKey(20),
		33: punctKey('['), 30: punctKey(']'), 50: punctKey('`'), 44: punctKey('/'), 42: punctKey('\\'),
	}
	for code, want := range cases {
		if got := macKeyFromCode[code]; got != want {
			t.Errorf("code %d = %v, want %v", code, got, want)
		}
	}
	if _, ok := macKeyFromCode[200]; ok {
		t.Error("code 200 is mapped")
	}
	// Every table entry is unique (keypad Enter is the only alias).
	seen := map[uint16]bool{}
	for _, list := range [][]uint16{macLetterCodes[:], macDigitCodes[:], macPunctuationCodes[:], macFCodes[:], {49, 53, 36, 48, 51}} {
		for _, c := range list {
			if seen[c] {
				t.Errorf("code %d used twice", c)
			}
			seen[c] = true
		}
	}
}

func TestMacSoloModifiersUseDeviceBits(t *testing.T) {
	// Right Option held: generic Alt plus the right-side bit.
	if got := macHeldModifiers(cgFlagAlt | 0x40); got != setOf(modRightOption) {
		t.Errorf("right option: %v", got)
	}
	if got := macHeldModifiers(cgFlagAlt | 0x20); got != setOf(modLeftOption) {
		t.Errorf("left option: %v", got)
	}
	if got := macHeldModifiers(cgFlagSecondaryFn | cgFlagCommand | 0x08); got != setOf(modFn, modLeftCommand) {
		t.Errorf("fn + left command: %v", got)
	}
	if m, ok := macSoloFromCode(63); !ok || m != modFn {
		t.Error("key code 63 is not fn")
	}
	ev, ok := macRawEvent(kCGEventFlagsChanged, 61, cgFlagAlt|0x40, false, false)
	if !ok || ev.kind != rawModifiers || !ev.hasChanged || ev.changed != modRightOption || !ev.held.has(modRightOption) {
		t.Errorf("flagsChanged event: %+v", ev)
	}
	ev, _ = macRawEvent(kCGEventKeyDown, 49, cgFlagAlt|0x20, true, false)
	if ev.kind != rawKeyDown || ev.key.kind != keySpace || !ev.mods.option || !ev.repeat {
		t.Errorf("keyDown event: %+v", ev)
	}
	if ev, _ := macRawEvent(kCGEventScrollWheel, 0, 0, false, false); ev.kind != rawPointer {
		t.Errorf("scroll: %+v", ev)
	}
	if _, ok := macRawEvent(5, 0, 0, false, false); ok {
		t.Error("mouse motion produced an event")
	}
	mask := macEventMask()
	if mask&(1<<5) != 0 || mask&(1<<kCGEventScrollWheel) == 0 || mask&(1<<kCGEventFlagsChanged) == 0 {
		t.Errorf("event mask %#x", mask)
	}
}

func TestMacTapEventsDriveTheEngine(t *testing.T) {
	e := engineWith(t, HotkeyConfig{PushToTalk: "right_option", Toggle: "left_command"}, false)
	ev, _ := macRawEvent(kCGEventFlagsChanged, 61, cgFlagAlt|0x40, false, false)
	expectEvents(t, "right option down", e.handle(ev, at(0)), HotkeyPushToTalkDown)
	ev, _ = macRawEvent(kCGEventFlagsChanged, 61, 0, false, false)
	expectEvents(t, "right option up", e.handle(ev, at(10)), HotkeyPushToTalkUp)
	// Left Option does not press the right-side binding.
	ev, _ = macRawEvent(kCGEventFlagsChanged, 58, cgFlagAlt|0x20, false, false)
	expectEvents(t, "left option", e.handle(ev, at(20)))
	ev, _ = macRawEvent(kCGEventFlagsChanged, 58, 0, false, false)
	e.handle(ev, at(30))
	ev, _ = macRawEvent(kCGEventFlagsChanged, 55, cgFlagCommand|0x08, false, false)
	e.handle(ev, at(100))
	ev, _ = macRawEvent(kCGEventFlagsChanged, 55, 0, false, false)
	expectEvents(t, "left command tap", e.handle(ev, at(200)), HotkeyToggle)
}

func TestHostOfMatchesStyleRules(t *testing.T) {
	cases := map[string]string{
		"https://www.GitHub.com/watzon/sayso?tab=1": "github.com",
		"http://user@mail.google.com:8080/x":        "mail.google.com",
		"docs.rs/serde":                             "docs.rs",
		"file:///Users/x/a.txt":                     "",
		"https://localhost/":                        "",
		"about:blank":                               "",
		"":                                          "",
	}
	for in, want := range cases {
		if got := hostOf(in); got != want {
			t.Errorf("hostOf(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestUTF16ChunksKeepSurrogatePairs(t *testing.T) {
	text := strings.Repeat("a", 19) + "😀" + "b"
	chunks := utf16Chunks(text, 20)
	if len(chunks) != 2 || len(chunks[0]) != 19 || len(chunks[1]) != 3 {
		t.Fatalf("chunks: %v", chunks)
	}
	if len(utf16Chunks("", 20)) != 0 {
		t.Error("empty text gave chunks")
	}
}

func TestParseNowPlaying(t *testing.T) {
	got := parseNowPlaying(`{"title":"Song","bundleIdentifier":"com.spotify.client","playing":true,"processIdentifier":812}`)
	if got == nil || got.player != "com.spotify.client 812" || !got.playing {
		t.Errorf("spotify: %+v", got)
	}
	got = parseNowPlaying(`{"parentApplicationBundleIdentifier":"com.apple.Safari","playing":false}`)
	if got == nil || got.player != "com.apple.Safari" || got.playing {
		t.Errorf("safari: %+v", got)
	}
	if parseNowPlaying("null") != nil || parseNowPlaying("garbage") != nil {
		t.Error("null or garbage gave a player")
	}
}

func TestMacSettingsURLs(t *testing.T) {
	for kind, suffix := range map[string]string{
		settingsMicrophone: "Privacy_Microphone", settingsAccessibility: "Privacy_Accessibility", settingsInputMonitoring: "Privacy_ListenEvent",
	} {
		url, err := macSettingsURL(kind)
		if err != nil || !strings.HasPrefix(url, "x-apple.systempreferences:") || !strings.HasSuffix(url, suffix) {
			t.Errorf("%s: %q %v", kind, url, err)
		}
	}
	if _, err := macSettingsURL("camera"); err == nil {
		t.Error("unknown pane accepted")
	}
}

func TestDisplayLevelAndFraming(t *testing.T) {
	if displayLevel(0) != 0 || displayLevel(1) != 1 {
		t.Error("level ends")
	}
	// -26 dBFS (normal speech) is about 0.67.
	if l := displayLevel(0.0501); l < 0.6 || l > 0.72 {
		t.Errorf("speech level %v", l)
	}
	got := make(chan AudioChunk, 8)
	f := newAudioFramer(func(c AudioChunk) { got <- c })
	f.push(make([]float32, 500))
	f.push(make([]float32, 150))
	f.close()
	close(got)
	n := 0
	for c := range got {
		n++
		if len(c.Samples) != frameSamples || c.RMS != 0 {
			t.Errorf("chunk %d samples, rms %v", len(c.Samples), c.RMS)
		}
	}
	if n != 2 || len(f.pending) != 10 {
		t.Errorf("%d chunks, %d pending", n, len(f.pending))
	}
}

// ---------------------------------------------------------------------------
// Live checks against the real frameworks. They prompt for nothing and post
// no keys.
// ---------------------------------------------------------------------------

func TestMacNewLoadsEveryFramework(t *testing.T) {
	s, err := New()
	if err != nil {
		t.Fatal(err)
	}
	if s.Audio == nil || s.Hotkeys == nil || s.Inserter == nil || s.Context == nil || s.Permissions == nil ||
		s.Sounds == nil || s.Media == nil || s.LoginItem == nil || s.Dock == nil {
		t.Fatalf("missing service: %+v", s)
	}
	st := s.Permissions.Status()
	if !st.NeedsInputPerms {
		t.Error("macOS needs input permissions")
	}
	t.Logf("permissions: microphone=%v accessibility=%v input monitoring=%v (mic status %d)",
		st.Microphone, st.Accessibility, st.InputMonitoring, microphoneAuthorization())
	// An unbundled test binary is never a registered login item.
	if on, err := s.LoginItem.Enabled(); err != nil || on {
		t.Errorf("login item: %v %v", on, err)
	}
	// The test binary has no MediaRemote adapter beside it, so nothing pauses.
	if was, err := s.Media.Pause(); was || err != nil {
		t.Errorf("media pause: %v %v", was, err)
	}
}

func TestMacListsInputDevices(t *testing.T) {
	devices, err := (&macAudio{}).ListDevices()
	if err != nil {
		t.Fatal(err)
	}
	defaults := 0
	for _, d := range devices {
		t.Logf("device %q id=%q default=%v", d.Name, d.ID, d.IsDefault)
		if d.ID == "" || d.Name == "" {
			t.Errorf("empty field: %+v", d)
		}
		if d.IsDefault {
			defaults++
		}
	}
	if len(devices) > 0 && defaults != 1 {
		t.Errorf("%d default devices", defaults)
	}
}

func TestMacFrontmostApp(t *testing.T) {
	app, err := macContext{}.ActiveApp()
	if err != nil {
		t.Skipf("no frontmost app in this session: %v", err)
	}
	t.Logf("frontmost: %+v", app)
	if app.Name == "" && app.BundleID == "" {
		t.Error("frontmost app has neither name nor bundle id")
	}
}

func TestMacClipboardRoundTripRestoresTheClipboard(t *testing.T) {
	if err := loadDarwin(); err != nil {
		t.Fatal(err)
	}
	saved := snapshotPasteboard()
	t.Cleanup(func() {
		if _, ok := restorePasteboard(saved); !ok {
			t.Error("could not restore the clipboard")
		}
		after := snapshotPasteboard()
		if !snapshotsEqual(saved, after) {
			t.Error("the clipboard differs after the restore")
		}
	})
	ins := &macInserter{}
	text := "sayso clipboard test ✓ " + time.Now().Format(time.RFC3339Nano)
	if err := ins.WriteClipboard(text); err != nil {
		t.Fatal(err)
	}
	got, err := ins.ReadClipboard()
	if err != nil || got != text {
		t.Fatalf("read %q, %v; want %q", got, err, text)
	}
	// A multi-type snapshot survives a round trip.
	snap := pasteboardSnapshot{{{uti: "public.utf8-plain-text", data: []byte("one")}, {uti: "com.example.sayso-test", data: []byte{0, 1, 2}}}}
	if _, ok := restorePasteboard(snap); !ok {
		t.Fatal("restore of a test snapshot failed")
	}
	if back := snapshotPasteboard(); !snapshotsEqual(snap, back) {
		t.Errorf("snapshot round trip: %+v", back)
	}
}

func snapshotsEqual(a, b pasteboardSnapshot) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		want := map[string][]byte{}
		for _, e := range a[i] {
			want[e.uti] = e.data
		}
		for _, e := range b[i] {
			if w, ok := want[e.uti]; ok && !bytes.Equal(w, e.data) {
				return false
			}
			delete(want, e.uti)
		}
		if len(want) != 0 {
			return false
		}
	}
	return true
}

func TestMacHotkeysStartNeedsAPermission(t *testing.T) {
	h := newMacHotkeys()
	if err := h.Configure(HotkeyConfig{Toggle: "ctrl+opt+cmd+f19", PushToTalk: "right_option"}); err != nil {
		t.Fatal(err)
	}
	granted := cgPreflightListenEventAccess() || axIsProcessTrusted()
	err := h.Start(func(HotkeyEvent) {})
	if !granted {
		if err == nil {
			t.Error("started without Input Monitoring or Accessibility")
		}
		return
	}
	if err != nil {
		t.Fatalf("start with permission: %v", err)
	}
	h.SetRecording(true)
	h.SetRecording(false)
	if err := h.Stop(); err != nil {
		t.Error(err)
	}
}

func TestMacSoundPlaysSilence(t *testing.T) {
	if err := (macSounds{}).Play(CueStart, silentWAV(1600), 0.05); err != nil {
		t.Fatal(err)
	}
	if err := (macSounds{}).Play(CueStart, []byte("not a wav"), 0.05); err == nil {
		t.Error("garbage audio played")
	}
}

// TestMacLiveCapture records half a second. It runs only with
// SAYSO_LIVE_AUDIO=1 and a granted microphone, because a capture can make
// macOS ask for the microphone.
func TestMacLiveCapture(t *testing.T) {
	if os.Getenv("SAYSO_LIVE_AUDIO") != "1" || microphoneAuthorization() != micAuthorized {
		t.Skip("set SAYSO_LIVE_AUDIO=1 with a granted microphone")
	}
	a := &macAudio{}
	chunks := make(chan AudioChunk, 256)
	if err := a.Start(context.Background(), "", func(c AudioChunk) { chunks <- c }); err != nil {
		t.Fatal(err)
	}
	time.Sleep(500 * time.Millisecond)
	if err := a.Stop(); err != nil {
		t.Fatal(err)
	}
	n := len(chunks)
	var peak float32
	for i := 0; i < n; i++ {
		c := <-chunks
		if c.RMS > peak {
			peak = c.RMS
		}
	}
	t.Logf("%d chunks in 500 ms, peak level %.3f", n, peak)
	if n < 15 {
		t.Errorf("%d chunks in 500 ms", n)
	}
	if err := a.Start(context.Background(), "", func(AudioChunk) {}); err != nil {
		t.Errorf("restart: %v", err)
	}
	_ = a.Stop()
}

// silentWAV makes a 16 kHz mono 16-bit WAV of n zero samples.
func silentWAV(n int) []byte {
	var b bytes.Buffer
	data := n * 2
	b.WriteString("RIFF")
	binary.Write(&b, binary.LittleEndian, uint32(36+data))
	b.WriteString("WAVEfmt ")
	for _, v := range []any{uint32(16), uint16(1), uint16(1), uint32(16000), uint32(32000), uint16(2), uint16(16)} {
		binary.Write(&b, binary.LittleEndian, v)
	}
	b.WriteString("data")
	binary.Write(&b, binary.LittleEndian, uint32(data))
	b.Write(make([]byte, data))
	return b.Bytes()
}
