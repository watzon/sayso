package config

import (
	"errors"
	"fmt"
	"math/bits"
	"runtime"
	"strconv"
	"strings"
	"time"
)

// Hotkeys (crates/sayso-core/src/hotkey.rs). A hotkey is a chord (modifiers
// plus one key, "opt+space"), one modifier on its own ("right_option"), or a
// set of side-specific modifiers ("left_command+left_option"). The modifier
// names are the Mac ones; on Linux and Windows option is Alt and command is
// the Super/Windows key. The parser takes each system's names.

// KeyKind is the kind of a regular key.
type KeyKind int

const (
	KeySpace KeyKind = iota
	KeyEscape
	KeyReturn
	KeyTab
	KeyBackspace
	KeyLetter
	KeyDigit
	KeyF
	KeyPunctuation
)

// Key is a regular key. Char holds the letter (lowercase) or punctuation
// character; Num holds the digit or the F number.
type Key struct {
	Kind KeyKind
	Char rune
	Num  uint8
}

// Punctuation is the punctuation keys of a US keyboard, by the character they
// type without Shift, in the fixed order the platform key tables follow.
var Punctuation = [11]rune{'`', '-', '=', '[', ']', '\\', ';', '\'', ',', '.', '/'}

// PunctuationIndex is the position of c in Punctuation, or -1.
func PunctuationIndex(c rune) int {
	for i, p := range Punctuation {
		if p == c {
			return i
		}
	}
	return -1
}

// Label is the keycap label of the key.
func (k Key) Label() string {
	switch k.Kind {
	case KeySpace:
		return "Space"
	case KeyEscape:
		return "Esc"
	case KeyReturn:
		return "↵"
	case KeyTab:
		return "⇥"
	case KeyBackspace:
		return "⌫"
	case KeyLetter:
		return strings.ToUpper(string(k.Char))
	case KeyDigit:
		return strconv.Itoa(int(k.Num))
	case KeyF:
		return fmt.Sprintf("F%d", k.Num)
	}
	return string(k.Char)
}

func (k Key) token() string {
	switch k.Kind {
	case KeySpace:
		return "space"
	case KeyEscape:
		return "esc"
	case KeyReturn:
		return "return"
	case KeyTab:
		return "tab"
	case KeyBackspace:
		return "backspace"
	case KeyLetter:
		return strings.ToLower(string(k.Char))
	case KeyDigit:
		return strconv.Itoa(int(k.Num))
	case KeyF:
		return fmt.Sprintf("f%d", k.Num)
	}
	return string(k.Char)
}

func parseKey(token string) (Key, bool) {
	t := asciiLower(token)
	switch t {
	case "space":
		return Key{Kind: KeySpace}, true
	case "esc", "escape":
		return Key{Kind: KeyEscape}, true
	case "return", "enter":
		return Key{Kind: KeyReturn}, true
	case "tab":
		return Key{Kind: KeyTab}, true
	case "backspace", "delete":
		return Key{Kind: KeyBackspace}, true
	}
	if len(t) == 1 && t[0] >= 'a' && t[0] <= 'z' {
		return Key{Kind: KeyLetter, Char: rune(t[0])}, true
	}
	if len(t) == 1 && t[0] >= '0' && t[0] <= '9' {
		return Key{Kind: KeyDigit, Num: t[0] - '0'}, true
	}
	if strings.HasPrefix(t, "f") {
		if n, err := strconv.ParseUint(t[1:], 10, 8); err == nil && n >= 1 && n <= 20 {
			return Key{Kind: KeyF, Num: uint8(n)}, true
		}
	}
	if r := []rune(t); len(r) == 1 && PunctuationIndex(r[0]) >= 0 {
		return Key{Kind: KeyPunctuation, Char: r[0]}, true
	}
	return Key{}, false
}

// SoloModifier is a modifier key on its own. The values are the bit
// positions of ModifierSet, in the Rust declaration order.
type SoloModifier uint8

const (
	RightOption SoloModifier = iota
	LeftOption
	RightCommand
	LeftCommand
	RightControl
	LeftControl
	RightShift
	LeftShift
	Fn
)

// AllSoloModifiers is the order of iteration and display.
var AllSoloModifiers = [9]SoloModifier{Fn, LeftCommand, LeftControl, LeftOption, LeftShift, RightCommand, RightControl, RightOption, RightShift}

func (m SoloModifier) token() string {
	return [...]string{"right_option", "left_option", "right_command", "left_command", "right_control", "left_control", "right_shift", "left_shift", "fn"}[m]
}

func parseSolo(token string) (SoloModifier, bool) {
	t := strings.TrimSpace(token)
	for _, m := range AllSoloModifiers {
		if strings.EqualFold(t, m.token()) {
			return m, true
		}
	}
	return 0, false
}

