package sayso

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
)

// Launcher starts Sayso one way. It returns at once, without waiting for the
// host; an error means this way did not apply or failed.
type Launcher func(ctx context.Context) error

// BundleID is the id of Sayso on macOS and in Flatpak.
const BundleID = "dev.sayso.Sayso"

// DefaultLaunchers are the ways to start Sayso on this system, in order:
// LaunchServices on macOS; the install folders on Windows; on Linux the
// folders of the tarball and the packages, then the Flatpak, then sayso on
// the PATH (only /app/bin/sayso inside the Flatpak of Sayso).
func DefaultLaunchers() []Launcher {
	switch runtime.GOOS {
	case "darwin":
		return []Launcher{program("/usr/bin/open", "-g", "-b", BundleID)}
	case "windows":
		var out []Launcher
		for _, env := range []string{"LOCALAPPDATA", "ProgramFiles"} {
			if base := os.Getenv(env); base != "" {
				dir := filepath.Join(base, "Sayso")
				if env == "LOCALAPPDATA" {
					dir = filepath.Join(base, "Programs", "Sayso")
				}
				out = append(out, file(filepath.Join(dir, "Sayso.exe")))
			}
		}
		return out
	}
	if os.Getenv("FLATPAK_ID") == BundleID {
		return []Launcher{file("/app/bin/sayso")}
	}
	var out []Launcher
	home, _ := os.UserHomeDir()
	for _, dir := range []string{filepath.Join(home, ".local", "lib", "sayso"), "/usr/local/lib/sayso", "/usr/lib/sayso"} {
		out = append(out, file(filepath.Join(dir, "sayso")))
	}
	out = append(out, flatpak(home), func(ctx context.Context) error {
		p, err := exec.LookPath("sayso")
		if err != nil {
			return err
		}
		return detach(exec.Command(p))
	})
	return out
}

func program(name string, args ...string) Launcher {
	return func(ctx context.Context) error { return exec.CommandContext(ctx, name, args...).Run() }
}

func file(path string) Launcher {
	return func(context.Context) error {
		if fi, err := os.Stat(path); err != nil || fi.IsDir() {
			return fmt.Errorf("no %s", path)
		}
		return detach(exec.Command(path))
	}
}

func flatpak(home string) Launcher {
	return func(context.Context) error {
		installed := false
		for _, dir := range []string{"/var/lib/flatpak/app/" + BundleID, filepath.Join(home, ".local", "share", "flatpak", "app", BundleID)} {
			if _, err := os.Stat(dir); err == nil {
				installed = true
			}
		}
		p, err := exec.LookPath("flatpak")
		if !installed || err != nil {
			return errors.New("the Flatpak of Sayso is not installed")
		}
		return detach(exec.Command(p, "run", BundleID))
	}
}

// detach starts Sayso as a process of its own, without the input, the
// output, or the SAYSO_HOST_SOCKET of this app.
func detach(cmd *exec.Cmd) error {
	for _, kv := range os.Environ() {
		if !strings.HasPrefix(kv, "SAYSO_HOST_SOCKET=") {
			cmd.Env = append(cmd.Env, kv)
		}
	}
	detachProcess(cmd)
	if err := cmd.Start(); err != nil {
		return err
	}
	return cmd.Process.Release()
}

// StartSayso tries each launcher until one works; nil launchers are
// DefaultLaunchers. It does not wait for the host.
func StartSayso(ctx context.Context, launchers []Launcher) error {
	if launchers == nil {
		launchers = DefaultLaunchers()
	}
	var errs []error
	for _, l := range launchers {
		err := l(ctx)
		if err == nil {
			return nil
		}
		errs = append(errs, err)
	}
	return &Error{Code: ErrHostNotRunning.Code, Message: "cannot start Sayso: " + errors.Join(errs...).Error()}
}
