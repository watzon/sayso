// Package touch turns taps into keys: the keyboard's geometry, a bivariate
// Gaussian model of where a finger lands for each key, and the anchored
// hit-target expansion that lets a likely next key borrow part of its
// neighbours' border while every key keeps a core of its own.
package touch

import (
	"errors"
	"fmt"
	"math"
	"slices"
)

// MaxKeys is the most keys a layout can have.
const MaxKeys = 96

// maxNeighbours is the most adjacent keys kept for each key.
const maxNeighbours = 10

// Function key codes; letters, space, and marks use their code point.
const (
	CodeShift     = -1
	CodeBackspace = -2
	CodeEnter     = -3
	CodeSymbols   = -4
	CodeOther     = -5
)

// Kind is what a key types.
type Kind uint8

const (
	KindLetter   Kind = iota // a to z
	KindSpace                // the space bar
	KindMark                 // another character: ' , . ? and the like
	KindFunction             // shift, backspace, enter, ...
)

// Key is one key: what it types and its rectangle (top-left corner, size),
// in any unit as long as all keys and taps use the same one.
type Key struct {
	Code       rune
	X, Y, W, H float32
}

// Layout is a keyboard's keys with what the model needs precomputed.
type Layout struct {
	Keys       []Key
	kinds      []Kind
	rows       []uint8 // row of each key, top row 0
	rowCount   int
	letter     [26]int16 // key of each letter, -1 if none
	space      int16     // the space bar, -1 if none
	pitch      float32   // median letter key width
	neighbours [][maxNeighbours]int16
	nbCount    []uint8
}

// NewLayout checks the keys and precomputes the rows, the key of each
// letter, the key pitch, and each key's neighbours.
func NewLayout(keys []Key) (*Layout, error) {
	if len(keys) == 0 || len(keys) > MaxKeys {
		return nil, fmt.Errorf("touch: %d keys (1 to %d)", len(keys), MaxKeys)
	}
	l := &Layout{Keys: slices.Clone(keys), space: -1}
	for i := range l.letter {
		l.letter[i] = -1
	}
	var widths []float32
	l.kinds = make([]Kind, len(keys))
	for i := range l.Keys {
		k := &l.Keys[i]
		if !(k.W > 0 && k.H > 0) || isBad(k.X) || isBad(k.Y) || isBad(k.W) || isBad(k.H) {
			return nil, fmt.Errorf("touch: key %d has a bad rectangle", i)
		}
		if k.Code >= 'A' && k.Code <= 'Z' {
			k.Code += 'a' - 'A'
		}
		switch {
		case k.Code >= 'a' && k.Code <= 'z':
			l.kinds[i] = KindLetter
			if l.letter[k.Code-'a'] >= 0 {
				return nil, fmt.Errorf("touch: two keys type %q", k.Code)
			}
			l.letter[k.Code-'a'] = int16(i)
			widths = append(widths, k.W)
		case k.Code == ' ':
			l.kinds[i] = KindSpace
			if l.space < 0 {
				l.space = int16(i)
			}
		case k.Code > ' ':
			l.kinds[i] = KindMark
		default:
			l.kinds[i] = KindFunction
		}
	}
	if len(widths) == 0 {
		return nil, errors.New("touch: no letter keys")
	}
	slices.Sort(widths)
	l.pitch = widths[len(widths)/2]
	l.assignRows()
	l.findNeighbours()
	return l, nil
}

func isBad(f float32) bool { return math.IsNaN(float64(f)) || math.IsInf(float64(f), 0) }

// assignRows groups keys whose centres are within half a key height.
func (l *Layout) assignRows() {
	type yc struct {
		y float32
		i int
	}
	order := make([]yc, len(l.Keys))
	for i, k := range l.Keys {
		order[i] = yc{k.Y + k.H/2, i}
	}
	slices.SortFunc(order, func(a, b yc) int { return cmpF(a.y, b.y) })
	l.rows = make([]uint8, len(l.Keys))
	row, start := 0, order[0].y
	for _, o := range order {
		if o.y-start > l.Keys[o.i].H/2 {
			row++
			start = o.y
		}
		l.rows[o.i] = uint8(row)
	}
	l.rowCount = row + 1
}

