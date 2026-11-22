package platform

import (
	"context"
	"encoding/binary"
	"math"
	"os"
	"reflect"
	"strings"
	"sync"
	"testing"
	"time"
	"unicode/utf16"
	"unsafe"
)

// ---------------------------------------------------------------------------
// Resampler and audio pipeline
// ---------------------------------------------------------------------------

func sine(freq float64, rate uint32, seconds float64) []float32 {
	n := int(float64(rate) * seconds)
	out := make([]float32, n)
	for i := range out {
		out[i] = float32(math.Sin(2*math.Pi*freq*float64(i)/float64(rate))) * 0.5
	}
	return out
}

func convert(in []float32, rate uint32, chunk int) []float32 {
	r := newStreamResampler(rate, 16_000)
	var out []float32
	for len(in) > 0 {
		n := min(chunk, len(in))
		out = r.process(in[:n], out)
		in = in[n:]
	}
	return out
}

func TestOutputLengthFollowsTheRateRatio(t *testing.T) {
	for _, rate := range []uint32{48_000, 44_100, 24_000, 8_000} {
		if n := len(convert(sine(440, rate, 2), rate, 441)); math.Abs(float64(n)-32_000) > 32 {
			t.Errorf("%d Hz: %d samples", rate, n)
		}
	}
}

func TestSameRatePassesThroughUnchanged(t *testing.T) {
	in := sine(300, 16_000, 0.1)
	if !reflect.DeepEqual(convert(in, 16_000, 100), in) {
		t.Fatal("changed")
	}
}

func TestAToneKeepsItsLevelAndFrequency(t *testing.T) {
	out := convert(sine(1_000, 48_000, 1), 48_000, 480)
	steady := out[1_000 : len(out)-1_000]
	if r := frameRMS(steady); math.Abs(float64(r)-0.5/math.Sqrt2) > 0.01 {
		t.Fatalf("rms %v", r)
	}
	out = convert(sine(1_000, 44_100, 1), 44_100, 441)
	steady = out[1_000 : len(out)-1_000]
	crossings := 0
	for i := 1; i < len(steady); i++ {
		if steady[i-1] < 0 && steady[i] >= 0 {
			crossings++
		}
	}
	if hz := float64(crossings) / (float64(len(steady)) / 16_000); math.Abs(hz-1_000) > 10 {
		t.Fatalf("%v Hz", hz)
	}
}

func TestContentAboveTheNewNyquistIsRemoved(t *testing.T) {
	out := convert(sine(12_000, 48_000, 1), 48_000, 480)
	if r := frameRMS(out[1_000 : len(out)-1_000]); r > 0.01 {
		t.Fatalf("aliased energy %v", r)
	}
}

func TestChunkSizeDoesNotChangeTheOutputAndUpsamplingWorks(t *testing.T) {
	in := sine(700, 44_100, 0.5)
	whole, small := convert(in, 44_100, len(in)), convert(in, 44_100, 97)
	if len(whole) != len(small) {
		t.Fatalf("%d vs %d", len(whole), len(small))
	}
	for i := range whole {
		if math.Abs(float64(whole[i]-small[i])) > 1e-5 {
			t.Fatalf("sample %d: %v vs %v", i, whole[i], small[i])
		}
	}
	up := convert(sine(500, 8_000, 1), 8_000, 160)
	if math.Abs(float64(len(up))-16_000) > 32 || math.Abs(float64(frameRMS(up[1_000:len(up)-1_000]))-0.5/math.Sqrt2) > 0.01 {
		t.Fatalf("upsampled %d samples", len(up))
	}
	// A ratio with too many phases takes the direct path.
	if d := convert(sine(1_000, 44_101, 0.5), 44_101, 500); math.Abs(float64(len(d))-8_000) > 32 {
		t.Fatalf("direct path: %d samples", len(d))
	}
}

