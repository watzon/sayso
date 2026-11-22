package platform

// macOS virtual key codes (ANSI layout) and CGEventFlags, mapped to the
// platform-neutral hotkey types. Ported from sayso-platform-macos/src/keymap.rs.

const (
	kcEscape      uint16 = 53
	kcReturn      uint16 = 36
	kcKeypadEnter uint16 = 76
	kcV           uint16 = 9
)

// macLetterCodes are the key codes of a to z.
var macLetterCodes = [26]uint16{0, 11, 8, 2, 14, 3, 5, 4, 34, 38, 40, 37, 46, 45, 31, 35, 12, 15, 1, 17, 32, 9, 13, 7, 16, 6}

// macDigitCodes are the key codes of 0 to 9.
var macDigitCodes = [10]uint16{29, 18, 19, 20, 21, 23, 22, 26, 28, 25}

// macPunctuationCodes follow punctuationKeys: ` - = [ ] \ ; ' , . /
var macPunctuationCodes = [11]uint16{50, 27, 24, 33, 30, 42, 41, 39, 43, 47, 44}

// macFCodes are the key codes of F1 to F20.
var macFCodes = [20]uint16{122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111, 105, 107, 113, 106, 64, 79, 80, 90}

// macKeyFromCode maps a key code to a regular key. Keypad Enter is Return.
var macKeyFromCode = func() map[uint16]hotkeyKey {
	m := map[uint16]hotkeyKey{
		49:            {kind: keySpace},
		kcEscape:      {kind: keyEscape},
		kcReturn:      {kind: keyReturn},
		kcKeypadEnter: {kind: keyReturn},
		48:            {kind: keyTab},
		51:            {kind: keyBackspace},
	}
	for i, c := range macLetterCodes {
		m[c] = letterKey(byte('a' + i))
	}
	for i, c := range macDigitCodes {
		m[c] = digitKey(byte(i))
	}
	for i, c := range macPunctuationCodes {
		m[c] = punctKey(punctuationKeys[i])
	}
	for i, c := range macFCodes {
		m[c] = fKey(uint8(i + 1))
	}
	return m
}()

// macSoloSpec is how one modifier key shows in flagsChanged events: its key
// code, and the device-dependent flag bit that is set while that exact key is
// down (so Left Option does not match Right Option). Fn uses SecondaryFn.
type macSoloSpec struct {
	code    uint16
	downBit uint64
}

var macSoloSpecs = map[soloModifier]macSoloSpec{
	modRightOption:  {61, 0x40},
	modLeftOption:   {58, 0x20},
	modRightCommand: {54, 0x10},
	modLeftCommand:  {55, 0x08},
	modRightControl: {62, 0x2000},
	modLeftControl:  {59, 0x01},
	modRightShift:   {60, 0x04},
	modLeftShift:    {56, 0x02},
	modFn:           {63, cgFlagSecondaryFn},
}

func macSoloFromCode(code uint16) (soloModifier, bool) {
	for m, s := range macSoloSpecs {
		if s.code == code {
			return m, true
		}
	}
	return 0, false
}

// macHeldModifiers are the side-specific modifiers down in flags.
func macHeldModifiers(flags uint64) modifierSet {
	var s modifierSet
	for m, spec := range macSoloSpecs {
		if flags&spec.downBit != 0 {
			s |= setOf(m)
		}
	}
	return s
}

// macModifiers reads the side-independent modifier flags.
func macModifiers(flags uint64) modifiers {
	return modifiers{
		command:  flags&cgFlagCommand != 0,
		option:   flags&cgFlagAlt != 0,
		control:  flags&cgFlagControl != 0,
		shift:    flags&cgFlagShift != 0,
		function: flags&cgFlagSecondaryFn != 0,
	}
}

// macRawEvent turns the fields of one CGEvent into an engine event.
func macRawEvent(etype uint32, code uint16, flags uint64, repeat, secure bool) (rawKeyEvent, bool) {
	ev := rawKeyEvent{code: code, held: macHeldModifiers(flags), mods: macModifiers(flags), repeat: repeat}
	switch etype {
	case kCGEventLeftMouseDown, kCGEventRightMouseDown, kCGEventOtherMouseDown, kCGEventScrollWheel:
		return rawKeyEvent{kind: rawPointer}, true
	case kCGEventKeyDown:
		ev.kind = rawKeyDown
	case kCGEventKeyUp:
		ev.kind = rawKeyUp
	case kCGEventFlagsChanged:
		ev.kind = rawModifiers
		ev.secure = secure
		ev.changed, ev.hasChanged = macSoloFromCode(code)
	default:
		return rawKeyEvent{}, false
	}
	ev.key = macKeyFromCode[code]
	return ev, true
}

// macEventMask holds keys, modifier changes, mouse buttons, and scrolls.
// Mouse motion alone must not cancel a modifier tap, so it is left out.
func macEventMask() uint64 {
	var m uint64
	for _, t := range []uint{kCGEventKeyDown, kCGEventKeyUp, kCGEventFlagsChanged,
		kCGEventLeftMouseDown, kCGEventRightMouseDown, kCGEventOtherMouseDown, kCGEventScrollWheel} {
		m |= 1 << t
	}
	return m
}
