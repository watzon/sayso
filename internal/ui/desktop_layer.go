package ui

import (
	"time"

	"github.com/egoist/mygo"

	"github.com/watzon/sayso/internal/platform"
)

// The window layer under Desktop's tray path: the hub, the tray popover and
// the app's activation. mygoLayer is MyGo's; the tests give a fake that
// records each call, to prove that a click on the tray icon shows the
// popover and nothing else.

// layerWindow is what the tray path asks of a window; *mygo.Window has it.
type layerWindow interface {
	Show()
	Focus()
	Hide()
	IsVisible() bool
	IsDestroyed() bool
}

type shellLayer interface {
	// hub is the hub window, nil before Start.
	hub() layerWindow
	// popover is the tray popover; make makes it on first use. Nil
	// without a tray item, or when it is not made and make is false.
	popover(make bool) layerWindow
	// showInDock puts Sayso in the Dock, for the hub (macOS).
	showInDock()
	// giveBack gives the keyboard back to the app the user came from,
	// after delay, if Sayso still has it.
	giveBack(delay time.Duration)
}

// mygoLayer is the layer of the running app.
type mygoLayer struct{ d *Desktop }

func (l mygoLayer) hub() layerWindow {
	if l.d.hub == nil {
		return nil
	}
	return l.d.hub
}

func (l mygoLayer) popover(make bool) layerWindow {
	w := l.d.wins[WinPopover]
	if (w == nil || w.IsDestroyed()) && make {
		if w = l.d.newPopover(); w != nil {
			l.d.wins[WinPopover] = w
		}
	}
	if w == nil {
		return nil
	}
	return w
}

func (mygoLayer) showInDock() { mygo.App.SetActivationPolicy(mygo.ActivationPolicyRegular) }

func (mygoLayer) giveBack(delay time.Duration) {
	time.AfterFunc(delay, func() {
		mygo.RunOnMain(func() {
			if mygo.App.IsActive() {
				platform.DeactivateApp()
			}
		})
	})
}

// popoverOptions are the tray popover's flyout options.
//
// It must not be a Popover: MyGo shows an NSPopover of a tray item with
// [NSApp activateIgnoringOtherApps:YES], so that it takes the keyboard,
// and activating Sayso brings its main window, the hub, in front of the
// app the user is in. Without Popover, MyGo draws the flyout in its own
// nonactivating panel, which becomes the key window without activating
// the app: the popover gets the keyboard, and every other window of
// Sayso stays where it was. Focusable keeps it closing as a menu does: on
// a press outside it (another app, the desktop, the menu bar or a window
// of Sayso), when another window takes the keyboard, and on Escape. A
// press on the tray icon is left to the icon's click, which toggles it.
// The view draws its own sheet: Shadow and CornerRadius give it the
// system's shadow around 12 px corners.
func popoverOptions(tray *mygo.Tray, content mygo.Content) mygo.FlyoutOptions {
	return mygo.FlyoutOptions{
		Tray: tray, Placement: mygo.PlacementBottom, Gap: 6, Width: PopoverW, Height: PopoverH,
		Shadow: true, Focusable: true, CornerRadius: 12, Content: content,
	}
}
