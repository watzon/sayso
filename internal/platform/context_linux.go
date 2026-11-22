package platform

// The focused app on Linux: the WM_CLASS of the active X11 window (also
// XWayland windows under Wayland), matched to its desktop entry for the id
// and the name. Ported from sayso-platform-linux/src/context.rs.
//
// Native Wayland windows need the wlr foreign-toplevel protocol, which is
// not ported, so under Wayland only X11 apps are seen. Linux has no page
// address, so Site stays empty.

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"sync"
)

type desktopEntry struct {
	id, name, icon, wmClass string
	listed                  bool
}

type linuxContext struct {
	session linuxSession
	once    sync.Once
	entries map[string]desktopEntry
}

func (c *linuxContext) ActiveApp() (ActiveApp, error) {
	if !c.session.hasX11 {
		return ActiveApp{}, errors.New("the active app is not known in this session")
	}
	class, _, err := x11ActiveWindowClass()
	if err != nil {
		return ActiveApp{}, err
	}
	if class == "" {
		return ActiveApp{}, errors.New("the active window has no class")
	}
	c.once.Do(func() { c.entries = loadDesktopEntries(xdgDataDirs()) })
	app := ActiveApp{BundleID: class, Name: class}
	if e, ok := entryForClass(c.entries, class); ok {
		app.BundleID, app.Name = e.id, e.name
	}
	return app, nil
}

// xdgDataDirs are the folders that hold applications/, most specific first.
func xdgDataDirs() []string {
	home := os.Getenv("HOME")
	dataHome := os.Getenv("XDG_DATA_HOME")
	if dataHome == "" {
		dataHome = filepath.Join(home, ".local/share")
	}
	dirs := []string{dataHome, filepath.Join(home, ".local/share/flatpak/exports/share"), "/var/lib/flatpak/exports/share"}
	system := os.Getenv("XDG_DATA_DIRS")
	if system == "" {
		system = "/usr/local/share:/usr/share"
	}
	return append(dirs, strings.Split(system, ":")...)
}

func loadDesktopEntries(dirs []string) map[string]desktopEntry {
	entries := map[string]desktopEntry{}
	for _, dir := range dirs {
		files, _ := filepath.Glob(filepath.Join(dir, "applications", "*.desktop"))
		for _, f := range files {
			id := strings.TrimSuffix(filepath.Base(f), ".desktop")
			// The first folder wins, as in the XDG lookup.
			if _, ok := entries[id]; ok {
				continue
			}
			text, err := os.ReadFile(f)
			if err != nil {
				continue
			}
			if e, ok := parseDesktopEntry(id, string(text)); ok {
				entries[id] = e
			}
		}
	}
	return entries
}

// parseDesktopEntry reads the [Desktop Entry] group of a desktop file.
func parseDesktopEntry(id, text string) (desktopEntry, bool) {
	values := map[string]string{}
	inGroup := false
	for _, line := range strings.Split(text, "\n") {
		line = strings.TrimSpace(line)
		if strings.HasPrefix(line, "[") {
			inGroup = line == "[Desktop Entry]"
			continue
		}
		if !inGroup || strings.HasPrefix(line, "#") {
			continue
		}
		if k, v, ok := strings.Cut(line, "="); ok {
			k = strings.TrimSpace(k)
			if _, seen := values[k]; !seen {
				values[k] = strings.TrimSpace(v)
			}
		}
	}
	name, ok := values["Name"]
	if !ok {
		return desktopEntry{}, false
	}
	isTrue := func(k string) bool { return strings.EqualFold(values[k], "true") }
	typ, hasType := values["Type"]
	return desktopEntry{
		id:      id,
		name:    name,
		icon:    values["Icon"],
		wmClass: values["StartupWMClass"],
		listed:  (!hasType || typ == "Application") && !isTrue("NoDisplay") && !isTrue("Hidden"),
	}, true
}

// entryForClass finds the entry of a WM_CLASS class: the entry id itself (a
// Wayland app id), its StartupWMClass, an id equal to the class, or an id
// that ends with ".class" (org.gnome.Nautilus for Nautilus).
func entryForClass(entries map[string]desktopEntry, class string) (desktopEntry, bool) {
	if e, ok := entries[class]; ok {
		return e, true
	}
	lower := strings.ToLower(class)
	var byID, bySuffix *desktopEntry
	for _, e := range entries {
		e := e
		if e.wmClass != "" && strings.EqualFold(e.wmClass, class) {
			return e, true
		}
		id := strings.ToLower(e.id)
		if id == lower && byID == nil {
			byID = &e
		} else if strings.HasSuffix(id, "."+lower) && bySuffix == nil {
			bySuffix = &e
		}
	}
	if byID != nil {
		return *byID, true
	}
	if bySuffix != nil {
		return *bySuffix, true
	}
	return desktopEntry{}, false
}
