package update

import (
	"errors"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"sync/atomic"
	"time"
)

// StatusKind is the state the updater reports.
type StatusKind int

const (
	StatusIdle StatusKind = iota
	StatusChecking
	StatusAvailable
	StatusDownloading
	StatusPreparing
	StatusReady
	StatusUnreadable // signed with an unknown key or schema: a newer Sayso is needed
	StatusFailed
)

// Status is one report of the updater.
type Status struct {
	Kind            StatusKind
	LastCheck       *uint64 // Idle
	Version         Semver
	NotesURL        string // Available
	Manual          Manual // Available: why the user must update by hand
	Package         Package
	Received, Total uint64 // Downloading
	Failure         Failure
}

// Options configure an updater.
type Options struct {
	URL          string
	HTTPSOnly    bool
	Platform     string
	AutoCheck    bool
	Automatic    bool
	FirstDelay   time.Duration
	Interval     time.Duration
	DownloadsDir string
	Context      Context
	Installer    Installer // nil: Manual says why
	Manual       Manual
	Package      Package
	Lock         *InstallLock
}

// OptionsForThisBuild sets up the official updater. ok is false for a build
// that does not update (not official, no keys). A copy that started while a
// swap ran, and whose bundle is now another version, exits.
func OptionsForThisBuild(dataDir, cacheDir string, autoCheck, automatic bool) (Options, bool) {
	in, manual, pkg := DetectInstaller()
	var lock *InstallLock
	waited := false
	if in != nil {
		var err error
		if lock, waited, err = SharedLock(in.LockPath()); err != nil {
			slog.Warn("update: no install lock", "error", err)
			lock = nil
		}
	}
	running, err := ParseVersion(Version)
	if err != nil {
		return Options{}, false
	}
	if in != nil && lock != nil && waited {
		if v, ok := in.InstalledVersion(); ok && !v.Equal(running) {
			os.Exit(0)
		}
	}
	o := Options{URL: ManifestURL, HTTPSOnly: true, Platform: Platform(), AutoCheck: autoCheck, Automatic: automatic,
		FirstDelay: 30 * time.Second, Interval: 24 * time.Hour, DownloadsDir: filepath.Join(cacheDir, "updates"),
		Context:   Context{StatePath: filepath.Join(dataDir, "update-state.json"), Keys: ReleasePublicKeys(), URLPrefix: URLPrefix, Running: running},
		Installer: in, Manual: manual, Package: pkg, Lock: lock}
	if in != nil && lock == nil {
		o.Installer, o.Manual = nil, ManualNoPermission
	}
	if !IsOfficialBuild() {
		// A development build may point at a test server.
		url, key := os.Getenv("SAYSO_UPDATE_URL"), os.Getenv("SAYSO_UPDATE_KEY")
		if url == "" || key == "" {
			return o, false
		}
		id, k, ok := strings.Cut(key, ":")
		pk, err := ParsePublicKey(id, k)
		if !ok || err != nil {
			return o, false
		}
		o.HTTPSOnly = strings.HasPrefix(url, "https://")
		o.Context.URLPrefix = url[:strings.LastIndexByte(url, '/')+1]
		o.URL = url
		o.Context.Keys = []PublicKey{pk}
		o.FirstDelay = 2 * time.Second
		return o, true
	}
	return o, len(o.Context.Keys) > 0
}

// AtStart finishes or reports a prepared update.
func (o *Options) AtStart() AtStart {
	if o.Installer == nil || o.Lock == nil {
		return AtStart{}
	}
	return AtStartup(&o.Context, o.Installer, o.Lock)
}

type command int

const (
	cmdCheck command = iota
	cmdInstall
	cmdAutoOn
	cmdAutoOff
	cmdAutomaticOn
	cmdAutomaticOff
)

// Updater checks, downloads, and prepares updates on its own goroutine.
type Updater struct {
	commands chan command
	w        *worker
}

type offered struct {
	envelope string
	release  Release
}

type worker struct {
	o        Options
	cancel   atomic.Bool
	offer    *offered
	onStatus func(Status)
}

// Spawn starts the updater. onStatus is called on the updater goroutine.
func Spawn(o Options, onStatus func(Status)) *Updater {
	u := &Updater{commands: make(chan command, 16), w: &worker{o: o, onStatus: onStatus}}
	go u.loop()
	return u
}

