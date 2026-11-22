package update

import (
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

func key(seed byte) ed25519.PrivateKey {
	s := make([]byte, 32)
	for i := range s {
		s[i] = seed
	}
	return ed25519.NewKeyFromSeed(s)
}

func public(t *testing.T, id string, k ed25519.PrivateKey) PublicKey {
	pk, err := ParsePublicKey(id, base64.StdEncoding.EncodeToString(k.Public().(ed25519.PublicKey)))
	if err != nil {
		t.Fatal(err)
	}
	return pk
}

func payload(version string) string {
	b, _ := json.Marshal(map[string]any{
		"schema": 1, "version": version, "published": "2026-10-10T12:00:00Z",
		"notes_url": URLPrefix + "tag/v" + version,
		"assets": map[string]any{"macos-aarch64": map[string]any{
			"url": URLPrefix + "download/v" + version + "/Sayso-" + version + "-macos-arm64.dmg", "sha256": strings.Repeat("ab", 32), "size": 41234567}},
	})
	return string(b)
}

func envelope(p, id string, k ed25519.PrivateKey) []byte {
	b, _ := json.Marshal(Sign(p, id, k))
	return b
}

func TestSemver(t *testing.T) {
	for _, bad := range []string{"1.2", "01.2.3", "1.2.3-", "1.2.3+", "a.b.c", "1.2.3-01"} {
		if _, err := ParseVersion(bad); err == nil {
			t.Error(bad)
		}
	}
	order := []string{"1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta", "1.0.0-beta.2", "1.0.0-beta.11", "1.0.0-rc.1", "1.0.0", "1.0.1", "1.10.0"}
	for i := 1; i < len(order); i++ {
		if MustVersion(order[i-1]).CmpPrecedence(MustVersion(order[i])) >= 0 {
			t.Error(order[i-1], order[i])
		}
	}
	a, b := MustVersion("1.0.0+x"), MustVersion("1.0.0+y")
	if a.CmpPrecedence(b) != 0 || a.Equal(b) || a.String() != "1.0.0+x" {
		t.Fatal("build metadata")
	}
	if _, err := ParseVersion(Version); err != nil {
		t.Fatal("Version is not semver", err)
	}
}

func TestVerify(t *testing.T) {
	k := key(1)
	keys := []PublicKey{public(t, "2026-10", k)}
	kind, m, err := Verify(envelope(payload("0.3.0"), "2026-10", k), keys)
	if err != nil || kind != VerifiedManifest || m.Version.String() != "0.3.0" {
		t.Fatal(kind, err)
	}
	// Unknown key, unknown schema.
	if kind, _, err := Verify(envelope(payload("0.3.0"), "other", key(2)), keys); err != nil || kind != VerifiedUnknownKey {
		t.Fatal(kind, err)
	}
	p2 := strings.Replace(payload("0.3.0"), `"schema":1`, `"schema":2`, 1)
	if kind, _, _ := Verify(envelope(p2, "2026-10", k), keys); kind != VerifiedUnknownSchema {
		t.Fatal(kind)
	}
	// A known key with a wrong signature fails, also beside a good one.
	env := Sign(payload("0.3.0"), "2026-10", k)
	env.Signatures = append(env.Signatures, EnvelopeSignature{Key: "2026-10", Signature: base64.StdEncoding.EncodeToString(make([]byte, 64))})
	b, _ := json.Marshal(env)
	var me *ManifestError
	if _, _, err := Verify(b, keys); !errors.As(err, &me) || me.Kind != "bad_signature" {
		t.Fatal(err)
	}
	// Tampered payload.
	tampered := Sign(payload("0.3.0"), "2026-10", k)
	tampered.Payload = payload("9.9.9")
	b, _ = json.Marshal(tampered)
	if _, _, err := Verify(b, keys); !errors.As(err, &me) || me.Kind != "bad_signature" {
		t.Fatal(err)
	}
	// Unknown envelope fields, size, values that are not permitted.
	if _, _, err := Verify([]byte(`{"payload":"x","signatures":[],"extra":1}`), keys); !errors.As(err, &me) || me.Kind != "malformed" {
		t.Fatal(err)
	}
	if _, _, err := Verify(make([]byte, ManifestMaxBytes+1), keys); !errors.As(err, &me) || me.Kind != "too_large" {
		t.Fatal(err)
	}
	for _, p := range []string{
		payload("0.3.0-beta.1"),
		strings.Replace(payload("0.3.0"), "tag/v0.3.0", "../evil", 1),
		strings.Replace(payload("0.3.0"), URLPrefix+"download", "https://evil.example/download", 1),
		strings.Replace(payload("0.3.0"), strings.Repeat("ab", 32), strings.Repeat("AB", 32), 1),
		strings.Replace(payload("0.3.0"), "41234567", "0", 1),
	} {
		if _, _, err := Verify(envelope(p, "2026-10", k), keys); !errors.As(err, &me) || me.Kind != "not_permitted" {
			t.Errorf("%s: %v", p, err)
		}
	}
	if len(ReleasePublicKeys()) != 1 {
		t.Fatal("release key")
	}
}

func TestOffer(t *testing.T) {
	_, m, _ := Verify(envelope(payload("0.3.0"), "k", key(1)), []PublicKey{public(t, "k", key(1))})
	if o, _ := OfferFor(m, MustVersion("0.3.0"), nil, "macos-aarch64"); o != OfferUpToDate {
		t.Fatal(o)
	}
	if o, r := OfferFor(m, MustVersion("0.2.0"), nil, "macos-aarch64"); o != OfferNewer || r.Asset.Size != 41234567 {
		t.Fatal(o)
	}
	seen := MustVersion("0.4.0")
	if o, _ := OfferFor(m, MustVersion("0.2.0"), &seen, "macos-aarch64"); o != OfferReplayed {
		t.Fatal(o)
	}
	if o, _ := OfferFor(m, MustVersion("0.2.0"), nil, "windows-x86_64"); o != OfferUpToDate {
		t.Fatal(o)
	}
}

func TestStateAndRecord(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "update-state.json")
	if s := LoadState(path); s.HighestSeen != nil {
		t.Fatal("default")
	}
	var s UpdateState
	s.Saw(MustVersion("0.4.0"), 100)
	s.Saw(MustVersion("0.3.0"), 200)
	if s.HighestSeen.String() != "0.4.0" || *s.LastCheck != 200 {
		t.Fatal(s)
	}
	s.Save(path)
	data, _ := os.ReadFile(path)
	if string(data) != "{\n  \"highest_seen\": \"0.4.0\",\n  \"last_check\": 200,\n  \"notice\": null\n}" {
		t.Fatalf("%s", data)
	}
	if entries, _ := os.ReadDir(dir); len(entries) != 1 {
		t.Fatal("temp file left")
	}
	os.WriteFile(path, []byte("{"), 0o644)
	if LoadState(path).HighestSeen != nil {
		t.Fatal("broken file")
	}
	rp := filepath.Join(dir, "update.json")
	r := Record{FromVersion: MustVersion("0.1.0"), ToVersion: MustVersion("0.2.0"), InstallPath: "/Applications/Sayso.app", Phase: PhaseStaged, Attempts: 1}
	r.Fail(rp, Failure{Kind: "replayed", Detail: "0.1.5"})
	back := LoadRecord(rp)
	if back.Failure.Kind != "replayed" || !strings.Contains(*back.Error, "offered version 0.1.5") || back.Phase != PhaseFailed {
		t.Fatal(back)
	}
	data, _ = os.ReadFile(rp)
	if !strings.Contains(string(data), `"failure": {`) || !strings.Contains(string(data), `"kind": "replayed"`) {
		t.Fatalf("%s", data)
	}
	// A record of an older Sayso has no failure kind.
	os.WriteFile(rp, []byte(`{"envelope":"","from_version":"0.1.0","to_version":"0.2.0","install_path":"/x","phase":"failed","attempts":1,"error":"The update failed in some way."}`), 0o644)
	if b := LoadRecord(rp); b.Failure != nil || *b.Error != "The update failed in some way." {
		t.Fatal(b)
	}
	for _, f := range []Failure{{Kind: "busy"}, {Kind: "relaunch_and_restore", Detail: "a", Back: "b"}, {Kind: "disk", Detail: "full"}} {
		b, _ := json.Marshal(f)
		var g Failure
		if json.Unmarshal(b, &g) != nil || g != f {
			t.Error(string(b))
		}
	}
	if b, _ := json.Marshal(Failure{Kind: "busy"}); string(b) != `{"kind":"busy"}` {
		t.Fatal(string(b))
	}
}

