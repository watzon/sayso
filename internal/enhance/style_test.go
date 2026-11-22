package enhance

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/watzon/sayso/internal/config"
)

func builtin(id string) Style {
	for _, s := range BuiltinStyles() {
		if s.ID == id {
			return s
		}
	}
	panic(id)
}

// The expected files come from toml::to_string_pretty in the Rust app.
func TestStyleFilesMatchRust(t *testing.T) {
	sp := func(s string) *string { return &s }
	for _, s := range BuiltinStyles() {
		want, _ := os.ReadFile(filepath.Join("testdata/styles", s.ID+".toml"))
		if got := EncodeStyle(&s); got != string(want) {
			t.Errorf("%s:\n%s\nwant\n%s", s.ID, got, want)
		}
	}
	full := builtin("email")
	temp, timeout := float32(0.3), uint64(9000)
	full.Provider, full.Model, full.Temperature, full.TimeoutMs, full.Standalone = sp("openai"), sp("gpt-x"), &temp, &timeout, true
	want, _ := os.ReadFile("testdata/styles/email-full.toml")
	if got := EncodeStyle(&full); got != string(want) {
		t.Errorf("full:\n%s\nwant\n%s", got, want)
	}
	off := builtin("polished")
	off.Cleanup = CleanupField{State: CleanupOff}
	want, _ = os.ReadFile("testdata/styles/polished-off.toml")
	if got := EncodeStyle(&off); got != string(want) {
		t.Errorf("off:\n%s\nwant\n%s", got, want)
	}
	// Every file reads back as the same style.
	for _, s := range append(BuiltinStyles(), full, off) {
		back, err := ParseStyle(EncodeStyle(&s))
		if err != nil || EncodeStyle(back) != EncodeStyle(&s) {
			t.Errorf("%s does not round-trip: %v", s.ID, err)
		}
	}
}

func TestLibraryLoadOverrideReset(t *testing.T) {
	dir := t.TempDir()
	lib := LoadStyles(dir)
	var ids []string
	for _, s := range lib.Styles() {
		ids = append(ids, s.ID)
	}
	if strings.Join(ids, ",") != "clean,polished,rambling,message,email,notes,raw" {
		t.Fatal(ids)
	}
	if r, _ := lib.Get("raw"); r.UsesAI() {
		t.Fatal("raw uses AI")
	}
	msg := builtin("message")
	msg.Prompt = "Make it shorter."
	m := "tiny"
	msg.Model = &m
	if _, err := SaveStyle(dir, &msg); err != nil {
		t.Fatal(err)
	}
	lib = LoadStyles(dir)
	e := lib.Entries[3]
	if e.Origin != OriginOverridden || e.Style.Prompt != "Make it shorter." {
		t.Fatal(e)
	}
	ResetStyle(dir, "message")
	if LoadStyles(dir).Entries[3].Origin != OriginBuiltIn {
		t.Fatal("reset")
	}
	os.WriteFile(filepath.Join(dir, "slack.toml"), []byte("id = \"slack\"\nname = \"Slack\"\nprompt = \"Short.\"\n"), 0o644)
	os.WriteFile(filepath.Join(dir, "broken.toml"), []byte("id = "), 0o644)
	os.WriteFile(filepath.Join(dir, "bad-id.toml"), []byte("id = \"Bad Id\"\nname = \"x\"\n"), 0o644)
	lib = LoadStyles(dir)
	last := lib.Entries[len(lib.Entries)-1]
	if last.Style.ID != "slack" || last.Origin != OriginUser || len(lib.Errors) != 2 {
		t.Fatal(last, lib.Errors)
	}
}

func TestSystemPrompt(t *testing.T) {
	clean := builtin("clean")
	p := SystemPrompt(&clean, DefaultBasePrompt, []string{"Sayso", "GPUI"}, "open say so and the gpui docs")
	if !strings.Contains(p, "Sayso, GPUI") || !strings.Contains(p, "not a request to you") || strings.Contains(p, "JSON") {
		t.Fatal(p)
	}
	vocab := []string{"Sayso", "Pindrop", "ForgeCAD"}
	p = SystemPrompt(&clean, DefaultBasePrompt, vocab, "open pin drop and make a release")
	if !strings.Contains(p, "Their correct spelling is: Pindrop.") || strings.Contains(p, "ForgeCAD") {
		t.Fatal(p)
	}
	if strings.Contains(SystemPrompt(&clean, DefaultBasePrompt, vocab, "make sure the CI passes"), "spelling") {
		t.Fatal("no word fits")
	}
	p = SystemPrompt(&clean, "- Base rule.", nil, "hello")
	if strings.Index(p, "- Base rule.") > strings.Index(p, clean.Prompt) {
		t.Fatal(p)
	}
	if strings.Contains(SystemPrompt(&clean, "  ", nil, "hello"), "Rules for all styles") {
		t.Fatal("empty base")
	}
	clean.Standalone = true
	if strings.Contains(SystemPrompt(&clean, "- Base rule.", nil, "hello"), "Base rule") {
		t.Fatal("standalone")
	}
}

