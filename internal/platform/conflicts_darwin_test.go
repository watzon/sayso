package platform

import (
	"reflect"
	"testing"
)

// Ported from the tests of sayso-platform-macos/src/conflicts.rs.

func hk(t *testing.T, s string) hotkey {
	t.Helper()
	h, err := parseHotkey(s)
	if err != nil {
		t.Fatal(err)
	}
	return h
}

func sys(h, name string, on bool) HotkeyConflict {
	return HotkeyConflict{Hotkey: h, Name: name, Enabled: on}
}

func TestSpotlightIsFoundWithItsEnabledFlag(t *testing.T) {
	list := []symbolicHotkey{{49, 0x100, false}}
	got := matchSystem(list, hk(t, "cmd+space"))
	if want := []HotkeyConflict{sys("cmd+space", "Spotlight search", false)}; !reflect.DeepEqual(got, want) {
		t.Errorf("got %v, want %v", got, want)
	}
}

func TestCarbonMasksAreNotCGFlags(t *testing.T) {
	// 0x100000 is Cmd in CGEventFlags; in the Carbon mask it means nothing.
	if got := matchSystem([]symbolicHotkey{{49, 0x100000, true}}, hk(t, "cmd+space")); len(got) != 0 {
		t.Errorf("got %v", got)
	}
}

func TestSystemRowsGetTheirNames(t *testing.T) {
	list := []symbolicHotkey{{49, 0x1000 | 0x100, true}, {49, 0x1000, false}, {49, 0x1000 | 0x800, true}}
	for _, c := range []struct {
		h, name string
		on      bool
	}{
		{"ctrl+cmd+space", "Emoji & Symbols", true},
		{"ctrl+space", "Select previous input source", false},
		{"ctrl+opt+space", "Select next input source", true},
	} {
		got := matchSystem(list, hk(t, c.h))
		if want := []HotkeyConflict{sys(hk(t, c.h).String(), c.name, c.on)}; !reflect.DeepEqual(got, want) {
			t.Errorf("%s: got %v, want %v", c.h, got, want)
		}
	}
}

func TestSystemRowsIgnoreExtraBitsButNeedEveryModifier(t *testing.T) {
	list := []symbolicHotkey{{49, 0x100 | 0x80000, true}}
	if len(matchSystem(list, hk(t, "cmd+space"))) != 1 {
		t.Error("cmd+space not found")
	}
	if len(matchSystem(list, hk(t, "cmd+shift+space"))) != 0 || len(matchSystem(list, hk(t, "space"))) != 0 {
		t.Error("a row matched other modifiers")
	}
}

func TestUnknownRowsGetAGenericNameAndDuplicatesCollapse(t *testing.T) {
	list := []symbolicHotkey{{40, 0x100, true}, {40, 0x100, true}}
	got := matchSystem(list, hk(t, "cmd+k"))
	if want := []HotkeyConflict{sys("cmd+k", "A macOS keyboard shortcut", true)}; !reflect.DeepEqual(got, want) {
		t.Errorf("got %v", got)
	}
}

func TestFnConflictFollowsTheKeyboardSetting(t *testing.T) {
	if _, ok := fnConflict(0, true); ok {
		t.Error("Fn doing nothing conflicts")
	}
	for _, c := range []struct {
		usage int64
		set   bool
		name  string
	}{{1, true, "Select next input source"}, {2, true, "Emoji & Symbols"}, {3, true, "Start Dictation (press twice)"}, {0, false, "Emoji & Symbols"}} {
		got, ok := fnConflict(c.usage, c.set)
		if !ok || got.Name != c.name || !got.Enabled {
			t.Errorf("%d/%v: %v %v", c.usage, c.set, got, ok)
		}
	}
}

func unknown(appSetting) settingValue { return unknownSetting }

func appNames(hits []HotkeyConflict) []string {
	var out []string
	for _, h := range hits {
		s := h.Name
		if h.Confirmed {
			s += "!"
		}
		out = append(out, s)
	}
	return out
}

func running(ids ...string) map[string]bool {
	m := map[string]bool{}
	for _, id := range ids {
		m[id] = true
	}
	return m
}

func TestSoloModifiersNeverConflict(t *testing.T) {
	if len(matchSystem([]symbolicHotkey{{61, 0, true}}, hk(t, "right_option"))) != 0 {
		t.Error("system")
	}
	if len(matchKnownApps(hk(t, "right_option"), running("com.openai.chat"), unknown)) != 0 {
		t.Error("apps")
	}
}

func TestKnownAppsMatchOnlyWhenRunning(t *testing.T) {
	toggle := hk(t, "opt+space")
	if len(matchKnownApps(toggle, running("com.apple.finder"), unknown)) != 0 {
		t.Error("Finder conflicts")
	}
	got := appNames(matchKnownApps(toggle, running("com.apple.finder", "com.raycast.macos", "com.openai.chat"), unknown))
	if !reflect.DeepEqual(got, []string{"ChatGPT", "Raycast"}) {
		t.Errorf("got %v", got)
	}
	if got := appNames(matchKnownApps(toggle, running("com.openai.codex"), unknown)); !reflect.DeepEqual(got, []string{"ChatGPT"}) {
		t.Errorf("codex: got %v", got)
	}
	if len(matchKnownApps(hk(t, "ctrl+cmd+v"), running("com.openai.chat"), unknown)) != 0 {
		t.Error("an app matched a key it does not use")
	}
}

