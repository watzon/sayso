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
