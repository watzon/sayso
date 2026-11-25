package config

import (
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"

	"github.com/watzon/sayso/internal/models"
)

// The golden files come from the Rust crate (sayso-core Config::to_toml) on
// macOS, so they check the exact text there.
func golden(t *testing.T, name string) string {
	t.Helper()
	if runtime.GOOS != "darwin" {
		t.Skip("the golden files hold macOS defaults")
	}
	b, err := os.ReadFile(filepath.Join("testdata", name))
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

func diff(t *testing.T, got, want string) {
	t.Helper()
	if got == want {
		return
	}
	g, w := strings.Split(got, "\n"), strings.Split(want, "\n")
	for i := 0; i < len(g) || i < len(w); i++ {
		var a, b string
		if i < len(g) {
			a = g[i]
		}
		if i < len(w) {
			b = w[i]
		}
		if a != b {
			t.Fatalf("line %d differs:\n got: %q\nwant: %q\n\nfull output:\n%s", i+1, a, b, got)
		}
	}
}

func TestDefaultMatchesRustOutput(t *testing.T) {
	diff(t, Default().ToTOML(), golden(t, "default.darwin.golden.toml"))
}

func TestRichFileRoundTripMatchesRust(t *testing.T) {
	want := golden(t, "rich.darwin.golden.toml")
	text, _ := os.ReadFile("testdata/rich.toml")
	loaded := Parse(string(text))
	diff(t, loaded.Config.ToTOML(), want)
	var fields []string
	for _, i := range loaded.Issues {
		fields = append(fields, i.Field+" "+i.Problem.Error())
	}
	if strings.Join(fields, "|") != `style_hotkeys.notes "ctrl+banana" is not a hotkey|style_hotkeys.raw "3" is not a hotkey` {
		t.Fatal(fields)
	}
	// The written file reads back to the same config.
	again := Parse(loaded.Config.ToTOML())
	diff(t, again.Config.ToTOML(), want)
}

func TestArraysRoundTripMatchesRust(t *testing.T) {
	want := golden(t, "arrays.darwin.golden.toml")
	text, _ := os.ReadFile("testdata/arrays.toml")
	loaded := Parse(string(text))
	diff(t, loaded.Config.ToTOML(), want)
	if loaded.Issues[0].Problem.Error() != `"["a", "b"]" is not a hotkey` {
		t.Fatal(loaded.Issues[0].Problem.Error())
	}
}

func TestEmptyFileGivesDefaults(t *testing.T) {
	l := Parse("")
	if len(l.Issues) != 0 || l.Config.ToTOML() != Default().ToTOML() {
		t.Fatal(l.Issues)
	}
}

func TestMissingFileIsCreatedAndUnreadableIsReported(t *testing.T) {
	dir := t.TempDir()
	l, err := Load(filepath.Join(dir, "config.toml"))
	if err != nil || !l.Created {
		t.Fatal(l, err)
	}
	bad := filepath.Join(dir, "bad.toml")
	os.WriteFile(bad, []byte{0xff, 0xfe, 0x00}, 0o644)
	_, err = Load(bad)
	if err == nil {
		t.Fatal("non-UTF-8 file loaded")
	}
	u := Unreadable(bad, err)
	if u.Created || len(u.Issues) != 1 || !strings.Contains(u.Issues[0].Problem.Error(), "bad.toml") {
		t.Fatal(u)
	}
}

func TestClearedHotkeyStaysClearedAndMissingLineIsDefault(t *testing.T) {
	c := Default()
	c.Hotkeys.Toggle, c.Hotkeys.PasteLast = nil, nil
	text := c.ToTOML()
	if !strings.Contains(text, `toggle = ""`) {
		t.Fatal(text)
	}
	back := Parse(text)
	if len(back.Issues) != 0 || back.Config.Hotkeys.Toggle != nil || back.Config.Hotkeys.PasteLast != nil {
		t.Fatal(back.Issues)
	}
	l := Parse("[hotkeys]\npush_to_talk = \"right_option\"\n")
	if len(l.Issues) != 0 || *l.Config.Hotkeys.Toggle != ToggleDefault() || *l.Config.Hotkeys.PushToTalk != SoloKey(RightOption) {
		t.Fatal(l.Issues)
	}
}

func TestOneBadSectionKeepsTheOthers(t *testing.T) {
	l := Parse("[sounds]\nvolume = \"loud\"\n[history]\nkeep_audio_days = 7\n[dictation]\nmin_duration_ms = -5\n")
	if *l.Config.History.KeepAudioDays != 7 || l.Config.Sounds != Default().Sounds {
		t.Fatal("sections")
	}
	if len(l.Issues) != 2 || l.Issues[0].Field != "dictation" || l.Issues[1].Field != "sounds" {
		t.Fatal(l.Issues)
	}
	if l.Issues[1].Problem.Error() != `invalid type: string "loud", expected f32 (using defaults for this section)` {
		t.Fatal(l.Issues[1].Problem.Error())
	}
	if l.Issues[0].Problem.Error() != "invalid value: integer `-5`, expected u64 (using defaults for this section)" {
		t.Fatal(l.Issues[0].Problem.Error())
	}
}

func TestSyntaxErrorReportsAndUsesDefaults(t *testing.T) {
	l := Parse("[sounds\nvolume = 1")
	if l.Issues[0].Field != "config.toml" || l.Issues[0].Problem.Kind != ProblemSyntax || l.Config.ToTOML() != Default().ToTOML() {
		t.Fatal(l.Issues)
	}
}

func TestValidation(t *testing.T) {
	c := Default()
	c.Sounds.Volume = 3
	missing := "missing"
	c.AI.DefaultProvider = &missing
	c.Hotkeys.PushToTalk = c.Hotkeys.Toggle
	fields := map[string]Problem{}
	for _, i := range c.Validate() {
		fields[i.Field] = i.Problem
	}
	if _, ok := fields["sounds.volume"]; !ok {
		t.Fatal(fields)
	}
	if fields["hotkeys.push_to_talk"].Kind != ProblemPushToTalkIsToggle || fields["ai.default_provider"].Kind != ProblemUnknownProvider {
		t.Fatal(fields)
	}

	mod := Default()
	mod.Hotkeys.Toggle = hk(MustHotkey("left_control+left_option"))
	mod.Hotkeys.PushToTalk = hk(MustHotkey("left_control"))
	mod.Hotkeys.PasteLast, mod.Hotkeys.CycleStyle, mod.Hotkeys.Incognito = mod.Hotkeys.Toggle, mod.Hotkeys.Toggle, mod.Hotkeys.Toggle
	n := 0
	for _, i := range mod.Validate() {
		if strings.HasPrefix(i.Field, "hotkeys.") {
			n++
		}
	}
	if n != 4 {
		t.Fatal("want 4 hotkey conflicts, got", n)
	}
	valid := Parse("[hotkeys]\ntoggle = 'left_control'\npush_to_talk = 'ctrl+opt+d'\n")
	invalid := Parse("[hotkeys]\ntoggle = 'left_control'\npush_to_talk = 'left_control+left_option'\n")
	if len(valid.Issues) != 0 || len(invalid.Issues) != 1 || invalid.Issues[0].Problem.Kind != ProblemHotkeyConflictsWithToggle {
		t.Fatal(valid.Issues, invalid.Issues)
	}
}

func TestPreviewAndRoleModelsAreChecked(t *testing.T) {
	c := Default()
	c.Dictation.PreviewModel = models.ParseRoleChoice("no-such-model")
	if c.Validate()[0].Problem.Kind != ProblemUnknownModel {
		t.Fatal(c.Validate())
	}
	var batch models.ModelID
	for _, m := range models.Catalog() {
		if !m.LivePreview {
			batch = m.ID
			break
		}
	}
	c.Dictation.PreviewModel = models.RoleChoice{Model: batch}
	if i := c.Validate()[0]; i.Problem.Kind != ProblemNotAStreamingModel || i.Field != "dictation.preview_model" {
		t.Fatal(i)
	}
	c = Default()
	c.Models.Vad = models.RoleChoice{Model: models.DefaultModel()}
	c.Models.Diarization = models.ParseRoleChoice("silero-vad")
	issues := c.Validate()
	if len(issues) != 2 || issues[0].Field != "models.vad" || issues[1].Problem.Kind != ProblemNotADiarizationModel {
		t.Fatal(issues)
	}
	c.Models.Vad = models.ParseRoleChoice("basic")
	c.Models.Diarization = models.Auto
	if len(c.Validate()) != 0 {
		t.Fatal("basic VAD is a VAD")
	}
	if !strings.Contains(Default().ToTOML(), "[models]\nvad = \"auto\"\ndiarization = \"auto\"") {
		t.Fatal("models section")
	}
}

func TestSpeechProviderValidation(t *testing.T) {
	l := Parse(`[[speech.providers]]
id = "a:b"
name = "x"
kind = "open_ai"

[[speech.providers]]
id = "c"
name = "C"
kind = "open_ai_compatible"

[[speech.providers]]
id = "c"
name = "C2"
kind = "groq"
`)
	var msgs []string
	for _, i := range l.Issues {
		msgs = append(msgs, i.Problem.Error())
	}
	joined := strings.Join(msgs, "|")
	for _, want := range []string{"colon", "two providers", "base_url"} {
		if !strings.Contains(joined, want) {
			t.Fatal(joined)
		}
	}
}

func TestCodexCLIBecomesChatGPT(t *testing.T) {
	l := Parse("[[ai.providers]]\nid = \"codex\"\nname = \"Work\"\nkind = \"codex_cli\"\npath = \"/x\"\nmodel = \"gpt-5\"\n")
	p := l.Config.AI.Providers[0]
	if p.Kind != KindChatGPT || p.Name != "Work" || p.ModelName() != "gpt-5" {
		t.Fatalf("%+v", p)
	}
	text := l.Config.ToTOML()
	if !strings.Contains(text, "[[ai.chatgpt_accounts]]") || strings.Contains(text, "codex") && strings.Contains(text, "codex_cli") {
		t.Fatal(text)
	}
}

func TestUnknownProviderIsKeptAndNeverRuns(t *testing.T) {
	l := Parse("[[ai.providers]]\nid = \"f\"\nname = \"F\"\nkind = \"quantum\"\n\n[[ai.providers]]\nid = \"a\"\nname = \"A\"\nkind = \"apple_intelligence\"\n")
	if len(l.Config.AI.Providers) != 1 || len(l.Config.AI.UnknownProviders) != 1 {
		t.Fatal(l.Config.AI)
	}
	if !strings.Contains(l.Config.ToTOML(), `kind = "quantum"`) {
		t.Fatal("unknown provider dropped")
	}
	// An OpenAI-compatible entry without base_url is unknown, not a bad section.
	l = Parse("[[ai.providers]]\nid = \"o\"\nname = \"O\"\nkind = \"open_ai_compatible\"\n")
	if len(l.Issues) != 0 || len(l.Config.AI.UnknownProviders) != 1 {
		t.Fatal(l.Issues)
	}
}

func TestSaveIsAtomicAndBacksUpBeforeCleanupModels(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "config.toml")
	c := Default()
	if err := c.Save(path); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(dir, "config.tmp")); err == nil {
		t.Fatal("temp file left")
	}
	c.AI.Providers = append(c.AI.Providers, Provider{ID: "tidy", Name: "Tidy", Kind: KindCleanupModel, Model: strp("m")})
	if err := c.Save(path); err != nil {
		t.Fatal(err)
	}
	backup, err := os.ReadFile(path + ".before-cleanup-models")
	if err != nil || strings.Contains(string(backup), "cleanup_models") {
		t.Fatal("backup", err)
	}
	b, _ := os.ReadFile(path)
	if !strings.Contains(string(b), "[[ai.cleanup_models]]") {
		t.Fatal(string(b))
	}
	// One time only: a later save does not replace the backup.
	os.WriteFile(path+".before-cleanup-models", []byte("keep"), 0o644)
	c.Save(path)
	if b, _ := os.ReadFile(path + ".before-cleanup-models"); string(b) != "keep" {
		t.Fatal("backup replaced")
	}
}