func waveFormatEx(tag uint16, channels uint16, rate uint32, bits uint16, sub string) []byte {
	b := make([]byte, 18)
	le := binary.LittleEndian
	le.PutUint16(b[0:], tag)
	le.PutUint16(b[2:], channels)
	le.PutUint32(b[4:], rate)
	le.PutUint16(b[12:], channels*bits/8)
	le.PutUint16(b[14:], bits)
	if tag == 0xFFFE {
		le.PutUint16(b[16:], 22)
		ext := make([]byte, 22)
		le.PutUint16(ext[0:], bits)
		g := mustGUID(sub)
		le.PutUint32(ext[6:], g.Data1)
		le.PutUint16(ext[10:], g.Data2)
		le.PutUint16(ext[12:], g.Data3)
		copy(ext[14:], g.Data4[:])
		b = append(b, ext...)
	}
	return b
}

func TestMixFormatsParse(t *testing.T) {
	f, err := parseWaveFormat(waveFormatEx(0xFFFE, 2, 48_000, 32, "{00000003-0000-0010-8000-00aa00389b71}"))
	if err != nil || !f.float || f.channels != 2 || f.rate != 48_000 {
		t.Fatalf("%+v %v", f, err)
	}
	f, err = parseWaveFormat(waveFormatEx(1, 1, 44_100, 16, ""))
	if err != nil || f.float || f.bits != 16 {
		t.Fatalf("%+v %v", f, err)
	}
	if _, err := parseWaveFormat(waveFormatEx(0xFFFE, 2, 48_000, 32, "{00000092-0000-0010-8000-00aa00389b71}")); err == nil {
		t.Fatal("AC3 parsed")
	}
	if _, err := parseWaveFormat([]byte{1, 2}); err == nil {
		t.Fatal("short parsed")
	}
	pcm24 := sampleFormat{channels: 1, rate: 16_000, bits: 24}
	got := pcm24.toFloat([]byte{0, 0, 0x40, 0, 0, 0xC0}, nil)
	if !reflect.DeepEqual(got, []float32{0.5, -0.5}) {
		t.Fatalf("%v", got)
	}
}

func TestThePipelineTurns48kStereoInto20msMonoChunks(t *testing.T) {
	f := sampleFormat{channels: 2, rate: 48_000, float: true, bits: 32}
	p := newPipeline(f)
	var chunks [][]float32
	one := sine(440, 48_000, 1)
	raw := make([]byte, 0, len(one)*8)
	for _, s := range one {
		raw = binary.LittleEndian.AppendUint32(raw, math.Float32bits(s))
		raw = binary.LittleEndian.AppendUint32(raw, math.Float32bits(s))
	}
	for len(raw) > 0 {
		n := min(4800, len(raw))
		p.push(raw[:n], func(s []float32) { chunks = append(chunks, s) })
		raw = raw[n:]
	}
	if len(chunks) < 49 || len(chunks) > 50 {
		t.Fatalf("%d chunks", len(chunks))
	}
	for _, c := range chunks {
		if len(c) != frameSamples {
			t.Fatalf("a chunk of %d samples", len(c))
		}
	}
	if c := chunkOf(chunks[25]); c.RMS < 0.3 || c.RMS > 0.4 || c.Time.IsZero() {
		t.Fatalf("rms %v", c.RMS)
	}
	if downmixed := downmix([]float32{1, 0, -1, -1}, 2, nil); !reflect.DeepEqual(downmixed, []float32{0.5, -1}) {
		t.Fatalf("%v", downmixed)
	}
}

// ---------------------------------------------------------------------------
// Inserter
// ---------------------------------------------------------------------------

func TestInputMatchesTheWin64Layout(t *testing.T) {
	if s := unsafe.Sizeof(input{}); s != 40 {
		t.Fatalf("INPUT is %d bytes, Windows wants 40", s)
	}
	if o := unsafe.Offsetof(input{}.Ki); o != 8 {
		t.Fatalf("the union is at %d", o)
	}
	if s := unsafe.Sizeof(kbdLLHookStruct{}); s != 24 {
		t.Fatalf("KBDLLHOOKSTRUCT is %d bytes", s)
	}
}

func TestChunksNeverSplitASurrogatePair(t *testing.T) {
	text := strings.Repeat("a😀", 100)
	chunks := utf16Chunks(text, 3)
	var all []uint16
	for _, c := range chunks {
		if len(c) > 3 || utf16.IsSurrogate(rune(c[len(c)-1])) && c[len(c)-1] < 0xDC00 {
			t.Fatalf("bad chunk %v", c)
		}
		all = append(all, c...)
	}
	if string(utf16.Decode(all)) != text {
		t.Fatal("text lost")
	}
}

