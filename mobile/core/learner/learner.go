// Package learner adapts the keyboard to its user: it undoes and then stops
// corrections the user rejects, learns the typos the user fixes by hand,
// keeps a personal lexicon and fading personal bigrams, and learns where
// the user's taps land on each key. Everything it learns goes to an
// append-only journal, unless privacy mode is on, in which case it learns
// and writes nothing.
package learner

import (
	"fmt"
	"math"
	"slices"
	"strconv"
	"strings"
	"time"
)

// Thresholds.
const (
	// RevertsToSuppress: after this many reverts of one correction, it is
	// never made again.
	RevertsToSuppress = 2
	// FixesToPromote: after this many manual fixes of one typo to one word,
	// the typo is corrected to it.
	FixesToPromote = 2
	// KeepsToLearn: a word the model lacks joins the personal lexicon after
	// it is kept this many times.
	KeepsToLearn = 2
	// ngramHalfLife is how fast personal bigrams fade.
	ngramHalfLife = 30 * 24 * time.Hour
	// maxNgrams bounds the personal bigrams; the weakest go first.
	maxNgrams = 20000
	// maxWordLen bounds stored words and replacement texts.
	maxWordLen = 128
	// snapshotEvery: offsets go to the journal every this many confirmed words.
	snapshotEvery = 25
)

type pair struct{ typed, target string }

// Replacement is a text replacement from Sayso's dictionary: typing From
// writes To.
type Replacement struct {
	From, To      string
	CaseSensitive bool
}

type kept struct {
	display string // the latest spelling
	n       int
}

type ngram struct {
	count float64
	last  int64 // unix seconds
}

// Learner is the learned state. It is not safe for concurrent use.
type Learner struct {
	privacy bool
	now     func() time.Time
	j       *journal
	err     error // the first journal write error

	words        map[string]bool  // explicit personal words
	keeps        map[string]*kept // kept spellings the model lacks, by lower case
	replacements map[string]Replacement
	reverts      map[pair]int
	suppressed   map[pair]bool
	fixes        map[pair]int
	promoted     map[string]string // typed (lower) → target
	ngrams       map[[2]string]*ngram
	following    map[string]map[string]*ngram // the same bigrams by first word
	offsets      Offsets
	applied      [MaxOffsetKeys][2]float32 // offsets in use, moved at most MaxStep per update
	rowOf        [MaxOffsetKeys]uint8      // each key's row in the current layout
	confirmed    int

	// Version counts changes of the personal lexicon, so the engine knows
	// when to rebuild its trie.
	Version int
}

// New makes an empty learner that keeps nothing on disk.
func New() *Learner {
	l := &Learner{now: time.Now}
	l.clear()
	return l
}

func (l *Learner) clear() {
	l.words = map[string]bool{}
	l.keeps = map[string]*kept{}
	l.replacements = map[string]Replacement{}
	l.reverts = map[pair]int{}
	l.suppressed = map[pair]bool{}
	l.fixes = map[pair]int{}
	l.promoted = map[string]string{}
	l.ngrams = map[[2]string]*ngram{}
	l.following = map[string]map[string]*ngram{}
	l.offsets = Offsets{}
	l.applied = [MaxOffsetKeys][2]float32{}
	l.confirmed = 0
	l.Version++
}

// SetClock replaces the clock (tests).
func (l *Learner) SetClock(now func() time.Time) { l.now = now }

// Open replays the journal at path and appends to it from then on. A
// journal that has grown large is compacted.
func (l *Learner) Open(path string) error {
	if l.j != nil {
		l.j.close()
		l.j = nil
	}
	l.clear()
	if path == "" {
		return nil
	}
	if err := readJournal(path, l.apply); err != nil {
		return err
	}
	l.jumpApplied()
	j, err := openJournal(path)
	if err != nil {
		return err
	}
	l.j = j
	if j.size > compactAt {
		return l.compact()
	}
	return nil
}

// Close closes the journal.
func (l *Learner) Close() error {
	if l.j == nil {
		return nil
	}
	err := l.j.close()
	l.j = nil
	return err
}

// Err is the first journal write error since Open, if any.
func (l *Learner) Err() error { return l.err }