func asset(url string, body []byte) Asset {
	sum := sha256.Sum256(body)
	return Asset{URL: url, SHA256: hex.EncodeToString(sum[:]), Size: uint64(len(body))}
}

func TestFetchAndDownload(t *testing.T) {
	body := []byte(strings.Repeat("sayso", 20000))
	var agent string
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		agent = r.UserAgent()
		switch r.URL.Path {
		case "/latest.json":
			w.Write([]byte("{}"))
		case "/big.json":
			w.Write(make([]byte, ManifestMaxBytes+100))
		case "/v1/Sayso-1.dmg", "/v1/same":
			w.Write(body)
		default:
			w.WriteHeader(404)
		}
	}))
	defer srv.Close()
	if b, err := Fetch(srv.URL+"/latest.json", false); err != nil || string(b) != "{}" || !strings.HasPrefix(agent, "Sayso/"+Version+" (") {
		t.Fatal(err, agent)
	}
	if b, _ := Fetch(srv.URL+"/big.json", false); len(b) != ManifestMaxBytes+1 {
		t.Fatal(len(b))
	}
	var ce *CheckError
	if _, err := Fetch(srv.URL+"/none", false); !errors.As(err, &ce) || ce.Kind != "not_found" {
		t.Fatal(err)
	}
	if _, err := Fetch(srv.URL+"/latest.json", true); !errors.As(err, &ce) || ce.Kind != "network" {
		t.Fatal("https only")
	}
	dir := t.TempDir()
	var cancel atomic.Bool
	var last uint64
	path, err := Download(asset(srv.URL+"/v1/Sayso-1.dmg", body), dir, false, &cancel, func(n uint64) { last = n })
	if err != nil || filepath.Base(path) != "Sayso-1.dmg" || last != uint64(len(body)) {
		t.Fatal(path, err)
	}
	bad := asset(srv.URL+"/v1/same", body)
	bad.SHA256 = strings.Repeat("0", 64)
	var de *DownloadError
	if _, err := Download(bad, dir, false, &cancel, func(uint64) {}); !errors.As(err, &de) || de.Kind != "mismatch" {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(dir, "same.part")); err == nil {
		t.Fatal("part left")
	}
	if DownloadFileName(Asset{URL: "https://x/../.."}) != "update" || DownloadFileName(Asset{URL: "https://x/a b$.dmg"}) != "ab.dmg" {
		t.Fatal("file name")
	}
}