// Label is the keycap label of the modifier on this system.
func (m SoloModifier) Label() string { return soloLabel(runtime.GOOS, m) }

func soloLabel(goos string, m SoloModifier) string {
	if goos == "darwin" {
		return [...]string{"Right ⌥", "Left ⌥", "Right ⌘", "Left ⌘", "Right ⌃", "Left ⌃", "Right ⇧", "Left ⇧", "fn"}[m]
	}
	super := "Super"
	if goos == "windows" {
		super = "Win"
	}
	return [...]string{"Right Alt", "Left Alt", "Right " + super, "Left " + super, "Right Ctrl", "Left Ctrl", "Right Shift", "Left Shift", "fn"}[m]
}

// Modifiers are the side-free modifiers of a chord.
type Modifiers struct {
	Command, Option, Control, Shift, Function bool
}

func (m Modifiers) IsEmpty() bool {
	return !(m.Command || m.Option || m.Control || m.Shift || m.Function)
}

// ModifierSet is a side-specific set of modifier keys, independent of press order.
type ModifierSet uint16

// Insert adds a modifier and reports whether it was new.
func (s *ModifierSet) Insert(m SoloModifier) bool {
	bit := ModifierSet(1) << m
	fresh := *s&bit == 0
	*s |= bit
	return fresh
}

func (s ModifierSet) Contains(m SoloModifier) bool  { return s&(1<<m) != 0 }
func (s ModifierSet) Len() int                      { return bits.OnesCount16(uint16(s)) }
func (s ModifierSet) IsEmpty() bool                 { return s == 0 }
func (s ModifierSet) IsSubsetOf(o ModifierSet) bool { return s&o == s }

// Each lists the modifiers in AllSoloModifiers order.
func (s ModifierSet) Each() []SoloModifier {
	var out []SoloModifier
	for _, m := range AllSoloModifiers {
		if s.Contains(m) {
			out = append(out, m)
		}
	}
	return out
}

// SetOf builds a set.
func SetOf(ms ...SoloModifier) ModifierSet {
	var s ModifierSet
	for _, m := range ms {
		s.Insert(m)
	}
	return s
}

// HotkeyKind is the shape of a hotkey.
type HotkeyKind uint8

const (
	HotkeyChord HotkeyKind = iota
	HotkeySolo
	HotkeyModifierOnly
)

// Hotkey is comparable with ==.
type Hotkey struct {
	Kind      HotkeyKind
	Modifiers Modifiers    // Chord
	Key       Key          // Chord
	Solo      SoloModifier // Solo
	Set       ModifierSet  // ModifierOnly
}

// Chord, SoloKey, and ModifierOnlyKey build hotkeys.
func Chord(m Modifiers, k Key) Hotkey      { return Hotkey{Kind: HotkeyChord, Modifiers: m, Key: k} }
func SoloKey(m SoloModifier) Hotkey        { return Hotkey{Kind: HotkeySolo, Solo: m} }
func ModifierOnlyKey(s ModifierSet) Hotkey { return Hotkey{Kind: HotkeyModifierOnly, Set: s} }

// Hotkey parse errors.
var (
	ErrHotkeyEmpty    = errors.New("the hotkey is empty")
	ErrWrongKeyCount  = errors.New("a hotkey needs exactly one key after its modifiers")
	errUnknownKeyBase = errors.New("unknown key name")
)

// UnknownKeyError is an unknown key name.
type UnknownKeyError struct{ Name string }

func (e *UnknownKeyError) Error() string { return fmt.Sprintf("unknown key name %q", e.Name) }
func (e *UnknownKeyError) Unwrap() error { return errUnknownKeyBase }

// ModifierKeys is the set of a modifier-only hotkey; ok is false for a chord.
func (h Hotkey) ModifierKeys() (ModifierSet, bool) {
	switch h.Kind {
	case HotkeySolo:
		return SetOf(h.Solo), true
	case HotkeyModifierOnly:
		return h.Set, true
	}
	return 0, false
}

// ModifierOnly makes a hotkey of held modifiers: none for an empty set, Solo
// for one, ModifierOnly for more.
func ModifierOnly(s ModifierSet) (Hotkey, bool) {
	switch s.Len() {
	case 0:
		return Hotkey{}, false
	case 1:
		return SoloKey(s.Each()[0]), true
	}
	return ModifierOnlyKey(s), true
}

// Overlaps is true when the same key press can fire both: the same hotkey,
// or two modifier sets where one holds the other.
func (h Hotkey) Overlaps(o Hotkey) bool {
	if h == o {
		return true
	}
	a, okA := h.ModifierKeys()
	b, okB := o.ModifierKeys()
	return okA && okB && (a.IsSubsetOf(b) || b.IsSubsetOf(a))
}

