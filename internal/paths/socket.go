package paths

import (
	"errors"
	"fmt"
	"path/filepath"
	"runtime"
)

// The Host API socket, as crates/sayso-protocol/src/socket.rs finds it:
//
//  1. SAYSO_HOST_SOCKET, when it is set.
//  2. Linux: $XDG_RUNTIME_DIR/sayso/host.sock, or
//     $XDG_RUNTIME_DIR/app/<app id>/host.sock in a Flatpak.
//  3. macOS: sayso/host.sock in the user's temporary folder
//     (confstr(_CS_DARWIN_USER_TEMP_DIR)).
//  4. Else: /tmp/sayso-<uid>/host.sock.
//  5. Windows: the named pipe \\.\pipe\sayso-host-<user SID>.

// SocketEnvVar is the environment variable with the path of the socket.
const SocketEnvVar = "SAYSO_HOST_SOCKET"

// SocketFile is the file name of the socket in its folder.
const SocketFile = "host.sock"

// System is the system for the socket rules.
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
	default:
		return OtherUnix
	}
}

// SocketEnv is what the socket rules read from the system.
type SocketEnv interface {
	// Var returns an environment variable; empty means not set.
	Var(key string) string
	// DarwinUserTempDir is confstr(_CS_DARWIN_USER_TEMP_DIR) on macOS, or "".
	DarwinUserTempDir() string
	// UID is the Unix user id; ok is false where there is none.
	UID() (uint32, bool)
	// UserSID is the Windows SID of the user, such as S-1-5-21-..., or "".
	UserSID() string
}

// HostSocketPath is the path of the host socket (the pipe name on Windows).
func HostSocketPath() (string, error) {
	return HostSocketPathFor(CurrentSystem(), SystemSocketEnv{})
}

// HostSocketPathFor applies the rules for `system` with `env`. Paths use `/`
// on Unix systems whatever the running system is, as the rules name them.
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
		runtimeDir := env.Var("XDG_RUNTIME_DIR")
		if runtimeDir == "" {
			return inTmp()
		}
		if app := env.Var("FLATPAK_ID"); app != "" {
			// All sandboxes of the app share this folder.
			return joinUnix(runtimeDir, "app", app, SocketFile), nil
		}
		return joinUnix(runtimeDir, "sayso", SocketFile), nil
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
	default:
		return inTmp()
	}
}

// ClientSocketPath is where an app looks for the host: the same as
// HostSocketPath, except inside a Flatpak on Linux, where it looks in the
// folder outside the sandbox.
func ClientSocketPath() (string, error) {
	return ClientSocketPathFor(CurrentSystem(), SystemSocketEnv{})
}

func ClientSocketPathFor(system System, env SocketEnv) (string, error) {
	if system == Linux && env.Var(SocketEnvVar) == "" {
		if runtimeDir := env.Var("XDG_RUNTIME_DIR"); runtimeDir != "" {
			return joinUnix(runtimeDir, "sayso", SocketFile), nil
		}
	}
	return HostSocketPathFor(system, env)
}

func joinUnix(parts ...string) string {
	if runtime.GOOS != "windows" {
		return filepath.Join(parts...)
	}
	out := parts[0]
	for _, p := range parts[1:] {
		if len(out) > 0 && out[len(out)-1] == '/' {
			out += p
		} else {
			out += "/" + p
		}
	}
	return out
}
