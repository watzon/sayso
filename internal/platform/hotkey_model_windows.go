package platform

import (
	"errors"
	"fmt"
	"math/bits"
	"strconv"
	"strings"
	"time"
)

// The hotkey model of sayso-core (hotkey.rs) and the Windows key table of
// sayso-platform-windows (keymap.rs). The modifier names are the Mac ones:
// option is Alt and command is the Windows key. The parser takes both.

type keyKind int

const (
	keySpace keyKind = iota
	keyEscape
	keyReturn
	keyTab
	keyBackspace
	keyLetter
	keyDigit
	keyF
	keyPunctuation
)

// hotkeyKey is the key of a chord. ch is the letter, the digit, the F number,
// or the punctuation character.
type hotkeyKey struct {
	kind keyKind
	ch   rune
}

// punctuationKeys are the punctuation keys of a US keyboard, in the order the
// key tables follow.
var punctuationKeys = []rune{'`', '-', '=', '[', ']', '\\', ';', '\'', ',', '.', '/'}

// soloModifier is one modifier key on its own side.
type soloModifier int

const (
	rightOption soloModifier = iota
	leftOption
	rightCommand
	leftCommand
	rightControl
	leftControl
	rightShift
	leftShift
	fnKey
)

var soloTokens = []string{"right_option", "left_option", "right_command", "left_command", "right_control", "left_control", "right_shift", "left_shift", "fn"}

func parseSolo(token string) (soloModifier, bool) {
	t := strings.ToLower(strings.TrimSpace(token))
	// The Windows names of the sided keys.
	switch t {
	case "right_alt":
		t = "right_option"
	case "left_alt":
		t = "left_option"
	case "right_win", "right_super":
		t = "right_command"
	case "left_win", "left_super":
		t = "left_command"
	case "right_ctrl":
		t = "right_control"
	case "left_ctrl":
		t = "left_control"
	}
	for i, s := range soloTokens {
		if s == t {
			return soloModifier(i), true
		}
	}
	return 0, false
}

// modifierSet is a set of sided modifier keys.
type modifierSet uint16

func (s modifierSet) has(m soloModifier) bool { return s&(1<<m) != 0 }
func (s modifierSet) len() int                { return bits.OnesCount16(uint16(s)) }
func (s modifierSet) subsetOf(o modifierSet) bool {
	return s&o == s
}

// modifiers are the sideless modifiers of a chord.
type modifiers struct{ command, option, control, shift, function bool }

func (m modifiers) empty() bool { return m == modifiers{} }

type hotkeyShape int

const (
	shapeChord hotkeyShape = iota
	shapeSolo
	shapeModifierOnly
)

// hotkey is a chord (modifiers and one key), one modifier key, or modifier
// keys held together.
type hotkey struct {
	shape hotkeyShape
	mods  modifiers
	key   hotkeyKey
	solo  soloModifier
	set   modifierSet
}

func (h hotkey) modifierKeys() (modifierSet, bool) {
	switch h.shape {
	case shapeSolo:
		return 1 << h.solo, true
	case shapeModifierOnly:
		return h.set, true
	}
	return 0, false
}

func modifierOnly(set modifierSet) (hotkey, bool) {
	switch set.len() {
	case 0:
		return hotkey{}, false
	case 1:
		return hotkey{shape: shapeSolo, solo: soloModifier(bits.TrailingZeros16(uint16(set)))}, true
	}
	return hotkey{shape: shapeModifierOnly, set: set}, true
}

func parseKey(token string) (hotkeyKey, bool) {
	t := strings.ToLower(token)
	switch t {
	case "space":
		return hotkeyKey{kind: keySpace}, true
	case "esc", "escape":
		return hotkeyKey{kind: keyEscape}, true
	case "return", "enter":
		return hotkeyKey{kind: keyReturn}, true
	case "tab":
		return hotkeyKey{kind: keyTab}, true
	case "backspace", "delete":
		return hotkeyKey{kind: keyBackspace}, true
	}
	r := []rune(t)
	if len(r) == 1 {
		switch c := r[0]; {
		case c >= 'a' && c <= 'z':
			return hotkeyKey{kind: keyLetter, ch: c}, true
		case c >= '0' && c <= '9':
			return hotkeyKey{kind: keyDigit, ch: c - '0'}, true
		}
		for _, p := range punctuationKeys {
			if p == r[0] {
				return hotkeyKey{kind: keyPunctuation, ch: p}, true
			}
		}
	}
	if strings.HasPrefix(t, "f") {
		if n, err := strconv.Atoi(t[1:]); err == nil && n >= 1 && n <= 20 {
			return hotkeyKey{kind: keyF, ch: rune(n)}, true
		}
	}
	return hotkeyKey{}, false
}