// ConflictsWithToggle: modifier-only actions cannot share a gesture; chords
// can share the toggle's modifiers because a regular key cancels it.
func (h Hotkey) ConflictsWithToggle(toggle Hotkey) bool { return h.Overlaps(toggle) }

// ToggleDefault is Option+Space on macOS, Ctrl+Space on Windows (Alt+Space
// opens the window menu), Ctrl+Alt+Space on Linux.
func ToggleDefault() Hotkey { return toggleDefaultFor(runtime.GOOS) }

func toggleDefaultFor(goos string) Hotkey {
	m := Modifiers{Control: true, Option: true}
	switch goos {
	case "darwin":
		m = Modifiers{Option: true}
	case "windows":
		m = Modifiers{Control: true}
	}
	return Chord(m, Key{Kind: KeySpace})
}

// PasteLastDefault is Ctrl+Cmd+V on macOS, Alt+Shift+V on Windows, Ctrl+Alt+V on Linux.
func PasteLastDefault() Hotkey { return pasteLastDefaultFor(runtime.GOOS) }

func pasteLastDefaultFor(goos string) Hotkey {
	m := Modifiers{Control: true, Option: true}
	switch goos {
	case "darwin":
		m = Modifiers{Control: true, Command: true}
	case "windows":
		m = Modifiers{Option: true, Shift: true}
	}
	return Chord(m, Key{Kind: KeyLetter, Char: 'v'})
}

type modifierNames struct{ control, option, shift, command string }

func namesFor(goos string) modifierNames {
	switch goos {
	case "darwin":
		return modifierNames{"⌃", "⌥", "⇧", "⌘"}
	case "windows":
		return modifierNames{"Ctrl", "Alt", "Shift", "Win"}
	}
	return modifierNames{"Ctrl", "Alt", "Shift", "Super"}
}

// Keycaps are the keycap labels for the UI: ["⌥", "Space"] on macOS,
// ["Ctrl", "Alt", "Space"] elsewhere.
func (h Hotkey) Keycaps() []string { return h.keycapsFor(runtime.GOOS) }

func (h Hotkey) keycapsFor(goos string) []string {
	switch h.Kind {
	case HotkeySolo:
		return []string{soloLabel(goos, h.Solo)}
	case HotkeyModifierOnly:
		var caps []string
		for _, m := range h.Set.Each() {
			caps = append(caps, soloLabel(goos, m))
		}
		return caps
	}
	n := namesFor(goos)
	var caps []string
	if h.Modifiers.Function {
		caps = append(caps, "fn")
	}
	type on struct {
		set bool
		cap string
	}
	var order []on
	if goos == "darwin" {
		// The Mac order: ⌃ ⌥ ⇧ ⌘.
		order = []on{{h.Modifiers.Control, n.control}, {h.Modifiers.Option, n.option}, {h.Modifiers.Shift, n.shift}, {h.Modifiers.Command, n.command}}
	} else {
		// The PC order: Super, Ctrl, Alt, Shift.
		order = []on{{h.Modifiers.Command, n.command}, {h.Modifiers.Control, n.control}, {h.Modifiers.Option, n.option}, {h.Modifiers.Shift, n.shift}}
	}
	for _, o := range order {
		if o.set {
			caps = append(caps, o.cap)
		}
	}
	return append(caps, h.Key.Label())
}

// CompactLabel is the keycaps as one label: "⌃⌘V" on macOS, "Alt+Shift+V" elsewhere.
func (h Hotkey) CompactLabel() string {
	sep := "+"
	if runtime.GOOS == "darwin" {
		sep = ""
	}
	return strings.Join(h.Keycaps(), sep)
}

