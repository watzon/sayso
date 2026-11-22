package update

import (
	"log/slog"
	"path/filepath"
	"strings"
)

// Manual is why an install cannot update itself.
type Manual int

const (
	ManualNone Manual = iota
	ManualUnsupported
	ManualPackaged
	ManualWrongFolder
	ManualNoPermission
	ManualNoExchange
)

// Package is a Linux package that updates through its own channel.
type Package int

const (
	PackageNone Package = iota
	PackageAppImage
	PackageFlatpak
	PackageAUR
	PackageNix
)

// Installer installs a prepared update for one kind of install.
type Installer interface {
	InstallPath() string
	LockPath() string
	RecordPath() string
	InstalledVersion() (Semver, bool)
	Stage(file string, version Semver) error
	StagingExists() bool
	VerifyStaged() error
	Exchange() error
	Relaunch() error
	RemoveStaging()
}

// packageOf names the package from the place of the executable.
func packageOf(exe string, flatpak bool, appdir, aurDir string) Package {
	if flatpak {
		return PackageFlatpak
	}
	if appdir != "" {
		if dir, err := filepath.EvalSymlinks(appdir); err == nil && within(exe, dir) {
			return PackageAppImage
		}
	}
	if within(exe, "/nix/store") {
		return PackageNix
	}
	if aurDir != "" {
		dir := aurDir
		if real, err := filepath.EvalSymlinks(aurDir); err == nil {
			dir = real
		}
		if filepath.Dir(exe) == dir {
			return PackageAUR
		}
	}
	return PackageNone
}

func within(p, dir string) bool {
	rel, err := filepath.Rel(dir, p)
	return err == nil && rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)) && !filepath.IsAbs(rel)
}

// Context is what a swap checks against.
type Context struct {
	StatePath string
	Keys      []PublicKey
	URLPrefix string
	Running   Semver
}

// AtStartKind is what start-up found.
type AtStartKind int

const (
	AtStartNothing AtStartKind = iota
	AtStartRelaunching
	AtStartStale
	AtStartReady
	AtStartUpdated
	AtStartFailed
)

// AtStart is the result of AtStartup.
type AtStart struct {
	Kind    AtStartKind
	Version Semver
	Failure Failure
}

// ErrBusy is a swap while another Sayso of this install runs.
type swapBusy struct{}

func (swapBusy) Error() string { return "busy" }

// AtStartup finishes or reports a prepared update.
func AtStartup(c *Context, in Installer, lock *InstallLock) AtStart {
	path := in.RecordPath()
	r := LoadRecord(path)
	if r == nil {
		in.RemoveStaging()
		return AtStart{}
	}
	if r.InstallPath != in.InstallPath() {
		in.RemoveStaging()
		DeleteRecord(path)
		return AtStart{}
	}
	runningNew := c.Running.Equal(r.ToVersion)
	switch r.Phase {
	case PhaseStaged:
		if !in.StagingExists() {
			DeleteRecord(path)
			return AtStart{}
		}
		err := Swap(c, in, lock)
		switch e := err.(type) {
		case nil:
			return AtStart{Kind: AtStartRelaunching}
		case swapBusy:
			return AtStart{Kind: AtStartReady, Version: r.ToVersion}
		case Failure:
			return AtStart{Kind: AtStartFailed, Failure: e}
		}
	case PhaseSwapping, PhaseSwapped:
		if runningNew {
			return AtStart{Kind: AtStartUpdated, Version: r.ToVersion}
		}
		if v, ok := in.InstalledVersion(); ok && v.Equal(r.ToVersion) {
			return AtStart{Kind: AtStartStale}
		}
		in.RemoveStaging()
		f := Failure{Kind: "not_installed"}
		r.Fail(path, f)
		return AtStart{Kind: AtStartFailed, Failure: f}
	case PhaseFailed:
		f := Failure{Kind: "unknown"}
		if r.Failure != nil {
			f = *r.Failure
		} else if r.Error != nil {
			f = Failure{Kind: "recorded", Detail: *r.Error}
		}
		return AtStart{Kind: AtStartFailed, Failure: f}
	}
	return AtStart{}
}

