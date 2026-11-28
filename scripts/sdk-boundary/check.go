package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"path"
	"regexp"
	"sort"
	"strings"

	"github.com/pelletier/go-toml/v2"
)

// Repo is the tracked files of a checkout.
type Repo interface {
	Files() []string
	Read(name string) ([]byte, error)
}

// Violation is one broken rule.
type Violation struct {
	Rule, File, Message string
}

func (v Violation) String() string {
	if v.File == "" {
		return fmt.Sprintf("[%s] %s", v.Rule, v.Message)
	}
	return fmt.Sprintf("[%s] %s: %s", v.Rule, v.File, v.Message)
}

const (
	rustDir       = "sdk/rust/"
	goModule      = "github.com/watzon/sayso/sdk/go"
	appModule     = "github.com/watzon/sayso"
	rustCrateName = "sayso-sdk"
)

func isRustFile(name string) bool {
	base := path.Base(name)
	for _, part := range strings.Split(name, "/") {
		if part == ".cargo" {
			return true
		}
	}
	return strings.HasSuffix(base, ".rs") || base == "Cargo.toml" || base == "Cargo.lock" || base == "build.rs" || strings.HasPrefix(base, "rust-toolchain")
}

var nativeAddonDeps = regexp.MustCompile(`^(node-gyp|node-addon-api|prebuild.*|napi.*|@napi-rs/.*|neon.*|node-pre-gyp|@mapbox/node-pre-gyp|cmake-js)$`)

// CheckRust: all Rust is the one crate sayso-sdk in sdk/rust, with no native
// Node addon anywhere.
func CheckRust(repo Repo) []Violation {
	var out []Violation
	var manifests []string
	for _, f := range repo.Files() {
		base := path.Base(f)
		if isRustFile(f) && !strings.HasPrefix(f, rustDir) {
			out = append(out, Violation{"rust", f, "Rust files belong in sdk/rust"})
		}
		if base == "Cargo.toml" {
			manifests = append(manifests, f)
		}
		switch {
		case strings.HasSuffix(base, ".node"):
			out = append(out, Violation{"rust", f, "no native Node addon (*.node)"})
		case base == "binding.gyp":
			out = append(out, Violation{"rust", f, "no native Node addon (binding.gyp)"})
		case base == "package.json":
			out = append(out, checkPackageJSON(repo, f)...)
		}
	}
	for _, m := range manifests {
		if m != rustDir+"Cargo.toml" {
			out = append(out, Violation{"rust", m, "the only Cargo.toml is sdk/rust/Cargo.toml"})
			continue
		}
		out = append(out, checkCrate(repo, m)...)
	}
	return out
}

func checkPackageJSON(repo Repo, name string) []Violation {
	data, err := repo.Read(name)
	if err != nil {
		return []Violation{{"rust", name, err.Error()}}
	}
	var pkg map[string]json.RawMessage
	if err := json.Unmarshal(data, &pkg); err != nil {
		return []Violation{{"rust", name, "not JSON: " + err.Error()}}
	}
	var out []Violation
	for _, key := range []string{"napi", "gypfile", "binary"} {
		if _, ok := pkg[key]; ok {
			out = append(out, Violation{"rust", name, fmt.Sprintf("no native Node addon (key %q)", key)})
		}
	}
	for _, field := range []string{"dependencies", "devDependencies", "optionalDependencies", "peerDependencies"} {
		var deps map[string]string
		if raw, ok := pkg[field]; ok && json.Unmarshal(raw, &deps) == nil {
			for _, dep := range sortedKeys(deps) {
				if nativeAddonDeps.MatchString(dep) {
					out = append(out, Violation{"rust", name, fmt.Sprintf("no native Node addon (%s %s)", field, dep)})
				}
			}
		}
	}
	var scripts map[string]string
	if raw, ok := pkg["scripts"]; ok && json.Unmarshal(raw, &scripts) == nil {
		for _, k := range sortedKeys(scripts) {
			if strings.Contains(scripts[k], "node-gyp") || strings.Contains(scripts[k], "napi ") {
				out = append(out, Violation{"rust", name, fmt.Sprintf("no native Node addon (script %s)", k)})
			}
		}
	}
	return out
}

