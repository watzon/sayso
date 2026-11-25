package main

import (
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"slices"
	"strings"
	"testing"
)

func repoFile(t *testing.T, path string) string {
	t.Helper()
	b, err := os.ReadFile(filepath.Join("..", "..", filepath.FromSlash(path)))
	if err != nil {
		t.Fatal(err)
	}
	return strings.ReplaceAll(string(b), "\r\n", "\n")
}

// Sayso 0.13 to 0.16 installed builds of the engine and the cleanup runtime
// for each instruction set in the app folder. An install over them deletes
// them and stops the ones that still run, and an uninstall stops them too.
func TestTheWindowsInstallerRemovesTheHelpersOfOldVersions(t *testing.T) {
	iss := repoFile(t, "packaging/windows/Sayso.iss")
	names := regexp.MustCompile(`HelperNames = '([^']*)'`).FindStringSubmatch(iss)
	if names == nil {
		t.Fatal("no HelperNames in Sayso.iss")
	}
	helpers := strings.Split(names[1], ",")
	for _, exe := range []string{"sayso-engine-avx", "sayso-engine-baseline", "sayso-cleanup", "sayso-cleanup-avx", "sayso-cleanup-baseline"} {
		if !strings.Contains(iss, `Type: files; Name: "{app}\`+exe+`.exe"`) {
			t.Errorf("[InstallDelete] does not remove %s.exe", exe)
		}
		if !strings.Contains(iss, `Parameters: "/IM `+exe+`.exe /F"`) {
			t.Errorf("[UninstallRun] does not stop %s.exe", exe)
		}
		if !slices.Contains(helpers, exe) {
			t.Errorf("PrepareToInstall does not stop %s.exe", exe)
		}
	}
	if !slices.Contains(helpers, "sayso-engine") || !strings.Contains(iss, "function PrepareToInstall(") {
		t.Fatal("PrepareToInstall does not stop the engine")
	}
}

// fakeTarball is the bin/ folder of a Linux tarball of this version.
func fakeTarball(t *testing.T, dir string) {
	t.Helper()
	for _, f := range []string{"bin/sayso", "bin/sayso-engine", "bin/libsherpa-onnx-c-api.so", "bin/libonnxruntime.so.1",
		"bin/llama/sayso-cleanup", "bin/llama/libllama.so", "share/applications/dev.sayso.Sayso.desktop",
		"share/metainfo/dev.sayso.Sayso.metainfo.xml", "share/icons/hicolor/64x64/apps/dev.sayso.Sayso.png",
		"lib/udev/rules.d/70-sayso-uinput.rules"} {
		p := filepath.Join(dir, filepath.FromSlash(f))
		os.MkdirAll(filepath.Dir(p), 0o755)
		content := "new " + f
		if strings.HasSuffix(f, ".desktop") {
			content = "[Desktop Entry]\nExec=sayso\n"
		}
		if err := os.WriteFile(p, []byte(content), 0o755); err != nil {
			t.Fatal(err)
		}
	}
}

// install.sh over a user install of 0.16: the engine libraries come along,
// and the files that only 0.16 had are gone.
func TestTheLinuxInstallScriptReplacesAnOldInstall(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("POSIX sh")
	}
	tarball, home := t.TempDir(), t.TempDir()
	fakeTarball(t, tarball)
	b, _ := os.ReadFile(filepath.Join("..", "..", "packaging", "linux", "install.sh"))
	os.WriteFile(filepath.Join(tarball, "install.sh"), b, 0o755)
	lib := filepath.Join(home, ".local", "lib", "sayso")
	os.MkdirAll(lib, 0o755)
	for _, old := range []string{"sayso", "sayso-engine", "sayso-bridge", "sayso-cleanup", "sayso-cleanup-avx", "sayso-cleanup-baseline", "libold-engine.so"} {
		os.WriteFile(filepath.Join(lib, old), []byte("old"), 0o755)
	}
	cmd := exec.Command("sh", filepath.Join(tarball, "install.sh"))
	cmd.Env = append(os.Environ(), "HOME="+home, "XDG_DATA_HOME="+filepath.Join(home, ".local", "share"), "PATH=/usr/bin:/bin")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("%v\n%s", err, out)
	}
	var have []string
	filepath.Walk(lib, func(p string, info os.FileInfo, err error) error {
		if err == nil && !info.IsDir() {
			rel, _ := filepath.Rel(lib, p)
			have = append(have, filepath.ToSlash(rel))
		}
		return nil
	})
	slices.Sort(have)
	want := []string{"libonnxruntime.so.1", "libsherpa-onnx-c-api.so", "llama/libllama.so", "llama/sayso-cleanup", "sayso", "sayso-engine"}
	if !slices.Equal(have, want) {
		t.Fatalf("the install folder has %v, want %v", have, want)
	}
	if b, _ := os.ReadFile(filepath.Join(lib, "sayso-engine")); string(b) != "new bin/sayso-engine" {
		t.Fatal("the engine was not replaced")
	}
}

