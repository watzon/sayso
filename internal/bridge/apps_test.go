package bridge

import (
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"strings"
	"testing"
)

const appsExample = `
[[apps]]
name = "Notetaker"            # The name from the system, which Sayso shows.
team_id = "ABCDE12345"        # macOS, signed apps.
identifier = "app.justsayso.notetaker"
allowed = true
granted = ["session.live", "audio.mic", "audio.system", "transcribe.file"]
denied = ["dictation.observe"]
first_seen = 2026-10-29T10:00:00Z

[[apps]]
name = "transcribe"
path = "/opt/transcribe/bin/transcribe"
allowed = false
denied = ["transcribe.file"]
`

var signedNotetaker = Identity{Kind: IdentitySigned, TeamID: "ABCDE12345", Identifier: "app.justsayso.notetaker"}

func TestTheRecordOfTheDocsParses(t *testing.T) {
	apps, err := ParseApps(appsExample)
	if err != nil {
		t.Fatal(err)
	}
	r := apps.Find(signedNotetaker)
	if r == nil || r.Name != "Notetaker" || !r.Allowed {
		t.Fatalf("%+v", r)
	}
	if got := r.Capabilities(); !reflect.DeepEqual(got, []Capability{CapModelsRead, CapSessionLive, CapAudioMic, CapAudioSystem, CapTranscribeFile}) {
		t.Fatal(got)
	}
	if got := r.DeniedCapabilities(); !reflect.DeepEqual(got, []Capability{CapDictationObserve}) {
		t.Fatal(got)
	}
	byPath := apps.Find(Identity{Kind: IdentityPath, Path: "/opt/transcribe/bin/transcribe"})
	if byPath == nil || byPath.Allowed {
		t.Fatal(byPath)
	}
	if apps.Find(Identity{Kind: IdentityPath, Path: "/opt/other"}) != nil || !apps.AnyApproved() {
		t.Fatal("find")
	}
	enc := apps.Encode()
	if !strings.Contains(enc, "first_seen = 2026-10-29T10:00:00Z") {
		t.Fatal(enc)
	}
	again, err := ParseApps(enc)
	if err != nil || !reflect.DeepEqual(again.Apps[0].Granted, apps.Apps[0].Granted) {
		t.Fatal(err, enc)
	}
}

func TestRecordsWithNoOrTwoProgramsMatchNothing(t *testing.T) {
	apps, err := ParseApps("[[apps]]\nname = \"x\"\nallowed = true\n\n[[apps]]\nname = \"y\"\nteam_id = \"T\"\nallowed = true\n\n[[apps]]\nname = \"z\"\nteam_id = \"T\"\nidentifier = \"i\"\npath = \"/x\"\nallowed = true\n")
	if err != nil {
		t.Fatal(err)
	}
	for _, r := range apps.Apps {
		if _, ok := r.Identity(); ok {
			t.Fatal(r.Name)
		}
	}
	if apps.AnyApproved() {
		t.Fatal("approved")
	}
	both, _ := ParseApps("[[apps]]\nname = \"x\"\npath = \"/opt/x\"\nflatpak = \"dev.example.Notes\"\nallowed = true\n")
	if _, ok := both.Apps[0].Identity(); ok {
		t.Fatal("two identities")
	}
}

func TestCapabilityRules(t *testing.T) {
	apps, _ := ParseApps("[[apps]]\nname = \"x\"\npath = \"/x\"\nallowed = true\ngranted = [\"camera.read\", \"ai.enhance\", \"models.read\"]\n")
	if got := apps.Apps[0].Capabilities(); !reflect.DeepEqual(got, []Capability{CapModelsRead, CapAiEnhance}) {
		t.Fatal(got)
	}
	apps, _ = ParseApps("[[apps]]\nname = \"x\"\npath = \"/x\"\nallowed = true\ngranted = [\"audio.mic\", \"ai.enhance\"]\ndenied = [\"audio.mic\"]\n")
	if got := apps.Apps[0].Capabilities(); !reflect.DeepEqual(got, []Capability{CapModelsRead, CapAiEnhance}) {
		t.Fatal("denied wins", got)
	}
	ex, _ := ParseApps(appsExample)
	asked := []Capability{CapModelsRead, CapAudioMic, CapDictationObserve, CapAiEnhance, "camera.read", CapAiEnhance}
	if got := unanswered(ex.Find(signedNotetaker), asked); !reflect.DeepEqual(got, []Capability{CapAiEnhance}) {
		t.Fatal(got)
	}
	noAnswer, _ := ParseApps("[[apps]]\nname = \"x\"\npath = \"/x\"\ngranted = [\"audio.mic\"]\n")
	if noAnswer.Apps[0].Allowed {
		t.Fatal("no answer is not approved")
	}
	if _, err := ParseApps("[[apps]]\nname = \"x\"\nallowed = \"yes\"\n"); err == nil {
		t.Fatal("wrong type")
	}
	shown, _ := ParseApps("[[apps]]\nname = \"No\\u202Etes\\n\"\npath = \"/x\"\n\n[[apps]]\nname = \"\\u200B\"\npath = \"/y\"\n")
	if shown.Apps[0].ShownName() != "Notes" || shown.Apps[1].ShownName() != "?" {
		t.Fatal(shown.Apps[0].ShownName(), shown.Apps[1].ShownName())
	}
}

