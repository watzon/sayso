package paths

import (
	"path/filepath"
	"runtime"
	"testing"
)

type fakeEnv struct {
	vars     map[string]string
	existing map[string]bool
	home     string
}

func (f fakeEnv) Var(k string) string  { return f.vars[k] }
func (f fakeEnv) Home() string         { return f.home }
func (f fakeEnv) Exists(p string) bool { return f.existing[p] }
func env(vars map[string]string, existing ...string) fakeEnv {
	e := fakeEnv{vars: vars, existing: map[string]bool{}, home: filepath.FromSlash("/Users/test")}
	for _, p := range existing {
		e.existing[filepath.FromSlash(p)] = true
	}
	return e
}

func fs(p string) string { return filepath.FromSlash(p) }

func TestXDGConfigHomeWins(t *testing.T) {
	for _, goos := range []string{"darwin", "linux", "windows"} {
		p := ResolveFor(goos, env(map[string]string{"XDG_CONFIG_HOME": fs("/x")}, "/Users/test/.config/sayso"))
		if p.ConfigDir != fs("/x/sayso") || p.ConfigSource != XdgVariable {
			t.Fatalf("%s: %+v", goos, p)
		}
	}
}

func TestDotConfigUsedOnlyWhenItExists(t *testing.T) {
	with := ResolveFor("darwin", env(nil, "/Users/test/.config/sayso"))
	if with.ConfigDir != fs("/Users/test/.config/sayso") || with.ConfigSource != DotConfig {
		t.Fatalf("%+v", with)
	}
	without := ResolveFor("darwin", env(nil))
	if without.ConfigDir != fs("/Users/test/Library/Application Support/Sayso") || without.ConfigSource != PlatformDefault {
		t.Fatalf("%+v", without)
	}
	linux := ResolveFor("linux", env(nil))
	if linux.ConfigDir != fs("/Users/test/.config/sayso") {
		t.Fatalf("%+v", linux)
	}
}

func TestDataAndCacheDefaults(t *testing.T) {
	mac := ResolveFor("darwin", env(nil))
	if mac.DataDir != fs("/Users/test/Library/Application Support/Sayso") || mac.CacheDir != fs("/Users/test/Library/Caches/Sayso") {
		t.Fatalf("%+v", mac)
	}
	lin := ResolveFor("linux", env(nil))
	if lin.DataDir != fs("/Users/test/.local/share/sayso") || lin.CacheDir != fs("/Users/test/.cache/sayso") {
		t.Fatalf("%+v", lin)
	}
	x := ResolveFor("darwin", env(map[string]string{"XDG_DATA_HOME": fs("/d"), "XDG_CACHE_HOME": fs("/c")}))
	if x.DataDir != fs("/d/sayso") || x.CacheDir != fs("/c/sayso") || x.DataSource != XdgVariable {
		t.Fatalf("%+v", x)
	}
}

func TestWindowsDefaultsUseAppData(t *testing.T) {
	p := ResolveFor("windows", env(map[string]string{"APPDATA": fs("/R"), "LOCALAPPDATA": fs("/L")}))
	if p.ConfigDir != fs("/R/Sayso") || p.DataDir != fs("/L/Sayso") || p.CacheDir != fs("/L/Sayso/Cache") {
		t.Fatalf("%+v", p)
	}
	fallback := ResolveFor("windows", env(nil))
	if fallback.ConfigDir != fs("/Users/test/AppData/Roaming/Sayso") || fallback.DataDir != fs("/Users/test/AppData/Local/Sayso") {
		t.Fatalf("%+v", fallback)
	}
}

func TestDerivedFiles(t *testing.T) {
	p := ResolveFor("linux", env(nil))
	cases := map[string]string{
		p.ConfigFile():    "/Users/test/.config/sayso/config.toml",
		p.StylesDir():     "/Users/test/.config/sayso/styles",
		p.DatabaseFile():  "/Users/test/.local/share/sayso/sayso.db",
		p.AudioDir():      "/Users/test/.local/share/sayso/audio",
		p.ModelsDir():     "/Users/test/.local/share/sayso/models",
		p.ModelStoreDir(): "/Users/test/.local/share/sayso/models/v1",
		p.AppsFile():      "/Users/test/.local/share/sayso/apps.toml",
		p.StateFile():     "/Users/test/.local/share/sayso/state.json",
		p.LogDir():        "/Users/test/.cache/sayso/logs",
	}
	for got, want := range cases {
		if got != fs(want) {
			t.Errorf("got %s want %s", got, want)
		}
	}
}

