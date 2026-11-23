package platform

// The focus tracker of Linux under X11: the frame of the window that
// _NET_ACTIVE_WINDOW names, in root pixels (GDK's monitors are those
// divided by its scale, so the frame says Pixels). Watch hears changes of
// _NET_ACTIVE_WINDOW on the root window. Wayland tells no client where
// another app's window is, so there is no tracker there (New leaves Focus
// nil) and the overlay stays on the primary display.

import (
	"errors"
	"os"
	"sync"

	"github.com/jezek/xgb"
	"github.com/jezek/xgb/xproto"
)

type x11Focus struct{ self int }

func newX11Focus() *x11Focus { return &x11Focus{self: os.Getpid()} }

func x11Atom(conn *xgb.Conn, name string) xproto.Atom {
	r, err := xproto.InternAtom(conn, true, uint16(len(name)), name).Reply()
	if err != nil || r == nil {
		return xproto.AtomNone
	}
	return r.Atom
}

func (f *x11Focus) Focused() (FocusFrame, bool) {
	conn, err := xgb.NewConn()
	if err != nil {
		return FocusFrame{}, false
	}
	defer conn.Close()
	root := xproto.Setup(conn).DefaultScreen(conn).Root
	active := x11Atom(conn, "_NET_ACTIVE_WINDOW")
	if active == xproto.AtomNone {
		return FocusFrame{}, false
	}
	r, err := xproto.GetProperty(conn, false, root, active, xproto.AtomWindow, 0, 1).Reply()
	if err != nil || r == nil || len(r.Value) < 4 {
		return FocusFrame{}, false
	}
	win := xproto.Window(xgb.Get32(r.Value))
	if win == 0 || x11SkipWindow(conn, win, f.self) {
		return FocusFrame{}, false
	}
	geo, err := xproto.GetGeometry(conn, xproto.Drawable(win)).Reply()
	if err != nil || geo == nil {
		return FocusFrame{}, false
	}
	at, err := xproto.TranslateCoordinates(conn, win, root, 0, 0).Reply()
	if err != nil || at == nil {
		return FocusFrame{}, false
	}
	rect := Rect{X: int(at.DstX), Y: int(at.DstY), Width: int(geo.Width), Height: int(geo.Height)}
	if rect.Empty() {
		return FocusFrame{}, false
	}
	return FocusFrame{Rect: rect, Pixels: true}, true
}

// x11SkipWindow is true for the desktop, a dock or panel, and Sayso's
// overlay.
func x11SkipWindow(conn *xgb.Conn, win xproto.Window, self int) bool {
	if typ := x11Atom(conn, "_NET_WM_WINDOW_TYPE"); typ != xproto.AtomNone {
		desktop, dock := x11Atom(conn, "_NET_WM_WINDOW_TYPE_DESKTOP"), x11Atom(conn, "_NET_WM_WINDOW_TYPE_DOCK")
		if p, err := xproto.GetProperty(conn, false, win, typ, xproto.AtomAtom, 0, 16).Reply(); err == nil && p != nil {
			for i := 0; i+4 <= len(p.Value); i += 4 {
				if a := xproto.Atom(xgb.Get32(p.Value[i:])); a == desktop || a == dock {
					return true
				}
			}
		}
	}
	if pidAtom := x11Atom(conn, "_NET_WM_PID"); pidAtom != xproto.AtomNone {
		p, err := xproto.GetProperty(conn, false, win, pidAtom, xproto.AtomCardinal, 0, 1).Reply()
		if err != nil || p == nil || len(p.Value) < 4 || int(xgb.Get32(p.Value)) != self {
			return false
		}
		name, err := xproto.GetProperty(conn, false, win, xproto.AtomWmName, xproto.AtomString, 0, 64).Reply()
		return err == nil && name != nil && string(name.Value) == overlayTitle
	}
	return false
}

// Watch listens for PropertyNotify of _NET_ACTIVE_WINDOW on the root
// window on a connection of its own; stop closes it.
func (f *x11Focus) Watch(changed func()) (func(), error) {
	conn, err := xgb.NewConn()
	if err != nil {
		return nil, err
	}
	root := xproto.Setup(conn).DefaultScreen(conn).Root
	active := x11Atom(conn, "_NET_ACTIVE_WINDOW")
	if active == xproto.AtomNone {
		conn.Close()
		return nil, errors.New("X11: the window manager does not report the active window")
	}
	if err := xproto.ChangeWindowAttributesChecked(conn, root, xproto.CwEventMask, []uint32{xproto.EventMaskPropertyChange}).Check(); err != nil {
		conn.Close()
		return nil, err
	}
	done := make(chan struct{})
	go func() {
		defer close(done)
		for {
			ev, err := conn.WaitForEvent()
			if ev == nil && err == nil {
				return // closed
			}
			if p, ok := ev.(xproto.PropertyNotifyEvent); ok && p.Atom == active {
				changed()
			}
		}
	}()
	var once sync.Once
	return func() {
		once.Do(func() {
			conn.Close()
			<-done
		})
	}, nil
}

var _ FocusTracker = (*x11Focus)(nil)
