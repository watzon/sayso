package lexicon

import (
	"bytes"
	"errors"
	"math"
	"os"
	"path/filepath"
	"testing"
)

// small builds a model of a few words with known n-grams.
func small(t testing.TB) []byte {
	t.Helper()
	b := &Builder{Locale: "en-US", BOSBow: 0.5}
	add := func(w string, cost float64, flags uint8) int {
		b.Entries = append(b.Entries, Entry{Display: w, Cost: cost, Flags: flags, Bow: 0.4})
		return len(b.Entries) - 1
	}
	the := add("the", 2, 0)
	cat := add("cat", 6, 0)
	car := add("car", 5.5, 0)
	cart := add("cart", 8, 0)
	dont := add("don't", 5, FlagContraction)
	add("dont", 12, FlagUnseen|FlagNoAutocorrect)
	add("London", 7, FlagProper)
	add("café", 9, 0)
	sat := add("sat", 7, 0)
	b.Bigrams = []Bigram{{the, cat, 1.5}, {the, car, 2.0}, {cat, sat, 1.0}, {BOSIndex, the, 1.2}, {BOSIndex, dont, 3}}
	b.Trigrams = []Trigram{{BOSIndex, the, cat, 0.7}}
	b.TriBow = map[[2]int]float64{{BOSIndex, the}: 0.3}
	_ = cart
	var buf bytes.Buffer
	if err := b.Write(&buf); err != nil {
		t.Fatal(err)
	}
	return buf.Bytes()
}

func near(a, b float32) bool { return math.Abs(float64(a-b)) < 0.1 }

func TestRoundTrip(t *testing.T) {
	m, err := FromBytes(small(t))
	if err != nil {
		t.Fatal(err)
	}
	if m.WordCount() != 9 || m.Locale() != "en-US" {
		t.Fatal(m.WordCount(), m.Locale())
	}
	// Key-mates come most probable first.
	lo, n := m.Lookup([]byte("dont"))
	if n != 2 || string(m.Word(lo)) != "don't" || string(m.Word(lo+1)) != "dont" {
		t.Fatalf("dont: %d %q", n, m.Word(lo))
	}
	if id, ok := m.Find([]byte("london"), true); !ok || string(m.Word(id)) != "London" || m.Flags(id)&FlagProper == 0 {
		t.Fatal("London")
	}
	if _, ok := m.Find([]byte("london"), false); ok {
		t.Fatal("exact find ignored case")
	}
	if id, ok := m.Find([]byte("café"), false); !ok || !near(m.Unigram(id), 9) {
		t.Fatal("café")
	}
	// The trie: "ca" has children r and t; "car" is a word and has child t.
	ca, ok := m.Walk([]byte("ca"))
	if !ok || m.Node(ca).ChildCount != 3 { // f (café), r, t
		t.Fatalf("ca children %d", m.Node(ca).ChildCount)
	}
	if string(m.Word(m.BestWord(ca))) != "car" {
		t.Fatalf("best under ca: %q", m.Word(m.BestWord(ca)))
	}
	car, _ := m.Walk([]byte("car"))
	if m.Node(car).TermCount != 1 || m.Node(car).ChildCount != 1 {
		t.Fatal("car node")
	}

	id := func(w string) WordID { i, _ := m.Find([]byte(w), false); return i }
	// Trigram, bigram, and backoff costs.
	c := m.NewContext(m.BOS(), id("the"))
	if got := m.Cost(&c, id("cat")); !near(got, 0.7) {
		t.Fatalf("trigram cost %v", got)
	}
	if got := m.Cost(&c, id("car")); !near(got, 0.3+2.0) {
		t.Fatalf("bigram cost with trigram backoff %v", got)
	}
	if got := m.Cost(&c, id("sat")); !near(got, 0.3+0.4+7) {
		t.Fatalf("unigram backoff %v", got)
	}
	c = m.NewContext(NoWord, id("cat"))
	if got := m.Cost(&c, id("sat")); !near(got, 1.0) {
		t.Fatalf("bigram %v", got)
	}
	c = m.NewContext(NoWord, NoWord)
	if got := m.Cost(&c, id("the")); !near(got, 2) {
		t.Fatalf("unigram %v", got)
	}
	var succ []string
	c = m.NewContext(m.BOS(), id("the"))
	m.Successors(&c, func(w WordID, _ float32) bool { succ = append(succ, string(m.Word(w))); return true })
	if len(succ) != 3 || succ[0] != "cat" {
		t.Fatal(succ)
	}
}