func TestCreateAllInTempDir(t *testing.T) {
	root := t.TempDir()
	p := ResolveFor(runtime.GOOS, env(map[string]string{
		"XDG_CONFIG_HOME": filepath.Join(root, "c"), "XDG_DATA_HOME": filepath.Join(root, "d"), "XDG_CACHE_HOME": filepath.Join(root, "k"),
	}))
	if err := p.CreateAll(); err != nil {
		t.Fatal(err)
	}
	for _, dir := range []string{p.StylesDir(), p.AudioDir(), p.ModelsDir(), p.LogDir()} {
		if !(SystemEnv{}).Exists(dir) {
			t.Errorf("missing %s", dir)
		}
	}
}

func TestDisplayUsesTilde(t *testing.T) {
	got := Display(fs("/Users/test/.config/sayso/config.toml"), fs("/Users/test"))
	if got != "~"+fs("/.config/sayso/config.toml") {
		t.Fatal(got)
	}
	if Display(fs("/other/x"), fs("/Users/test")) != fs("/other/x") {
		t.Fatal("outside home changed")
	}
}

func TestInstanceNames(t *testing.T) {
	if InstanceMutexName("") != `Local\dev.sayso.Sayso.instance` {
		t.Fatal(InstanceMutexName(""))
	}
	// FNV-1a over lowercase UTF-16 units, low byte then high byte.
	a, b := InstanceMutexName(`C:\Dev\Data`), InstanceMutexName(`c:\dev\data`)
	if a != b || len(a) != len(InstanceMutex)+9 {
		t.Fatal(a, b)
	}
}

type sockEnv struct {
	vars map[string]string
	temp string
	uid  *uint32
	sid  string
}

func (s sockEnv) Var(k string) string       { return s.vars[k] }
func (s sockEnv) DarwinUserTempDir() string { return s.temp }
func (s sockEnv) UID() (uint32, bool) {
	if s.uid == nil {
		return 0, false
	}
	return *s.uid, true
}
func (s sockEnv) UserSID() string { return s.sid }

func TestSocketRules(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("unix path forms")
	}
	uid := uint32(501)
	for _, sys := range []System{Linux, MacOS, Windows, OtherUnix} {
		got, _ := HostSocketPathFor(sys, sockEnv{vars: map[string]string{SocketEnvVar: "/x.sock"}})
		if got != "/x.sock" {
			t.Fatal(sys, got)
		}
	}
	got, _ := HostSocketPathFor(Linux, sockEnv{vars: map[string]string{"XDG_RUNTIME_DIR": "/run/user/1"}})
	if got != "/run/user/1/sayso/host.sock" {
		t.Fatal(got)
	}
	flat := sockEnv{vars: map[string]string{"XDG_RUNTIME_DIR": "/run/user/1", "FLATPAK_ID": "dev.sayso.Sayso"}}
	got, _ = HostSocketPathFor(Linux, flat)
	if got != "/run/user/1/app/dev.sayso.Sayso/host.sock" {
		t.Fatal(got)
	}
	got, _ = ClientSocketPathFor(Linux, flat)
	if got != "/run/user/1/sayso/host.sock" {
		t.Fatal(got)
	}
	got, _ = HostSocketPathFor(MacOS, sockEnv{temp: "/var/folders/xy/T"})
	if got != "/var/folders/xy/T/sayso/host.sock" {
		t.Fatal(got)
	}
	got, _ = HostSocketPathFor(OtherUnix, sockEnv{uid: &uid})
	if got != "/tmp/sayso-501/host.sock" {
		t.Fatal(got)
	}
	if _, err := HostSocketPathFor(OtherUnix, sockEnv{}); err == nil {
		t.Fatal("want error without uid")
	}
	got, _ = HostSocketPathFor(Windows, sockEnv{sid: "S-1-5-21-1-2-3-1001"})
	if got != `\\.\pipe\sayso-host-S-1-5-21-1-2-3-1001` {
		t.Fatal(got)
	}
	if _, err := HostSocketPathFor(Windows, sockEnv{}); err == nil {
		t.Fatal("want error without SID")
	}
}

func TestRealEnvironmentGivesASocketPath(t *testing.T) {
	p, err := HostSocketPath()
	if err != nil || p == "" {
		t.Fatal(p, err)
	}
	if runtime.GOOS == "darwin" && filepath.Base(filepath.Dir(p)) != "sayso" {
		t.Fatal(p)
	}
}