func TestTypingUsesUnicodeEventsAndReturnForLineBreaks(t *testing.T) {
	in := textInputs([]uint16{'h', '\n', 0xE9})
	if len(in) != 6 || in[0].Ki.Flags&keyEventFUnicode == 0 || in[0].Ki.Scan != 'h' || in[2].Ki.Vk != vkReturn || in[3].Ki.Flags&keyEventFKeyUp == 0 || in[4].Ki.Scan != 0xE9 {
		t.Fatalf("%+v", in)
	}
	for _, i := range in {
		if i.Ki.ExtraInfo != saysoInputTag {
			t.Fatal("untagged input")
		}
	}
}

func TestPasteIsCtrlVOrShiftInsertForTerminals(t *testing.T) {
	first, second := pasteInputs([]uint16{vkLMenu}, false)
	vks := func(in []input) (out []uint16) {
		for _, i := range in {
			out = append(out, i.Ki.Vk)
		}
		return out
	}
	if !reflect.DeepEqual(vks(first), []uint16{vkLControl, vkLMenu, vkV}) || first[1].Ki.Flags&keyEventFKeyUp == 0 || !reflect.DeepEqual(vks(second), []uint16{vkV, vkLControl}) {
		t.Fatalf("%v %v", vks(first), vks(second))
	}
	first, _ = pasteInputs(nil, true)
	if !reflect.DeepEqual(vks(first), []uint16{vkLShift, vkInsert}) || first[1].Ki.Flags&keyEventFExtended == 0 {
		t.Fatalf("%v", vks(first))
	}
	if !isTerminalClass("mintty") || isTerminalClass("Chrome_WidgetWin_1") || isTerminalClass("CASCADIA_HOSTING_WINDOW_CLASS") {
		t.Fatal("terminal classes")
	}
}

func TestOnlyMemoryFormatsAreSaved(t *testing.T) {
	for f, want := range map[uint32]bool{cfUnicodeText: true, 1: true, 8: true, 0xC100: true, 2: false, 3: false, 9: false, 14: false, 0x80: false, 0x8E: false, 0x200: false, 0x3FF: false, 0: false} {
		if isMemoryFormat(f) != want {
			t.Errorf("format %#x", f)
		}
	}
}

// The real clipboard belongs to the user: the test saves every format first
// and puts all of it back.
func TestTheClipboardRoundTripsAndIsRestored(t *testing.T) {
	saved, err := snapshotClipboard()
	if err != nil {
		t.Skipf("the clipboard is busy: %v", err)
	}
	defer func() {
		if !restoreClipboard(saved) {
			t.Error("the clipboard was not restored")
		}
	}()
	var w windowsInserter
	text := "Sayso clipboard test ✓ 😀"
	if err := w.WriteClipboard(text); err != nil {
		t.Fatal(err)
	}
	if got, err := w.ReadClipboard(); err != nil || got != text {
		t.Fatalf("%q %v", got, err)
	}
}

// ---------------------------------------------------------------------------
// Context, permissions, login item, sounds
// ---------------------------------------------------------------------------

