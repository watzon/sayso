package ui

import (
	"fmt"
	"strings"

	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/theme"
)

// The hotkey recorder: shows a hotkey as keycaps; "Change" listens for
// the next chord pressed in the window (Esc stops), "Clear" removes it.

// recorder draws a recorder for a hotkey named name. set gets the new
// hotkey, or nil for Clear.
func (a *App) recorder(c *mu.Context, p theme.Colors, key, name string, hk *config.Hotkey, set func(*config.Hotkey)) {
	listening := a.set.recording == key
	if listening {
		if h, done := captureChord(c); done {
			a.set.recording = ""
			if h != nil {
				set(h)
			}
		}
	}
	mu.Row(c).Gap(8).AlignItems(mu.Center).Children(func() {
		debossed(mu.Row(c).Height(36).PaddingStart(4).PaddingEnd(12).Gap(10).Radius(10).AlignItems(mu.Center), p).Children(func() {
			switch {
			case listening:
				uiText(c, i18n.T("settings.common.press_keys"), 13, 500, p.Accent).PaddingStart(8)
			case hk == nil:
				uiText(c, i18n.T("onboarding.hotkeys.no_key"), 13, 400, p.Graphite).PaddingStart(8)
			default:
				keycaps(c, p, hk.Keycaps(), false)
			}
			label := i18n.T("settings.common.change")
			if hk == nil {
				label = i18n.T("settings.common.record_key")
			}
			if listening {
				label = i18n.T("onboarding.hotkeys.cancel")
			}
			if a.link(c, p, key, label, 13, 600, "") {
				if listening {
					a.set.recording = ""
				} else {
					a.set.recording = key
				}
			}
		})
		if hk != nil && !listening && a.button(c, p, key+".clear", i18n.T("settings.common.clear"), bopt{kind: btnGhost}) {
			set(nil)
		}
	})
}

// captureChord reads the keys held now. done is true when a chord was
// pressed (h) or Esc stopped the recording (h nil).
func captureChord(c *mu.Context) (h *config.Hotkey, done bool) {
	for _, k := range c.KeysDown() {
		if k == mu.KeyEscape {
			return nil, true
		}
		name, ok := keyName(k)
		if !ok {
			continue
		}
		mods := c.Modifiers()
		var parts []string
		if mods&mu.Ctrl != 0 {
			parts = append(parts, "ctrl")
		}
		if mods&mu.Alt != 0 {
			parts = append(parts, "opt")
		}
		if mods&mu.Shift != 0 {
			parts = append(parts, "shift")
		}
		if mods&mu.Super != 0 {
			parts = append(parts, "cmd")
		}
		hk, err := config.ParseHotkey(strings.Join(append(parts, name), "+"))
		if err != nil {
			continue
		}
		return &hk, true
	}
	return nil, false
}

var namedKeys = map[mu.Key]string{
	mu.KeySpace: "space", mu.KeyEnter: "return", mu.KeyTab: "tab", mu.KeyBackspace: "delete",
	mu.KeyLeft: "left", mu.KeyRight: "right", mu.KeyUp: "up", mu.KeyDown: "down",
	mu.KeyHome: "home", mu.KeyEnd: "end", mu.KeyPageUp: "pageup", mu.KeyPageDown: "pagedown",
	mu.KeyMinus: "-", mu.KeyEqual: "=", mu.KeyComma: ",", mu.KeyPeriod: ".", mu.KeySlash: "/",
	mu.KeySemicolon: ";", mu.KeyQuote: "'", mu.KeyBracketLeft: "[", mu.KeyBracketRight: "]",
	mu.KeyBackslash: "\\", mu.KeyBackquote: "`",
}

// keyName is the config name of a key.
func keyName(k mu.Key) (string, bool) {
	if n, ok := namedKeys[k]; ok {
		return n, true
	}
	letters := []mu.Key{mu.KeyA, mu.KeyB, mu.KeyC, mu.KeyD, mu.KeyE, mu.KeyF, mu.KeyG, mu.KeyH, mu.KeyI, mu.KeyJ, mu.KeyK, mu.KeyL, mu.KeyM,
		mu.KeyN, mu.KeyO, mu.KeyP, mu.KeyQ, mu.KeyR, mu.KeyS, mu.KeyT, mu.KeyU, mu.KeyV, mu.KeyW, mu.KeyX, mu.KeyY, mu.KeyZ}
	for i, l := range letters {
		if k == l {
			return string(rune('a' + i)), true
		}
	}
	digits := []mu.Key{mu.Key0, mu.Key1, mu.Key2, mu.Key3, mu.Key4, mu.Key5, mu.Key6, mu.Key7, mu.Key8, mu.Key9}
	for i, d := range digits {
		if k == d {
			return string(rune('0' + i)), true
		}
	}
	fkeys := []mu.Key{mu.KeyF1, mu.KeyF2, mu.KeyF3, mu.KeyF4, mu.KeyF5, mu.KeyF6, mu.KeyF7, mu.KeyF8, mu.KeyF9, mu.KeyF10, mu.KeyF11, mu.KeyF12}
	for i, f := range fkeys {
		if k == f {
			return fmt.Sprintf("f%d", i+1), true
		}
	}
	return "", false
}