var errWrongKeyCount = errors.New("a hotkey needs exactly one key after its modifiers")

// parseHotkey reads "alt+space", "ctrl+win+v", "right_option", or
// "left_command+left_option".
func parseHotkey(s string) (hotkey, error) {
	s = strings.TrimSpace(s)
	if s == "" {
		return hotkey{}, errors.New("the hotkey is empty")
	}
	if solo, ok := parseSolo(s); ok {
		return hotkey{shape: shapeSolo, solo: solo}, nil
	}
	parts := strings.Split(s, "+")
	var set modifierSet
	onlyModifiers := true
	for _, p := range parts {
		m, ok := parseSolo(p)
		if !ok {
			onlyModifiers = false
			break
		}
		if set.has(m) {
			return hotkey{}, errWrongKeyCount
		}
		set |= 1 << m
	}
	if onlyModifiers {
		h, ok := modifierOnly(set)
		if !ok {
			return hotkey{}, errWrongKeyCount
		}
		return h, nil
	}
	h := hotkey{shape: shapeChord}
	haveKey := false
	for _, p := range parts {
		switch t := strings.ToLower(strings.TrimSpace(p)); t {
		case "cmd", "command", "super", "win", "meta", "logo":
			h.mods.command = true
		case "opt", "option", "alt":
			h.mods.option = true
		case "ctrl", "control":
			h.mods.control = true
		case "shift":
			h.mods.shift = true
		case "fn":
			h.mods.function = true
		default:
			if haveKey {
				return hotkey{}, errWrongKeyCount
			}
			k, ok := parseKey(t)
			if !ok {
				return hotkey{}, fmt.Errorf("unknown key %s", t)
			}
			h.key, haveKey = k, true
		}
	}
	if !haveKey {
		return hotkey{}, errWrongKeyCount
	}
	return h, nil
}

// modifierCapture follows modifier-only gestures: the keys held together
// without a regular key in between. A toggle tap ends within 400 ms of its
// first modifier.
type modifierCapture struct {
	candidate  modifierSet
	active     bool
	tapStarted time.Time
	tapping    bool
}

const tapWindow = 400 * time.Millisecond

func (c *modifierCapture) clear() { *c = modifierCapture{} }

func (c *modifierCapture) press(held modifierSet) {
	if held.len() == 1 || (c.active && c.candidate.subsetOf(held)) {
		c.candidate, c.active = held, true
	} else {
		c.candidate, c.active = 0, false
	}
}

func (c *modifierCapture) pressTap(held modifierSet, now time.Time) {
	started := now
	if c.tapping {
		started = c.tapStarted
	}
	c.press(held)
	c.tapStarted, c.tapping = started, c.active
}

func (c *modifierCapture) release(m soloModifier) (hotkey, bool) {
	if !c.active || !c.candidate.has(m) {
		return hotkey{}, false
	}
	set := c.candidate
	c.clear()
	return modifierOnly(set)
}

func (c *modifierCapture) releaseTap(m soloModifier, now time.Time) (hotkey, bool) {
	if !c.tapping || now.Sub(c.tapStarted) > tapWindow {
		c.clear()
		return hotkey{}, false
	}
	return c.release(m)
}

// Virtual-key codes.
const (
	vkBack     = 0x08
	vkTab      = 0x09
	vkReturn   = 0x0D
	vkShift    = 0x10
	vkControl  = 0x11
	vkMenu     = 0x12
	vkEscape   = 0x1B
	vkSpace    = 0x20
	vkInsert   = 0x2D
	vkLWin     = 0x5B
	vkRWin     = 0x5C
	vkF1       = 0x70
	vkLShift   = 0xA0
	vkRShift   = 0xA1
	vkLControl = 0xA2
	vkRControl = 0xA3
	vkLMenu    = 0xA4
	vkRMenu    = 0xA5
	vkV        = 0x56
	// vkMask is an unassigned key, sent so that the release of a used Alt or
	// Win key opens no menu.
	vkMask = 0xE8
)

var punctuationVKs = []uint16{0xC0, 0xBD, 0xBB, 0xDB, 0xDD, 0xDC, 0xBA, 0xDE, 0xBC, 0xBE, 0xBF}

func vkOf(k hotkeyKey) (uint16, bool) {
	switch k.kind {
	case keySpace:
		return vkSpace, true
	case keyEscape:
		return vkEscape, true
	case keyReturn:
		return vkReturn, true
	case keyTab:
		return vkTab, true
	case keyBackspace:
		return vkBack, true
	case keyLetter:
		return uint16(k.ch - 'a' + 'A'), true
	case keyDigit:
		return uint16('0' + k.ch), true
	case keyF:
		return vkF1 + uint16(k.ch) - 1, true
	case keyPunctuation:
		for i, p := range punctuationKeys {
			if p == k.ch {
				return punctuationVKs[i], true
			}
		}
	}
	return 0, false
}

