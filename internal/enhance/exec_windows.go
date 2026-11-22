//go:build windows

package enhance

// Windows: start command line tools without a console window, and look
// through npm's .cmd shims (sayso-enhance win_shim.rs).
//
// npm installs a tool such as claude as claude.cmd, which runs
// node <package>\cli.js %*. A batch file goes through cmd.exe, which cannot
// pass every argument intact, so Sayso reads the shim and runs node with the
// script itself.

import (
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"syscall"
)

// createNoWindow is CREATE_NO_WINDOW: a GUI app's child gets no console window.
const createNoWindow = 0x08000000

// newToolCommand is node and the script for an npm shim, else the program.
func newToolCommand(program string, args []string) *exec.Cmd {
	var cmd *exec.Cmd
	if node, script, ok := shimTarget(program); ok {
		cmd = exec.Command(node, append([]string{script}, args...)...)
	} else {
		cmd = exec.Command(program, args...)
	}
	cmd.SysProcAttr = &syscall.SysProcAttr{CreationFlags: createNoWindow}
	return cmd
}

func isTextBusy(error) bool { return false }

// shimTarget is the node binary and the script of an npm .cmd shim.
func shimTarget(program string) (node, script string, ok bool) {
	ext := strings.ToLower(filepath.Ext(program))
	if ext != ".cmd" && ext != ".bat" {
		return "", "", false
	}
	text, err := os.ReadFile(program)
	if err != nil {
		return "", "", false
	}
	dir := filepath.Dir(program)
	script = scriptInShim(string(text), dir)
	if script == "" {
		return "", "", false
	}
	// npm puts node.exe beside the shim when node is not on PATH.
	node = "node"
	if local := filepath.Join(dir, "node.exe"); isFile(local) {
		node = local
	}
	return node, script, true
}

// scriptInShim is the quoted %dp0%\...\*.js (npm's cmd-shim) or
// %~dp0\...\*.js argument of a shim, resolved against its folder.
func scriptInShim(text, dir string) string {
	parts := strings.Split(text, `"`)
	for i := 1; i < len(parts); i += 2 {
		quoted := parts[i]
		lower := strings.ToLower(quoted)
		for _, prefix := range []string{`%dp0%\`, `%~dp0\`, `%dp0%/`, `%~dp0/`} {
			if !strings.HasPrefix(lower, prefix) {
				continue
			}
			if strings.HasSuffix(lower, ".js") || strings.HasSuffix(lower, ".mjs") || strings.HasSuffix(lower, ".cjs") {
				path := filepath.Join(dir, strings.ReplaceAll(quoted[len(prefix):], "/", `\`))
				if isFile(path) {
					return path
				}
				return ""
			}
		}
	}
	return ""
}

func isFile(path string) bool {
	info, err := os.Stat(path)
	return err == nil && info.Mode().IsRegular()
}

// toolDirs are the folders to search after PATH: the native installers'
// folders and npm's global folder.
func toolDirs() []string {
	var dirs []string
	if home := os.Getenv("USERPROFILE"); home != "" {
		dirs = append(dirs, filepath.Join(home, ".local", "bin"), filepath.Join(home, ".claude", "local"), filepath.Join(home, ".bun", "bin"))
	}
	if appdata := os.Getenv("APPDATA"); appdata != "" {
		dirs = append(dirs, filepath.Join(appdata, "npm"))
	}
	if local := os.Getenv("LOCALAPPDATA"); local != "" {
		dirs = append(dirs, filepath.Join(local, "pnpm"), filepath.Join(local, "Volta", "bin"))
	}
	return dirs
}

// executableNames are name with each executable extension of PATHEXT, .exe
// first: a native install beats an npm shim in the same folder.
func executableNames(name string) []string {
	pathext := os.Getenv("PATHEXT")
	if pathext == "" {
		pathext = ".COM;.EXE;.BAT;.CMD"
	}
	var exts []string
	for _, e := range strings.Split(pathext, ";") {
		if e != "" {
			exts = append(exts, strings.ToLower(e))
		}
	}
	sort.SliceStable(exts, func(i, j int) bool { return exts[i] == ".exe" && exts[j] != ".exe" })
	names := make([]string, len(exts))
	for i, e := range exts {
		names[i] = name + e
	}
	return names
}

// findExecutable is the first file called name with an executable extension in dirs.
func findExecutable(name string, dirs []string) string {
	names := executableNames(name)
	for _, d := range dirs {
		for _, n := range names {
			if candidate := filepath.Join(d, n); isFile(candidate) {
				return candidate
			}
		}
	}
	return ""
}
