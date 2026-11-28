package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/watzon/sayso/sdk/go/audio"
	"github.com/watzon/sayso/sdk/go/protocol"
)

type line struct {
	V        int              `json:"v"`
	Type     string           `json:"type"`
	TMs      uint64           `json:"t_ms"`
	PCM      string           `json:"pcm"`
	Ms       uint64           `json:"ms"`
	Code     string           `json:"code"`
	State    string           `json:"state"`
	Excludes []string         `json:"excludes"`
	Devices  []map[string]any `json:"devices"`
}

func lines(t *testing.T, b []byte) []line {
	t.Helper()
	var out []line
	for _, l := range strings.Split(strings.TrimSpace(string(b)), "\n") {
		var m line
		if err := json.Unmarshal([]byte(l), &m); err != nil || m.V != 1 || m.Type == "" {
			t.Fatalf("not a message: %q", l)
		}
		out = append(out, m)
	}
	return out
}

func runCmd(t *testing.T, stdin io.Reader, args ...string) (int, []line) {
	t.Helper()
	var buf bytes.Buffer
	o := &out{w: bufio.NewWriter(&buf)}
	code := run(args, stdin, o)
	return code, lines(t, buf.Bytes())
}

func speechWAV(t *testing.T, ms int) string {
	t.Helper()
	samples := make([]int16, ms*16)
	for i := range samples {
		if (i/1600)%2 == 0 {
			samples[i] = 8000
		}
	}
	path := filepath.Join(t.TempDir(), "speech.wav")
	if err := audio.WriteWAV(path, samples); err != nil {
		t.Fatal(err)
	}
	return path
}

func TestDecodeGivesFramesOf20msThenTheLength(t *testing.T) {
	code, got := runCmd(t, nil, "decode", speechWAV(t, 1010))
	if code != 0 {
		t.Fatal(code, got)
	}
	last := got[len(got)-1]
	if last.Type != "done" || last.Ms != 1010 {
		t.Fatalf("%+v", last)
	}
	var samples int
	for i, l := range got[:len(got)-1] {
		pcm, err := protocol.DecodeI16(l.PCM)
		if err != nil || l.Type != "audio" || l.TMs != uint64(i*20) {
			t.Fatalf("%d: %+v %v", i, l, err)
		}
		if i < len(got)-2 && len(pcm) != frame {
			t.Fatal(len(pcm))
		}
		samples += len(pcm)
	}
	if samples != 1010*16 {
		t.Fatal(samples)
	}
}

func TestErrorsHaveTheirCodes(t *testing.T) {
	text := filepath.Join(t.TempDir(), "notes.txt")
	os.WriteFile(text, []byte("hello"), 0o600)
	for _, c := range []struct {
		args []string
		code string
	}{
		{[]string{}, "bad_request"},
		{[]string{"play"}, "bad_request"},
		{[]string{"decode"}, "bad_request"},
		{[]string{"decode", text}, "failed"},
		{[]string{"capture", "--source", "speakers"}, "bad_request"},
		{[]string{"permission", "camera"}, "bad_request"},
	} {
		code, got := runCmd(t, nil, c.args...)
		if code != 1 || len(got) != 1 || got[0].Type != "error" || got[0].Code != c.code {
			t.Errorf("%v: %d %+v", c.args, code, got)
		}
	}
}

func TestAFakeDeviceCapturesUntilStdinCloses(t *testing.T) {
	t.Setenv("SAYSO_AUDIO_FAKE_WAV", speechWAV(t, 200))
	if _, got := runCmd(t, nil, "devices"); len(got[0].Devices) != 1 || got[0].Devices[0]["id"] != "fake" {
		t.Fatalf("%+v", got)
	}
	r, w := io.Pipe()
	time.AfterFunc(500*time.Millisecond, func() { w.Close() })
	code, got := runCmd(t, r, "capture", "--source", "system", "--exclude-pid", "42")
	if code != 0 || got[0].Type != "started" || strings.Join(got[0].Excludes, ",") != "sayso,app" {
		t.Fatalf("%d %+v", code, got[0])
	}
	audioLines := got[1:]
	if len(audioLines) < 10 {
		t.Fatal(len(audioLines))
	}
	first, _ := protocol.DecodeI16(audioLines[0].PCM)
	if first[0] != 8000 || audioLines[0].TMs != 0 || audioLines[1].TMs != 20 {
		t.Fatalf("%+v", audioLines[:2])
	}
	tail, _ := protocol.DecodeI16(audioLines[len(audioLines)-1].PCM)
	for _, s := range tail {
		if s != 0 {
			t.Fatal("after the file comes silence")
		}
	}
}

func TestAMicCaptureOfTheFakeDeviceExcludesNothing(t *testing.T) {
	t.Setenv("SAYSO_AUDIO_FAKE_WAV", speechWAV(t, 100))
	r, w := io.Pipe()
	time.AfterFunc(100*time.Millisecond, func() { w.Close() })
	code, got := runCmd(t, r, "capture", "--source", "mic")
	if code != 0 || got[0].Type != "started" || got[0].Excludes == nil || len(got[0].Excludes) != 0 {
		t.Fatalf("%d %+v", code, got)
	}
}

func TestPermissionGivesAState(t *testing.T) {
	_, got := runCmd(t, nil, "permission", "mic")
	if s := got[0].State; got[0].Type != "permission" || (s != "granted" && s != "denied" && s != "unknown") {
		t.Fatalf("%+v", got)
	}
}