func TestThinking(t *testing.T) {
	if got := SortThinking([]string{"high", "custom", "low", "low", "none"}); strings.Join(got, ",") != "none,low,high,custom" {
		t.Fatal(got)
	}
	if ThinkingLabel("xhigh") != "Extra high" || ThinkingLabel("medium") != "Medium" {
		t.Fatal("labels")
	}
	p := Provider{Kind: KindClaudeCLI}
	if p.ThinkingSent() != "low" {
		t.Fatal("cli default")
	}
	p.FitThinking([]string{"medium", "high"})
	if p.ThinkingChosen() != "medium" {
		t.Fatal(p.ThinkingChosen())
	}
	p.FitThinking(nil)
	if p.Thinking != nil {
		t.Fatal("no levels means none")
	}
	if (Provider{Kind: KindOpenAICompatible}).ThinkingSent() != "" {
		t.Fatal("http sends none by default")
	}
}

func TestMiscHelpers(t *testing.T) {
	if !IsLocalURL("http://localhost:11434/v1") || !IsLocalURL("http://box.local/x") || IsLocalURL("https://api.openai.com") {
		t.Fatal("IsLocalURL")
	}
	in := Insertion{TrailingSpace: true}
	if in.TextToInsert("hi") != "hi " || in.TextToInsert("hi\n") != "hi\n" || in.TextToInsert("") != "" {
		t.Fatal("trailing space")
	}
	if !(Onboarding{Step: 4}).ReachedKeys() || (Onboarding{Step: 3}).ReachedKeys() {
		t.Fatal("onboarding")
	}
	name := "  Chris "
	if (General{Name: &name}).DisplayName() != "Chris" {
		t.Fatal("name")
	}
	if ink, err := ParseInk("#aabbcc"); err != nil || ink.String() != "#AABBCC" {
		t.Fatal(ink, err)
	}
	h := Hotkeys{EnterSends: true, TakeEscape: true, Toggle: hk(MustHotkey("ctrl+return"))}
	if h.EnterSendsUsable() || !h.TakeEscapeUsable() {
		t.Fatal("usable")
	}
}