func keyFromVK(code uint16) (hotkeyKey, bool) {
	switch {
	case code == vkSpace:
		return hotkeyKey{kind: keySpace}, true
	case code == vkEscape:
		return hotkeyKey{kind: keyEscape}, true
	case code == vkReturn:
		return hotkeyKey{kind: keyReturn}, true
	case code == vkTab:
		return hotkeyKey{kind: keyTab}, true
	case code == vkBack:
		return hotkeyKey{kind: keyBackspace}, true
	case code >= 0x41 && code <= 0x5A:
		return hotkeyKey{kind: keyLetter, ch: rune(code-'A') + 'a'}, true
	case code >= 0x30 && code <= 0x39:
		return hotkeyKey{kind: keyDigit, ch: rune(code - '0')}, true
	case code >= vkF1 && code < vkF1+20:
		return hotkeyKey{kind: keyF, ch: rune(code-vkF1) + 1}, true
	}
	for i, vk := range punctuationVKs {
		if vk == code {
			return hotkeyKey{kind: keyPunctuation, ch: punctuationKeys[i]}, true
		}
	}
	return hotkeyKey{}, false
}

// modifierVKs are the sided modifier keys, one bit each in modKeys.
var modifierVKs = []uint16{vkLShift, vkRShift, vkLControl, vkRControl, vkLMenu, vkRMenu, vkLWin, vkRWin}

// modKeys is the set of sided modifier keys that are down.
type modKeys uint8

func modBit(vk uint16) (modKeys, bool) {
	for i, m := range modifierVKs {
		if m == vk {
			return 1 << i, true
		}
	}
	return 0, false
}

func modKeysOf(vks ...uint16) modKeys {
	var k modKeys
	for _, vk := range vks {
		k = k.with(vk, true)
	}
	return k
}

func (k modKeys) with(vk uint16, down bool) modKeys {
	bit, ok := modBit(vk)
	switch {
	case !ok:
		return k
	case down:
		return k | bit
	}
	return k &^ bit
}

func (k modKeys) has(vk uint16) bool {
	bit, ok := modBit(vk)
	return ok && k&bit != 0
}

func (k modKeys) modifiers() modifiers {
	any := func(a, b uint16) bool { return k.has(a) || k.has(b) }
	return modifiers{command: any(vkLWin, vkRWin), option: any(vkLMenu, vkRMenu), control: any(vkLControl, vkRControl), shift: any(vkLShift, vkRShift)}
}

func isModifierVK(vk uint16) bool {
	_, ok := modBit(vk)
	return ok
}

// sidedVK tells the left and right Shift, Ctrl, and Alt apart.
func sidedVK(vk uint16, scan uint32, extended bool) uint16 {
	switch {
	case vk == vkShift && scan&0xFF == 0x36:
		return vkRShift
	case vk == vkShift:
		return vkLShift
	case vk == vkControl && extended:
		return vkRControl
	case vk == vkControl:
		return vkLControl
	case vk == vkMenu && extended:
		return vkRMenu
	case vk == vkMenu:
		return vkLMenu
	}
	return vk
}

var soloVKs = []uint16{vkRMenu, vkLMenu, vkRWin, vkLWin, vkRControl, vkLControl, vkRShift, vkLShift, 0}

func soloVK(m soloModifier) (uint16, bool) {
	vk := soloVKs[m]
	return vk, vk != 0
}

func soloFromVK(vk uint16) (soloModifier, bool) {
	for i, v := range soloVKs {
		if v != 0 && v == vk {
			return soloModifier(i), true
		}
	}
	return 0, false
}

// canDetect: Windows cannot see the Fn key.
func canDetect(set modifierSet) bool { return !set.has(fnKey) }

func modifierKeysOf(held modKeys) modifierSet {
	var s modifierSet
	for i, vk := range soloVKs {
		if vk != 0 && held.has(vk) {
			s |= 1 << i
		}
	}
	return s
}

// releaseHasMeaning: a lone Alt release opens the menu bar, a lone Win
// release the Start menu.
func releaseHasMeaning(vk uint16) bool {
	return vk == vkLMenu || vk == vkRMenu || vk == vkLWin || vk == vkRWin
}

func anyReleaseHasMeaning(set modifierSet) bool {
	for i, vk := range soloVKs {
		if vk != 0 && set.has(soloModifier(i)) && releaseHasMeaning(vk) {
			return true
		}
	}
	return false
}