// findNeighbours keeps, for each key, the keys within half a pitch of its
// rectangle, nearest first.
func (l *Layout) findNeighbours() {
	n := len(l.Keys)
	l.neighbours = make([][maxNeighbours]int16, n)
	l.nbCount = make([]uint8, n)
	type nd struct {
		d float32
		j int
	}
	for i := range l.Keys {
		var near []nd
		for j := range l.Keys {
			if j == i {
				continue
			}
			if d := rectGap(l.Keys[i], l.Keys[j]); d <= l.pitch/2 {
				near = append(near, nd{d, j})
			}
		}
		slices.SortFunc(near, func(a, b nd) int { return cmpF(a.d, b.d) })
		for _, x := range near {
			if int(l.nbCount[i]) == maxNeighbours {
				break
			}
			l.neighbours[i][l.nbCount[i]] = int16(x.j)
			l.nbCount[i]++
		}
	}
}

func cmpF(a, b float32) int {
	switch {
	case a < b:
		return -1
	case a > b:
		return 1
	}
	return 0
}

// rectGap is the distance between two rectangles, 0 when they touch.
func rectGap(a, b Key) float32 {
	dx := max(0, max(a.X-(b.X+b.W), b.X-(a.X+a.W)))
	dy := max(0, max(a.Y-(b.Y+b.H), b.Y-(a.Y+a.H)))
	return float32(math.Hypot(float64(dx), float64(dy)))
}

// Pitch is the median width of a letter key.
func (l *Layout) Pitch() float32 { return l.pitch }

// Kind is what key i types.
func (l *Layout) Kind(i int) Kind { return l.kinds[i] }

// Row is key i's row, the top row being 0; RowCount is the number of rows.
func (l *Layout) Row(i int) int { return int(l.rows[i]) }
func (l *Layout) RowCount() int { return l.rowCount }
func (l *Layout) Len() int      { return len(l.Keys) }
func (l *Layout) Space() int    { return int(l.space) }
func (l *Layout) Center(i int) (float32, float32) {
	k := &l.Keys[i]
	return k.X + k.W/2, k.Y + k.H/2
}

// LetterKey is the key that types letter c ('a' to 'z'), or -1.
func (l *Layout) LetterKey(c byte) int {
	if c < 'a' || c > 'z' {
		return -1
	}
	return int(l.letter[c-'a'])
}

// Neighbours is the keys next to key i, nearest first.
func (l *Layout) Neighbours(i int) []int16 { return l.neighbours[i][:l.nbCount[i]] }

// QWERTY is a plain phone QWERTY layout of the given key size, with the
// usual offsets of the middle and bottom rows, a shift and a backspace key,
// and a space bar between a symbols key and a period. For tests, the replay
// benchmark, and as an example of the geometry the apps send.
func QWERTY(keyW, keyH float32) []Key {
	var keys []Key
	row := func(letters string, x0, y float32) {
		for i, c := range letters {
			keys = append(keys, Key{Code: c, X: x0 + float32(i)*keyW, Y: y, W: keyW, H: keyH})
		}
	}
	row("qwertyuiop", 0, 0)
	row("asdfghjkl", keyW/2, keyH)
	row("zxcvbnm", 1.5*keyW, 2*keyH)
	keys = append(keys,
		Key{Code: CodeShift, X: 0, Y: 2 * keyH, W: 1.5 * keyW, H: keyH},
		Key{Code: CodeBackspace, X: 8.5 * keyW, Y: 2 * keyH, W: 1.5 * keyW, H: keyH},
		Key{Code: CodeSymbols, X: 0, Y: 3 * keyH, W: 2.5 * keyW, H: keyH},
		Key{Code: ' ', X: 2.5 * keyW, Y: 3 * keyH, W: 5 * keyW, H: keyH},
		Key{Code: '.', X: 7.5 * keyW, Y: 3 * keyH, W: keyW, H: keyH},
		Key{Code: CodeEnter, X: 8.5 * keyW, Y: 3 * keyH, W: 1.5 * keyW, H: keyH},
	)
	return keys
}