func TestTheFileIsReadAgainWhenItChanges(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "apps.toml")
	f := NewAppsFile(path)
	if len(f.Current().Apps) != 0 || f.IsBroken() {
		t.Fatal("a missing file has no answers")
	}
	os.WriteFile(path, []byte(appsExample), 0o600)
	if len(f.Current().Apps) != 2 {
		t.Fatal("read")
	}
	os.WriteFile(path, []byte(strings.Replace(appsExample, `, "audio.mic"`, "", 1)), 0o600)
	for _, c := range f.Current().Find(signedNotetaker).Capabilities() {
		if c == CapAudioMic {
			t.Fatal("not read again")
		}
	}
	os.WriteFile(path, []byte("[[apps]\nname = "), 0o600)
	if len(f.Current().Apps) != 0 || !f.IsBroken() {
		t.Fatal("broken")
	}
	os.Remove(path)
	if f.IsBroken() {
		t.Fatal("gone is not broken")
	}
	os.Mkdir(path, 0o700)
	if !NewAppsFile(path).IsBroken() {
		t.Fatal("a file that cannot be read is broken")
	}
}

func TestABrokenFileIsNeverWrittenOver(t *testing.T) {
	path := filepath.Join(t.TempDir(), "apps.toml")
	os.WriteFile(path, []byte("[[apps]\n"), 0o600)
	if err := NewAppsFile(path).answerFirst(CallerFromPath("/x"), nil, true); err == nil {
		t.Fatal("written")
	}
	if b, _ := os.ReadFile(path); string(b) != "[[apps]\n" {
		t.Fatal(string(b))
	}
}

func TestAnswersAndChangesGoToTheFile(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "sub", "apps.toml")
	f := NewAppsFile(path)
	changes := 0
	f.SetObserver(func() { changes++ })
	app := CallerFromPath("/opt/App")
	id := app.Identity.ID()
	if err := f.answerFirst(app, []Capability{CapAudioMic}, true); err != nil {
		t.Fatal(err)
	}
	r := NewAppsFile(path).Current().Record(id)
	if r == nil || !r.Allowed || !reflect.DeepEqual(r.Granted, []string{"audio.mic"}) || len(r.Denied) != 0 || r.FirstSeen == nil {
		t.Fatalf("%+v", r)
	}
	f.answerNew(id, []Capability{CapAiEnhance}, false)
	f.answerNew(id, []Capability{CapTranscribeFile}, true)
	r = f.Current().Record(id)
	if !reflect.DeepEqual(r.Capabilities(), []Capability{CapModelsRead, CapAudioMic, CapTranscribeFile}) || !reflect.DeepEqual(r.DeniedCapabilities(), []Capability{CapAiEnhance}) {
		t.Fatal(r.Capabilities(), r.DeniedCapabilities())
	}
	f.SetGrant(id, CapAudioMic, false)
	f.SetGrant(id, CapAiEnhance, true)
	r = f.Current().Record(id)
	if !reflect.DeepEqual(r.Capabilities(), []Capability{CapModelsRead, CapTranscribeFile, CapAiEnhance}) || !reflect.DeepEqual(r.DeniedCapabilities(), []Capability{CapAudioMic}) {
		t.Fatal(r.Capabilities(), r.DeniedCapabilities())
	}
	f.Remove(id)
	if f.Current().Record(id) != nil {
		t.Fatal("removed")
	}
	if changes != 6 {
		t.Fatal("changes", changes)
	}
	entries, _ := os.ReadDir(filepath.Dir(path))
	if len(entries) != 1 || entries[0].Name() != "apps.toml" {
		t.Fatal("a temporary file is left", entries)
	}
}

