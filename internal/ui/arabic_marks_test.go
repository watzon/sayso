package ui

import (
	"bytes"
	"runtime"
	"slices"
	"strings"
	"testing"
	"unicode/utf8"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/theme"
)

// A short Arabic label fits one line at its own width. Core Text gives some
// Arabic marks a negative advance (the fatha of "محسَّن" in Geeza Pro and
// SF Arabic), so MyGo measures the label narrower than Core Text then needs
// to set it, and a label sized to its text (a chip, a bullet, a badge) wraps
// at that width: single line, it is cut to "مح…". The Arabic catalog leaves
// such marks out of short labels; this finds one that comes back. Only on
// macOS, the system whose shaping this is.
func TestShortArabicLabelsFitTheirOwnWidth(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("Core Text only")
	}
	RegisterFonts()
	msgs := i18n.Messages("ar")
	keys := make([]string, 0, len(msgs))
	for k, s := range msgs {
		if !strings.ContainsFunc(s, arabicMark) || strings.Contains(s, "%{") || strings.ContainsAny(s, "\n.،") || utf8.RuneCountInString(s) > 32 {
			continue // a sentence wraps in a column; only labels are set to their width
		}
		keys = append(keys, k)
	}
	slices.Sort(keys)
	// Set single line, a cut label paints other pixels than the same label
	// free to wrap, which MyGo lays out whole on its one line.
	var (
		label  string
		size   float32
		weight int
		single bool
	)
	tt := mu.NewTester(func(c *mu.Context) {
		c.Root().Direction(mu.RightToLeft).Font(theme.UIFor("ar"))
		mu.Row(c).Children(func() {
			e := mu.Text(c, label).FontSize(size).FontWeight(weight).Shrink(0)
			if single {
				e.SingleLine()
			}
		})
	}, 400, 32)
	shot := func(one bool) []byte {
		single = one
		tt.Frame()
		return bytes.Clone(tt.Image().Pix)
	}
	for _, k := range keys {
		label = msgs[k]
		for _, f := range []struct {
			size   float32
			weight int
		}{{12, 500}, {13, 500}, {13, 600}, {14, 400}} {
			size, weight = f.size, f.weight
			if !bytes.Equal(shot(true), shot(false)) {
				t.Errorf("%s %q is cut single line at %v/%d: drop the marks it does not need", k, label, size, weight)
				break
			}
		}
	}
}

// arabicMark reports whether r is an Arabic vowel or other mark (tashkeel).
func arabicMark(r rune) bool { return r >= 0x064B && r <= 0x065F || r == 0x0670 }
