package ui

import (
	"fmt"
	"log"
	"sync"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/theme"
)

// themeCache keeps the built theme of each ink on each paper.
type themeCache struct {
	built map[themeKey]*theme.Theme
}

// themeKey is a value, not a string: every window looks up its theme in
// every build.
type themeKey struct {
	ink  config.Ink
	dark bool
}

func (t *themeCache) get(ink config.Ink, dark bool) *theme.Theme {
	k := themeKey{ink, dark}
	if th, ok := t.built[k]; ok {
		return th
	}
	if t.built == nil {
		t.built = map[themeKey]*theme.Theme{}
	}
	custom := ""
	if ink.IsCustom {
		custom = fmt.Sprintf("#%06X", ink.Custom&0xFFFFFF)
	}
	th := theme.BuildMyGoTheme(theme.InkPreset(ink.Named), custom, dark)
	t.built[k] = th
	return th
}

// dark resolves the theme mode against the system's appearance.
func (a *App) dark(system bool) bool {
	switch a.cfg.Appearance.Theme {
	case config.ThemeLight:
		return false
	case config.ThemeDark:
		return true
	}
	return system
}

// look sets the window's theme and direction for this frame and returns its
// colors. Views call it first.
func (a *App) look(c *mu.Context) theme.Colors {
	th := a.themes.get(a.cfg.Appearance.Ink, a.dark(c.Theme().Dark))
	c.SetTheme(th.UI)
	applyDirection(c)
	return th.Colors
}

// colors are this frame's colors, for helpers handed no palette.
func (a *App) colors(c *mu.Context) theme.Colors {
	return a.themes.get(a.cfg.Appearance.Ink, a.dark(c.Theme().Dark)).Colors
}

var (
	iconMu    sync.Mutex
	iconCache = map[string]*mu.SVG{}
	fontsOnce sync.Once
)

// icon is an embedded stroke icon by name, drawn in the text color.
func icon(name string) *mu.SVG {
	iconMu.Lock()
	defer iconMu.Unlock()
	if s, ok := iconCache[name]; ok {
		return s
	}
	var s *mu.SVG
	if data := theme.IconSVG(name); len(data) > 0 {
		var err error
		if s, err = mu.ParseSVG(data); err != nil {
			log.Printf("icon %s: %v", name, err)
		}
	}
	if s == nil {
		s = mu.MustParseSVG([]byte(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"/>`))
	}
	iconCache[name] = s
	return s
}

// RegisterFonts registers Sayso's fonts once.
func RegisterFonts() {
	fontsOnce.Do(func() {
		if err := theme.RegisterFonts(); err != nil {
			log.Printf("fonts: %v (using system fonts)", err)
		}
	})
}

// serifFont is a bundled serif with the fallbacks of the interface
// language, for the characters it lacks (CJK and Cyrillic).
func serifFont(family string) string { return theme.SerifFor(family, i18n.CurrentLocale()) }