func TestBrowserAddressesBecomeHosts(t *testing.T) {
	for in, want := range map[string]string{
		"https://www.Example.com/a?b": "example.com", "mail.google.com/mail": "mail.google.com",
		"http://user@host.io:8080/x": "host.io", "chrome://settings": "", "localhost:3000": "", "no spaces": "",
	} {
		if got, _ := hostOf(in); got != want {
			t.Errorf("%q: %q, want %q", in, got, want)
		}
	}
	if _, ok := cleanAddress("  search words "); ok {
		t.Error("a search is not an address")
	}
	if a, ok := cleanAddress(" github.com/watzon "); !ok || a != "github.com/watzon" {
		t.Error("address")
	}
	if k, ok := browserKindOf("MSEDGE.exe"); !ok || k != browserChromium {
		t.Error("edge")
	}
	if k, ok := browserKindOf("firefox.exe"); !ok || k != browserFirefox {
		t.Error("firefox")
	}
	for _, other := range []string{"code.exe", "chrome", "notchrome.exe", ""} {
		if _, ok := browserKindOf(other); ok {
			t.Errorf("%q is a browser", other)
		}
	}
	if stepFor(browserChromium, uiaEdit, "") != walkTake || stepFor(browserFirefox, uiaEdit, "x") != walkSkip ||
		stepFor(browserFirefox, uiaEdit, "urlbar-input") != walkTake || stepFor(browserChromium, uiaDocument, "") != walkSkip || stepFor(browserChromium, 50033, "") != walkDescend {
		t.Error("walk steps")
	}
	if appID(`C:\Program Files\Google\Chrome.EXE`) != "chrome.exe" || withoutExe("Notepad.exe ") != "Notepad" {
		t.Error("names")
	}
}

func TestTheAppInFrontIsReadable(t *testing.T) {
	app, err := windowsContext{}.ActiveApp()
	if err != nil {
		t.Skipf("no window in front (a service session): %v", err)
	}
	if !strings.HasSuffix(app.BundleID, ".exe") || app.Name == "" {
		t.Fatalf("%+v", app)
	}
	t.Logf("in front: %+v", app)
}

func TestPrivacySwitchesAndLoginState(t *testing.T) {
	if !micAllowed(nil) || !micAllowed([]string{"Allow", "allow"}) || micAllowed([]string{"Allow", " Deny "}) {
		t.Fatal("microphone switches")
	}
	s := windowsPermissions{}.Status()
	if !s.Accessibility || !s.InputMonitoring || s.NeedsInputPerms {
		t.Fatalf("%+v", s)
	}
	if !loginEnabled(`"C:\Sayso.exe"`, true, nil) || loginEnabled(" ", true, nil) || loginEnabled("x", false, nil) ||
		loginEnabled("x", true, []byte{3, 0}) || !loginEnabled("x", true, []byte{2, 0}) {
		t.Fatal("login state")
	}
	if runCommand(`C:\A B\Sayso.exe`) != `"C:\A B\Sayso.exe"` {
		t.Fatal("run command")
	}
	// Read only: a real Sayso install may own the Run value.
	if _, err := (windowsLoginItem{}).Enabled(); err != nil {
		t.Fatal(err)
	}
}

func tinyWAV(samples []int16) []byte {
	var data []byte
	for _, s := range samples {
		data = binary.LittleEndian.AppendUint16(data, uint16(s))
	}
	le := binary.LittleEndian
	b := []byte("RIFF")
	b = le.AppendUint32(b, uint32(36+len(data)))
	b = append(b, "WAVEfmt "...)
	b = le.AppendUint32(b, 16)
	b = le.AppendUint16(b, 1)
	b = le.AppendUint16(b, 1)
	b = le.AppendUint32(b, 16_000)
	b = le.AppendUint32(b, 32_000)
	b = le.AppendUint16(b, 2)
	b = le.AppendUint16(b, 16)
	b = append(b, "data"...)
	b = le.AppendUint32(b, uint32(len(data)))
	return append(b, data...)
}

func TestVolumeScalesPCMSamples(t *testing.T) {
	in := tinyWAV([]int16{1000, -2000})
	out := scaleWAV(in, 0.5)
	if v := int16(binary.LittleEndian.Uint16(out[44:])); v != 500 {
		t.Fatalf("%d", v)
	}
	if v := int16(binary.LittleEndian.Uint16(out[46:])); v != -1000 {
		t.Fatalf("%d", v)
	}
	if int16(binary.LittleEndian.Uint16(in[44:])) != 1000 {
		t.Fatal("the input changed")
	}
	if got := scaleWAV([]byte("not a wav"), 0.5); string(got) != "not a wav" {
		t.Fatal("garbage changed")
	}
	// Silent: volume 0 of a 10 ms cue.
	if err := (&windowsSounds{}).Play(CueStart, tinyWAV(make([]int16, 160)), 0); err != nil {
		t.Logf("no audio output here: %v", err)
	}
}

