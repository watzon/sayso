package i18n

import "testing"

func TestIsRTLTag(t *testing.T) {
	for tag, want := range map[string]bool{
		"ar": true, "ar-EG": true, "AR": true, "he": true, "he-IL": true, "fa_IR": true, "ur": true,
		"en": false, "pt-BR": false, "zh-CN": false, "": false, Pseudo: false, "arn": false,
	} {
		if got := IsRTLTag(tag); got != want {
			t.Errorf("IsRTLTag(%q) = %v, want %v", tag, got, want)
		}
	}
}

func TestIsRTLFollowsOverrideThenEnvThenLocale(t *testing.T) {
	t.Cleanup(func() { OverrideDirection(""); SetLocale(DefaultLocale) })
	SetLocale("en")
	t.Setenv("MYGO_DIRECTION", "")
	if IsRTL() {
		t.Fatal("English reads left to right")
	}
	OverrideDirection("rtl")
	if !IsRTL() {
		t.Fatal(`OverrideDirection("rtl") is right to left`)
	}
	t.Setenv("MYGO_DIRECTION", "rtl")
	OverrideDirection("ltr")
	if IsRTL() {
		t.Fatal("the override wins over MYGO_DIRECTION")
	}
	OverrideDirection("")
	if !IsRTL() {
		t.Fatal("MYGO_DIRECTION=rtl lays English out right to left")
	}
}

func TestInterpolationIsolatesValuesRightToLeftOnly(t *testing.T) {
	t.Cleanup(func() { OverrideDirection("") })
	vars := map[string]any{"hotkey": "⌘F", "count": 3}
	OverrideDirection("ltr")
	if got := interpolate("Press %{hotkey} (%{count})", vars); got != "Press ⌘F (3)" {
		t.Errorf("left to right: %q", got)
	}
	OverrideDirection("rtl")
	if got, want := interpolate("Press %{hotkey} (%{count})", vars), "Press \u2068⌘F\u2069 (3)"; got != want {
		t.Errorf("right to left: %q, want %q", got, want)
	}
	for _, v := range []string{"3", "1,234.5", "سبتمبر", "28 سبتمبر"} {
		if got := isolateArg(v); got != v {
			t.Errorf("isolateArg(%q) = %q, want it bare", v, got)
		}
	}
	// A sign before or after a number would move to its other side right
	// to left ("+5" shown as "5+").
	for _, v := range []string{"+5", "-3", "−2", "+1.5%", "5-10"} {
		if got := isolateArg(v); got != fsi+v+pdi {
			t.Errorf("isolateArg(%q) = %q, want it isolated", v, got)
		}
	}
	for _, v := range []string{"⌥", "⌥ Space", "parakeet-tdt-v3", "~/Library", "Safari", "0.16.0 (beta)"} {
		if got := isolateArg(v); got != fsi+v+pdi {
			t.Errorf("isolateArg(%q) = %q, want it isolated", v, got)
		}
	}
	if got := Isolate("~/Library"); got != "\u2066~/Library\u2069" {
		t.Errorf("Isolate: %q", got)
	}
	OverrideDirection("ltr")
	if got := Isolate("~/Library"); got != "~/Library" {
		t.Errorf("Isolate left to right: %q", got)
	}
}