// The Nix package takes the engine libraries, and the cleanup runtime from
// bin/llama, or from bin/ in a tarball of 0.16 and older.
func TestTheNixPackageInstallsBothTarballLayouts(t *testing.T) {
	bash, err := exec.LookPath("bash")
	if err != nil || runtime.GOOS == "windows" {
		t.Skip("needs bash")
	}
	nix := repoFile(t, "packaging/nix/package.nix")
	m := regexp.MustCompile(`(?s)installPhase = ''\n(.*?)\n  '';`).FindStringSubmatch(nix)
	if m == nil {
		t.Fatal("no installPhase in package.nix")
	}
	phase := strings.NewReplacer("runHook preInstall", "", "runHook postInstall", "").Replace(m[1])
	if exec.Command("install", "--version").Run() != nil {
		// BSD install has no -D (GNU: make the folders of the target).
		phase = `install() { case "$1" in -D*) mkdir -p "$(dirname "$3")"; command install "-${1#-D}" "$2" "$3" ;; *) command install "$@" ;; esac; }` + "\n" + phase
	}
	run := func(t *testing.T, oldLayout bool) []string {
		src, out := t.TempDir(), filepath.Join(t.TempDir(), "out")
		fakeTarball(t, src)
		if oldLayout {
			os.RemoveAll(filepath.Join(src, "bin", "llama"))
			os.WriteFile(filepath.Join(src, "bin", "sayso-cleanup"), []byte("old"), 0o755)
			os.WriteFile(filepath.Join(src, "bin", "sayso-cleanup-avx"), []byte("old"), 0o755)
		}
		cmd := exec.Command(bash, "-euc", phase)
		cmd.Dir = src
		cmd.Env = append(os.Environ(), "out="+out)
		if b, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("%v\n%s", err, b)
		}
		var have []string
		filepath.Walk(filepath.Join(out, "lib", "sayso"), func(p string, info os.FileInfo, err error) error {
			if err == nil && !info.IsDir() {
				rel, _ := filepath.Rel(filepath.Join(out, "lib", "sayso"), p)
				have = append(have, filepath.ToSlash(rel))
			}
			return nil
		})
		slices.Sort(have)
		return have
	}
	if got, want := run(t, false), []string{"libonnxruntime.so.1", "libsherpa-onnx-c-api.so", "llama/libllama.so", "llama/sayso-cleanup", "sayso", "sayso-engine"}; !slices.Equal(got, want) {
		t.Fatalf("this version: %v, want %v", got, want)
	}
	if got, want := run(t, true), []string{"libonnxruntime.so.1", "libsherpa-onnx-c-api.so", "sayso", "sayso-cleanup", "sayso-cleanup-avx", "sayso-engine"}; !slices.Equal(got, want) {
		t.Fatalf("a 0.16 tarball: %v, want %v", got, want)
	}
}

// No package of this version ships or waits for sayso-bridge, which only the
// Rust app had.
func TestTheLinuxPackagesNeedNoBridge(t *testing.T) {
	for _, f := range []string{"scripts/bundle-linux.sh", "scripts/package-linux.sh", "packaging/linux/nfpm.yaml",
		"packaging/linux/AppRun", "packaging/linux/aur/PKGBUILD.in", "packaging/nix/package.nix"} {
		text := repoFile(t, f)
		if strings.Contains(text, "sayso-bridge") || strings.Contains(text, "xdg-run/sayso") {
			t.Errorf("%s names sayso-bridge or its socket folder", f)
		}
	}
}
