package platform

// The kind of desktop session, read once from the environment. Ported from
// sayso-platform-linux/src/session.rs (detection only).

import (
	"os"
	"strings"
)

type sessionKind uint8

const (
	sessionNone sessionKind = iota
	sessionX11
	sessionWayland
)

type desktopKind uint8

const (
	desktopOther desktopKind = iota
	desktopGNOME
	desktopKDE
	desktopWlroots // sway, Hyprland, river, and other wlroots compositors
)

type linuxSession struct {
	kind    sessionKind
	desktop desktopKind
	// hasX11 is true when an X server (or XWayland) is reachable by name.
	hasX11 bool
}

func detectSession(getenv func(string) string) linuxSession {
	s := linuxSession{hasX11: getenv("DISPLAY") != "", desktop: desktopOf(getenv("XDG_CURRENT_DESKTOP"))}
	switch {
	case getenv("WAYLAND_DISPLAY") != "" || strings.EqualFold(getenv("XDG_SESSION_TYPE"), "wayland"):
		s.kind = sessionWayland
	case s.hasX11:
		s.kind = sessionX11
	}
	return s
}

// desktopOf reads the colon list of XDG_CURRENT_DESKTOP.
func desktopOf(current string) desktopKind {
	for _, part := range strings.Split(current, ":") {
		switch strings.ToLower(strings.TrimSpace(part)) {
		case "gnome", "ubuntu", "unity", "pop", "cosmic-gnome":
			return desktopGNOME
		case "kde", "plasma":
			return desktopKDE
		case "sway", "hyprland", "river", "wayfire", "labwc", "niri", "wlroots":
			return desktopWlroots
		}
	}
	return desktopOther
}

func currentSession() linuxSession { return detectSession(os.Getenv) }
