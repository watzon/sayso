package learner

import (
	"math"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestRevertsSuppress(t *testing.T) {
	l := New()
	if l.Reverted("Tim", "Tom") {
		t.Fatal("one revert should not suppress")
	}
	if !l.Reverted("tim", "tom") || !l.Suppressed("TIM", "Tom") {
		t.Fatal("two reverts should suppress")
	}
	if l.Suppressed("tim", "time") {
		t.Fatal("only that correction")
	}
	if _, ok := l.PersonalWords()["tim"]; !ok {
		t.Fatal("a twice-reverted word is kept twice, so it is learned")
	}
}

func TestFixesPromote(t *testing.T) {
	l := New()
	l.ManualFix("teh", "the")
	if _, ok := l.Promoted("teh"); ok {
		t.Fatal("one fix should not promote")
	}
	l.ManualFix("Teh", "the")
	if got, ok := l.Promoted("teh"); !ok || got != "the" {
		t.Fatal("two fixes should promote", got)
	}
	l.ManualFix("same", "Same") // a case change is not a fix
	if len(l.fixes) != 1 {
		t.Fatal(l.fixes)
	}
	// Reverting the promoted correction twice takes it back.
	l.Reverted("teh", "the")
	l.Reverted("teh", "the")
	if _, ok := l.Promoted("teh"); ok {
		t.Fatal("a suppressed correction should not stay promoted")
	}
}

func TestPrivacyLearnsAndWritesNothing(t *testing.T) {
	path := filepath.Join(t.TempDir(), "j")
	l := New()
	if err := l.Open(path); err != nil {
		t.Fatal(err)
	}
	l.AddWord("Sayso")
	before, _ := os.ReadFile(path)
	l.SetPrivacy(true)
	l.AddWord("secret")
	l.Kept("hunter2x")
	l.Kept("hunter2x")
	l.Reverted("a", "b")
	l.Reverted("a", "b")
	l.ManualFix("x", "y")
	l.ManualFix("x", "y")
	l.Committed("my", "password")
	l.Confirm([]Sample{{Key: 1, Row: 0, DX: 0.2, DY: 0.2}})
	after, _ := os.ReadFile(path)
	if string(before) != string(after) {
		t.Fatalf("privacy mode wrote:\n%s", after[len(before):])
	}
	if l.Suppressed("a", "b") || l.words["secret"] || l.Bigram("my", "password") != 0 || l.KeySamples(1) != 0 {
		t.Fatal("privacy mode learned")
	}
	if !l.words["Sayso"] {
		t.Fatal("what was learned before is still used")
	}
}

func TestJournalRoundTrip(t *testing.T) {
	path := filepath.Join(t.TempDir(), "j")
	clock := time.Unix(1_800_000_000, 0)
	l := New()
	l.SetClock(func() time.Time { return clock })
	if err := l.Open(path); err != nil {
		t.Fatal(err)
	}
	l.AddWord("Sayso")
	l.AddWord("tab\tword")
	l.AddReplacement(Replacement{From: "omw", To: "on my way"})
	l.Reverted("Tim", "Tom")
	l.Reverted("Tim", "Tom")
	l.ManualFix("teh", "the")
	l.ManualFix("teh", "the")
	l.Committed("good", "morning")
	l.Kept("Zorbl")
	l.Kept("Zorbl")
	for range snapshotEvery {
		l.Confirm([]Sample{{Key: 3, Row: 0, DX: 0.1, DY: 0.15}})
	}
	l.Close()

	m := New()
	m.SetClock(func() time.Time { return clock })
	if err := m.Open(path); err != nil {
		t.Fatal(err)
	}
	if !m.words["Sayso"] || !m.words["tab\tword"] || !m.Suppressed("tim", "tom") {
		t.Fatal("words or suppression lost")
	}
	if r, ok := m.Replacement("OMW"); !ok || r.To != "on my way" {
		t.Fatal("replacement lost")
	}
	if p, _ := m.Promoted("teh"); p != "the" {
		t.Fatal("promotion lost")
	}
	if m.Bigram("good", "morning") != 1 || m.PersonalWords()["Zorbl"] != 2 {
		t.Fatal("bigram or learned word lost")
	}
	m.SetRows([]int{0, 0, 0, 0})
	x1, y1 := l.KeyOffset(3)
	x2, y2 := m.KeyOffset(3)
	if math.Abs(float64(x1-x2)) > 1e-3 || math.Abs(float64(y1-y2)) > 1e-3 || x2 == 0 {
		t.Fatalf("offsets: %v,%v vs %v,%v", x1, y1, x2, y2)
	}

	// Reset empties the journal.
	if err := m.Reset(); err != nil {
		t.Fatal(err)
	}
	m.Close()
	n := New()
	if err := n.Open(path); err != nil {
		t.Fatal(err)
	}
	if len(n.words) != 0 || len(n.suppressed) != 0 || n.KeySamples(3) != 0 {
		t.Fatal("reset did not stick")
	}
}

func TestTornLastLineIsSkipped(t *testing.T) {
	path := filepath.Join(t.TempDir(), "j")
	if err := os.WriteFile(path, []byte("V\t1\nW\tgood\nW\ttor"), 0o600); err != nil {
		t.Fatal(err)
	}
	l := New()
	if err := l.Open(path); err != nil {
		t.Fatal(err)
	}
	if !l.words["good"] || l.words["tor"] {
		t.Fatal(l.words)
	}
}

func TestBigramsFade(t *testing.T) {
	now := time.Unix(1_800_000_000, 0)
	l := New()
	l.SetClock(func() time.Time { return now })
	l.Committed("good", "night")
	l.Committed("good", "night")
	now = now.Add(ngramHalfLife)
	if c := l.Bigram("good", "night"); math.Abs(c-1) > 1e-6 {
		t.Fatalf("after one half-life: %v", c)
	}
}

func TestOffsetsAdaptWithinBounds(t *testing.T) {
	l := New()
	l.SetRows([]int{0, 0, 1})
	prevX := float32(0)
	for i := range 300 {
		l.Confirm([]Sample{{Key: 0, Row: 0, DX: 0.2, DY: -0.1}, {Key: 1, Row: 0, DX: 0.6, DY: 0.6}})
		x, _ := l.KeyOffset(0)
		if d := x - prevX; d > MaxStep+1e-6 || d < -MaxStep-1e-6 {
			t.Fatalf("tap %d moved the offset by %v", i, d)
		}
		prevX = x
	}
	x, y := l.KeyOffset(0)
	if math.Abs(float64(x)-0.2) > 0.12 || math.Abs(float64(y)+0.1) > 0.12 {
		t.Fatalf("key 0 offset %v,%v, want near 0.2,-0.1", x, y)
	}
	x, y = l.KeyOffset(1)
	if x > MaxDrift+1e-6 || y > MaxDrift+1e-6 {
		t.Fatalf("drift cap broken: %v,%v", x, y)
	}
	// Key 2 has no taps of its own; it leans on the keyboard estimate.
	x, _ = l.KeyOffset(2)
	if x <= 0 {
		t.Fatal("an untaught key should take the keyboard's lean")
	}
	// Outliers teach nothing.
	n := l.KeySamples(0)
	l.Confirm([]Sample{{Key: 0, Row: 0, DX: 3, DY: 0}})
	if l.KeySamples(0) != n {
		t.Fatal("outlier counted")
	}
}

func TestCompaction(t *testing.T) {
	path := filepath.Join(t.TempDir(), "j")
	l := New()
	if err := l.Open(path); err != nil {
		t.Fatal(err)
	}
	for i := 0; l.j.size <= compactAt; i++ {
		l.Committed("the", "end")
		l.AddWord("w" + strings.Repeat("x", i%50))
	}
	l.Close()
	m := New()
	if err := m.Open(path); err != nil {
		t.Fatal(err)
	}
	st, _ := os.Stat(path)
	if st.Size() >= compactAt/4 {
		t.Fatalf("not compacted: %d bytes", st.Size())
	}
	if !m.words["w"+strings.Repeat("x", 49)] || m.Bigram("the", "end") < 1 {
		t.Fatal("compaction lost state")
	}
}

func TestFollowing(t *testing.T) {
	l := New()
	l.Committed("Good", "night")
	l.Committed("good", "morning")
	l.Committed("bad", "night")
	got := map[string]float64{}
	l.Following("GOOD", func(w string, c float64) { got[w] = c })
	if len(got) != 2 || got["night"] != 1 || got["morning"] != 1 {
		t.Fatal(got)
	}
}

func TestAddReplacementRefusesWhatItCannotKeep(t *testing.T) {
	l := New()
	if err := l.AddReplacement(Replacement{From: "sig", To: strings.Repeat("x", maxReplacementLen)}); err != nil {
		t.Fatalf("512 bytes fit: %v", err)
	}
	for _, r := range []Replacement{
		{From: "sig2", To: strings.Repeat("x", maxReplacementLen+1)},
		{From: strings.Repeat("a", maxWordLen+1), To: "x"},
		{From: "  ", To: "x"},
	} {
		if err := l.AddReplacement(r); err == nil {
			t.Errorf("%d-byte shortcut, %d-byte text: accepted", len(r.From), len(r.To))
		}
	}
	if got := len(l.Replacements()); got != 1 {
		t.Fatalf("%d replacements kept, want 1", got)
	}
}