// ---------------------------------------------------------------------------
// Native services
// ---------------------------------------------------------------------------

func TestTheKeyboardHookStartsAndStops(t *testing.T) {
	h := newWindowsHotkeys()
	if err := h.Configure(HotkeyConfig{Toggle: "ctrl+alt+shift+f19", PushToTalk: "ctrl+alt+shift+f20"}); err != nil {
		t.Fatal(err)
	}
	var mu sync.Mutex
	var got []HotkeyEvent
	if err := h.Start(func(e HotkeyEvent) { mu.Lock(); got = append(got, e); mu.Unlock() }); err != nil {
		t.Fatal(err)
	}
	if err := h.Start(func(HotkeyEvent) {}); err == nil {
		t.Fatal("a second start")
	}
	h.SetRecording(true)
	time.Sleep(50 * time.Millisecond)
	if err := h.Stop(); err != nil {
		t.Fatal(err)
	}
	if err := h.Stop(); err != nil {
		t.Fatal("a second stop")
	}
	// A new listener can start after it.
	if err := h.Start(func(HotkeyEvent) {}); err != nil {
		t.Fatal(err)
	}
	_ = h.Stop()
}

func TestServicesAreComplete(t *testing.T) {
	s, err := New()
	if err != nil {
		t.Fatal(err)
	}
	v := reflect.ValueOf(*s)
	for i := 0; i < v.NumField(); i++ {
		if v.Field(i).IsNil() {
			t.Errorf("%s is nil", v.Type().Field(i).Name)
		}
	}
}

func TestMicrophonesAreListed(t *testing.T) {
	devices, err := (&windowsAudio{}).ListDevices()
	if err != nil {
		t.Fatal(err)
	}
	defaults := 0
	for _, d := range devices {
		if d.ID == "" {
			t.Errorf("a device without an id: %+v", d)
		}
		if d.IsDefault {
			defaults++
		}
	}
	if defaults > 1 {
		t.Fatalf("%d defaults", defaults)
	}
	t.Logf("%d microphones", len(devices))
}

// The capture opens the real microphones, so it runs only when asked:
// SAYSO_TEST_MIC=1. A virtual microphone (Parsec) can give no audio at all
// while nothing streams to it, so one working device is enough.
func TestTheMicrophoneGives16kHzChunks(t *testing.T) {
	if os.Getenv("SAYSO_TEST_MIC") != "1" {
		t.Skip("set SAYSO_TEST_MIC=1 to open the microphone")
	}
	a := &windowsAudio{}
	devices, err := a.ListDevices()
	if err != nil || len(devices) == 0 {
		t.Skipf("no microphone: %v", err)
	}
	for _, d := range devices {
		var mu sync.Mutex
		var chunks []AudioChunk
		if err := a.Start(context.Background(), d.ID, func(c AudioChunk) { mu.Lock(); chunks = append(chunks, c); mu.Unlock() }); err != nil {
			t.Logf("%s: %v", d.Name, err)
			continue
		}
		time.Sleep(600 * time.Millisecond)
		if err := a.Stop(); err != nil {
			t.Fatal(err)
		}
		mu.Lock()
		got := chunks
		mu.Unlock()
		for _, c := range got {
			if len(c.Samples) != frameSamples || c.RMS < 0 || c.RMS > 1 {
				t.Fatalf("%s: %d samples, rms %v", d.Name, len(c.Samples), c.RMS)
			}
		}
		t.Logf("%s: %d chunks in 600 ms", d.Name, len(got))
		if len(got) >= 20 {
			return
		}
	}
	t.Fatal("no microphone gave audio")
}

// Read only: the media sessions and their state are listed, nothing pauses.
func TestMediaSessionsCanBeRead(t *testing.T) {
	m := &windowsMedia{}
	err := m.run(func() error {
		sessions, err := gsmtcSessions()
		if err != nil {
			return err
		}
		for _, s := range sessions {
			st, can := sessionState(s)
			t.Logf("session: status %d, can pause %v", st, can)
			s.release()
		}
		t.Logf("%d media sessions", len(sessions))
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	// Resume with nothing paused does nothing.
	if err := m.Resume(); err != nil {
		t.Fatal(err)
	}
}