type cargoManifest struct {
	Package *struct {
		Name    string
		Version string
		License string
	}
	Lib *struct {
		CrateType []string `toml:"crate-type"`
	}
	Workspace         map[string]any
	Dependencies      map[string]any
	DevDependencies   map[string]any `toml:"dev-dependencies"`
	BuildDependencies map[string]any `toml:"build-dependencies"`
	Target            map[string]struct {
		Dependencies map[string]any
	}
}

func readCargo(repo Repo, name string) (*cargoManifest, error) {
	data, err := repo.Read(name)
	if err != nil {
		return nil, err
	}
	var m cargoManifest
	if err := toml.Unmarshal(data, &m); err != nil {
		return nil, fmt.Errorf("not TOML: %v", err)
	}
	return &m, nil
}

func checkCrate(repo Repo, name string) []Violation {
	m, err := readCargo(repo, name)
	if err != nil {
		return []Violation{{"rust", name, err.Error()}}
	}
	var out []Violation
	if m.Package == nil || m.Package.Name != rustCrateName {
		out = append(out, Violation{"rust", name, "the crate must be [package] name = \"sayso-sdk\""})
	}
	if m.Workspace != nil && m.Workspace["members"] != nil {
		out = append(out, Violation{"rust", name, "one crate: no workspace members"})
	}
	if m.Lib != nil {
		for _, t := range m.Lib.CrateType {
			if t == "cdylib" || t == "staticlib" {
				out = append(out, Violation{"rust", name, "no crate-type " + t})
			}
		}
	}
	deps := []map[string]any{m.Dependencies, m.DevDependencies, m.BuildDependencies}
	for _, t := range m.Target {
		deps = append(deps, t.Dependencies)
	}
	for _, d := range deps {
		for _, dep := range sortedKeys(d) {
			if strings.HasPrefix(dep, "napi") || strings.HasPrefix(dep, "neon") {
				out = append(out, Violation{"rust", name, "no Node addon dependency " + dep})
			}
		}
	}
	return out
}

var (
	replaceLine = regexp.MustCompile(`(?m)^\s*replace\b`)
	moduleLine  = regexp.MustCompile(`(?m)^\s*module\s+(\S+)`)
)

// CheckGo: sdk/go is its own module with no replace, and imports nothing of
// the app. deps are the packages of `go list -deps -test ./...` in sdk/go.
func CheckGo(repo Repo, deps []string) []Violation {
	const mod = "sdk/go/go.mod"
	data, err := repo.Read(mod)
	if err != nil {
		return []Violation{{"go", mod, "sdk/go needs its own go.mod"}}
	}
	var out []Violation
	if m := moduleLine.FindSubmatch(data); m == nil || string(m[1]) != goModule {
		out = append(out, Violation{"go", mod, "the module must be " + goModule})
	}
	if replaceLine.Match(data) {
		out = append(out, Violation{"go", mod, "no replace directive"})
	}
	for _, d := range deps {
		d, _, _ = strings.Cut(d, " ")
		d = strings.TrimSuffix(strings.TrimSuffix(d, ".test"), "_test")
		if (d == appModule || strings.HasPrefix(d, appModule+"/")) && d != goModule && !strings.HasPrefix(d, goModule+"/") {
			out = append(out, Violation{"go", "sdk/go", "imports the app package " + d})
		}
	}
	return out
}

var (
	mitMarkers = [][]byte{[]byte("MIT License"), []byte("Permission is hereby granted, free of charge")}
	gplMarkers = regexp.MustCompile(`(?i)GNU (Lesser |Affero )?General Public License|SPDX-License-Identifier:\s*\(?\s*(A|L)?GPL`)
	sdkDirs    = []string{"sdk/go", "sdk/typescript", "sdk/rust"}
)