// SetPrivacy turns privacy mode on or off. In privacy mode nothing is
// learned and nothing is written; what was learned before is still used.
func (l *Learner) SetPrivacy(on bool) { l.privacy = on }

// Privacy reports whether privacy mode is on.
func (l *Learner) Privacy() bool { return l.privacy }

// record applies a record and appends it to the journal. In privacy mode it
// does nothing.
func (l *Learner) record(fields ...string) {
	if l.privacy {
		return
	}
	for _, f := range fields {
		if len(f) > maxWordLen*4 {
			return
		}
	}
	l.apply(fields)
	if l.j != nil {
		if err := l.j.append(encodeRecord(fields...)); err != nil && l.err == nil {
			l.err = err
		}
	}
}

func norm(s string) string { return strings.ToLower(strings.TrimSpace(s)) }

// apply changes the state for one record; unknown or malformed records are
// ignored, so newer journals can be read.
func (l *Learner) apply(f []string) {
	arg := func(i int) string {
		if i < len(f) {
			return f[i]
		}
		return ""
	}
	switch f[0] {
	case "Z":
		v := l.Version
		l.clear()
		l.Version = v + 1
	case "W":
		if w := strings.TrimSpace(arg(1)); w != "" {
			l.words[w] = true
			l.Version++
		}
	case "w":
		w := strings.TrimSpace(arg(1))
		delete(l.words, w)
		delete(l.keeps, norm(w))
		l.Version++
	case "K":
		if w := strings.TrimSpace(arg(1)); w != "" {
			k := l.keeps[norm(w)]
			if k == nil {
				k = &kept{}
				l.keeps[norm(w)] = k
			}
			k.display = w
			k.n++
			if k.n == KeepsToLearn {
				l.Version++
			}
		}
	case "R":
		from := norm(arg(1))
		if from != "" {
			l.replacements[from] = Replacement{From: strings.TrimSpace(arg(1)), To: arg(2), CaseSensitive: arg(3) == "1"}
		}
	case "r":
		delete(l.replacements, norm(arg(1)))
	case "X":
		p := pair{norm(arg(1)), norm(arg(2))}
		l.reverts[p]++
		if l.reverts[p] >= RevertsToSuppress {
			l.suppressed[p] = true
			if l.promoted[p.typed] == p.target {
				delete(l.promoted, p.typed)
			}
		}
	case "F":
		p := pair{norm(arg(1)), norm(arg(2))}
		if p.typed == p.target || p.target == "" {
			return
		}
		l.fixes[p]++
		if l.fixes[p] >= FixesToPromote && !l.suppressed[p] {
			l.promoted[p.typed] = strings.TrimSpace(arg(2))
		}
	case "N":
		t, _ := strconv.ParseInt(arg(3), 10, 64)
		l.bump([2]string{norm(arg(1)), norm(arg(2))}, t)
	case "G":
		// G <w1> <w2> <count> <unix>: a bigram's faded count, from compaction.
		k := [2]string{norm(arg(1)), norm(arg(2))}
		c, _ := strconv.ParseFloat(arg(3), 64)
		t, _ := strconv.ParseInt(arg(4), 10, 64)
		if k[0] != "" && k[1] != "" && c > 0 && !math.IsInf(c, 0) {
			l.bump(k, t)
			l.ngrams[k].count = c
		}
	case "O":
		// O <level b|r|k> <index> <x> <y> <n>
		i, _ := strconv.Atoi(arg(2))
		x, _ := strconv.ParseFloat(arg(3), 64)
		y, _ := strconv.ParseFloat(arg(4), 64)
		n, _ := strconv.ParseFloat(arg(5), 64)
		e := estimate{X: clamp(x, MaxDrift), Y: clamp(y, MaxDrift), N: math.Max(0, n)}
		switch {
		case arg(1) == "b":
			l.offsets.Board = e
		case arg(1) == "r" && i >= 0 && i < MaxOffsetKeys:
			l.offsets.Rows[i] = e
		case arg(1) == "k" && i >= 0 && i < MaxOffsetKeys:
			l.offsets.Keys[i] = e
		}
	}
}

