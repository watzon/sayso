package platform

// Pause and resume media players over MPRIS on the session bus. Every player
// that plays and can pause is paused; the resume plays only those that are
// still paused. Ported from sayso-platform-linux/src/media.rs.

import (
	"errors"
	"strings"
	"sync"

	"github.com/godbus/dbus/v5"
)

const (
	mprisPrefix     = "org.mpris.MediaPlayer2."
	mprisPath       = dbus.ObjectPath("/org/mpris/MediaPlayer2")
	mprisPlayer     = "org.mpris.MediaPlayer2.Player"
	mprisPlayerctld = "org.mpris.MediaPlayer2.playerctld"
)

type linuxMedia struct {
	mu     sync.Mutex
	conn   *dbus.Conn
	paused []string
}

func (m *linuxMedia) bus() (*dbus.Conn, error) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.conn != nil && m.conn.Connected() {
		return m.conn, nil
	}
	conn, err := dbus.ConnectSessionBus()
	if err != nil {
		return nil, err
	}
	m.conn = conn
	return conn, nil
}

// mprisPlayers keeps the player names. playerctld is a proxy for another
// player, so it is left out.
func mprisPlayers(names []string) []string {
	var out []string
	for _, n := range names {
		if strings.HasPrefix(n, mprisPrefix) && n != mprisPlayerctld {
			out = append(out, n)
		}
	}
	return out
}

func mprisProperty[T any](conn *dbus.Conn, name, property string) (T, bool) {
	var zero T
	v, err := conn.Object(name, mprisPath).GetProperty(mprisPlayer + "." + property)
	if err != nil {
		return zero, false
	}
	t, ok := v.Value().(T)
	return t, ok
}

func (m *linuxMedia) Pause() (bool, error) {
	conn, err := m.bus()
	if err != nil {
		return false, err
	}
	var names []string
	if err := conn.BusObject().Call("org.freedesktop.DBus.ListNames", 0).Store(&names); err != nil {
		return false, err
	}
	var paused []string
	var errs []error
	for _, name := range mprisPlayers(names) {
		status, _ := mprisProperty[string](conn, name, "PlaybackStatus")
		canPause, _ := mprisProperty[bool](conn, name, "CanPause")
		if status != "Playing" || !canPause {
			continue
		}
		if err := conn.Object(name, mprisPath).Call(mprisPlayer+".Pause", 0).Err; err != nil {
			errs = append(errs, err)
			continue
		}
		paused = append(paused, name)
	}
	m.mu.Lock()
	m.paused = paused
	m.mu.Unlock()
	return len(paused) > 0, errors.Join(errs...)
}

func (m *linuxMedia) Resume() error {
	m.mu.Lock()
	players := m.paused
	m.paused = nil
	m.mu.Unlock()
	if len(players) == 0 {
		return nil
	}
	conn, err := m.bus()
	if err != nil {
		return err
	}
	var errs []error
	for _, name := range players {
		// A player the user started or stopped meanwhile is left alone.
		if status, _ := mprisProperty[string](conn, name, "PlaybackStatus"); status != "Paused" {
			continue
		}
		if err := conn.Object(name, mprisPath).Call(mprisPlayer+".Play", 0).Err; err != nil {
			errs = append(errs, err)
		}
	}
	return errors.Join(errs...)
}