// ParseHotkey parses "opt+space", "ctrl+cmd+v", a solo modifier such as
// "right_option" or "fn", or a set such as "left_command+left_option".
func ParseHotkey(s string) (Hotkey, error) {
	s = strings.TrimSpace(s)
	if s == "" {
		return Hotkey{}, ErrHotkeyEmpty
	}
	if solo, ok := parseSolo(s); ok {
		return SoloKey(solo), nil
	}
	var set ModifierSet
	modifierOnly := true
	for _, part := range strings.Split(s, "+") {
		m, ok := parseSolo(part)
		if !ok {
			modifierOnly = false
			break
		}
		if !set.Insert(m) {
			return Hotkey{}, ErrWrongKeyCount
		}
	}
	if modifierOnly {
		h, ok := ModifierOnly(set)
		if !ok {
			return Hotkey{}, ErrWrongKeyCount
		}
		return h, nil
	}
	var mods Modifiers
	var key *Key
	for _, part := range strings.Split(s, "+") {
		part = strings.TrimSpace(part)
		switch asciiLower(part) {
		// The Windows and Super keys sit where Command does on a Mac keyboard.
		case "cmd", "command", "super", "win", "meta", "logo":
			mods.Command = true
		case "opt", "option", "alt":
			mods.Option = true
		case "ctrl", "control":
			mods.Control = true
		case "shift":
			mods.Shift = true
		case "fn":
			mods.Function = true
		default:
			if key != nil {
				return Hotkey{}, ErrWrongKeyCount
			}
			lower := asciiLower(part)
			k, ok := parseKey(lower)
			if !ok {
				return Hotkey{}, &UnknownKeyError{lower}
			}
			key = &k
		}
	}
	if key == nil {
		return Hotkey{}, ErrWrongKeyCount
	}
	return Chord(mods, *key), nil
}

// MustHotkey parses or panics, for constants.
func MustHotkey(s string) Hotkey {
	h, err := ParseHotkey(s)
	if err != nil {
		panic(err)
	}
	return h
}

// String formats the config-file form, with this system's names for option
// and command ("opt"/"alt", "cmd"/"win"/"super").
func (h Hotkey) String() string { return h.formatFor(runtime.GOOS) }

func (h Hotkey) formatFor(goos string) string {
	switch h.Kind {
	case HotkeySolo:
		return h.Solo.token()
	case HotkeyModifierOnly:
		var parts []string
		for _, m := range h.Set.Each() {
			parts = append(parts, m.token())
		}
		return strings.Join(parts, "+")
	}
	option, command := "alt", "super"
	switch goos {
	case "darwin":
		option, command = "opt", "cmd"
	case "windows":
		command = "win"
	}
	var parts []string
	if h.Modifiers.Function {
		parts = append(parts, "fn")
	}
	if h.Modifiers.Control {
		parts = append(parts, "ctrl")
	}
	if h.Modifiers.Option {
		parts = append(parts, option)
	}
	if h.Modifiers.Shift {
		parts = append(parts, "shift")
	}
	if h.Modifiers.Command {
		parts = append(parts, command)
	}
	parts = append(parts, h.Key.token())
	return strings.Join(parts, "+")
}

func (h Hotkey) MarshalText() ([]byte, error) { return []byte(h.String()), nil }

func (h *Hotkey) UnmarshalText(b []byte) error {
	v, err := ParseHotkey(string(b))
	if err != nil {
		return err
	}
	*h = v
	return nil
}

func asciiLower(s string) string {
	b := []byte(s)
	for i, c := range b {
		if c >= 'A' && c <= 'Z' {
			b[i] = c + 32
		}
	}
	return string(b)
}

// ModifierCapture is the modifier-only part of hotkey recording, shared by
// the native listeners.
type ModifierCapture struct {
	candidate    ModifierSet
	hasCandidate bool
	tapStarted   time.Time
	hasTap       bool
}

// TapWindow is how long a deliberate toggle tap may take from its first modifier.
const TapWindow = 400 * time.Millisecond

func (c *ModifierCapture) Clear() { *c = ModifierCapture{} }

// PressTap records held modifiers for a timed tap.
func (c *ModifierCapture) PressTap(held ModifierSet, now time.Time) {
	started := now
	if c.hasTap {
		started = c.tapStarted
	}
	c.Press(held)
	c.hasTap = c.hasCandidate
	if c.hasTap {
		c.tapStarted = started
	}
}

// ReleaseTap gives the hotkey when the tap ended within TapWindow.
func (c *ModifierCapture) ReleaseTap(m SoloModifier, now time.Time) (Hotkey, bool) {
	if !c.hasTap || now.Sub(c.tapStarted) > TapWindow {
		c.Clear()
		return Hotkey{}, false
	}
	return c.Release(m)
}

func (c *ModifierCapture) IsActive() bool { return c.hasCandidate }

// Press records held modifiers (untimed, for the recorder).
func (c *ModifierCapture) Press(held ModifierSet) {
	if held.Len() == 1 || (c.hasCandidate && c.candidate.IsSubsetOf(held)) {
		c.candidate, c.hasCandidate = held, true
	} else {
		c.candidate, c.hasCandidate = 0, false
	}
}

// Release gives the captured hotkey when a held modifier goes up.
func (c *ModifierCapture) Release(m SoloModifier) (Hotkey, bool) {
	if !c.hasCandidate || !c.candidate.Contains(m) {
		return Hotkey{}, false
	}
	set := c.candidate
	c.Clear()
	return ModifierOnly(set)
}
