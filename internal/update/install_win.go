package update

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
)

// The update of Windows (docs/updates.md, "Windows"): Sayso downloads and
// verifies the installer of the new version, and Setup installs it in view
// of the user. Stage keeps the verified installer, Exchange starts it, and
// Sayso quits; Setup waits for that and starts the new version.

// WinKind is the kind of copy that runs.
type WinKind int

const (
	// WinPerUser is an install for the current user: Setup updates its
	// folder in silent mode, with a progress window.
	WinPerUser WinKind = iota
	// WinPortable is a copy that Setup did not install: Setup runs with its
	// pages, so the user sees where it installs.
	WinPortable
)

// stagedInstaller is the name of the verified installer in the staging
// folder.
const stagedInstaller = "Sayso-update-setup.exe"

// WinInstall updates Sayso on Windows through its installer.
type WinInstall struct {
	dir     string // the folder of Sayso.exe
	staging string // the installer, update.json, and the Setup log
	kind    WinKind
	// start starts the installer with the command line, detached.
	start func(exe, cmdLine string) error
	// installed is the version of the install for the current user.
	installed func() (Semver, bool)
}

// WinStagingDir is the staging folder of the copy in dir, in the local app
// data: one for each folder, so an install and a portable copy never take
// each other's update.
func WinStagingDir(localAppData, dir string) string {
	sum := sha256.Sum256([]byte(strings.ToLower(filepath.Clean(dir))))
	return filepath.Join(localAppData, "Sayso", "Cache", "updates", "staged-"+hex.EncodeToString(sum[:4]))
}

// SameFolder compares two Windows folders, without case.
func SameFolder(a, b string) bool {
	return strings.EqualFold(filepath.Clean(a), filepath.Clean(b))
}

func (w *WinInstall) installer() string { return filepath.Join(w.staging, stagedInstaller) }

// SetupCommandLine is the command line of the installer: an update
// (/UPDATE=1, which makes Setup wait for Sayso to quit and start it again),
// for the current user, with a log next to the installer. Sayso quits by
// itself, so Setup closes and restarts no app.
func SetupCommandLine(installer, dir, log string, kind WinKind) string {
	args := []string{quoteArg(installer)}
	if kind == WinPerUser {
		args = append(args, "/SILENT")
	}
	args = append(args, "/NORESTART", "/CURRENTUSER", "/NOCLOSEAPPLICATIONS", "/NORESTARTAPPLICATIONS", "/UPDATE=1")
	if kind == WinPerUser {
		args = append(args, `/DIR="`+filepath.Clean(dir)+`"`)
	}
	args = append(args, `/LOG="`+log+`"`)
	return strings.Join(args, " ")
}

func quoteArg(s string) string {
	if strings.ContainsAny(s, " \t") {
		return `"` + s + `"`
	}
	return s
}

func (w *WinInstall) InstallPath() string { return w.dir }

// LockPath is not used: the install lock of Windows is in the process.
func (w *WinInstall) LockPath() string { return filepath.Join(w.staging, "install.lock") }

func (w *WinInstall) RecordPath() string { return filepath.Join(w.staging, "update.json") }

func (w *WinInstall) InstalledVersion() (Semver, bool) {
	if w.installed == nil {
		return Semver{}, false
	}
	return w.installed()
}

// Stage moves the downloaded installer, which Download checked, into the
// staging folder.
func (w *WinInstall) Stage(file string, _ Semver) error {
	w.RemoveStaging()
	if err := os.MkdirAll(w.staging, 0o755); err != nil {
		return err
	}
	if err := os.Rename(file, w.installer()); err == nil {
		return nil
	}
	if err := copyFile(file, w.installer()); err != nil {
		w.RemoveStaging()
		return err
	}
	return nil
}

func copyFile(from, to string) error {
	in, err := os.Open(from)
	if err != nil {
		return err
	}
	defer in.Close()
	out, err := os.Create(to)
	if err != nil {
		return err
	}
	if _, err := io.Copy(out, in); err != nil {
		out.Close()
		return err
	}
	return out.Close()
}

func (w *WinInstall) StagingExists() bool {
	st, err := os.Stat(w.installer())
	return err == nil && st.Mode().IsRegular()
}

// VerifyStaged checks the installer against the asset of this system in the
// signed update information of the record. The swap checked the signature
// just before.
func (w *WinInstall) VerifyStaged() error {
	r := LoadRecord(w.RecordPath())
	if r == nil {
		return errors.New("no update record")
	}
	var env Envelope
	if err := json.Unmarshal([]byte(r.Envelope), &env); err != nil {
		return err
	}
	var m Manifest
	if err := json.Unmarshal([]byte(env.Payload), &m); err != nil {
		return err
	}
	a, ok := m.Assets[Platform()]
	if !ok {
		return fmt.Errorf("the update has no file for %s", Platform())
	}
	f, err := os.Open(w.installer())
	if err != nil {
		return err
	}
	defer f.Close()
	h := sha256.New()
	n, err := io.Copy(h, f)
	if err != nil {
		return err
	}
	if uint64(n) != a.Size || hex.EncodeToString(h.Sum(nil)) != a.SHA256 {
		return errors.New("the installer is not the file of the release")
	}
	return nil
}

// Exchange starts the installer. Sayso quits next, and Setup replaces the
// files and starts the new version.
func (w *WinInstall) Exchange() error {
	if w.start == nil {
		return errors.New("this system cannot start the installer")
	}
	log := filepath.Join(w.staging, "update-setup.log")
	return w.start(w.installer(), SetupCommandLine(w.installer(), w.dir, log, w.kind))
}

// Relaunch leaves the start to Setup, which starts the new version when it
// is done. A portable copy stays the old version, so it has no update to
// confirm: its record goes.
func (w *WinInstall) Relaunch() error {
	if w.kind == WinPortable {
		DeleteRecord(w.RecordPath())
	}
	return nil
}

func (w *WinInstall) RemoveStaging() {
	os.Remove(w.installer())
	os.Remove(filepath.Join(w.staging, "update-setup.log"))
	os.Remove(w.staging)
}
