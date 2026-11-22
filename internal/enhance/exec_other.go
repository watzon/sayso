//go:build !windows

package enhance

import (
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"syscall"
)

// newToolCommand is the command for a tool.
func newToolCommand(program string, args []string) *exec.Cmd {
	return exec.Command(program, args...)
}

func isTextBusy(err error) bool { return errors.Is(err, syscall.ETXTBSY) }

// toolDirs are the folders to search after PATH. An app started from the Dock
// has a short PATH, so the usual install folders are listed here.
func toolDirs() []string {
	var dirs []string
	if home, err := os.UserHomeDir(); err == nil && home != "" {
		for _, d := range []string{".local/bin", ".claude/local", ".claude/local/node_modules/.bin", ".npm-global/bin", ".bun/bin"} {
			dirs = append(dirs, filepath.Join(home, d))
		}
	}
	return append(dirs, "/opt/homebrew/bin", "/usr/local/bin")
}

// findExecutable is the first executable file called name in dirs.
func findExecutable(name string, dirs []string) string {
	for _, d := range dirs {
		candidate := filepath.Join(d, name)
		if info, err := os.Stat(candidate); err == nil && info.Mode().IsRegular() && info.Mode().Perm()&0o111 != 0 {
			return candidate
		}
	}
	return ""
}
