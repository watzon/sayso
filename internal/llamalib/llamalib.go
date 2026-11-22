// Package llamalib finds and fetches the prebuilt llama.cpp shared libraries
// that the cleanup runtime (cmd/sayso-cleanup) loads with yzma.
//
// The libraries come from one pinned llama.cpp release (pins.go). Each
// platform has one archive with a pinned SHA-256, and the files that Sayso
// keeps from it, each with its own SHA-256. The libraries live in their own
// folder, never beside the speech engine, so the ggml of llama.cpp and the ggml
// of whisper.cpp and parakeet.cpp cannot meet.
//
// This package does not load the libraries, and the app may import it: it does
// not import yzma. docs/cleanup-models.md, "Runtime", has the design.
package llamalib

import (
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"runtime"

	"github.com/watzon/sayso/internal/paths"
)

// EnvLibDir names a folder with the libraries. It wins over every other
// place, and its files are not checked: it is for development.
const EnvLibDir = "SAYSO_LLAMA_LIB_DIR"

// Asset is the archive of one platform and the files Sayso keeps from it.
type Asset struct {
	GOOS, GOARCH string
	// Backend is what the libraries run on: "metal" or "cpu".
	Backend string
	URL     string
	SHA256  string
	Size    int64
	// Root is the folder in the archive that holds the files ("" for none).
	Root  string
	Files []File
	// Links are symbolic links that the libraries need (Unix only): the name
	// that yzma opens and the name that the other libraries link to.
	Links []Link
}

// File is one file of an asset.
type File struct {
	Name   string
	SHA256 string
	Size   int64
}

// Link is a symbolic link in the folder, to a file of the same folder.
type Link struct{ Name, Target string }

// Name is the folder name of an asset, the same in every cache.
func (a *Asset) Name() string { return Tag + "-" + a.GOOS + "-" + a.GOARCH }

// For is the asset of a platform.
func For(goos, goarch string) (*Asset, bool) {
	for i := range assets {
		if assets[i].GOOS == goos && assets[i].GOARCH == goarch {
			return &assets[i], true
		}
	}
	return nil, false
}

// Current is the asset of this platform.
func Current() (*Asset, bool) { return For(runtime.GOOS, runtime.GOARCH) }

// LibraryNames are the files that yzma opens, in a folder of goos.
func LibraryNames(goos string) []string {
	var names []string
	for _, n := range []string{"ggml", "ggml-base", "llama"} {
		switch goos {
		case "windows":
			names = append(names, n+".dll")
		case "darwin":
			names = append(names, "lib"+n+".dylib")
		default:
			names = append(names, "lib"+n+".so")
		}
	}
	return names
}

// HasLibraries is true when dir has the files that yzma opens.
func HasLibraries(dir, goos string) bool {
	if dir == "" {
		return false
	}
	for _, n := range LibraryNames(goos) {
		if fi, err := os.Stat(filepath.Join(dir, n)); err != nil || fi.IsDir() {
			return false
		}
	}
	return true
}

// Verify checks every file of a in dir against its SHA-256, and every link
// against its target. Any other entry fails it too: llama.cpp loads every ggml
// backend in the folder, so a folder holds only pinned files.
func Verify(dir string, a *Asset) error {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return err
	}
	pinned := map[string]bool{}
	for _, f := range a.Files {
		pinned[f.Name] = true
	}
	if runtime.GOOS != "windows" {
		for _, l := range a.Links {
			pinned[l.Name] = true
		}
	}
	for _, e := range entries {
		if !pinned[e.Name()] {
			return fmt.Errorf("%s is not a pinned file", e.Name())
		}
	}
	for _, f := range a.Files {
		got, err := fileSHA256(filepath.Join(dir, f.Name))
		if err != nil {
			return err
		}
		if got != f.SHA256 {
			return fmt.Errorf("%s: the SHA-256 is %s, not %s", f.Name, got, f.SHA256)
		}
	}
	if runtime.GOOS == "windows" {
		return nil
	}
	for _, l := range a.Links {
		target, err := os.Readlink(filepath.Join(dir, l.Name))
		if err != nil {
			return err
		}
		if target != l.Target {
			return fmt.Errorf("%s points to %s, not %s", l.Name, target, l.Target)
		}
	}
	return nil
}

func fileSHA256(path string) (string, error) {
	f, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer f.Close()
	h := sha256.New()
	if _, err := io.Copy(h, f); err != nil {
		return "", err
	}
	return hex.EncodeToString(h.Sum(nil)), nil
}

