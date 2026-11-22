package update

import (
	"encoding/json"
	"fmt"
	"os"
	"runtime"
	"strings"

	"github.com/pelletier/go-toml/v2"
)

// release-notes.toml: the "What is new" window after an update
// (sayso-app/src/whats_new.rs). The app embeds the file and passes its text.

// NotesRelease is the notes of one version.
type NotesRelease struct {
	Version string      `toml:"version"`
	Title   string      `toml:"title"`
	Item    []NotesItem `toml:"item"`
	Note    *NotesNote  `toml:"note"`
}

// NotesItem is one new feature. Systems is empty for all systems.
type NotesItem struct {
	Icon    string   `toml:"icon"`
	Title   string   `toml:"title"`
	Text    string   `toml:"text"`
	Systems []string `toml:"systems"`
}

// NotesNote is an announcement with one button.
type NotesNote struct {
	Title  string `toml:"title"`
	Text   string `toml:"text"`
	Action string `toml:"action"`
	URL    string `toml:"url"`
}

// NotesIcons are the icon names the window knows.
var NotesIcons = []string{"sparkle", "lock", "shield", "history", "search", "mic", "audio", "styles", "models", "dictionary",
	"keyboard", "overlay", "cloud", "download", "settings", "code", "globe"}

// ParseReleaseNotes reads the file; a field with a wrong name is an error.
func ParseReleaseNotes(text string) ([]NotesRelease, error) {
	var doc struct {
		Release []NotesRelease `toml:"release"`
	}
	d := toml.NewDecoder(stringsReader(text))
	d.DisallowUnknownFields()
	if err := d.Decode(&doc); err != nil {
		return nil, err
	}
	if doc.Release == nil {
		return nil, fmt.Errorf("missing field `release`")
	}
	for i, r := range doc.Release {
		if _, err := ParseVersion(r.Version); err != nil {
			return nil, fmt.Errorf("release %d: %v", i, err)
		}
		if r.Title == "" {
			return nil, fmt.Errorf("release %s: missing field `title`", r.Version)
		}
	}
	return doc.Release, nil
}

// ReleaseIn is the notes of version for the system os ("macos", "windows",
// "linux"), without the items of other systems; false when nothing is left.
func ReleaseIn(releases []NotesRelease, version Semver, os string) (NotesRelease, bool) {
	for _, r := range releases {
		v, err := ParseVersion(r.Version)
		if err != nil || !v.Equal(version) {
			continue
		}
		var items []NotesItem
		for _, it := range r.Item {
			keep := len(it.Systems) == 0
			for _, s := range it.Systems {
				keep = keep || s == os
			}
			if keep {
				items = append(items, it)
			}
		}
		r.Item = items
		return r, len(items) > 0 || r.Note != nil
	}
	return NotesRelease{}, false
}

// ThisOS is the system name the notes use.
func ThisOS() string {
	if runtime.GOOS == "darwin" {
		return "macos"
	}
	return runtime.GOOS
}

// WhatsNewDue is true on the first start of a newer version. A first install
// is not an update: onboarding marks the version seen.
func WhatsNewDue(seen *Semver, running Semver) bool {
	return seen == nil || seen.CmpPrecedence(running) < 0
}

const whatsNewKey = "whats_new"

// WhatsNewSeen reads the seen version from the app's state file.
func WhatsNewSeen(stateFile string) *Semver {
	var state map[string]any
	data, err := os.ReadFile(stateFile)
	if err != nil || json.Unmarshal(data, &state) != nil {
		return nil
	}
	s, _ := state[whatsNewKey].(string)
	v, err := ParseVersion(s)
	if err != nil {
		return nil
	}
	return &v
}

// MarkWhatsNewSeen records this version, keeping the other state keys.
func MarkWhatsNewSeen(stateFile string) error {
	state := map[string]any{}
	if data, err := os.ReadFile(stateFile); err == nil {
		json.Unmarshal(data, &state)
		if state == nil {
			state = map[string]any{}
		}
	}
	state[whatsNewKey] = Version
	data, err := prettyJSON(state)
	if err != nil {
		return err
	}
	return os.WriteFile(stateFile, data, 0o644)
}

func stringsReader(s string) *strings.Reader { return strings.NewReader(s) }