func TestInstallLock(t *testing.T) {
	path := filepath.Join(t.TempDir(), "install.lock")
	a, waited, err := SharedLock(path)
	if err != nil || waited {
		t.Fatal(err)
	}
	rel, ok := a.TryExclusive()
	if !ok {
		t.Fatal("alone")
	}
	if _, ok := a.TryExclusive(); ok {
		t.Fatal("one holder in a process")
	}
	rel()
	b, _, _ := SharedLock(path)
	if _, ok := a.TryExclusive(); ok && os.PathSeparator == '/' {
		t.Fatal("another holder")
	}
	b.Close()
	if rel, ok := a.TryExclusive(); !ok {
		t.Fatal("after the other left")
	} else {
		rel()
	}
}

// fakeInstaller records what the swap does.
type fakeInstaller struct {
	dir                   string
	staged, verifyFails   bool
	relaunchErr           error
	exchanges, relaunches int
	installed             string
}

func (f *fakeInstaller) InstallPath() string { return filepath.Join(f.dir, "Sayso.app") }
func (f *fakeInstaller) LockPath() string    { return filepath.Join(f.dir, "install.lock") }
func (f *fakeInstaller) RecordPath() string  { return filepath.Join(f.dir, "update.json") }
func (f *fakeInstaller) InstalledVersion() (Semver, bool) {
	v, err := ParseVersion(f.installed)
	return v, err == nil
}
func (f *fakeInstaller) Stage(string, Semver) error { f.staged = true; return nil }
func (f *fakeInstaller) StagingExists() bool        { return f.staged }
func (f *fakeInstaller) VerifyStaged() error {
	if f.verifyFails {
		return errors.New("bad signature")
	}
	return nil
}
func (f *fakeInstaller) Exchange() error { f.exchanges++; return nil }
func (f *fakeInstaller) Relaunch() error { f.relaunches++; return f.relaunchErr }
func (f *fakeInstaller) RemoveStaging()  { f.staged = false }

func setup(t *testing.T) (*Context, *fakeInstaller, *InstallLock, ed25519.PrivateKey) {
	dir := t.TempDir()
	k := key(3)
	c := &Context{StatePath: filepath.Join(dir, "state.json"), Keys: []PublicKey{public(t, "k", k)}, URLPrefix: URLPrefix, Running: MustVersion("0.2.0")}
	f := &fakeInstaller{dir: dir, staged: true}
	lock, _, err := SharedLock(f.LockPath())
	if err != nil {
		t.Fatal(err)
	}
	return c, f, lock, k
}

