package platform

// Hotkey conflicts on macOS, ported from sayso-platform-macos/src/conflicts.rs.
//
// Two sources:
//  1. The system shortcuts, from CopySymbolicHotKeys. It lists the defaults
//     of macOS too (the plist does not), but gives no names, so the well
//     known ones are named by key and modifiers (Carbon mask).
//  2. A small table of apps that take a hotkey by default, checked against
//     the running apps. Carbon cannot see the hotkeys of other apps, so this
//     is a hint, confirmed where Sayso can read the app's setting.

import (
	"encoding/json"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/ebitengine/purego/objc"
)

// The Carbon modifier mask of CopySymbolicHotKeys (not CGEventFlags).
const (
	carbonCmd     uint32 = 1 << 8
	carbonShift   uint32 = 1 << 9
	carbonOption  uint32 = 1 << 11
	carbonControl uint32 = 1 << 12
	carbonAll            = carbonCmd | carbonShift | carbonOption | carbonControl
)

// symbolicHotkey is one row of CopySymbolicHotKeys.
type symbolicHotkey struct {
	keycode    int64
	carbonMods uint32
	enabled    bool
}

// carbonMask is the Carbon mask of m. Fn is left out: arrow and F keys set it.
func carbonMask(m modifiers) uint32 {
	var f uint32
	if m.command {
		f |= carbonCmd
	}
	if m.option {
		f |= carbonOption
	}
	if m.control {
		f |= carbonControl
	}
	if m.shift {
		f |= carbonShift
	}
	return f
}

// macKeyCode is the key code of k (Return, not keypad Enter).
func macKeyCode(k hotkeyKey) (uint16, bool) {
	if k.kind == keyReturn {
		return kcReturn, true
	}
	for code, key := range macKeyFromCode {
		if key == k {
			return code, true
		}
	}
	return 0, false
}

// macSystemLabels name the system shortcuts Sayso recognizes. The app shows
// each name in the interface language (settings.recorder.shortcut.*), so a
// new name goes there too. The comments are com.apple.symbolichotkeys ids.
var macSystemLabels = []struct {
	code uint16
	mask uint32
	name string
}{
	{49, carbonCmd, "Spotlight search"},                            // 64
	{49, carbonCmd | carbonOption, "Finder search window"},         // 65
	{49, carbonControl, "Select previous input source"},            // 60
	{49, carbonControl | carbonOption, "Select next input source"}, // 61
	{49, carbonControl | carbonCmd, "Emoji & Symbols"},
	{126, carbonControl, "Mission Control"},
	{125, carbonControl, "Application windows"},
	{123, carbonControl, "Move left a space"},
	{124, carbonControl, "Move right a space"},
	{20, carbonCmd | carbonShift, "Screenshot (whole screen)"},
	{21, carbonCmd | carbonShift, "Screenshot (selection)"},
	{23, carbonCmd | carbonShift, "Screenshot and recording options"},
	{50, carbonCmd, "Move focus to next window of the app"},
	{2, carbonCmd | carbonOption, "Dock: turn hiding on or off"},
}

func macSystemLabel(code int64, mask uint32) string {
	for _, l := range macSystemLabels {
		if int64(l.code) == code && l.mask == mask {
			return l.name
		}
	}
	return "A macOS keyboard shortcut"
}

// matchSystem are the system shortcuts with the key and modifiers of h.
// Modifier-only hotkeys and keys without a code never match.
func matchSystem(list []symbolicHotkey, h hotkey) []HotkeyConflict {
	if h.kind != hotkeyChord {
		return nil
	}
	code, ok := macKeyCode(h.key)
	if !ok {
		return nil
	}
	want := carbonMask(h.mods)
	var out []HotkeyConflict
	for _, e := range list {
		if e.keycode != int64(code) || e.carbonMods&carbonAll != want {
			continue
		}
		c := HotkeyConflict{Hotkey: h.String(), Name: macSystemLabel(e.keycode, want), Enabled: e.enabled}
		dup := false
		for _, o := range out {
			dup = dup || o == c
		}
		if !dup {
			out = append(out, c)
		}
	}
	return out
}

// appSetting is where an app keeps its hotkey, when Sayso can read it.
type appSetting int

const (
	settingNone appSetting = iota
	// settingRaycast: `defaults read com.raycast.macos raycastGlobalHotkey`,
	// a value like "Option-49".
	settingRaycast
	// settingCodex: the ChatGPT app's keybindings.json in $CODEX_HOME
	// (default ~/.codex).
	settingCodex
)

// knownApp is an app that uses a hotkey by default.
type knownApp struct {
	bundleIDs []string
	name      string
	hotkey    string
	setting   appSetting
}