func (l *Learner) bump(k [2]string, t int64) {
	if k[0] == "" || k[1] == "" {
		return
	}
	g := l.ngrams[k]
	if g == nil {
		if len(l.ngrams) >= maxNgrams {
			l.evict(t)
		}
		g = &ngram{}
		l.ngrams[k] = g
		f := l.following[k[0]]
		if f == nil {
			f = map[string]*ngram{}
			l.following[k[0]] = f
		}
		f[k[1]] = g
	}
	g.count = decayed(g.count, g.last, t) + 1
	g.last = t
}

func decayed(count float64, last, now int64) float64 {
	if now <= last {
		return count
	}
	return count * math.Exp2(-float64(now-last)/ngramHalfLife.Seconds())
}

// evict drops the weakest tenth of the personal bigrams.
func (l *Learner) evict(now int64) {
	type kv struct {
		k [2]string
		v float64
	}
	all := make([]kv, 0, len(l.ngrams))
	for k, g := range l.ngrams {
		all = append(all, kv{k, decayed(g.count, g.last, now)})
	}
	slices.SortFunc(all, func(a, b kv) int {
		if a.v != b.v {
			if a.v < b.v {
				return -1
			}
			return 1
		}
		return strings.Compare(a.k[0]+"\x00"+a.k[1], b.k[0]+"\x00"+b.k[1])
	})
	for _, x := range all[:len(all)/10+1] {
		delete(l.ngrams, x.k)
		if f := l.following[x.k[0]]; f != nil {
			delete(f, x.k[1])
			if len(f) == 0 {
				delete(l.following, x.k[0])
			}
		}
	}
}

// ---- Events ----

// AddWord adds a word to the personal lexicon (for example from Sayso's
// dictionary). Adding a word it has is a no-op.
func (l *Learner) AddWord(w string) {
	w = strings.TrimSpace(w)
	if w == "" || len(w) > maxWordLen || l.words[w] {
		return
	}
	l.record("W", w)
}

// RemoveWord removes a word from the personal lexicon, learned or added.
func (l *Learner) RemoveWord(w string) {
	w = strings.TrimSpace(w)
	if !l.words[w] && l.keeps[norm(w)] == nil {
		return
	}
	l.record("w", w)
}

// AddReplacement adds or changes a text replacement.
func (l *Learner) AddReplacement(r Replacement) {
	from := strings.TrimSpace(r.From)
	if from == "" || len(from) > maxWordLen || len(r.To) > maxWordLen*4 {
		return
	}
	if old, ok := l.replacements[norm(from)]; ok && old.To == r.To && old.CaseSensitive == r.CaseSensitive {
		return
	}
	cs := "0"
	if r.CaseSensitive {
		cs = "1"
	}
	l.record("R", from, r.To, cs)
}

// RemoveReplacement removes the replacement of from.
func (l *Learner) RemoveReplacement(from string) {
	if _, ok := l.replacements[norm(from)]; ok {
		l.record("r", from)
	}
}

// Reverted records that the user undid the correction of typed to target;
// it returns true when the correction is now suppressed. The typed word
// also counts as kept.
func (l *Learner) Reverted(typed, target string) bool {
	l.record("X", typed, target)
	l.Kept(typed)
	return l.Suppressed(typed, target)
}

// ManualFix records that the user replaced typed with target by hand.
func (l *Learner) ManualFix(typed, target string) {
	if norm(typed) == norm(target) {
		return
	}
	l.record("F", typed, target)
}

// Kept records that a spelling the model lacks was committed as typed.
func (l *Learner) Kept(w string) {
	w = strings.TrimSpace(w)
	if w == "" || len(w) > maxWordLen || l.words[w] {
		return
	}
	if k := l.keeps[norm(w)]; k != nil && k.n >= KeepsToLearn {
		return
	}
	l.record("K", w)
}

// Committed records a bigram the user wrote.
func (l *Learner) Committed(prev, w string) {
	if prev == "" || w == "" || len(prev) > maxWordLen || len(w) > maxWordLen {
		return
	}
	l.record("N", prev, w, strconv.FormatInt(l.now().Unix(), 10))
}

