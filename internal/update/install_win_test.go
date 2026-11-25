package update

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestSetupCommandLine(t *testing.T) {
	installer := `C:\Users\Ann Lee\AppData\Local\Sayso\Cache\updates\staged-1\Sayso-update-setup.exe`
	log := `C:\Users\Ann Lee\AppData\Local\Sayso\Cache\updates\staged-1\update-setup.log`
	got := SetupCommandLine(installer, `C:\Users\Ann Lee\AppData\Local\Programs\Sayso\`, log, WinPerUser)
	want := `"` + installer + `" /SILENT /NORESTART /CURRENTUSER /NOCLOSEAPPLICATIONS /NORESTARTAPPLICATIONS /UPDATE=1 /DIR="C:\Users\Ann Lee\AppData\Local\Programs\Sayso" /LOG="` + log + `"`
	if filepath.Separator == '\\' && got != want {
		t.Errorf("per user:\n got %s\nwant %s", got, want)
	}
	// A portable copy shows the pages of Setup and lets it pick the folder.
	got = SetupCommandLine(`C:\t\setup.exe`, `D:\portable\Sayso`, `C:\t\log.txt`, WinPortable)
	if got != `C:\t\setup.exe /NORESTART /CURRENTUSER /NOCLOSEAPPLICATIONS /NORESTARTAPPLICATIONS /UPDATE=1 /LOG="C:\t\log.txt"` {
		t.Errorf("portable: %s", got)
	}
}

func TestWinStagingDirIsPerFolder(t *testing.T) {
	a := WinStagingDir(`C:\L`, `C:\Programs\Sayso`)
	if a != WinStagingDir(`C:\L`, `c:\programs\sayso`) {
		t.Error("the case of the folder changed the staging folder")
	}
	if a == WinStagingDir(`C:\L`, `D:\Sayso`) {
		t.Error("two folders share a staging folder")
	}
}

// winRelease serves a signed latest.json with an installer for this system.
func winRelease(t *testing.T, version string, installer []byte) (*httptest.Server, *Context) {
	t.Helper()
	k := key(5)
	var srv *httptest.Server
	srv = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/releases/latest.json":
			sum := sha256.Sum256(installer)
			p, _ := json.Marshal(map[string]any{"schema": 1, "version": version, "published": "x", "notes_url": srv.URL + "/releases/tag/v" + version,
				"assets": map[string]any{Platform(): map[string]any{"url": srv.URL + "/releases/Sayso-" + version + "-windows-x64-setup.exe", "sha256": hex.EncodeToString(sum[:]), "size": len(installer)}}})
			w.Write(envelope(string(p), "k", k))
		case "/releases/Sayso-" + version + "-windows-x64-setup.exe":
			w.Write(installer)
		default:
			w.WriteHeader(404)
		}
	}))
	t.Cleanup(srv.Close)
	dir := t.TempDir()
	c := &Context{StatePath: filepath.Join(dir, "update-state.json"), Keys: []PublicKey{public(t, "k", k)}, URLPrefix: srv.URL + "/releases/", Running: MustVersion("0.2.0")}
	return srv, c
}

type started struct{ exe, cmdLine string }

func winInstall(t *testing.T, kind WinKind, starts *[]started) *WinInstall {
	dir := t.TempDir()
	return &WinInstall{dir: filepath.Join(dir, "Programs", "Sayso"), staging: filepath.Join(dir, "staging"), kind: kind,
		start:     func(exe, cmdLine string) error { *starts = append(*starts, started{exe, cmdLine}); return nil },
		installed: func() (Semver, bool) { return MustVersion("0.2.0"), true }}
}

// waitFor reads statuses until one of kind, and returns those it read.
func waitFor(t *testing.T, statuses chan Status, kind StatusKind) []Status {
	t.Helper()
	var seen []Status
	timeout := time.After(10 * time.Second)
	for {
		select {
		case s := <-statuses:
			seen = append(seen, s)
			if s.Kind == kind {
				return seen
			}
			if s.Kind == StatusFailed {
				t.Fatalf("failed: %v", s.Failure)
			}
		case <-timeout:
			t.Fatalf("no status %d; saw %v", kind, seen)
		}
	}
}

// The whole update of Windows, with a Setup that only records its start:
// check, download with progress, stage, Restart (the checks and the start of
// Setup), and the confirm of the new version.
func TestWindowsUpdateDownloadsStagesAndStartsSetup(t *testing.T) {
	installer := []byte(strings.Repeat("MZ setup ", 40000))
	srv, c := winRelease(t, "0.3.0", installer)
	var starts []started
	w := winInstall(t, WinPerUser, &starts)
	lock, _, _ := SharedLock(w.LockPath())
	statuses := make(chan Status, 256)
	u := Spawn(Options{URL: srv.URL + "/releases/latest.json", Platform: Platform(), DownloadsDir: filepath.Join(w.staging, "..", "dl"),
		Context: *c, Installer: w, Lock: lock}, func(s Status) { statuses <- s })
	defer u.Stop()

	u.CheckNow()
	if s := waitFor(t, statuses, StatusAvailable); s[len(s)-1].Manual != ManualNone {
		t.Fatalf("manual %d", s[len(s)-1].Manual)
	}
	u.Install()
	seen := waitFor(t, statuses, StatusReady)
	progressed := false
	for _, s := range seen {
		if s.Kind == StatusDownloading && s.Total == uint64(len(installer)) && s.Received == s.Total {
			progressed = true
		}
	}
	if !progressed || !w.StagingExists() || LoadRecord(w.RecordPath()).Phase != PhaseStaged {
		t.Fatalf("download: %v, staged %v", seen, w.StagingExists())
	}
	if len(starts) != 0 {
		t.Fatal("Setup started before Restart")
	}

	u.Restart()
	if s := waitFor(t, statuses, StatusRestarting); s[len(s)-1].Version.String() != "0.3.0" {
		t.Fatal(s)
	}
	if len(starts) != 1 || starts[0].exe != w.installer() {
		t.Fatalf("starts %v", starts)
	}
	for _, arg := range []string{"/SILENT", "/CURRENTUSER", "/UPDATE=1", `/DIR="` + w.dir + `"`} {
		if !strings.Contains(starts[0].cmdLine, arg) {
			t.Errorf("%s misses %s", starts[0].cmdLine, arg)
		}
	}
	if r := LoadRecord(w.RecordPath()); r == nil || r.Phase != PhaseSwapped {
		t.Fatalf("record %+v", r)
	}

	// Setup starts the new version, which confirms.
	c.Running = MustVersion("0.3.0")
	if got := AtStartup(c, w, lock); got.Kind != AtStartUpdated {
		t.Fatal(got)
	}
	if !Confirm(c, w, lock) || LoadRecord(w.RecordPath()) != nil || w.StagingExists() {
		t.Fatal("confirm")
	}
}

// An installer that changed after the download never starts.
func TestWindowsRestartRefusesAChangedInstaller(t *testing.T) {
	installer := []byte(strings.Repeat("MZ setup ", 1000))
	srv, c := winRelease(t, "0.3.0", installer)
	var starts []started
	w := winInstall(t, WinPerUser, &starts)
	lock, _, _ := SharedLock(w.LockPath())
	statuses := make(chan Status, 64)
	u := Spawn(Options{URL: srv.URL + "/releases/latest.json", Platform: Platform(), Automatic: true, DownloadsDir: filepath.Join(w.staging, "..", "dl"),
		Context: *c, Installer: w, Lock: lock}, func(s Status) { statuses <- s })
	defer u.Stop()
	u.CheckNow()
	waitFor(t, statuses, StatusReady)
	if err := os.WriteFile(w.installer(), append(installer, '!'), 0o644); err != nil {
		t.Fatal(err)
	}
	u.Restart()
	var failed Status
	select {
	case failed = <-statuses:
	case <-time.After(5 * time.Second):
		t.Fatal("no status")
	}
	if failed.Kind != StatusFailed || failed.Failure.Kind != "verify" || len(starts) != 0 {
		t.Fatalf("%+v, starts %v", failed, starts)
	}
}

// A portable copy starts Setup with its pages, and keeps no record: it is
// not the copy that Setup updates.
func TestWindowsPortableCopyHandsOverToSetup(t *testing.T) {
	_, c := winRelease(t, "0.3.0", []byte("MZ"))
	var starts []started
	w := winInstall(t, WinPortable, &starts)
	lock, _, _ := SharedLock(w.LockPath())
	if err := os.MkdirAll(w.staging, 0o755); err != nil {
		t.Fatal(err)
	}
	src := filepath.Join(t.TempDir(), "dl.exe")
	os.WriteFile(src, []byte("MZ"), 0o644)
	if err := w.Stage(src, MustVersion("0.3.0")); err != nil {
		t.Fatal(err)
	}
	sum := sha256.Sum256([]byte("MZ"))
	p, _ := json.Marshal(map[string]any{"schema": 1, "version": "0.3.0", "published": "x", "notes_url": c.URLPrefix + "tag/v0.3.0",
		"assets": map[string]any{Platform(): map[string]any{"url": c.URLPrefix + "s.exe", "sha256": hex.EncodeToString(sum[:]), "size": 2}}})
	r := Record{Envelope: string(envelope(string(p), "k", key(5))), FromVersion: c.Running, ToVersion: MustVersion("0.3.0"), InstallPath: w.dir, Phase: PhaseStaged}
	if err := r.Save(w.RecordPath()); err != nil {
		t.Fatal(err)
	}
	if err := Swap(c, w, lock); err != nil {
		t.Fatal(err)
	}
	if len(starts) != 1 || strings.Contains(starts[0].cmdLine, "/SILENT") || strings.Contains(starts[0].cmdLine, "/DIR=") || !strings.Contains(starts[0].cmdLine, "/UPDATE=1") {
		t.Fatalf("starts %v", starts)
	}
	if LoadRecord(w.RecordPath()) != nil {
		t.Error("the portable copy kept its record")
	}
}