func TestStyleRules(t *testing.T) {
	r := StyleRules{Enabled: true, Rules: []StyleRule{
		{Style: "email", Apps: []string{"com.apple.mail"}, Sites: []string{"mail.google.com"}},
		{Style: "message", Apps: []string{"com.tinyspeck.slackmacgap"}, Sites: []string{"facebook.com/messages", "facebook.com"}},
	}}
	cases := []struct{ app, url, want string }{
		{"com.apple.mail", "", "email"},
		{"COM.APPLE.MAIL", "", "email"},
		{"com.google.Chrome", "https://www.facebook.com/messages/t/1", "message"},
		{"com.google.Chrome", "https://mail.google.com/mail/u/0", "email"},
		{"com.apple.mail", "https://example.com", "email"},
		{"x", "ftp://mail.google.com", ""},
	}
	for _, c := range cases {
		got, _ := r.Resolve(c.app, c.url)
		if got != c.want {
			t.Errorf("%s %s: got %q want %q", c.app, c.url, got, c.want)
		}
	}
	if got := SiteChoices("https://www.Facebook.com/messages/t/"); strings.Join(got, ",") != "facebook.com,facebook.com/messages" {
		t.Fatal(got)
	}
	tt := TypedTargets("com.apple.mail")
	if len(tt) != 2 || tt[0].IsSite {
		t.Fatal(tt)
	}
	r.SetTargets("notes", []string{"com.apple.mail"}, nil)
	if s, _ := r.StyleOfApp("com.apple.mail"); s != "notes" {
		t.Fatal(s)
	}
	var fresh StyleRules
	fresh.TurnOn(StarterRules("darwin", true))
	if !fresh.Seeded || len(fresh.Rules) != 2 {
		t.Fatal(fresh)
	}
	fresh.RemoveStyle("email")
	fresh.TurnOn(StarterRules("darwin", true))
	if len(fresh.Rules) != 1 {
		t.Fatal("starter added twice")
	}
}