// Confirm teaches the offsets from the taps of a word the user kept.
func (l *Learner) Confirm(samples []Sample) {
	if l.privacy || len(samples) == 0 {
		return
	}
	for _, s := range samples {
		l.offsets.add(s)
		if s.Key >= 0 && s.Key < MaxOffsetKeys && s.Row >= 0 && s.Row < MaxOffsetKeys {
			l.rowOf[s.Key] = uint8(s.Row)
		}
	}
	l.syncApplied()
	l.confirmed++
	if l.confirmed%snapshotEvery == 0 {
		l.snapshotOffsets()
	}
}

// syncApplied moves each key's offset in use toward its estimate by at most
// MaxStep per axis.
func (l *Learner) syncApplied() {
	for k := range l.applied {
		row := l.rowOf[k]
		x, y := l.offsets.Of(k, int(row))
		a := &l.applied[k]
		a[0] += float32(clamp(float64(x-a[0]), MaxStep))
		a[1] += float32(clamp(float64(y-a[1]), MaxStep))
	}
}

func (l *Learner) snapshotOffsets() {
	if l.j == nil || l.privacy {
		return
	}
	for _, line := range l.offsetLines() {
		if err := l.j.append(line); err != nil && l.err == nil {
			l.err = err
		}
	}
}

func (l *Learner) offsetLines() []string {
	f := func(v float64) string { return strconv.FormatFloat(v, 'f', 4, 64) }
	line := func(level string, i int, e estimate) string {
		return encodeRecord("O", level, strconv.Itoa(i), f(e.X), f(e.Y), f(e.N))
	}
	var out []string
	if l.offsets.Board.N > 0 {
		out = append(out, line("b", 0, l.offsets.Board))
	}
	for i, e := range l.offsets.Rows {
		if e.N > 0 {
			out = append(out, line("r", i, e))
		}
	}
	for i, e := range l.offsets.Keys {
		if e.N > 0 {
			out = append(out, line("k", i, e))
		}
	}
	return out
}

// Reset forgets everything learned (personal words and replacements too)
// and empties the journal. It works in privacy mode as well.
func (l *Learner) Reset() error {
	l.clear()
	if l.j == nil {
		return nil
	}
	return l.j.rewrite(nil)
}

// compact rewrites the journal as the records of the current state.
func (l *Learner) compact() error {
	var lines []string
	for _, w := range sortedKeys(l.words) {
		lines = append(lines, encodeRecord("W", w))
	}
	for _, w := range sortedKeys(l.keeps) {
		k := l.keeps[w]
		for range min(k.n, KeepsToLearn) {
			lines = append(lines, encodeRecord("K", k.display))
		}
	}
	for _, k := range sortedKeys(l.replacements) {
		r := l.replacements[k]
		cs := "0"
		if r.CaseSensitive {
			cs = "1"
		}
		lines = append(lines, encodeRecord("R", r.From, r.To, cs))
	}
	for _, p := range sortedPairs(l.reverts) {
		for range min(l.reverts[p], RevertsToSuppress) {
			lines = append(lines, encodeRecord("X", p.typed, p.target))
		}
	}
	for _, p := range sortedPairs(l.fixes) {
		target := p.target
		if t, ok := l.promoted[p.typed]; ok && norm(t) == p.target {
			target = t
		}
		for range min(l.fixes[p], FixesToPromote) {
			lines = append(lines, encodeRecord("F", p.typed, target))
		}
	}
	now := l.now().Unix()
	keys := make([][2]string, 0, len(l.ngrams))
	for k := range l.ngrams {
		keys = append(keys, k)
	}
	slices.SortFunc(keys, func(a, b [2]string) int { return strings.Compare(a[0]+"\x00"+a[1], b[0]+"\x00"+b[1]) })
	for _, k := range keys {
		g := l.ngrams[k]
		if c := decayed(g.count, g.last, now); c >= 0.05 {
			lines = append(lines, encodeRecord("G", k[0], k[1], strconv.FormatFloat(c, 'f', 3, 64), strconv.FormatInt(now, 10)))
		}
	}
	lines = append(lines, l.offsetLines()...)
	return l.j.rewrite(lines)
}

func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	slices.Sort(keys)
	return keys
}

