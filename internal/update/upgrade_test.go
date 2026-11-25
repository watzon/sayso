package update

import (
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"testing"
)

// What the updater of the Rust app (0.14 to 0.16, crates/sayso-update) reads,
// so an installed Rust app finds and accepts the first Go release.
const (
	rustManifestURL = "https://github.com/watzon/sayso/releases/latest/download/latest.json"
	rustURLPrefix   = "https://github.com/watzon/sayso/releases/"
	rustSchema      = 1
	rustMaxBytes    = 64 * 1024
	rustKeyID       = "2026-10"
	rustKey         = "LxKkwF10F2Oski5tz8EgwPfhNPd90oOmm7/MX66Atgk="
)

// The asset keys: std::env::consts OS-ARCH of the Rust app.
var rustPlatforms = []string{"linux-aarch64", "linux-x86_64", "macos-aarch64", "windows-x86_64"}

func TestTheUpdaterReadsWhatTheRustUpdaterRead(t *testing.T) {
	if ManifestURL != rustManifestURL || URLPrefix != rustURLPrefix || ManifestSchema != rustSchema || ManifestMaxBytes != rustMaxBytes {
		t.Fatalf("the manifest contract changed: %s %s %d %d", ManifestURL, URLPrefix, ManifestSchema, ManifestMaxBytes)
	}
	found := false
	for _, k := range ReleaseKeys {
		found = found || (k[0] == rustKeyID && k[1] == rustKey)
	}
	if !found {
		t.Fatal("the release key 2026-10 that the Rust apps trust is gone")
	}
	workflow, err := os.ReadFile("../../.github/workflows/release.yml")
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(workflow), `SAYSO_UPDATE_KEY_ID: "`+rustKeyID+`"`) {
		t.Fatal("release.yml does not sign with the key 2026-10")
	}
}

// releasePayloadScript is the Python of the step "Write the payload" in
// release.yml.
func releasePayloadScript(t *testing.T) string {
	t.Helper()
	b, err := os.ReadFile("../../.github/workflows/release.yml")
	if err != nil {
		t.Fatal(err)
	}
	m := regexp.MustCompile(`(?s)<<'PY'\n(.*?)\n\s*PY\n`).FindSubmatch(b)
	if m == nil {
		t.Fatal("release.yml has no payload script")
	}
	var lines []string
	for _, l := range strings.Split(string(m[1]), "\n") {
		lines = append(lines, strings.TrimPrefix(l, "          "))
	}
	return strings.Join(lines, "\n")
}

// The manifest that release.yml makes has the shape, the platform keys, and
// the file names that the Rust apps expect, and it verifies.
func TestTheReleaseManifestFitsTheRustApps(t *testing.T) {
	python, err := exec.LookPath("python3")
	if err != nil {
		t.Skip("no python3")
	}
	const version = "0.17.0"
	dir := t.TempDir()
	dist := filepath.Join(dir, "dist")
	os.Mkdir(dist, 0o755)
	names := []string{
		"Sayso-0.17.0-macos-arm64.dmg", "Sayso-0.17.0-windows-x64-setup.exe",
		"sayso-0.17.0-linux-x86_64.tar.gz", "sayso-0.17.0-linux-aarch64.tar.gz",
		"sayso-engine-0.17.0-windows-x64.zip",
	}
	for _, arch := range []string{"x86_64", "aarch64"} {
		for _, ext := range []string{"deb", "rpm", "flatpak"} {
			names = append(names, "sayso-0.17.0-linux-"+arch+"."+ext)
		}
		names = append(names, "sayso-0.17.0-"+arch+".AppImage", "sayso-0.17.0-"+arch+".AppImage.zsync")
	}
	for _, p := range []string{"macos-arm64", "macos-x86_64", "linux-x86_64", "linux-arm64"} {
		names = append(names, "sayso-engine-0.17.0-"+p+".tar.gz")
	}
	for _, n := range names {
		if err := os.WriteFile(filepath.Join(dist, n), []byte(n), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	script := filepath.Join(t.TempDir(), "payload.py")
	os.WriteFile(script, []byte(releasePayloadScript(t)), 0o644)
	cmd := exec.Command(python, "-I", script, version)
	cmd.Dir = dir
	cmd.Env = append(os.Environ(), "TAG=v"+version, "GITHUB_REPOSITORY=watzon/sayso")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("%v\n%s", err, out)
	}
	payload, err := os.ReadFile(filepath.Join(dir, "payload.json"))
	if err != nil {
		t.Fatal(err)
	}

	var top map[string]json.RawMessage
	if err := json.Unmarshal(payload, &top); err != nil {
		t.Fatal(err)
	}
	var keys []string
	for k := range top {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	if strings.Join(keys, ",") != "assets,notes_url,published,schema,version" {
		t.Fatalf("payload fields: %v", keys)
	}
	var m Manifest
	if err := json.Unmarshal(payload, &m); err != nil {
		t.Fatal(err)
	}
	var platforms []string
	for p := range m.Assets {
		platforms = append(platforms, p)
	}
	sort.Strings(platforms)
	if strings.Join(platforms, ",") != strings.Join(rustPlatforms, ",") {
		t.Fatalf("platforms: %v", platforms)
	}
	wantFile := map[string]string{
		"macos-aarch64": "Sayso-0.17.0-macos-arm64.dmg", "windows-x86_64": "Sayso-0.17.0-windows-x64-setup.exe",
		"linux-x86_64": "sayso-0.17.0-linux-x86_64.tar.gz", "linux-aarch64": "sayso-0.17.0-linux-aarch64.tar.gz",
	}
	for p, a := range m.Assets {
		if a.URL != rustURLPrefix+"download/v"+version+"/"+wantFile[p] || a.Size != uint64(len(wantFile[p])) || len(a.SHA256) != 64 {
			t.Fatalf("%s: %+v", p, a)
		}
	}
	if m.Schema != rustSchema || m.Version.String() != version || m.NotesURL != rustURLPrefix+"tag/v"+version {
		t.Fatalf("%+v", m)
	}

	pub, priv, _ := ed25519.GenerateKey(rand.Reader)
	text := strings.TrimSpace(string(payload))
	env, _ := json.Marshal(Envelope{Payload: text, Signatures: []EnvelopeSignature{{Key: rustKeyID, Signature: base64.StdEncoding.EncodeToString(ed25519.Sign(priv, []byte(text)))}}})
	key, _ := ParsePublicKey(rustKeyID, base64.StdEncoding.EncodeToString(pub))
	if kind, _, err := VerifyFor(env, []PublicKey{key}, rustURLPrefix); err != nil || kind != VerifiedManifest {
		t.Fatalf("the signed manifest does not verify: %v %v", kind, err)
	}
}

// update-state.json as serde_json wrote it (to_vec_pretty), with a notice
// still due and a prerelease seen.
func TestARustUpdateStateLoads(t *testing.T) {
	path := filepath.Join(t.TempDir(), "update-state.json")
	os.WriteFile(path, []byte("{\n  \"highest_seen\": \"0.16.1-beta.2\",\n  \"last_check\": 1763900000,\n  \"notice\": \"0.16.0\"\n}"), 0o600)
	s := LoadState(path)
	if s.HighestSeen == nil || s.HighestSeen.String() != "0.16.1-beta.2" || s.LastCheck == nil || *s.LastCheck != 1763900000 ||
		s.Notice == nil || s.Notice.String() != "0.16.0" {
		t.Fatalf("%+v", s)
	}
}