func stage(f *fakeInstaller, k ed25519.PrivateKey, version string) {
	r := Record{Envelope: string(envelope(payload(version), "k", k)), FromVersion: MustVersion("0.2.0"), ToVersion: MustVersion(version), InstallPath: f.InstallPath(), Phase: PhaseStaged}
	r.Save(f.RecordPath())
}

func TestSwapAtStartAndConfirm(t *testing.T) {
	c, f, lock, k := setup(t)
	if AtStartup(c, f, lock).Kind != AtStartNothing {
		t.Fatal("no record")
	}
	f.staged = true
	stage(f, k, "0.3.0")
	if got := AtStartup(c, f, lock); got.Kind != AtStartRelaunching || f.exchanges != 1 || f.relaunches != 1 {
		t.Fatal(got, f)
	}
	r := LoadRecord(f.RecordPath())
	if r.Phase != PhaseSwapped || r.Attempts != 1 || !AwaitsConfirm(f) {
		t.Fatal(r)
	}
	// The new version starts and confirms.
	c.Running = MustVersion("0.3.0")
	if got := AtStartup(c, f, lock); got.Kind != AtStartUpdated {
		t.Fatal(got)
	}
	if !Confirm(c, f, lock) || LoadRecord(f.RecordPath()) != nil || LoadState(c.StatePath).Notice.String() != "0.3.0" {
		t.Fatal("confirm")
	}
}

func TestSwapFailures(t *testing.T) {
	c, f, lock, k := setup(t)
	// A record signed by an unknown key.
	r := Record{Envelope: string(envelope(payload("0.3.0"), "k", key(9))), ToVersion: MustVersion("0.3.0"), InstallPath: f.InstallPath(), Phase: PhaseStaged}
	r.Save(f.RecordPath())
	if err := Swap(c, f, lock); err.(Failure).Kind != "bad_signature" || f.exchanges != 0 || f.staged {
		t.Fatal(err)
	}
	if got := AtStartup(c, f, lock); got.Kind != AtStartFailed || got.Failure.Kind != "bad_signature" {
		t.Fatal(got)
	}
	// Not newer.
	f.staged = true
	stage(f, k, "0.2.0")
	if err := Swap(c, f, lock); err.(Failure).Kind != "not_newer" {
		t.Fatal(err)
	}
	// Verification of the staged app fails.
	f.staged, f.verifyFails = true, true
	stage(f, k, "0.3.0")
	if err := Swap(c, f, lock); err.(Failure).Kind != "verify" {
		t.Fatal(err)
	}
	// The relaunch fails: the old app goes back.
	f.staged, f.verifyFails, f.relaunchErr = true, false, errors.New("no open")
	stage(f, k, "0.3.0")
	if err := Swap(c, f, lock); err.(Failure).Kind != "relaunch" || f.exchanges != 2 {
		t.Fatal(err, f.exchanges)
	}
	// Busy while another holds the exclusive lock.
	f.staged = true
	stage(f, k, "0.3.0")
	rel, _ := lock.TryExclusive()
	if err := Swap(c, f, lock); !IsBusy(err) {
		t.Fatal(err)
	}
	rel()
	// A swap that did not finish and an old version still installed.
	r = Record{Envelope: "", ToVersion: MustVersion("0.3.0"), InstallPath: f.InstallPath(), Phase: PhaseSwapping}
	r.Save(f.RecordPath())
	f.installed = "0.2.0"
	if got := AtStartup(c, f, lock); got.Failure.Kind != "not_installed" {
		t.Fatal(got)
	}
	ClearFailure(f, lock)
	if LoadRecord(f.RecordPath()) != nil {
		t.Fatal("clear")
	}
	// A record for another folder is dropped.
	r.InstallPath = "/elsewhere"
	r.Save(f.RecordPath())
	if AtStartup(c, f, lock).Kind != AtStartNothing || LoadRecord(f.RecordPath()) != nil {
		t.Fatal("moved")
	}
}