func TestLookupsDoNotAllocate(t *testing.T) {
	m, err := FromBytes(small(t))
	if err != nil {
		t.Fatal(err)
	}
	key := []byte("cart")
	word := []byte("London")
	allocs := testing.AllocsPerRun(1000, func() {
		m.Lookup(key)
		id, _ := m.Find(word, true)
		c := m.NewContext(m.BOS(), id)
		m.Cost(&c, id)
		_ = m.Word(id)
	})
	if allocs != 0 {
		t.Fatalf("%v allocs per lookup", allocs)
	}
}

func TestRejectsDamage(t *testing.T) {
	good := small(t)
	cases := map[string]func(b []byte) []byte{
		"magic":     func(b []byte) []byte { b[0] = 'X'; return b },
		"version":   func(b []byte) []byte { b[4] = 9; return b },
		"checksum":  func(b []byte) []byte { b[len(b)-20] ^= 0xff; return b },
		"truncated": func(b []byte) []byte { return b[:len(b)-8] },
		"empty":     func(b []byte) []byte { return b[:10] },
	}
	for name, damage := range cases {
		b := damage(append([]byte(nil), good...))
		if _, err := FromBytes(b); !errors.Is(err, ErrFormat) {
			t.Errorf("%s: %v", name, err)
		}
	}
}

func TestMisalignedAndFD(t *testing.T) {
	good := small(t)
	// A misaligned slice is copied, not cast.
	buf := make([]byte, len(good)+1)
	copy(buf[1:], good)
	if _, err := FromBytes(buf[1:]); err != nil {
		t.Fatal(err)
	}
	// OpenFD at an odd offset, as for an asset inside an APK.
	path := filepath.Join(t.TempDir(), "pack.bin")
	prefix := bytes.Repeat([]byte{7}, 4100)
	if err := os.WriteFile(path, append(prefix, good...), 0o644); err != nil {
		t.Fatal(err)
	}
	f, err := os.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	m, err := OpenFD(int(f.Fd()), int64(len(prefix)), int64(len(good)))
	if err != nil {
		t.Fatal(err)
	}
	if lo, n := m.Lookup([]byte("cat")); n != 1 || string(m.Word(lo)) != "cat" {
		t.Fatal("lookup through fd")
	}
	if err := m.Close(); err != nil {
		t.Fatal(err)
	}
}

func TestBuilderRejectsBadWords(t *testing.T) {
	for _, w := range []string{"", "a-b", "x1", "naïve", "1"} {
		b := &Builder{Entries: []Entry{{Display: w}}}
		err := b.Write(&bytes.Buffer{})
		if w == "naïve" {
			if err != nil {
				t.Errorf("%q: %v", w, err)
			}
			continue
		}
		if err == nil {
			t.Errorf("%q accepted", w)
		}
	}
	b := &Builder{Entries: []Entry{{Display: "a"}, {Display: "a"}}}
	if b.Write(&bytes.Buffer{}) == nil {
		t.Error("duplicate accepted")
	}
}

func TestAppendKey(t *testing.T) {
	cases := map[string]string{"Don't": "dont", "café": "cafe", "I’m": "im", "NASA": "nasa"}
	for in, want := range cases {
		if got, ok := KeyOf(in); !ok || got != want {
			t.Errorf("%q: %q %v", in, got, ok)
		}
	}
	for _, bad := range []string{"", "'", "a b", "x2"} {
		if _, ok := KeyOf(bad); ok {
			t.Errorf("%q accepted", bad)
		}
	}
}