func TestAReadSettingReplacesTheDefault(t *testing.T) {
	toggle := hk(t, "opt+space")
	ray := running("com.raycast.macos")
	moved := func(appSetting) settingValue { return settingValue{known: true, hotkeys: []hotkey{hk(t, "cmd+space")}} }
	same := func(appSetting) settingValue { return settingValue{known: true, hotkeys: []hotkey{toggle}} }
	off := func(appSetting) settingValue { return settingValue{known: true} }
	if len(matchKnownApps(toggle, ray, moved)) != 0 {
		t.Error("moved Raycast conflicts")
	}
	if got := appNames(matchKnownApps(toggle, ray, same)); !reflect.DeepEqual(got, []string{"Raycast!"}) {
		t.Errorf("same: %v", got)
	}
	if got := appNames(matchKnownApps(hk(t, "cmd+space"), ray, moved)); !reflect.DeepEqual(got, []string{"Raycast!"}) {
		t.Errorf("moved to the user's key: %v", got)
	}
	if len(matchKnownApps(toggle, ray, off)) != 0 {
		t.Error("Raycast with its hotkey off conflicts")
	}
}

func TestDashedKeycodesParse(t *testing.T) {
	one := func(s string) settingValue { return settingValue{known: true, hotkeys: []hotkey{hk(t, s)}} }
	for in, want := range map[string]settingValue{
		"Command-49":      one("cmd+space"),
		"Option-49\n":     one("opt+space"),
		"Control-Shift-2": one("ctrl+shift+d"),
		"":                {known: true},
		"Hyper-49":        unknownSetting,
		"Option-x":        unknownSetting,
	} {
		if got := parseDashedKeycode(in); !reflect.DeepEqual(got, want) {
			t.Errorf("%q: got %v, want %v", in, got, want)
		}
	}
}

func TestElectronAcceleratorsParse(t *testing.T) {
	for in, want := range map[string]string{
		"Alt+Space": "opt+space", "CommandOrControl+Shift+K": "cmd+shift+k",
		"Super+Alt+P": "cmd+opt+p", "Ctrl+Option+D": "ctrl+opt+d",
	} {
		if got, ok := parseAccelerator(in); !ok || got != hk(t, want) {
			t.Errorf("%s: %v %v", in, got, ok)
		}
	}
	if _, ok := parseAccelerator("Hyper+Space"); ok {
		t.Error("Hyper parsed")
	}
}

func TestCodexKeymapUsesDefaultsAndOverrides(t *testing.T) {
	keys := func(ss ...string) settingValue {
		v := settingValue{known: true}
		for _, s := range ss {
			v.hotkeys = append(v.hotkeys, hk(t, s))
		}
		return v
	}
	cases := []struct {
		in   []byte
		want settingValue
	}{
		{nil, keys("opt+space")},
		{[]byte(`[{"command": "openAvatarOverlay", "key": null}]`), keys()},
		{[]byte(`[{"command":"openAvatarOverlay","key":"CmdOrCtrl+Shift+Space"},
			{"command":"globalDictationHold","key":"Ctrl+Alt+D"},
			{"command":"newTask","key":"Alt+Space"}]`), keys("cmd+shift+space", "ctrl+opt+d")},
		{[]byte(`[{"command":"globalDictationHold","key":"Hyper"}]`), keys("opt+space")},
		{[]byte("{not json"), unknownSetting},
	}
	for _, c := range cases {
		if got := parseCodexKeymap(c.in); !reflect.DeepEqual(got, c.want) {
			t.Errorf("%s: got %v, want %v", c.in, got, c.want)
		}
	}
}

func TestChatGPTCountsOnlyWhenItsKeymapSaysSo(t *testing.T) {
	toggle := hk(t, "opt+space")
	codex := running("com.openai.codex")
	off := func(appSetting) settingValue {
		return parseCodexKeymap([]byte(`[{"command":"openAvatarOverlay","key":null}]`))
	}
	if len(matchKnownApps(toggle, codex, off)) != 0 {
		t.Error("ChatGPT with Show Mini off conflicts")
	}
	def := func(appSetting) settingValue { return parseCodexKeymap(nil) }
	if got := appNames(matchKnownApps(toggle, codex, def)); !reflect.DeepEqual(got, []string{"ChatGPT!"}) {
		t.Errorf("got %v", got)
	}
}

func TestEveryKnownAppHotkeyParses(t *testing.T) {
	for _, a := range knownApps {
		if _, err := parseHotkey(a.hotkey); err != nil {
			t.Errorf("%s: %v", a.name, err)
		}
	}
}
