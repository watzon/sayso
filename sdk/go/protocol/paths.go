package protocol

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
)

// SocketEnvVar is the environment variable with the path of the socket.
const SocketEnvVar = "SAYSO_HOST_SOCKET"

// SocketFile is the file name of the socket in its folder.
const SocketFile = "host.sock"

// Environment variables that the engines read.
const (
	ModelsDirEnv     = "SAYSO_MODELS_DIR"
	ModelStoreEnv    = "SAYSO_MODEL_STORE"
	ModelStoreFolder = "v1"
)

// System is a system of the socket rules (sdk/protocol/paths.md).
type System int

const (
	Linux System = iota
	MacOS
	Windows
	OtherUnix
)

// CurrentSystem is the system this binary runs on.
func CurrentSystem() System {
	switch runtime.GOOS {
	case "linux":
		return Linux
	case "darwin":
		return MacOS
	case "windows":
		return Windows
	}
	return OtherUnix
}

// SocketEnv is what the socket rules read from the system.
type SocketEnv interface {
	// Var is an environment variable; empty means not set.
	Var(key string) string
	// DarwinUserTempDir is confstr(_CS_DARWIN_USER_TEMP_DIR) without its last
	// "/" on macOS, or "".
	DarwinUserTempDir() string
	// UID is the Unix user id; ok is false where there is none.
	UID() (uint32, bool)
	// UserSID is the Windows SID of the user, or "".
	UserSID() string
}

// SystemSocketEnv reads the real environment.
type SystemSocketEnv struct{}

func (SystemSocketEnv) Var(key string) string     { return os.Getenv(key) }
func (SystemSocketEnv) DarwinUserTempDir() string { return strings.TrimRight(darwinUserTempDir(), "/") }
func (SystemSocketEnv) UID() (uint32, bool)       { return userID() }
func (SystemSocketEnv) UserSID() string           { return userSID() }

// HostSocketPath is where the host of this user binds its socket (the pipe
// name on Windows).
func HostSocketPath() (string, error) { return HostSocketPathFor(CurrentSystem(), SystemSocketEnv{}) }

// ClientSocketPath is where a client of this user looks for the host.
func ClientSocketPath() (string, error) {
	return ClientSocketPathFor(CurrentSystem(), SystemSocketEnv{})
}

// HostSocketPathFor applies the rules of system to env. Unix paths use "/"
// whatever system runs this.
func HostSocketPathFor(system System, env SocketEnv) (string, error) {
	if p := env.Var(SocketEnvVar); p != "" {
		return p, nil
	}
	inTmp := func() (string, error) {
		uid, ok := env.UID()
		if !ok {
			return "", errors.New("cannot find the user id")
		}
		return fmt.Sprintf("/tmp/sayso-%d/%s", uid, SocketFile), nil
	}
	switch system {
	case Linux:
		dir := env.Var("XDG_RUNTIME_DIR")
		if dir == "" {
			return inTmp()
		}
		if app := env.Var("FLATPAK_ID"); app != "" {
			return joinUnix(dir, "app", app, SocketFile), nil
		}
		return joinUnix(dir, "sayso", SocketFile), nil
	case MacOS:
		if temp := env.DarwinUserTempDir(); temp != "" {
			return joinUnix(temp, "sayso", SocketFile), nil
		}
		return inTmp()
	case Windows:
		sid := env.UserSID()
		if sid == "" {
			return "", errors.New("cannot find the SID of the user")
		}
		return `\\.\pipe\sayso-host-` + sid, nil
	}
	return inTmp()
}

// ClientSocketPathFor is HostSocketPathFor, except on Linux with
// XDG_RUNTIME_DIR, where a client looks outside a Flatpak sandbox.
func ClientSocketPathFor(system System, env SocketEnv) (string, error) {
	if system == Linux && env.Var(SocketEnvVar) == "" {
		if dir := env.Var("XDG_RUNTIME_DIR"); dir != "" {
			return joinUnix(dir, "sayso", SocketFile), nil
		}
	}
	return HostSocketPathFor(system, env)
}

func joinUnix(parts ...string) string {
	out := strings.TrimRight(parts[0], "/")
	for _, p := range parts[1:] {
		out += "/" + p
	}
	return out
}

// DataEnv is what the data folder rules read.
type DataEnv interface {
	Var(key string) string
	Home() string
}

// SystemDataEnv reads the real environment.
type SystemDataEnv struct{}

func (SystemDataEnv) Var(key string) string { return os.Getenv(key) }

func (SystemDataEnv) Home() string {
	if h, err := os.UserHomeDir(); err == nil {
		return h
	}
	return string(filepath.Separator)
}

// DataDirFor is the data folder of Sayso on goos. It joins with the
// separator of the running system.
func DataDirFor(goos string, env DataEnv) string {
	if xdg := env.Var("XDG_DATA_HOME"); xdg != "" {
		return filepath.Join(xdg, "sayso")
	}
	home := env.Home()
	switch goos {
	case "darwin":
		return filepath.Join(home, "Library", "Application Support", "Sayso")
	case "windows":
		local := env.Var("LOCALAPPDATA")
		if local == "" {
			local = filepath.Join(home, "AppData", "Local")
		}
		return filepath.Join(local, "Sayso")
	}
	return filepath.Join(home, ".local", "share", "sayso")
}

// DataDir is the data folder of Sayso on this system.
func DataDir() string { return DataDirFor(runtime.GOOS, SystemDataEnv{}) }

// ModelsDir is the models folder in a data folder (SAYSO_MODELS_DIR).
func ModelsDir(dataDir string) string { return filepath.Join(dataDir, "models") }

// ModelStoreDir is the model store in a models folder (SAYSO_MODEL_STORE).
func ModelStoreDir(modelsDir string) string { return filepath.Join(modelsDir, ModelStoreFolder) }

// CleanupModelsDir holds a folder for each cleanup model.
func CleanupModelsDir(modelsDir string) string { return filepath.Join(modelsDir, "cleanup") }