func TestAllowGrantsWhatTheUserRefused(t *testing.T) {
	f := NewAppsFile(filepath.Join(t.TempDir(), "apps.toml"))
	app := CallerFromPath("/opt/App")
	id := app.Identity.ID()
	f.answerFirst(app, []Capability{CapAudioMic, CapAiEnhance}, false)
	if r := f.Current().Record(id); r.Allowed || len(r.Capabilities()) != 1 || f.Current().AnyApproved() {
		t.Fatal("refused")
	}
	f.Allow(id)
	if r := f.Current().Record(id); !r.Allowed || !reflect.DeepEqual(r.Capabilities(), []Capability{CapModelsRead, CapAudioMic, CapAiEnhance}) {
		t.Fatal(r.Capabilities())
	}
}

func TestNamesAndIdentities(t *testing.T) {
	for in, want := range map[string]string{"Notes": "Notes", "  No\ttes\n": "Notes", "ev\u202Eil\u2066\u200F": "evil", "No\U000E0041\U000E0067tes\u180E\u0600\u3164\uFFA0\u2028": "Notes"} {
		if got, _ := CleanName(in); got != want {
			t.Errorf("%q: %q", in, got)
		}
	}
	if _, ok := CleanName("\u200B\uFEFF "); ok {
		t.Fatal("nothing left")
	}
	if n, _ := CleanName(strings.Repeat("é", 50)); len([]rune(n)) != 40 {
		t.Fatal(n)
	}
	if n, _ := CleanName(strings.Repeat("a", 39) + " x"); len([]rune(n)) != 39 {
		t.Fatal(n)
	}
	for _, name := range []string{"python3", "python3.12", "Python", "node", "node.exe", "bash", "zsh", "ruby", "perl5.34", "pwsh", "osascript", "pythonw.exe", "javaw.exe", "nodejs", "pypy3", "gawk", "mawk", "python3-intel64", "python3.12-intel64", "busybox", "wish", "wish8.6", "ld-linux-x86-64.so.2", "ld-linux-aarch64.so.1", "ld-musl-x86_64.so.1", "ld.so"} {
		if !isInterpreter(name) {
			t.Error(name)
		}
	}
	for _, name := range []string{"notes", "transcribe", "pythonista", "sayso-ext", "node-gyp-helper", "ldd", "wishlist"} {
		if isInterpreter(name) {
			t.Error(name)
		}
	}
	for summary, want := range map[string]string{"Developer ID Application: Example LLC (ABCDE12345)": "Example LLC", "Apple Development: Chris (XYZ)": "Chris (XYZ)", "Plain Name": "Plain Name"} {
		if got, _ := signerName(summary, "ABCDE12345"); got != want {
			t.Error(summary, got)
		}
	}
	signed := Identity{Kind: IdentitySigned, TeamID: "ABCDE12345", Identifier: "app.example"}
	if signed.ID() != "signed:ABCDE12345/app.example" {
		t.Fatal(signed.ID())
	}
	want := "path:/opt/App"
	if runtime.GOOS == "windows" {
		want = "path:/opt/app"
	}
	if got := (Identity{Kind: IdentityPath, Path: "/opt/App"}).ID(); got != want {
		t.Fatal(got)
	}
	if (Identity{Kind: IdentityPath, Path: "signed:TEAM/app.x"}).ID() == (Identity{Kind: IdentitySigned, TeamID: "TEAM", Identifier: "app.x"}).ID() {
		t.Fatal("a path is never a signed key")
	}
	if pathID("/x\xfe", false) == pathID("/x\xff", false) || pathID("/x\xff", false) == pathID("/x\uFFFD", false) {
		t.Fatal("paths that are not UTF-8 keep their own keys")
	}
	fp := CallerFromFlatpak("dev.example.Notes")
	if fp.Identity.ID() != "flatpak:dev.example.Notes" || fp.Name != "Notes" || fp.Interpreter {
		t.Fatalf("%+v", fp)
	}
	if got := VisiblePath("/opt/ev\u202Eil\nname\U000E0041"); got != "/opt/ev\uFFFDil\uFFFDname\uFFFD" {
		t.Fatal(got)
	}
	if fileName("/opt/ev\u202Eil\nname", false) != "evilname" {
		t.Fatal(fileName("/opt/ev\u202Eil\nname", false))
	}
	if shortReason("  Needs\na model\u202E for\U000E0041 notes ") != "Needs a model for notes" {
		t.Fatal(shortReason("  Needs\na model\u202E for\U000E0041 notes "))
	}
	if long := shortReason(strings.Repeat("x", 400)); !strings.HasSuffix(long, "...") || len(long) >= 400 {
		t.Fatal(long)
	}
}
