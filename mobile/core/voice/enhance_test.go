package voice

import (
	"strings"
	"testing"
)

func TestEnhanceRequest(t *testing.T) {
	sys, user := EnhanceRequest(Style{ID: "pirate", Prompt: "Talk like a pirate."}, []string{"GitHub", "Zebra"}, "push to git hub")
	if !strings.Contains(sys, "Talk like a pirate.") || !strings.Contains(sys, "Rules for all styles") || !strings.Contains(sys, "GitHub") || strings.Contains(sys, "Zebra") {
		t.Fatalf("system prompt:\n%s", sys)
	}
	if user != "<transcript>\npush to git hub\n</transcript>" {
		t.Fatalf("user %q", user)
	}
}

func TestCleanReply(t *testing.T) {
	for reply, want := range map[string]string{
		"  Hello.  ":                          "Hello.",
		"<transcript>\nHello.\n</transcript>": "Hello.",
		"Line one.\n\nLine two.":              "Line one.\n\nLine two.",
	} {
		if got, err := CleanReply("hello", reply); err != nil || got != want {
			t.Errorf("%q: %q %v", reply, got, err)
		}
	}
	for _, bad := range []string{"", "  <transcript></transcript> ", strings.Repeat("an answer ", 40)} {
		if _, err := CleanReply("hi", bad); err == nil {
			t.Errorf("accepted %q", bad)
		}
	}
}
