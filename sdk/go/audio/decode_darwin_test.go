package audio

import (
	"encoding/binary"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
)

func TestAnM4AFileReadsWithExtAudioFile(t *testing.T) {
	afconvert, err := exec.LookPath("afconvert")
	if err != nil {
		t.Skip("no afconvert")
	}
	dir := t.TempDir()
	wav := filepath.Join(dir, "tone.wav")
	b := tone(44100, 2, 2, 0.3)
	binary.LittleEndian.PutUint32(b[4:], uint32(len(b)-8))
	if err := os.WriteFile(wav, b, 0o600); err != nil {
		t.Fatal(err)
	}
	m4a := filepath.Join(dir, "tone.m4a")
	if out, err := exec.Command(afconvert, "-f", "m4af", "-d", "aac", wav, m4a).CombinedOutput(); err != nil {
		t.Fatal(string(out), err)
	}
	a, err := ReadFile(m4a)
	if err != nil {
		t.Fatal(err)
	}
	if a.LenMs() < 1950 || a.LenMs() > 2100 {
		t.Fatal(a.LenMs())
	}
	var peak float32
	for _, s := range a.AllFloats() {
		peak = max(peak, s)
	}
	if peak < 0.2 {
		t.Fatal("no tone", peak)
	}
}