func TestStyleHotkeys(t *testing.T) {
	c := Default()
	if strings.Contains(c.ToTOML(), "style_hotkeys") {
		t.Fatal("empty style hotkeys written")
	}
	c.StyleHotkeys.Set("email", hk(MustHotkey("ctrl+opt+e")))
	c.StyleHotkeys.Set("notes", hk(MustHotkey("right_shift")))
	c.StyleHotkeys.Set("raw", c.Hotkeys.Toggle)
	usable, refused := c.StyleHotkeyCheck()
	if len(usable) != 1 || len(refused) != 2 || refused[0].Problem.Kind != ProblemStyleHotkeyNeedsKey || refused[1].Problem.Kind != ProblemHotkeyInUse {
		t.Fatal(usable, refused)
	}
	back := Parse(c.ToTOML())
	if h, ok := back.Config.StyleHotkeys.Get("email"); !ok || h != MustHotkey("ctrl+opt+e") {
		t.Fatal("round trip")
	}
}

// A default provider that names no provider is reported and cleared on load,
// so the next provider the user adds becomes the default. An id of an entry
// this version does not know stays.
func TestDanglingDefaultProviderIsClearedOnLoad(t *testing.T) {
	l := Parse("[ai]\ndefault_provider = \"gone\"\n\n[[ai.providers]]\nid = \"o\"\nname = \"O\"\nkind = \"open_ai_compatible\"\nbase_url = \"http://localhost:1/v1\"\n")
	if len(l.Issues) != 1 || l.Issues[0].Field != "ai.default_provider" || l.Issues[0].Problem.Kind != ProblemUnknownProvider {
		t.Fatal(l.Issues)
	}
	if l.Config.AI.DefaultProvider != nil {
		t.Fatalf("default provider %q, want none", *l.Config.AI.DefaultProvider)
	}
	if _, ok := l.Config.AI.StyleProvider(nil); ok {
		t.Fatal("a style with no provider found one")
	}
	if p, ok := l.Config.AI.StyleProvider(new("o")); !ok || p.ID != "o" {
		t.Fatal(p, ok)
	}
	newer := Parse("[ai]\ndefault_provider = \"f\"\n\n[[ai.providers]]\nid = \"f\"\nname = \"F\"\nkind = \"quantum\"\n")
	if d := newer.Config.AI.DefaultProvider; d == nil || *d != "f" {
		t.Fatal("dropped the default of an unknown entry")
	}
}

func TestTelemetryIsOnUnlessTurnedOff(t *testing.T) {
	// A config from before [telemetry] existed, and one with the section but not the key.
	for _, text := range []string{"[updates]\ncheck = false\n", "[telemetry]\n"} {
		if l := Parse(text); !l.Config.Telemetry.Enabled || len(l.Issues) != 0 {
			t.Fatalf("%q: enabled %v, issues %v", text, l.Config.Telemetry.Enabled, l.Issues)
		}
	}
	off := Parse("[telemetry]\nenabled = false\n").Config
	if off.Telemetry.Enabled {
		t.Fatal("enabled = false read as on")
	}
	if !strings.Contains(off.ToTOML(), "[telemetry]\nenabled = false\n") || Parse(off.ToTOML()).Config.Telemetry.Enabled {
		t.Fatal("off does not survive a save")
	}
	if l := Parse("[telemetry]\nenabled = \"no\"\n"); !l.Config.Telemetry.Enabled || len(l.Issues) != 1 || l.Issues[0].Field != "telemetry" {
		t.Fatalf("a bad value: enabled %v, issues %v", l.Config.Telemetry.Enabled, l.Issues)
	}
}