// CheckMIT: one MIT license, the same bytes in sdk/ and in each SDK, the
// manifests say MIT, and no file under sdk/ has a GPL header.
func CheckMIT(repo Repo) []Violation {
	var out []Violation
	main, err := repo.Read("sdk/LICENSE")
	if err != nil {
		return []Violation{{"mit", "sdk/LICENSE", "missing"}}
	}
	for _, m := range mitMarkers {
		if !bytes.Contains(main, m) {
			out = append(out, Violation{"mit", "sdk/LICENSE", "not the MIT license"})
			break
		}
	}
	files := repo.Files()
	present := map[string]bool{}
	for _, f := range files {
		for _, d := range sdkDirs {
			if strings.HasPrefix(f, d+"/") {
				present[d] = true
			}
		}
	}
	for _, d := range sdkDirs {
		if !present[d] {
			continue
		}
		lic, err := repo.Read(d + "/LICENSE")
		switch {
		case err != nil:
			out = append(out, Violation{"mit", d + "/LICENSE", "missing"})
		case !bytes.Equal(lic, main):
			out = append(out, Violation{"mit", d + "/LICENSE", "differs from sdk/LICENSE"})
		}
	}
	if present["sdk/typescript"] {
		if data, err := repo.Read("sdk/typescript/package.json"); err == nil {
			var pkg struct{ License string }
			if json.Unmarshal(data, &pkg) != nil || pkg.License != "MIT" {
				out = append(out, Violation{"mit", "sdk/typescript/package.json", `"license" must be "MIT"`})
			}
		}
	}
	if present["sdk/rust"] {
		if m, err := readCargo(repo, "sdk/rust/Cargo.toml"); err == nil && (m.Package == nil || m.Package.License != "MIT") {
			out = append(out, Violation{"mit", "sdk/rust/Cargo.toml", `license must be "MIT"`})
		}
	}
	for _, f := range files {
		if !strings.HasPrefix(f, "sdk/") {
			continue
		}
		data, err := repo.Read(f)
		if err != nil || bytes.IndexByte(data, 0) >= 0 {
			continue
		}
		if gplMarkers.Match(data) {
			out = append(out, Violation{"mit", f, "a GPL license text or header under sdk/"})
		}
	}
	return out
}

var (
	goVersion   = regexp.MustCompile(`(?m)^\s*(?:const|var)\s+Version\s*=\s*"([^"]+)"`)
	versionFile = map[string]string{"app": "internal/update/version.go", "go": "sdk/go/version.go", "typescript": "sdk/typescript/package.json", "rust": "sdk/rust/Cargo.toml"}
)

// CheckVersions: the app and each SDK that exists have the same version.
func CheckVersions(repo Repo) []Violation {
	versions := map[string]string{}
	for _, who := range []string{"app", "go", "typescript", "rust"} {
		file := versionFile[who]
		data, err := repo.Read(file)
		if err != nil {
			if who == "app" || who == "go" {
				return []Violation{{"version", file, "missing"}}
			}
			continue
		}
		switch who {
		case "app", "go":
			if m := goVersion.FindSubmatch(data); m != nil {
				versions[who] = string(m[1])
			}
		case "typescript":
			var pkg struct{ Version string }
			json.Unmarshal(data, &pkg)
			versions[who] = pkg.Version
		case "rust":
			if m, err := readCargo(repo, file); err == nil && m.Package != nil {
				versions[who] = m.Package.Version
			}
		}
		if versions[who] == "" {
			return []Violation{{"version", file, "no version found"}}
		}
	}
	var out []Violation
	for _, who := range []string{"go", "typescript", "rust"} {
		if v, ok := versions[who]; ok && v != versions["app"] {
			out = append(out, Violation{"version", versionFile[who], fmt.Sprintf("version %s, but the app is %s", v, versions["app"])})
		}
	}
	return out
}

var attribution = []*regexp.Regexp{
	regexp.MustCompile(`(?im)^\s*co-authored-by:`),
	regexp.MustCompile(`(?im)^\s*(assisted|generated|written|created|made)-(by|with):`),
	regexp.MustCompile(`(?i)\bgenerated (with|by) `),
	regexp.MustCompile(`(?i)noreply@anthropic\.com|noreply@openai\.com`),
	regexp.MustCompile(`🤖`),
	regexp.MustCompile(`(?i)\b(claude code|chatgpt|github copilot|openai codex|gemini cli)\b`),
}

// Commit is one commit message.
type Commit struct{ Hash, Message string }

// CheckAttribution: no commit names an AI tool or carries its trailer.
func CheckAttribution(commits []Commit) []Violation {
	var out []Violation
	for _, c := range commits {
		for _, re := range attribution {
			if m := re.FindString(c.Message); m != "" {
				short := c.Hash
				if len(short) > 10 {
					short = short[:10]
				}
				out = append(out, Violation{"attribution", short, fmt.Sprintf("the message has %q", strings.TrimSpace(m))})
				break
			}
		}
	}
	return out
}

func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}
