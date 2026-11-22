package platform

// Linux evdev key codes (QWERTY positions), mapped to the platform-neutral
// hotkey types. An X11 key code is the evdev code plus 8. Ported from
// sayso-platform-linux/src/hotkeys/keymap.rs.

import (
	"errors"
	"strings"
)

const (
	evKeyEsc        uint16 = 1
	evKeyBackspace  uint16 = 14
	evKeyTab        uint16 = 15
	evKeyEnter      uint16 = 28
	evKeyLeftCtrl   uint16 = 29
	evKeyLeftShift  uint16 = 42
	evKeyRightShift uint16 = 54
	evKeyLeftAlt    uint16 = 56
	evKeySpace      uint16 = 57
	evKeyKPEnter    uint16 = 96
	evKeyRightCtrl  uint16 = 97
	evKeyRightAlt   uint16 = 100
	evKeyInsert     uint16 = 110
	evKeyLeftMeta   uint16 = 125
	evKeyRightMeta  uint16 = 126
	evKeyV          uint16 = 47

	x11KeycodeOffset = 8
)

var linuxLetterCodes = [26]uint16{30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44}
var linuxDigitCodes = [10]uint16{11, 2, 3, 4, 5, 6, 7, 8, 9, 10}

// linuxPunctuationCodes follow punctuationKeys: ` - = [ ] \ ; ' , . /
var linuxPunctuationCodes = [11]uint16{41, 12, 13, 26, 27, 43, 39, 40, 51, 52, 53}

// punctuationKeysyms are the XKB keysym names of punctuationKeys.
var punctuationKeysyms = [11]string{"grave", "minus", "equal", "bracketleft", "bracketright", "backslash", "semicolon", "apostrophe", "comma", "period", "slash"}

var linuxFCodes = [20]uint16{59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88, 183, 184, 185, 186, 187, 188, 189, 190}

var linuxSoloCodes = map[soloModifier]uint16{
	modLeftCommand:  evKeyLeftMeta,
	modRightCommand: evKeyRightMeta,
	modLeftControl:  evKeyLeftCtrl,
	modRightControl: evKeyRightCtrl,
	modLeftOption:   evKeyLeftAlt,
	modRightOption:  evKeyRightAlt,
	modLeftShift:    evKeyLeftShift,
	modRightShift:   evKeyRightShift,
	// Linux software cannot see the Fn key.
}

// linuxKeyFromCode maps an evdev code to a regular key. Keypad Enter is Return.
var linuxKeyFromCode = func() map[uint16]hotkeyKey {
	m := map[uint16]hotkeyKey{
		evKeySpace:     {kind: keySpace},
		evKeyEsc:       {kind: keyEscape},
		evKeyEnter:     {kind: keyReturn},
		evKeyKPEnter:   {kind: keyReturn},
		evKeyTab:       {kind: keyTab},
		evKeyBackspace: {kind: keyBackspace},
	}
	for i, c := range linuxLetterCodes {
		m[c] = letterKey(byte('a' + i))
	}
	for i, c := range linuxDigitCodes {
		m[c] = digitKey(byte(i))
	}
	for i, c := range linuxPunctuationCodes {
		m[c] = punctKey(punctuationKeys[i])
	}
	for i, c := range linuxFCodes {
		m[c] = fKey(uint8(i + 1))
	}
	return m
}()

// linuxCodeOfKey is the evdev code of a regular key.
func linuxCodeOfKey(k hotkeyKey) (uint16, bool) {
	switch k.kind {
	case keySpace:
		return evKeySpace, true
	case keyEscape:
		return evKeyEsc, true
	case keyReturn:
		return evKeyEnter, true
	case keyTab:
		return evKeyTab, true
	case keyBackspace:
		return evKeyBackspace, true
	case keyLetter:
		return linuxLetterCodes[k.ch-'a'], true
	case keyDigit:
		return linuxDigitCodes[k.ch-'0'], true
	case keyF:
		return linuxFCodes[k.n-1], true
	case keyPunct:
		if i := strings.IndexByte(punctuationKeys, k.ch); i >= 0 {
			return linuxPunctuationCodes[i], true
		}
	}
	return 0, false
}

func linuxSoloFromCode(code uint16) (soloModifier, bool) {
	for m, c := range linuxSoloCodes {
		if c == code {
			return m, true
		}
	}
	return 0, false
}

// keysymName is the XKB keysym name of a key, as the shortcuts
// specification and GNOME accelerators use it.
func keysymName(k hotkeyKey) string {
	switch k.kind {
	case keySpace:
		return "space"
	case keyEscape:
		return "Escape"
	case keyReturn:
		return "Return"
	case keyTab:
		return "Tab"
	case keyBackspace:
		return "BackSpace"
	case keyF:
		return "F" + k.String()[1:]
	case keyPunct:
		if i := strings.IndexByte(punctuationKeys, k.ch); i >= 0 {
			return punctuationKeysyms[i]
		}
	}
	return k.String()
}

// portalTrigger is the trigger text for the GlobalShortcuts portal, for
// example "CTRL+ALT+space".
func portalTrigger(h hotkey) string {
	var parts []string
	if h.mods.control {
		parts = append(parts, "CTRL")
	}
	if h.mods.option {
		parts = append(parts, "ALT")
	}
	if h.mods.shift {
		parts = append(parts, "SHIFT")
	}
	if h.mods.command {
		parts = append(parts, "LOGO")
	}
	return strings.Join(append(parts, keysymName(h.key)), "+")
}

var errLinuxFn = errors.New("the fn key cannot be part of a shortcut: Linux cannot see it")

// checkLinuxHotkey rejects what Linux cannot detect: anything with Fn.
func checkLinuxHotkey(value string) error {
	if value == "" {
		return nil
	}
	h, err := parseHotkey(value)
	if err != nil {
		return err
	}
	if (h.kind == hotkeyChord && h.mods.function) || (h.kind == hotkeyModifierOnly && h.keys.has(modFn)) {
		return errLinuxFn
	}
	return nil
}