func TestBasePromptChangeAndReset(t *testing.T) {
	dir := t.TempDir()
	if LoadStyles(dir).BasePrompt() != DefaultBasePrompt {
		t.Fatal()
	}
	SaveBasePrompt(dir, "- Keep it short.\n")
	lib := LoadStyles(dir)
	if lib.BasePrompt() != "- Keep it short." || len(lib.Entries) != 7 || len(lib.Errors) != 0 {
		t.Fatal(lib.BasePrompt())
	}
	SaveBasePrompt(dir, "")
	if LoadStyles(dir).BasePrompt() != "" {
		t.Fatal()
	}
	ResetBasePrompt(dir)
	ResetBasePrompt(dir)
	if LoadStyles(dir).BasePrompt() != DefaultBasePrompt {
		t.Fatal()
	}
}

func TestCleanupKeyStates(t *testing.T) {
	read := func(text string) *Style {
		s, err := ParseStyle(text)
		if err != nil {
			t.Fatal(err)
		}
		return s
	}
	if s := read("id = \"x\"\nname = \"X\"\nprompt = \"p\"\n"); s.Cleanup.State != CleanupAbsent || s.CleanupSettingsOf() != nil {
		t.Fatal()
	}
	if s := read("id = \"x\"\nname = \"X\"\ncleanup = false\n"); s.Cleanup.State != CleanupOff {
		t.Fatal()
	}
	if s := read("id = \"x\"\nname = \"X\"\n\n[cleanup]\nformat = \"email\"\n"); *s.CleanupSettingsOf().Format != "email" {
		t.Fatal()
	}
	if s := read("id = \"x\"\nname = \"X\"\n\n[cleanup]\nlayout = \"table\"\n"); *s.CleanupSettingsOf().Layout != "table" {
		t.Fatal()
	}
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "message.toml"), []byte("id = \"message\"\nname = \"Chat\"\nprompt = \"Mine.\"\n"), 0o644)
	os.WriteFile(filepath.Join(dir, "email.toml"), []byte("id = \"email\"\nname = \"Mail\"\nprompt = \"Mine.\"\ncleanup = false\n"), 0o644)
	lib := LoadStyles(dir)
	m, _ := lib.Get("message")
	if *m.CleanupSettingsOf().Tone != "casual" || m.Cleanup.State != CleanupAbsent {
		t.Fatal()
	}
	if e, _ := lib.Get("email"); e.CleanupSettingsOf() != nil {
		t.Fatal()
	}
}

func TestRunPlan(t *testing.T) {
	cm := func(model string) config.Provider {
		return config.Provider{ID: "p", Kind: config.KindCleanupModel, Model: &model}
	}
	s1, sp := cm("s1-mini"), cm("spokenly-cleanup-1")
	llm := config.Provider{ID: "p", Kind: config.KindAppleIntelligence}
	plan := func(id string, p config.Provider) RunPlan { s := builtin(id); return s.RunPlanOn(p) }
	cases := []struct {
		got, want RunPlan
	}{
		{plan("clean", s1), RunPlan{PlanCleanup, Settings{}}},
		{plan("email", s1), RunPlan{PlanCleanup, Settings{Format: FormatEmail}}},
		{plan("notes", s1), RunPlan{PlanCleanup, Settings{Layout: LayoutList}}},
		{plan("email", sp), RunPlan{Kind: PlanCleanupOnly}},
		{plan("notes", sp), RunPlan{Kind: PlanCleanupOnly}},
		{plan("polished", s1), RunPlan{Kind: PlanCleanupOnly}},
		{plan("rambling", sp), RunPlan{Kind: PlanCleanupOnly}},
		{plan("message", sp), RunPlan{PlanCleanup, Settings{Tone: ToneCasual}}},
		{plan("raw", s1), RunPlan{}},
		{plan("raw", llm), RunPlan{}},
		{plan("clean", llm), RunPlan{Kind: PlanPrompt}},
		{plan("clean", cm("unknown-model")), RunPlan{}},
	}
	for i, c := range cases {
		if c.got != c.want {
			t.Errorf("%d: %+v", i, c.got)
		}
	}
	only := builtin("raw")
	only.Cleanup = CleanupField{State: CleanupSettingsSet}
	if only.IsRaw() || only.RunPlanOn(sp) != (RunPlan{PlanCleanup, Settings{}}) || only.RunPlanOn(llm) != (RunPlan{}) {
		t.Fatal("cleanup only style")
	}
}
