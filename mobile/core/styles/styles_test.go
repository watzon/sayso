package styles

import (
	"strings"
	"testing"
)

// The desktop's style tests (internal/enhance/style_test.go) cover the
// prompts through its wrappers; these check the shared API.

func TestBuiltins(t *testing.T) {
	seen := map[string]bool{}
	for _, b := range Builtins {
		if !ValidID(b.ID) || seen[b.ID] || b.Name == "" {
			t.Fatalf("bad built-in %+v", b)
		}
		seen[b.ID] = true
		if (b.Prompt == "") != (b.ID == RawID) {
			t.Fatalf("only %q has no prompt, got %q", RawID, b.ID)
		}
	}
	if _, ok := Find(DefaultID); !ok {
		t.Fatal("the default style is not built in")
	}
	if _, ok := Find("nope"); ok {
		t.Fatal("found a style that does not exist")
	}
}

func TestSystemPrompt(t *testing.T) {
	p := SystemPrompt("  Be brief.  ", false, " base rules ", []string{"GitHub", "Kubernetes"}, "push it to git hub")
	for _, want := range []string{"Rules for all styles:\nbase rules", "Style (it decides the layout of the reply):\nBe brief.", "correct spelling is: GitHub."} {
		if !strings.Contains(p, want) {
			t.Errorf("prompt lacks %q:\n%s", want, p)
		}
	}
	if strings.Contains(p, "Kubernetes") {
		t.Error("a dictionary word the transcript cannot contain went in")
	}
	if strings.Contains(SystemPrompt("x", true, "base rules", nil, ""), "base rules") {
		t.Error("a standalone style got the base prompt")
	}
}

func TestValidID(t *testing.T) {
	for id, want := range map[string]bool{"clean": true, "my-style-2": true, "": false, "Caps": false, "a b": false, "é": false} {
		if ValidID(id) != want {
			t.Errorf("ValidID(%q) = %v", id, !want)
		}
	}
}
