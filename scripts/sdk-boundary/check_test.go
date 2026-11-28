package main

import (
	"errors"
	"os"
	"sort"
	"strings"
	"testing"
)

type memRepo map[string]string

func (m memRepo) Files() []string {
	var out []string
	for k := range m {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

func (m memRepo) Read(name string) ([]byte, error) {
	v, ok := m[name]
	if !ok {
		return nil, os.ErrNotExist
	}
	return []byte(v), nil
}

const mit = "MIT License\n\nCopyright (c) 2026 Chris Watson\n\nPermission is hereby granted, free of charge, to any person\n"

const goodCargo = `[package]
name = "sayso-sdk"
version = "0.17.2"
license = "MIT"

[workspace]

[dependencies]
serde = "1"
`

func clean() memRepo {
	return memRepo{
		"go.mod":                            "module github.com/watzon/sayso\n\nreplace github.com/watzon/sayso/sdk/go => ./sdk/go\n",
		"internal/update/version.go":        "package update\n\nvar Version = \"0.17.2\"\n",
		"sdk/LICENSE":                       mit,
		"sdk/README.md":                     "# SDKs\n",
		"sdk/go/LICENSE":                    mit,
		"sdk/go/go.mod":                     "module github.com/watzon/sayso/sdk/go\n\ngo 1.26.0\n",
		"sdk/go/version.go":                 "package sayso\n\nconst Version = \"0.17.2\"\n",
		"sdk/rust/LICENSE":                  mit,
		"sdk/rust/Cargo.toml":               goodCargo,
		"sdk/rust/src/lib.rs":               "pub fn x() {}\n",
		"sdk/rust/build.rs":                 "fn main() {}\n",
		"sdk/typescript/LICENSE":            mit,
		"sdk/typescript/package.json":       `{"name":"@justsayso/sdk","version":"0.17.2","license":"MIT","optionalDependencies":{"@justsayso/audio-darwin-arm64":"0.17.2"}}`,
		"packaging/npm/engine/package.json": `{"name":"@justsayso/engine","version":"0.17.2","license":"GPL-3.0-only","optionalDependencies":{"@justsayso/engine-darwin-arm64":"0.17.2"}}`,
	}
}

func with(edits map[string]string) memRepo {
	r := clean()
	for k, v := range edits {
		if v == "" {
			delete(r, k)
		} else {
			r[k] = v
		}
	}
	return r
}

func all(r memRepo, deps []string) []Violation {
	var out []Violation
	out = append(out, CheckRust(r)...)
	out = append(out, CheckGo(r, deps)...)
	out = append(out, CheckMIT(r)...)
	out = append(out, CheckVersions(r)...)
	out = append(out, CheckGPLPackages(r)...)
	return out
}

func TestACleanTreePasses(t *testing.T) {
	if v := all(clean(), []string{"fmt", "github.com/watzon/sayso/sdk/go/protocol", "golang.org/x/sys/windows"}); len(v) != 0 {
		t.Fatal(v)
	}
	if v := CheckAttribution([]Commit{{"abc", "Add the Go SDK\n\nThe client of the Host API.\n"}, {"def", "Run the styles with Claude CLI and AI providers"}}); len(v) != 0 {
		t.Fatal(v)
	}
}

func expect(t *testing.T, got []Violation, rule, file, text string) {
	t.Helper()
	for _, v := range got {
		if v.Rule == rule && v.File == file && strings.Contains(v.Message, text) {
			return
		}
	}
	t.Fatalf("want [%s] %s: %q, got %v", rule, file, text, got)
}

func TestRustOutsideTheCrateFails(t *testing.T) {
	for _, f := range []string{"crates/sayso-node/src/lib.rs", "native/x/build.rs", "Cargo.lock", "rust-toolchain.toml", ".cargo/config.toml", "sdk/go/x.rs"} {
		expect(t, CheckRust(with(map[string]string{f: "x"})), "rust", f, "belong in sdk/rust")
	}
}

func TestASecondCargoManifestFails(t *testing.T) {
	r := with(map[string]string{"sdk/rust/node/Cargo.toml": "[package]\nname = \"sayso-node\"\n"})
	expect(t, CheckRust(r), "rust", "sdk/rust/node/Cargo.toml", "the only Cargo.toml")
}

func TestTheCrateMustBeSaysoSdk(t *testing.T) {
	r := with(map[string]string{"sdk/rust/Cargo.toml": strings.Replace(goodCargo, `"sayso-sdk"`, `"sayso-protocol"`, 1)})
	expect(t, CheckRust(r), "rust", "sdk/rust/Cargo.toml", `name = "sayso-sdk"`)
}

func TestAWorkspaceWithMembersFails(t *testing.T) {
	r := with(map[string]string{"sdk/rust/Cargo.toml": strings.Replace(goodCargo, "[workspace]\n", "[workspace]\nmembers = [\"node\"]\n", 1)})
	expect(t, CheckRust(r), "rust", "sdk/rust/Cargo.toml", "no workspace members")
}

func TestANativeLibraryCrateTypeFails(t *testing.T) {
	for _, kind := range []string{"cdylib", "staticlib"} {
		r := with(map[string]string{"sdk/rust/Cargo.toml": goodCargo + "\n[lib]\ncrate-type = [\"rlib\", \"" + kind + "\"]\n"})
		expect(t, CheckRust(r), "rust", "sdk/rust/Cargo.toml", "no crate-type "+kind)
	}
}

func TestNapiAndNeonDependenciesFail(t *testing.T) {
	for _, dep := range []string{"[dependencies]\nnapi = \"3\"", "[build-dependencies]\nnapi-build = \"2\"", "[target.'cfg(unix)'.dependencies]\nneon = \"1\""} {
		r := with(map[string]string{"sdk/rust/Cargo.toml": strings.Replace(goodCargo, "[dependencies]\nserde = \"1\"\n", "", 1) + dep + "\n"})
		expect(t, CheckRust(r), "rust", "sdk/rust/Cargo.toml", "no Node addon dependency")
	}
}

func TestNodeAddonsFailAnywhere(t *testing.T) {
	expect(t, CheckRust(with(map[string]string{"sdk/typescript/sayso.darwin-arm64.node": "\x00bin"})), "rust", "sdk/typescript/sayso.darwin-arm64.node", "*.node")
	expect(t, CheckRust(with(map[string]string{"packaging/x/binding.gyp": "{}"})), "rust", "packaging/x/binding.gyp", "binding.gyp")
	cases := map[string]string{
		`{"name":"x","napi":{"name":"sayso"}}`:                    `key "napi"`,
		`{"name":"x","gypfile":true}`:                             `key "gypfile"`,
		`{"name":"x","devDependencies":{"@napi-rs/cli":"3"}}`:     "@napi-rs/cli",
		`{"name":"x","dependencies":{"node-addon-api":"8"}}`:      "node-addon-api",
		`{"name":"x","optionalDependencies":{"prebuildify":"6"}}`: "prebuildify",
		`{"name":"x","scripts":{"install":"node-gyp rebuild"}}`:   "script install",
	}
	for pkg, text := range cases {
		expect(t, CheckRust(with(map[string]string{"sdk/typescript/package.json": pkg})), "rust", "sdk/typescript/package.json", text)
	}
}

func TestTheGoModuleNeedsItsOwnGoMod(t *testing.T) {
	expect(t, CheckGo(with(map[string]string{"sdk/go/go.mod": ""}), nil), "go", "sdk/go/go.mod", "own go.mod")
	expect(t, CheckGo(with(map[string]string{"sdk/go/go.mod": "module github.com/watzon/sayso\n"}), nil), "go", "sdk/go/go.mod", "the module must be")
}

func TestAReplaceInTheGoModuleFails(t *testing.T) {
	for _, gomod := range []string{
		"module github.com/watzon/sayso/sdk/go\n\nreplace github.com/watzon/sayso => ../..\n",
		"module github.com/watzon/sayso/sdk/go\n\nreplace (\n\tgolang.org/x/sys => ../sys\n)\n",
	} {
		expect(t, CheckGo(with(map[string]string{"sdk/go/go.mod": gomod}), nil), "go", "sdk/go/go.mod", "no replace")
	}
}

func TestAnImportOfTheAppFails(t *testing.T) {
	for _, dep := range []string{"github.com/watzon/sayso/internal/bridge", "github.com/watzon/sayso", "github.com/watzon/sayso/mobile/core/styles"} {
		expect(t, CheckGo(clean(), []string{"fmt", dep}), "go", "sdk/go", dep)
	}
	if v := CheckGo(clean(), []string{"github.com/watzon/sayso/sdk/go", "github.com/watzon/sayso/sdk/go/client", "github.com/watzon/sayso-other/x"}); len(v) != 0 {
		t.Fatal(v)
	}
	if v := CheckGo(clean(), []string{"github.com/watzon/sayso/sdk/go_test", "github.com/watzon/sayso/sdk/go.test", "github.com/watzon/sayso/sdk/go/client [github.com/watzon/sayso/sdk/go/client.test]"}); len(v) != 0 {
		t.Fatal(v)
	}
	expect(t, CheckGo(clean(), []string{"github.com/watzon/sayso/internal/bridge [github.com/watzon/sayso/sdk/go.test]"}), "go", "sdk/go", "internal/bridge")
	expect(t, CheckGo(clean(), []string{"github.com/watzon/sayso_test"}), "go", "sdk/go", "github.com/watzon/sayso")
}

func TestLicensesMustBeTheSameMIT(t *testing.T) {
	expect(t, CheckMIT(with(map[string]string{"sdk/LICENSE": ""})), "mit", "sdk/LICENSE", "missing")
	expect(t, CheckMIT(with(map[string]string{"sdk/LICENSE": "Apache License\n", "sdk/go/LICENSE": "Apache License\n", "sdk/rust/LICENSE": "Apache License\n", "sdk/typescript/LICENSE": "Apache License\n"})), "mit", "sdk/LICENSE", "not the MIT")
	for _, d := range []string{"sdk/go", "sdk/rust", "sdk/typescript"} {
		expect(t, CheckMIT(with(map[string]string{d + "/LICENSE": ""})), "mit", d+"/LICENSE", "missing")
		expect(t, CheckMIT(with(map[string]string{d + "/LICENSE": mit + "\n"})), "mit", d+"/LICENSE", "differs")
	}
	r := with(map[string]string{"sdk/typescript/LICENSE": "", "sdk/typescript/package.json": "", "sdk/rust/LICENSE": "", "sdk/rust/Cargo.toml": "", "sdk/rust/src/lib.rs": "", "sdk/rust/build.rs": ""})
	if v := CheckMIT(r); len(v) != 0 {
		t.Fatal("an SDK folder that does not exist yet needs no LICENSE:", v)
	}
}

func TestManifestsMustSayMIT(t *testing.T) {
	expect(t, CheckMIT(with(map[string]string{"sdk/typescript/package.json": `{"name":"@justsayso/sdk","version":"0.17.2","license":"GPL-3.0-only"}`})), "mit", "sdk/typescript/package.json", "MIT")
	expect(t, CheckMIT(with(map[string]string{"sdk/rust/Cargo.toml": strings.Replace(goodCargo, `"MIT"`, `"GPL-3.0-only"`, 1)})), "mit", "sdk/rust/Cargo.toml", "MIT")
}

func TestAGPLHeaderUnderSdkFails(t *testing.T) {
	for _, text := range []string{
		"// SPDX-License-Identifier: GPL-3.0-only\npackage x\n",
		"/* This program is free software ... the GNU General Public License as published by */\n",
		"// SPDX-License-Identifier: LGPL-2.1\n",
		"# GNU Affero General Public License\n",
	} {
		expect(t, CheckMIT(with(map[string]string{"sdk/go/x.go": text})), "mit", "sdk/go/x.go", "GPL")
	}
	if v := CheckMIT(with(map[string]string{"sdk/PROVENANCE.md": "Ported from the GPL-3.0-only app with the author's consent.\n"})); len(v) != 0 {
		t.Fatal("naming the app's license is not a header:", v)
	}
}

func TestVersionsMustAgree(t *testing.T) {
	expect(t, CheckVersions(with(map[string]string{"sdk/go/version.go": "package sayso\n\nconst Version = \"0.17.3\"\n"})), "version", "sdk/go/version.go", "0.17.3")
	expect(t, CheckVersions(with(map[string]string{"sdk/typescript/package.json": `{"version":"0.16.0","license":"MIT"}`})), "version", "sdk/typescript/package.json", "0.16.0")
	expect(t, CheckVersions(with(map[string]string{"sdk/rust/Cargo.toml": strings.Replace(goodCargo, "0.17.2", "0.18.0", 1)})), "version", "sdk/rust/Cargo.toml", "0.18.0")
	expect(t, CheckVersions(with(map[string]string{"sdk/go/version.go": ""})), "version", "sdk/go/version.go", "missing")
}

func TestAttributionInACommitFails(t *testing.T) {
	for _, msg := range []string{
		"Fix it\n\nCo-authored-by: Someone <a@b.c>\n",
		"Fix it\n\nco-authored-by: x\n",
		"Fix it\n\n🤖 Generated with something\n",
		"Fix it\n\nGenerated by a tool\n",
		"Fix it\n\nAssisted-by: a model\n",
		"Fix it\n\nWritten with Claude Code\n",
		"Fix it <noreply@anthropic.com>\n",
		"Fix it with ChatGPT\n",
	} {
		v := CheckAttribution([]Commit{{"0123456789abcdef", msg}})
		if len(v) != 1 || v[0].Rule != "attribution" || v[0].File != "0123456789" {
			t.Fatalf("%q: %v", msg, v)
		}
	}
}

func TestReadErrorsAreViolations(t *testing.T) {
	broken := brokenRepo{clean()}
	if v := CheckRust(broken); len(v) == 0 {
		t.Fatal("a package.json that cannot be read must not pass")
	}
}

type brokenRepo struct{ memRepo }

func (b brokenRepo) Read(name string) ([]byte, error) {
	if strings.HasSuffix(name, "package.json") {
		return nil, errors.New("unreadable")
	}
	return b.memRepo.Read(name)
}

func TestGPLPackagesMustSayGPLAndTheAppVersion(t *testing.T) {
	expect(t, CheckGPLPackages(with(map[string]string{"packaging/npm/engine/package.json": `{"name":"@justsayso/engine","version":"0.17.2","license":"MIT"}`})), "gpl-npm", "packaging/npm/engine/package.json", "GPL-3.0-only")
	expect(t, CheckGPLPackages(with(map[string]string{"packaging/npm/cleanup-darwin-arm64/package.json": `{"name":"@justsayso/cleanup-darwin-arm64","version":"0.17.1","license":"GPL-3.0-only"}`})), "gpl-npm", "packaging/npm/cleanup-darwin-arm64/package.json", "0.17.1")
	expect(t, CheckGPLPackages(with(map[string]string{"packaging/npm/engine/package.json": `{"name":"@justsayso/engine","version":"0.17.2","license":"GPL-3.0-only","optionalDependencies":{"@justsayso/engine-darwin-arm64":"0.16.0"}}`})), "gpl-npm", "packaging/npm/engine/package.json", "0.16.0")
	expect(t, CheckGPLPackages(with(map[string]string{"packaging/npm/engine/package.json": `{not json`})), "gpl-npm", "packaging/npm/engine/package.json", "")
}

func TestAnMITPackageNamesAGPLPackageOnlyAsAnOptionalPeer(t *testing.T) {
	for _, field := range []string{"dependencies", "optionalDependencies"} {
		pkg := `{"name":"@justsayso/sdk","version":"0.17.2","license":"MIT","` + field + `":{"@justsayso/cleanup":"0.17.2"}}`
		expect(t, CheckGPLPackages(with(map[string]string{"sdk/typescript/package.json": pkg})), "gpl-npm", "sdk/typescript/package.json", field)
	}
	required := `{"name":"@justsayso/sdk","version":"0.17.2","license":"MIT","peerDependencies":{"@justsayso/engine":"0.17.2"}}`
	expect(t, CheckGPLPackages(with(map[string]string{"sdk/typescript/package.json": required})), "gpl-npm", "sdk/typescript/package.json", "optional")
	optional := `{"name":"@justsayso/sdk","version":"0.17.2","license":"MIT","peerDependencies":{"@justsayso/engine":"0.17.2"},"peerDependenciesMeta":{"@justsayso/engine":{"optional":true}}}`
	if v := CheckGPLPackages(with(map[string]string{"sdk/typescript/package.json": optional})); len(v) != 0 {
		t.Fatal("an optional peer is allowed:", v)
	}
}