// BundledDir is the folder of the libraries in a package, for the cleanup
// runtime at exe: Contents/Frameworks/llama in Sayso.app (the runtime is in
// Contents/MacOS), else the folder of the runtime itself (llama/ beside the
// app, which holds the runtime and the libraries).
func BundledDir(exe, goos string) string {
	if exe == "" {
		return ""
	}
	dir := filepath.Dir(exe)
	if goos == "darwin" {
		return filepath.Join(dir, "..", "Frameworks", "llama")
	}
	return dir
}

// UserCacheDir is the folder that a download of a fills: in the cache folder
// of Sayso, so a new release of the libraries gets a new folder.
func UserCacheDir(a *Asset) string {
	return filepath.Join(paths.Resolve(paths.SystemEnv{}).CacheDir, "llama", a.Name())
}

// Source says where Locate found the libraries.
type Source string

const (
	FromEnv     Source = "env"
	FromBundle  Source = "bundle"
	FromCache   Source = "cache"
	FromNowhere Source = ""
)

// ErrNotFound means that no place has the libraries, and a download is needed.
var ErrNotFound = errors.New("the llama.cpp libraries are not installed")

// Locate finds the libraries for the cleanup runtime at exe: EnvLibDir, then
// the folder of the package, then the download in the user cache (checked
// against the pins). A package folder is not checked: it is part of the
// package, like the programs (on macOS the code signature covers it, and some
// package systems, such as Nix, rewrite the files).
func Locate(exe string) (string, Source, error) {
	if dir := os.Getenv(EnvLibDir); dir != "" {
		if !HasLibraries(dir, runtime.GOOS) {
			return "", FromNowhere, fmt.Errorf("%s=%s has no llama.cpp libraries", EnvLibDir, dir)
		}
		return dir, FromEnv, nil
	}
	if dir := BundledDir(exe, runtime.GOOS); HasLibraries(dir, runtime.GOOS) {
		return dir, FromBundle, nil
	}
	a, ok := Current()
	if !ok {
		return "", FromNowhere, fmt.Errorf("no llama.cpp build for %s/%s", runtime.GOOS, runtime.GOARCH)
	}
	dir := UserCacheDir(a)
	if !HasLibraries(dir, runtime.GOOS) {
		return "", FromNowhere, ErrNotFound
	}
	if err := Verify(dir, a); err != nil {
		return "", FromNowhere, fmt.Errorf("the downloaded llama.cpp libraries in %s: %w", dir, err)
	}
	return dir, FromCache, nil
}

// FFIModule is the libffi binding of yzma. The cleanup runtime is built with
// the tag ffi_no_embed, so the binding loads the libffi that ships with the
// runtime instead of writing its copy to the user cache. On macOS the build
// names the file (FFIFilenameFlag); on Windows libffi-8.dll is beside
// sayso-cleanup.exe, where the default search finds it. Linux uses the
// system's libffi.so.8 (a package dependency).
const FFIModule = "github.com/jupiterrider/ffi@v0.8.0"

// FFIFilenameFlag is the -ldflags -X value of the macOS build.
const FFIFilenameFlag = "github.com/jupiterrider/ffi.filename=@executable_path/../Frameworks/llama/libffi.8.dylib"

// FFIFile is the libffi of a platform in FFIModule (assets/libffi/<os>_<arch>/).
type FFIFile struct {
	GOOS, GOARCH, Name, SHA256 string
}

// FFIFiles are the files that ship; go.sum pins the module, and these hashes
// pin the files.
var FFIFiles = []FFIFile{
	{"darwin", "arm64", "libffi.8.dylib", "cfd3ba8ace2d36675055c8181b52ded057defc958e79d1283fdb09a19dd61efc"},
	{"darwin", "amd64", "libffi.8.dylib", "893f0b9459ee94b3106e36148c458ffaf7788eb68e1df962bb2cc6d714e9803a"},
	{"windows", "amd64", "libffi-8.dll", "c488ded1184b3df893f696ad7aba92e02e7aea7f90a61b3cc0f93b3dfc0ae3ec"},
}

// FFIFor is the libffi file of a platform, when the runtime ships one.
func FFIFor(goos, goarch string) (FFIFile, bool) {
	for _, f := range FFIFiles {
		if f.GOOS == goos && f.GOARCH == goarch {
			return f, true
		}
	}
	return FFIFile{}, false
}

// CopyChecked copies src to dst when src has the SHA-256 want.
func CopyChecked(src, dst, want string) error {
	got, err := fileSHA256(src)
	if err != nil {
		return err
	}
	if got != want {
		return fmt.Errorf("%s: the SHA-256 is %s, not %s", src, got, want)
	}
	data, err := os.ReadFile(src)
	if err != nil {
		return err
	}
	return os.WriteFile(dst, data, 0o755)
}
