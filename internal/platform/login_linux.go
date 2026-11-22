package platform

// Launch at login with an XDG autostart entry. Ported from
// sayso-platform-linux/src/login_item.rs and launch.rs. The file name is the
// app id, the same as the Rust version, so an existing entry carries over.

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
)

const autostartFileName = "dev.sayso.Sayso.desktop"

type linuxLoginItem struct {
	getenv     func(string) string
	executable func() (string, error)
}

func newLinuxLoginItem() linuxLoginItem {
	return linuxLoginItem{getenv: os.Getenv, executable: os.Executable}
}

func (l linuxLoginItem) file() (string, error) {
	variable := "XDG_CONFIG_HOME"
	if l.getenv("FLATPAK_ID") != "" {
		variable = "HOST_XDG_CONFIG_HOME"
	}
	config := l.getenv(variable)
	if config == "" {
		home := l.getenv("HOME")
		if home == "" {
			return "", errors.New("login item: no home folder")
		}
		config = filepath.Join(home, ".config")
	}
	return filepath.Join(config, "autostart", autostartFileName), nil
}

// launchCommand is `flatpak run <id>` in a Flatpak, the AppImage when Sayso
// runs from one, else this executable.
func (l linuxLoginItem) launchCommand() ([]string, error) {
	if id := l.getenv("FLATPAK_ID"); id != "" {
		return []string{"flatpak", "run", id}, nil
	}
	if img := l.getenv("APPIMAGE"); img != "" {
		return []string{img}, nil
	}
	exe, err := l.executable()
	if err != nil {
		return nil, errors.New("login item: cannot find the Sayso program")
	}
	return []string{exe}, nil
}

// autostartEntry quotes words with spaces.
func autostartEntry(words []string) string {
	quoted := make([]string, len(words))
	for i, w := range words {
		if strings.Contains(w, " ") {
			w = `"` + w + `"`
		}
		quoted[i] = w
	}
	return "[Desktop Entry]\nType=Application\nName=Sayso\nComment=Voice dictation\nExec=" + strings.Join(quoted, " ") +
		"\nIcon=dev.sayso.Sayso\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
}

// autostartEnabled: an entry that a desktop turned off says so.
func autostartEnabled(text string) bool {
	for _, line := range strings.Split(text, "\n") {
		line = strings.TrimSpace(line)
		if line == "Hidden=true" || line == "X-GNOME-Autostart-enabled=false" {
			return false
		}
	}
	return true
}

func (l linuxLoginItem) Enabled() (bool, error) {
	f, err := l.file()
	if err != nil {
		return false, err
	}
	text, err := os.ReadFile(f)
	if errors.Is(err, os.ErrNotExist) {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	return autostartEnabled(string(text)), nil
}

func (l linuxLoginItem) SetEnabled(enabled bool) error {
	f, err := l.file()
	if err != nil {
		return err
	}
	if !enabled {
		if err := os.Remove(f); err != nil && !errors.Is(err, os.ErrNotExist) {
			return err
		}
		return nil
	}
	words, err := l.launchCommand()
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(f), 0o755); err != nil {
		return err
	}
	return os.WriteFile(f, []byte(autostartEntry(words)), 0o644)
}