func (u *Updater) loop() {
	auto := u.w.o.AutoCheck
	var timer <-chan time.Time
	arm := func(d time.Duration) {
		if auto {
			timer = time.After(d)
		} else {
			timer = nil
		}
	}
	arm(u.w.o.FirstDelay)
	for {
		select {
		case c, ok := <-u.commands:
			if !ok {
				return
			}
			switch c {
			case cmdAutoOn, cmdAutoOff:
				on := c == cmdAutoOn
				if on != auto {
					auto = on
					arm(u.w.o.FirstDelay)
				}
			case cmdAutomaticOn:
				u.w.o.Automatic = true
			case cmdAutomaticOff:
				u.w.o.Automatic = false
			case cmdInstall:
				u.w.install(true)
			case cmdCheck:
				u.w.check(true)
				arm(u.w.o.Interval)
			}
		case <-timer:
			u.w.check(false)
			arm(u.w.o.Interval)
		}
	}
}

func (u *Updater) CheckNow() { u.commands <- cmdCheck }
func (u *Updater) Install()  { u.commands <- cmdInstall }
func (u *Updater) Cancel()   { u.w.cancel.Store(true) }
func (u *Updater) Stop()     { close(u.commands) }
func (u *Updater) SetAutoCheck(on bool) {
	u.commands <- map[bool]command{true: cmdAutoOn, false: cmdAutoOff}[on]
}
func (u *Updater) SetAutomatic(on bool) {
	u.commands <- map[bool]command{true: cmdAutomaticOn, false: cmdAutomaticOff}[on]
}

// Swap installs the prepared update now (the user pressed Restart).
func (u *Updater) Swap() error {
	if u.w.o.Installer == nil || u.w.o.Lock == nil {
		return Failure{Kind: "cannot_update_itself"}
	}
	return Swap(&u.w.o.Context, u.w.o.Installer, u.w.o.Lock)
}

// Confirm is called by a new version once it runs.
func (u *Updater) Confirm() bool {
	if u.w.o.Installer == nil || u.w.o.Lock == nil {
		return true
	}
	return Confirm(&u.w.o.Context, u.w.o.Installer, u.w.o.Lock)
}

// Notice is the version that was just installed, until dismissed.
func (u *Updater) Notice() *Semver { return LoadState(u.w.o.Context.StatePath).Notice }

// DismissNotice clears the notice.
func (u *Updater) DismissNotice() {
	p := u.w.o.Context.StatePath
	s := LoadState(p)
	if s.Notice != nil {
		s.Notice = nil
		s.Save(p)
	}
}

func (w *worker) available(r Release) Status {
	m := ManualNone
	if w.o.Installer == nil {
		m = w.o.Manual
	}
	return Status{Kind: StatusAvailable, Version: r.Version, NotesURL: r.NotesURL, Manual: m, Package: w.o.Package}
}

func (w *worker) recordPhase() (Phase, Semver, bool) {
	if w.o.Installer == nil {
		return "", Semver{}, false
	}
	r := LoadRecord(w.o.Installer.RecordPath())
	if r == nil || (r.Phase == PhaseStaged && !w.o.Installer.StagingExists()) {
		return "", Semver{}, false
	}
	return r.Phase, r.ToVersion, true
}

func (w *worker) check(byUser bool) {
	if byUser {
		w.onStatus(Status{Kind: StatusChecking})
		if w.o.Installer != nil && w.o.Lock != nil {
			ClearFailure(w.o.Installer, w.o.Lock)
		}
	}
	release, err := w.fetch()
	if err != nil {
		var f Failure
		errors.As(err, &f)
		if byUser {
			w.onStatus(Status{Kind: StatusFailed, Failure: f})
		} else {
			slog.Warn("update: the check failed", "error", err)
		}
		return
	}
	if release == nil {
		return
	}
	phase, version, ok := w.recordPhase()
	switch {
	case ok && phase == PhaseStaged && version.Equal(release.Version):
		w.onStatus(Status{Kind: StatusReady, Version: version})
	case ok && phase != PhaseStaged && !byUser:
	default:
		w.onStatus(w.available(*release))
		if w.o.Automatic && w.o.Installer != nil {
			w.install(false)
		}
	}
}