var knownApps = []knownApp{
	{[]string{"com.openai.codex"}, "ChatGPT", "opt+space", settingCodex},
	// The older ChatGPT app keeps its shortcut in its sandbox container.
	// Reading it makes macOS ask for access to another app's data, so only
	// the default counts.
	{[]string{"com.openai.chat"}, "ChatGPT", "opt+space", settingNone},
	{[]string{"com.raycast.macos"}, "Raycast", "opt+space", settingRaycast},
	{[]string{"com.runningwithcrayons.Alfred"}, "Alfred", "opt+space", settingNone},
}

// settingValue is what an app's setting says: known lists the hotkeys the
// app uses (none when empty); !known means unread, so the default counts.
type settingValue struct {
	known   bool
	hotkeys []hotkey
}

var unknownSetting = settingValue{}

// parseDashedKeycode reads "Option-49" or "Command-Shift-2". An empty value
// means no hotkey.
func parseDashedKeycode(value string) settingValue {
	value = strings.TrimSpace(value)
	if value == "" {
		return settingValue{known: true}
	}
	parts := strings.Split(value, "-")
	n, err := strconv.ParseUint(parts[len(parts)-1], 10, 16)
	if err != nil {
		return unknownSetting
	}
	key, ok := macKeyFromCode[uint16(n)]
	if !ok {
		return unknownSetting
	}
	h := hotkey{kind: hotkeyChord, key: key}
	for _, m := range parts[:len(parts)-1] {
		switch strings.ToLower(m) {
		case "command", "cmd":
			h.mods.command = true
		case "option", "alt":
			h.mods.option = true
		case "control", "ctrl":
			h.mods.control = true
		case "shift":
			h.mods.shift = true
		case "function", "fn":
			h.mods.function = true
		default:
			return unknownSetting
		}
	}
	return settingValue{known: true, hotkeys: []hotkey{h}}
}

// codexGlobalCommands are the ChatGPT app's commands with a system-wide
// shortcut, and the macOS default of each. Only "Show Mini" has one.
var codexGlobalCommands = []struct {
	command, def string
}{
	{"openAvatarOverlay", "Alt+Space"},
	{"hotkeyWindow", ""},
	{"globalDictationHold", ""},
	{"globalDictationSingleTap", ""},
	{"realtimeVoice", ""},
}

// parseAccelerator reads an Electron accelerator such as "Alt+Space" or
// "CommandOrControl+Shift+K"; false for keys Sayso has no name for.
func parseAccelerator(acc string) (hotkey, bool) {
	tokens := strings.Split(acc, "+")
	for i, t := range tokens {
		switch t = strings.ToLower(strings.TrimSpace(t)); t {
		case "commandorcontrol", "cmdorctrl", "super", "meta":
			tokens[i] = "cmd" // all Command on macOS
		case "return":
			tokens[i] = "enter"
		default:
			tokens[i] = t
		}
	}
	h, err := parseHotkey(strings.Join(tokens, "+"))
	return h, err == nil
}

// parseCodexKeymap reads the hotkeys of a ChatGPT keybindings.json: a list
// of {command, key}. An entry replaces its command's default; "key": null
// turns it off. No file (nil) means every command has its default.
func parseCodexKeymap(text []byte) settingValue {
	type entry struct {
		Command string  `json:"command"`
		Key     *string `json:"key"`
	}
	var entries []entry
	if text != nil {
		var raw []json.RawMessage
		if json.Unmarshal(text, &raw) != nil {
			return unknownSetting
		}
		for _, r := range raw {
			var e entry
			if json.Unmarshal(r, &e) == nil && e.Command != "" {
				entries = append(entries, e)
			}
		}
	}
	out := settingValue{known: true}
	for _, cmd := range codexGlobalCommands {
		var keys []string
		set := false
		for _, e := range entries {
			if e.Command != cmd.command {
				continue
			}
			set = true
			if e.Key != nil {
				keys = append(keys, *e.Key)
			}
		}
		if !set && cmd.def != "" {
			keys = []string{cmd.def}
		}
		for _, k := range keys {
			if h, ok := parseAccelerator(k); ok {
				out.hotkeys = append(out.hotkeys, h)
			}
		}
	}
	return out
}

// matchKnownApps are the known apps that run now and use h. When the app's
// setting can be read only a real match counts; else its default counts,
// and the conflict is not confirmed.
func matchKnownApps(h hotkey, running map[string]bool, read func(appSetting) settingValue) []HotkeyConflict {
	var out []HotkeyConflict
	for _, app := range knownApps {
		found := false
		for _, id := range app.bundleIDs {
			found = found || running[id]
		}
		if !found {
			continue
		}
		v := unknownSetting
		if app.setting != settingNone {
			v = read(app.setting)
		}
		uses := false
		if v.known {
			for _, k := range v.hotkeys {
				uses = uses || k == h
			}
		} else if d, err := parseHotkey(app.hotkey); err == nil {
			uses = d == h
		}
		if uses {
			out = append(out, HotkeyConflict{Hotkey: h.String(), App: true, Name: app.name, Confirmed: v.known})
		}
	}
	return out
}