func sortedPairs(m map[pair]int) []pair {
	keys := make([]pair, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	slices.SortFunc(keys, func(a, b pair) int { return strings.Compare(a.typed+"\x00"+a.target, b.typed+"\x00"+b.target) })
	return keys
}

// ---- Queries ----

// Suppressed reports whether correcting typed to target is suppressed.
func (l *Learner) Suppressed(typed, target string) bool {
	return l.suppressed[pair{norm(typed), norm(target)}]
}

// Promoted is the word a typo is corrected to, if the user taught one.
func (l *Learner) Promoted(typed string) (string, bool) {
	t, ok := l.promoted[norm(typed)]
	return t, ok
}

// Replacement is the replacement for a typed word, if any.
func (l *Learner) Replacement(typed string) (Replacement, bool) {
	r, ok := l.replacements[norm(typed)]
	if ok && r.CaseSensitive && r.From != strings.TrimSpace(typed) {
		return Replacement{}, false
	}
	return r, ok
}

// Replacements is every replacement, sorted by From.
func (l *Learner) Replacements() []Replacement {
	var out []Replacement
	for _, k := range sortedKeys(l.replacements) {
		out = append(out, l.replacements[k])
	}
	return out
}

// PersonalWords is the personal lexicon: added words, learned words, and
// the targets of replacements and promoted typos that are single words.
// Each comes with how often it was kept (added words count as KeepsToLearn).
func (l *Learner) PersonalWords() map[string]int {
	out := map[string]int{}
	for w := range l.words {
		out[w] = KeepsToLearn
	}
	for _, k := range l.keeps {
		if k.n >= KeepsToLearn {
			out[k.display] = max(out[k.display], k.n)
		}
	}
	return out
}

// Bigram is the faded count of the user's bigram (prev, w).
func (l *Learner) Bigram(prev, w string) float64 {
	g := l.ngrams[[2]string{norm(prev), norm(w)}]
	if g == nil {
		return 0
	}
	return decayed(g.count, g.last, l.now().Unix())
}

// Following calls fn for each word the user wrote after prev, with the
// faded count.
func (l *Learner) Following(prev string, fn func(w string, count float64)) {
	now := l.now().Unix()
	for w, g := range l.following[norm(prev)] {
		fn(w, decayed(g.count, g.last, now))
	}
}

// Bigrams calls fn for each personal bigram with its faded count.
func (l *Learner) Bigrams(fn func(prev, w string, count float64)) {
	now := l.now().Unix()
	for k, g := range l.ngrams {
		fn(k[0], k[1], decayed(g.count, g.last, now))
	}
}

// SetRows tells the learner each key's row in the current layout, and
// moves the offsets in use toward the estimates of those rows.
func (l *Learner) SetRows(rows []int) {
	for k, r := range rows {
		if k < MaxOffsetKeys && r >= 0 && r < MaxOffsetKeys {
			l.rowOf[k] = uint8(r)
		}
	}
	l.jumpApplied()
}

// jumpApplied puts the estimates in use at once, not MaxStep at a time: for
// a new layout or a replayed journal.
func (l *Learner) jumpApplied() {
	for k := range l.applied {
		x, y := l.offsets.Of(k, int(l.rowOf[k]))
		l.applied[k] = [2]float32{x, y}
	}
}

// KeyOffset is the learned offset of a key, in pitches.
func (l *Learner) KeyOffset(key int) (float32, float32) {
	if key < 0 || key >= MaxOffsetKeys {
		return 0, 0
	}
	return l.applied[key][0], l.applied[key][1]
}

// KeySamples is how many confirmed taps taught a key.
func (l *Learner) KeySamples(key int) int { return l.offsets.Samples(key) }

// String summarizes the state, for diagnostics.
func (l *Learner) String() string {
	return fmt.Sprintf("words %d, learned %d, replacements %d, suppressed %d, promoted %d, bigrams %d, board offset (%.3f, %.3f) from %d taps",
		len(l.words), len(l.PersonalWords())-len(l.words), len(l.replacements), len(l.suppressed), len(l.promoted), len(l.ngrams),
		l.offsets.Board.X, l.offsets.Board.Y, int(l.offsets.Board.N))
}