func (w *worker) fetch() (*Release, error) {
	c := &w.o.Context
	data, err := Fetch(w.o.URL, w.o.HTTPSOnly)
	if err != nil {
		if ce, ok := err.(*CheckError); ok && ce.Kind == "network" {
			return nil, Failure{Kind: "unreachable", Detail: err.Error()}
		}
		return nil, Failure{Kind: "check_failed", Detail: err.Error()}
	}
	kind, m, err := VerifyFor(data, c.Keys, c.URLPrefix)
	if err != nil {
		return nil, Failure{Kind: "not_accepted", Detail: err.Error()}
	}
	if kind != VerifiedManifest {
		w.onStatus(Status{Kind: StatusUnreadable})
		return nil, nil
	}
	state := LoadState(c.StatePath)
	offer, release := OfferFor(m, c.Running, state.HighestSeen, w.o.Platform)
	if offer == OfferReplayed {
		return nil, Failure{Kind: "replayed", Detail: m.Version.String()}
	}
	state.Saw(m.Version, uint64(time.Now().Unix()))
	if err := state.Save(c.StatePath); err != nil {
		slog.Warn("update: could not write the state", "error", err)
	}
	if offer == OfferNewer {
		w.offer = &offered{string(data), *release}
		return release, nil
	}
	w.offer = nil
	w.onStatus(Status{Kind: StatusIdle, LastCheck: state.LastCheck})
	return nil, nil
}

func (w *worker) install(byUser bool) {
	if w.offer == nil || w.o.Installer == nil {
		return
	}
	of := *w.offer
	phase, version, ok := w.recordPhase()
	if ok && phase == PhaseStaged && version.Equal(of.release.Version) {
		w.onStatus(Status{Kind: StatusReady, Version: version})
		return
	}
	if ok && phase != PhaseStaged && !byUser {
		return
	}
	f, cancelled := w.downloadAndStage(of)
	switch {
	case f == nil && !cancelled:
		w.onStatus(Status{Kind: StatusReady, Version: of.release.Version})
	case cancelled:
		w.onStatus(w.available(of.release))
	case byUser:
		w.onStatus(Status{Kind: StatusFailed, Failure: *f})
	default:
		slog.Warn("update: the automatic update failed", "error", *f)
		w.onStatus(w.available(of.release))
	}
}

func (w *worker) downloadAndStage(of offered) (*Failure, bool) {
	in, v, total := w.o.Installer, of.release.Version, of.release.Asset.Size
	w.cancel.Store(false)
	w.onStatus(Status{Kind: StatusDownloading, Version: v, Total: total})
	last := time.Now()
	file, err := Download(of.release.Asset, w.o.DownloadsDir, w.o.HTTPSOnly, &w.cancel, func(received uint64) {
		if time.Since(last) >= 120*time.Millisecond || received == total {
			last = time.Now()
			w.onStatus(Status{Kind: StatusDownloading, Version: v, Received: received, Total: total})
		}
	})
	if err != nil {
		de, _ := err.(*DownloadError)
		switch {
		case de != nil && de.Kind == "cancelled":
			return nil, true
		case de != nil && de.Kind == "mismatch":
			return &Failure{Kind: "mismatch"}, false
		case de != nil && de.Kind == "disk":
			return &Failure{Kind: "disk", Detail: de.Msg}, false
		}
		return &Failure{Kind: "download", Detail: err.Error()}, false
	}
	w.onStatus(Status{Kind: StatusPreparing, Version: v})
	if w.o.Lock == nil {
		os.Remove(file)
		return &Failure{Kind: "busy"}, false
	}
	release, ok := w.o.Lock.TryExclusive()
	if !ok {
		os.Remove(file)
		return &Failure{Kind: "busy"}, false
	}
	defer release()
	path := in.RecordPath()
	DeleteRecord(path)
	err = in.Stage(file, v)
	os.Remove(file)
	if err != nil {
		return &Failure{Kind: "prepare", Detail: err.Error()}, false
	}
	r := Record{Envelope: of.envelope, FromVersion: w.o.Context.Running, ToVersion: v, InstallPath: in.InstallPath(), Phase: PhaseStaged}
	if err := r.Save(path); err != nil {
		in.RemoveStaging()
		return &Failure{Kind: "write_record", Detail: err.Error()}, false
	}
	return nil, false
}