// fnConflict is what Fn does on its own (AppleFnUsageType). Unset counts as
// a conflict: the defaults differ between systems.
func fnConflict(usage int64, set bool) (HotkeyConflict, bool) {
	if !set {
		usage = 2
	}
	name := "A macOS keyboard shortcut"
	switch usage {
	case 0:
		return HotkeyConflict{}, false
	case 1:
		name = "Select next input source"
	case 2:
		name = "Emoji & Symbols"
	case 3:
		name = "Start Dictation (press twice)"
	}
	return HotkeyConflict{Hotkey: "fn", Name: name, Enabled: true}, true
}

// Conflicts are the system shortcuts, then the running apps, that use hotkey.
func (*macHotkeys) Conflicts(text string) []HotkeyConflict {
	h, err := parseHotkey(text)
	if err != nil || loadDarwin() != nil {
		return nil
	}
	out := matchSystem(macSymbolicHotkeys(), h)
	if h.kind == hotkeyModifierOnly && h.keys == setOf(modFn) {
		usage, err := strconv.ParseInt(defaultsRead("com.apple.HIToolbox", "AppleFnUsageType"), 10, 64)
		if c, ok := fnConflict(usage, err == nil); ok {
			out = append(out, c)
		}
	}
	return append(out, matchKnownApps(h, macRunningBundleIDs(), readAppSetting)...)
}

// defaultsRead is `defaults read domain key`, trimmed; "" when unset.
func defaultsRead(domain, key string) string {
	out, err := exec.Command("/usr/bin/defaults", "read", domain, key).Output()
	if err != nil {
		return ""
	}
	return strings.TrimSpace(string(out))
}

func readAppSetting(s appSetting) settingValue {
	switch s {
	case settingRaycast:
		out, err := exec.Command("/usr/bin/defaults", "read", "com.raycast.macos", "raycastGlobalHotkey").Output()
		if err != nil {
			return unknownSetting
		}
		return parseDashedKeycode(string(out))
	case settingCodex:
		home := os.Getenv("CODEX_HOME")
		if home == "" {
			h, err := os.UserHomeDir()
			if err != nil {
				return unknownSetting
			}
			home = filepath.Join(h, ".codex")
		}
		text, err := os.ReadFile(filepath.Join(home, "keybindings.json"))
		switch {
		case errors.Is(err, os.ErrNotExist):
			return parseCodexKeymap(nil)
		case err != nil:
			return unknownSetting
		}
		return parseCodexKeymap(text)
	}
	return unknownSetting
}

// macSymbolicHotkeys is the live list of system shortcuts; empty on failure.
func macSymbolicHotkeys() []symbolicHotkey {
	var raw uintptr
	if st := copySymbolicHotKeys(&raw); st != 0 || raw == 0 {
		return nil
	}
	defer cfRelease(raw)
	var out []symbolicHotkey
	withPool(func() {
		rows := objc.ID(raw)
		n := objc.Send[uint64](rows, sel("count"))
		codeKey, modsKey, onKey := nsString("kHISymbolicHotKeyCode"), nsString("kHISymbolicHotKeyModifiers"), nsString("kHISymbolicHotKeyEnabled")
		for i := uint64(0); i < n; i++ {
			row := rows.Send(sel("objectAtIndex:"), i)
			s := symbolicHotkey{keycode: -1}
			if v := row.Send(sel("objectForKey:"), codeKey); v != 0 {
				s.keycode = objc.Send[int64](v, sel("longLongValue"))
			}
			if v := row.Send(sel("objectForKey:"), modsKey); v != 0 {
				s.carbonMods = uint32(objc.Send[int64](v, sel("longLongValue")))
			}
			if v := row.Send(sel("objectForKey:"), onKey); v != 0 {
				s.enabled = objc.Send[bool](v, sel("boolValue"))
			}
			out = append(out, s)
		}
	})
	return out
}

// macRunningBundleIDs are the bundle ids of the running apps.
func macRunningBundleIDs() map[string]bool {
	out := map[string]bool{}
	withPool(func() {
		apps := class("NSWorkspace").Send(sel("sharedWorkspace")).Send(sel("runningApplications"))
		n := objc.Send[uint64](apps, sel("count"))
		for i := uint64(0); i < n; i++ {
			id := apps.Send(sel("objectAtIndex:"), i).Send(sel("bundleIdentifier"))
			if id != 0 {
				out[goStringFromCF(uintptr(id))] = true
			}
		}
	})
	return out
}