// IsBusy is true for the error of a swap while another copy runs.
func IsBusy(err error) bool { _, ok := err.(swapBusy); return ok }

// Swap puts the prepared update in place and starts it. The error is a
// Failure, or busy (IsBusy).
func Swap(c *Context, in Installer, lock *InstallLock) error {
	release, ok := lock.TryExclusive()
	if !ok {
		return swapBusy{}
	}
	defer release()
	r := LoadRecord(in.RecordPath())
	if r == nil || r.Phase != PhaseStaged {
		return Failure{Kind: "nothing_staged"}
	}
	return swapLocked(c, in, r)
}

func swapLocked(c *Context, in Installer, r *Record) error {
	path := in.RecordPath()
	giveUp := func(f Failure) error {
		in.RemoveStaging()
		r.Fail(path, f)
		return f
	}
	if f := checkStaged(c, in, r); f != nil {
		return giveUp(*f)
	}
	r.Phase = PhaseSwapping
	r.Attempts++
	if err := r.Save(path); err != nil {
		return giveUp(Failure{Kind: "write_record", Detail: err.Error()})
	}
	if err := in.Exchange(); err != nil {
		return giveUp(Failure{Kind: "exchange", Detail: err.Error()})
	}
	r.Phase = PhaseSwapped
	if err := r.Save(path); err != nil {
		slog.Warn("update: could not write the record after the exchange", "error", err)
	}
	if err := in.Relaunch(); err != nil {
		if back := in.Exchange(); back != nil {
			f := Failure{Kind: "relaunch_and_restore", Detail: err.Error(), Back: back.Error()}
			r.Fail(path, f)
			return f
		}
		return giveUp(Failure{Kind: "relaunch", Detail: err.Error()})
	}
	return nil
}

func checkStaged(c *Context, in Installer, r *Record) *Failure {
	kind, m, err := VerifyFor([]byte(r.Envelope), c.Keys, c.URLPrefix)
	if err != nil || kind != VerifiedManifest || !m.Version.Equal(r.ToVersion) {
		return &Failure{Kind: "bad_signature"}
	}
	if r.InstallPath != in.InstallPath() {
		return &Failure{Kind: "moved"}
	}
	if r.ToVersion.CmpPrecedence(c.Running) <= 0 {
		return &Failure{Kind: "not_newer"}
	}
	if !in.StagingExists() {
		return &Failure{Kind: "staging_gone"}
	}
	if err := in.VerifyStaged(); err != nil {
		return &Failure{Kind: "verify", Detail: err.Error()}
	}
	return nil
}

// AwaitsConfirm is true when the new version must confirm it started.
func AwaitsConfirm(in Installer) bool {
	r := LoadRecord(in.RecordPath())
	return r != nil && (r.Phase == PhaseSwapping || r.Phase == PhaseSwapped)
}

// Confirm ends an update that started: the record goes and the notice is set.
// False when another copy holds the lock.
func Confirm(c *Context, in Installer, lock *InstallLock) bool {
	release, ok := lock.TryExclusive()
	if !ok {
		return false
	}
	defer release()
	path := in.RecordPath()
	r := LoadRecord(path)
	if r == nil {
		return true
	}
	if !((r.Phase == PhaseSwapping || r.Phase == PhaseSwapped) && r.ToVersion.Equal(c.Running) && r.InstallPath == in.InstallPath()) {
		return true
	}
	in.RemoveStaging()
	DeleteRecord(path)
	in.RemoveStaging()
	s := LoadState(c.StatePath)
	v := r.ToVersion
	s.Notice = &v
	if err := s.Save(c.StatePath); err != nil {
		slog.Warn("update: could not write the state", "error", err)
	}
	return true
}

// ClearFailure removes a failed record, before a check by the user.
func ClearFailure(in Installer, lock *InstallLock) {
	release, ok := lock.TryExclusive()
	if !ok {
		return
	}
	defer release()
	path := in.RecordPath()
	if r := LoadRecord(path); r != nil && r.Phase == PhaseFailed {
		in.RemoveStaging()
		DeleteRecord(path)
		in.RemoveStaging()
	}
}