func TestUpdaterCheckDownloadStage(t *testing.T) {
	c, f, lock, k := setup(t)
	f.staged = false
	body := []byte("the new app")
	var srv *httptest.Server
	srv = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/releases/latest.json":
			sum := sha256.Sum256(body)
			p, _ := json.Marshal(map[string]any{"schema": 1, "version": "0.3.0", "published": "x", "notes_url": srv.URL + "/releases/tag/v0.3.0",
				"assets": map[string]any{"test-plat": map[string]any{"url": srv.URL + "/releases/app.dmg", "sha256": hex.EncodeToString(sum[:]), "size": len(body)}}})
			w.Write(envelope(string(p), "k", k))
		case "/releases/app.dmg":
			w.Write(body)
		}
	}))
	defer srv.Close()
	c.URLPrefix = srv.URL + "/releases/"
	statuses := make(chan Status, 32)
	u := Spawn(Options{URL: srv.URL + "/releases/latest.json", Platform: "test-plat", Automatic: true, DownloadsDir: filepath.Join(f.dir, "dl"),
		Context: *c, Installer: f, Lock: lock}, func(s Status) { statuses <- s })
	defer u.Stop()
	u.CheckNow()
	var kinds []StatusKind
	timeout := time.After(5 * time.Second)
	for {
		select {
		case s := <-statuses:
			kinds = append(kinds, s.Kind)
			if s.Kind == StatusFailed {
				t.Fatal(s.Failure)
			}
			if s.Kind == StatusReady {
				if s.Version.String() != "0.3.0" || !f.staged || LoadRecord(f.RecordPath()).Phase != PhaseStaged {
					t.Fatal(s)
				}
				if LoadState(c.StatePath).HighestSeen.String() != "0.3.0" {
					t.Fatal("state")
				}
				return
			}
		case <-timeout:
			t.Fatal(kinds)
		}
	}
}

func TestPackageOf(t *testing.T) {
	cases := []struct {
		exe     string
		flatpak bool
		appdir  string
		aur     string
		want    Package
	}{
		{"/home/a/.local/lib/sayso/sayso", false, "", "", PackageNone},
		{"/usr/lib/sayso/sayso", false, "", "", PackageNone},
		{"/usr/lib/sayso/sayso", false, "", "/usr/lib/sayso", PackageAUR},
		{"/home/a/.local/lib/sayso/sayso", false, "", "/usr/lib/sayso", PackageNone},
		{"/nix/store/abc-sayso-0.7.1/lib/sayso/sayso", false, "", "", PackageNix},
		{"/app/bin/sayso", true, "", "", PackageFlatpak},
	}
	for _, c := range cases {
		if got := packageOf(c.exe, c.flatpak, c.appdir, c.aur); got != c.want {
			t.Errorf("%s: %v", c.exe, got)
		}
	}
	dir, _ := filepath.EvalSymlinks(t.TempDir())
	if packageOf(filepath.Join(dir, "usr/bin/sayso"), false, dir, "") != PackageAppImage {
		t.Fatal("appimage")
	}
}

func TestReleaseNotes(t *testing.T) {
	data, err := os.ReadFile("../../release-notes.toml")
	if err != nil {
		t.Fatal(err)
	}
	releases, err := ParseReleaseNotes(string(data))
	if err != nil || len(releases) == 0 {
		t.Fatal(err)
	}
	for _, r := range releases {
		for _, it := range r.Item {
			known := false
			for _, n := range NotesIcons {
				known = known || n == it.Icon
			}
			if !known {
				t.Errorf("%s: unknown icon %q", r.Version, it.Icon)
			}
		}
	}
	text := "[[release]]\nversion = \"1.0.0\"\ntitle = \"T\"\n[[release.item]]\nicon = \"mic\"\ntitle = \"a\"\ntext = \"b\"\nsystems = [\"macos\"]\n"
	rs, _ := ParseReleaseNotes(text)
	if _, ok := ReleaseIn(rs, MustVersion("1.0.0"), "macos"); !ok {
		t.Fatal("mac")
	}
	if _, ok := ReleaseIn(rs, MustVersion("1.0.0"), "windows"); ok {
		t.Fatal("nothing for windows")
	}
	if _, err := ParseReleaseNotes(text + "colour = \"x\"\n"); err == nil {
		t.Fatal("wrong field name")
	}
	seen := MustVersion("0.1.0")
	if WhatsNewDue(&seen, MustVersion("0.1.0")) || !WhatsNewDue(&seen, MustVersion("0.2.0")) || !WhatsNewDue(nil, seen) {
		t.Fatal("due")
	}
	state := filepath.Join(t.TempDir(), "state.json")
	os.WriteFile(state, []byte(`{"other":1}`), 0o644)
	if MarkWhatsNewSeen(state) != nil || WhatsNewSeen(state).String() != Version {
		t.Fatal("seen")
	}
	if b, _ := os.ReadFile(state); !strings.Contains(string(b), `"other": 1`) {
		t.Fatal(fmt.Sprintf("%s", b))
	}
}
