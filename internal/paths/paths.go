// Package paths resolves Sayso's file locations, with the same XDG lookup as
// crates/sayso-core/src/paths.rs: the XDG variables win on every system, then
// ~/.config/sayso when it exists (config only), then the system default.
package paths

import (
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"unicode/utf16"
)

// Env is what path resolution reads. An interface so tests do not touch the
// real home directory.
type Env interface {
	// Var returns an environment variable. An empty value counts as not set.
	Var(key string) string
	Home() string
	Exists(path string) bool
}

// SystemEnv is the real process environment.
type SystemEnv struct{}

func (SystemEnv) Var(key string) string { return os.Getenv(key) }

func (SystemEnv) Home() string {
	if h := os.Getenv("HOME"); h != "" {
		return h
	}
	if h := os.Getenv("USERPROFILE"); h != "" {
		return h
	}
	return string(filepath.Separator)
}

func (SystemEnv) Exists(path string) bool {
	_, err := os.Stat(path)
	return err == nil
}

// Source says which rule picked a directory. Settings shows it next to the path.
type Source int

const (
	XdgVariable Source = iota
	DotConfig
	PlatformDefault
)

// Environment variables that the engines read.
const (
	// ModelsDirEnv is the models folder of an engine.
	ModelsDirEnv = "SAYSO_MODELS_DIR"
	// ModelStoreEnv is the model store folder (models/v1) of an engine.
	ModelStoreEnv = "SAYSO_MODEL_STORE"
	// ModelStoreFolder is the folder of the store in the models folder.
	ModelStoreFolder = "v1"
)

// Paths holds the resolved folders.
type Paths struct {
	ConfigDir    string
	ConfigSource Source
	DataDir      string
	DataSource   Source
	CacheDir     string
}

// Resolve resolves the folders for this system.
func Resolve(env Env) Paths { return ResolveFor(runtime.GOOS, env) }

// ResolveFor resolves the folders as `goos` would. Paths are joined with the
// separator of the running system.
func ResolveFor(goos string, env Env) Paths {
	home := env.Home()
	platform := platformDirs(goos, env, home)
	var p Paths
	if xdg := env.Var("XDG_CONFIG_HOME"); xdg != "" {
		p.ConfigDir, p.ConfigSource = filepath.Join(xdg, "sayso"), XdgVariable
	} else if dot := filepath.Join(home, ".config", "sayso"); env.Exists(dot) {
		p.ConfigDir, p.ConfigSource = dot, DotConfig
	} else {
		p.ConfigDir, p.ConfigSource = platform.config, PlatformDefault
	}
	if xdg := env.Var("XDG_DATA_HOME"); xdg != "" {
		p.DataDir, p.DataSource = filepath.Join(xdg, "sayso"), XdgVariable
	} else {
		p.DataDir, p.DataSource = platform.data, PlatformDefault
	}
	if xdg := env.Var("XDG_CACHE_HOME"); xdg != "" {
		p.CacheDir = filepath.Join(xdg, "sayso")
	} else {
		p.CacheDir = platform.cache
	}
	return p
}

type dirs struct{ config, data, cache string }

func platformDirs(goos string, env Env, home string) dirs {
	switch goos {
	case "darwin":
		// ~/Library/Application Support/Sayso and ~/Library/Caches/Sayso.
		support := filepath.Join(home, "Library", "Application Support", "Sayso")
		return dirs{support, support, filepath.Join(home, "Library", "Caches", "Sayso")}
	case "windows":
		// Config roams with the user; data and cache stay local, because
		// models are large and must not roam.
		roaming := env.Var("APPDATA")
		if roaming == "" {
			roaming = filepath.Join(home, "AppData", "Roaming")
		}
		local := env.Var("LOCALAPPDATA")
		if local == "" {
			local = filepath.Join(home, "AppData", "Local")
		}
		local = filepath.Join(local, "Sayso")
		return dirs{filepath.Join(roaming, "Sayso"), local, filepath.Join(local, "Cache")}
	default:
		return dirs{
			filepath.Join(home, ".config", "sayso"),
			filepath.Join(home, ".local", "share", "sayso"),
			filepath.Join(home, ".cache", "sayso"),
		}
	}
}

func (p Paths) ConfigFile() string   { return filepath.Join(p.ConfigDir, "config.toml") }
func (p Paths) StylesDir() string    { return filepath.Join(p.ConfigDir, "styles") }
func (p Paths) DatabaseFile() string { return filepath.Join(p.DataDir, "sayso.db") }
func (p Paths) AudioDir() string     { return filepath.Join(p.DataDir, "audio") }
func (p Paths) ModelsDir() string    { return filepath.Join(p.DataDir, "models") }

// ModelStoreDir is <models>/v1, the model store that engines share.
func (p Paths) ModelStoreDir() string { return filepath.Join(p.ModelsDir(), ModelStoreFolder) }

// AppsFile holds the user's answers about connected apps (docs/sdk.md, "Approval").
func (p Paths) AppsFile() string { return filepath.Join(p.DataDir, "apps.toml") }

// StateFile is runtime state that is not configuration, such as the pill position.
func (p Paths) StateFile() string { return filepath.Join(p.DataDir, "state.json") }
func (p Paths) LogDir() string    { return filepath.Join(p.CacheDir, "logs") }

// InstanceLockFile is the lock that macOS and Linux builds hold so that only
// one Sayso runs for each data folder. Windows uses a named mutex instead
// (see InstanceMutexName).
func (p Paths) InstanceLockFile() string { return filepath.Join(p.DataDir, "instance.lock") }

// CreateAll makes every folder.
func (p Paths) CreateAll() error {
	for _, dir := range []string{p.ConfigDir, p.StylesDir(), p.DataDir, p.AudioDir(), p.ModelsDir(), p.CacheDir, p.LogDir()} {
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return err
		}
	}
	return nil
}

// Display replaces a leading home directory with ~ for display.
func Display(path, home string) string {
	rel, err := filepath.Rel(home, path)
	if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(filepath.Separator)) || filepath.IsAbs(rel) {
		return path
	}
	if rel == "." {
		return "~" + string(filepath.Separator)
	}
	return "~" + string(filepath.Separator) + rel
}

// Windows single-instance names (crates/sayso-platform-windows/src/window.rs).
const (
	InstanceMutex = `Local\dev.sayso.Sayso.instance`
	ShowEvent     = `Local\dev.sayso.Sayso.show`
)

// InstanceScope is empty for the default data folder, so the installer finds
// the mutex by its fixed name, and the data folder otherwise.
func (p Paths) InstanceScope() string {
	if p.DataSource == PlatformDefault {
		return ""
	}
	return p.DataDir
}

// InstanceMutexName and ShowEventName give the names for a scope: a scope
// adds a dot and the FNV-1a hash of its lowercase UTF-16 text.
func InstanceMutexName(scope string) string { return InstanceMutex + scopeSuffix(scope) }
func ShowEventName(scope string) string     { return ShowEvent + scopeSuffix(scope) }

func scopeSuffix(scope string) string {
	if scope == "" {
		return ""
	}
	units := utf16.Encode([]rune(strings.ToLower(scope)))
	hash := uint32(0x811c9dc5)
	for _, u := range units {
		hash = (hash ^ uint32(u&0xff)) * 0x01000193
		hash = (hash ^ uint32(u>>8)) * 0x01000193
	}
	return fmt.Sprintf(".%08x", hash)
}
