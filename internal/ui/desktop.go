package ui

import (
	"log"
	"math"
	"runtime"
	"time"

	"github.com/egoist/mygo"
	maudio "github.com/egoist/mygo/audio"
	mu "github.com/egoist/mygo/ui"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/i18n"
	"github.com/watzon/sayso/internal/platform"
)

// Desktop is the Shell of the running app: MyGo windows, the tray item
// and its popover. Every method runs on the main thread.
type Desktop struct {
	a       *App
	hub     *mygo.Window
	overlay *mygo.Window
	wins    map[Window]*mygo.Window
	tray    *mygo.Tray
	// trayItem is tray as the tray path sees it; tests give a fake.
	trayItem trayItem
	// trayAttached: the menu stays on the icon (wireTray).
	trayAttached bool
	// layer is the window system under the tray path; tests give a fake.
	layer   shellLayer
	popover popoverToggle
	// popoverClosing is set once the popover's close was handled: Sayso
	// closing it, or the flyout's dismissal, of which OnHide and OnClosed
	// hear again.
	popoverClosing bool
	quitting       bool
	// region is the overlay's card, the part of its window that takes the mouse.
	region mu.Rect
	// focus is the display the keyboard focus was last on, which the
	// overlay follows (desktop_overlay.go); placed is where it last put
	// the overlay.
	focus  focusState
	placed overlayPlaced
	// wasRecording is the state ShowOverlay last saw, to place the overlay
	// as a dictation starts.
	wasRecording bool
	// focusTracker replaces the platform's tracker in tests.
	focusTracker platform.FocusTracker
}

var (
	_ Shell        = (*Desktop)(nil)
	_ overlayInput = (*Desktop)(nil)
)

// NewDesktop makes the shell of a and makes it a's shell. Call Start from
// mygo.App.WhenReady.
func NewDesktop(a *App) *Desktop {
	d := &Desktop{a: a, wins: map[Window]*mygo.Window{}}
	d.layer = mygoLayer{d}
	a.d.Shell = d
	if a.d.Post == nil {
		a.d.Post = mygo.RunOnMain
	}
	if a.d.NewPlayer == nil {
		a.d.NewPlayer = func(samples []float32) Player {
			return maudio.NewSound(16000, 1, samples).NewPlayer()
		}
	}
	return d
}

// Start makes the windows and the tray item. showHub opens the hub now.
func (d *Desktop) Start(showHub bool) {
	RegisterFonts()
	d.SetThemeMode(d.a.cfg.Appearance.Theme)
	// The system title bar is a Linux choice;
	// macOS and Windows always draw the paper up to the top edge.
	style := mygo.TitleBarHidden
	if runtime.GOOS == "linux" && d.a.cfg.Appearance.SystemTitleBar {
		style = mygo.TitleBarDefault
	}
	d.hub = mygo.NewWindow(mygo.WindowOptions{
		Title: "Sayso", Width: HubWidth, Height: HubHeight, MinWidth: HubMinWidth, MinHeight: HubMinHeight,
		TitleBarStyle: style, StateKey: "hub", Hidden: !showHub, Content: mu.View(d.a.HubView),
		// As the Rust app (app.rs): the sidebar's 52 px top padding leaves
		// room for the controls here.
		TrafficLightPosition: &mygo.Point{X: 18, Y: 18},
	})
	d.hub.OnClose(func(e *mygo.CloseEvent) {
		if !d.quitting { // closing the hub keeps the tray item
			e.PreventDefault()
			d.hub.Hide()
		}
	})
	d.overlay = d.newOverlay()
	d.newTray()
	mygo.App.OnBeforeQuit(func(*mygo.QuitEvent) {
		d.quitting = true
		d.stopFocus()
	})
	mygo.App.OnWindowAllClosed(func() {})
	mygo.Screen.OnDisplaysChanged(d.displaysChanged)
	d.startFocus()
	d.a.syncOverlay()
	if d.a.NeedsOnboarding() {
		d.ShowWindow(WinOnboarding, true)
	}
}

func (d *Desktop) overlaySize() (int, int) { return OverlayWindowSize(d.a.cfg.Overlay) }

func (d *Desktop) newOverlay() *mygo.Window {
	w, h := d.overlaySize()
	win := mygo.NewWindow(mygo.WindowOptions{
		Title: "Sayso Overlay", Width: w, Height: h, Hidden: true, Frameless: true, Transparent: true,
		AlwaysOnTop: true, SkipTaskbar: true, DisableResize: true, DisableMinimize: true, DisableMaximize: true,
		DisableFullScreen: true, DisableShadow: true, Content: mu.View(d.a.OverlayView),
	})
	win.SetVisibleOnAllWorkspaces(true)
	win.OnClose(func(e *mygo.CloseEvent) {
		if !d.quitting {
			e.PreventDefault()
			win.Hide()
		}
	})
	win.OnMove(func() { d.notePillHalf() })
	return win
}

// Invalidate redraws every window.
func (d *Desktop) Invalidate() {
	for _, w := range append([]*mygo.Window{d.hub, d.overlay}, d.windows()...) {
		if w != nil && !w.IsDestroyed() {
			w.Invalidate()
		}
	}
}

// InvalidateOverlay redraws the overlay alone.
func (d *Desktop) InvalidateOverlay() {
	if d.overlay != nil && !d.overlay.IsDestroyed() {
		d.overlay.Invalidate()
	}
}

func (d *Desktop) windows() []*mygo.Window {
	out := make([]*mygo.Window, 0, len(d.wins))
	for _, w := range d.wins {
		out = append(out, w)
	}
	return out
}

// ShowHub shows and focuses the hub, which activates Sayso: for "Open
// Sayso" and the other actions that ask for the hub.
func (d *Desktop) ShowHub() {
	if d.a.cfg.General.DockIconWithHub && runtime.GOOS == "darwin" {
		d.layer.showInDock()
	}
	h := d.layer.hub()
	if h == nil {
		return
	}
	h.Show()
	h.Focus()
}

// SetOverlayInputRegion makes only r, the visible card, take the mouse:
// the rest of the overlay window lets it through to what is under it.
func (d *Desktop) SetOverlayInputRegion(r mu.Rect) {
	d.region = r
	d.applyInputRegion()
}

func (d *Desktop) applyInputRegion() {
	ov := d.layer.overlay()
	if ov == nil {
		return
	}
	r := d.region
	if r.W <= 0 || r.H <= 0 {
		ov.SetInputRegion([]mygo.Rectangle{})
		return
	}
	x0, y0 := int(math.Floor(float64(r.X))), int(math.Floor(float64(r.Y)))
	x1, y1 := int(math.Ceil(float64(r.X+r.W))), int(math.Ceil(float64(r.Y+r.H)))
	ov.SetInputRegion([]mygo.Rectangle{{X: x0, Y: y0, Width: x1 - x0, Height: y1 - y0}})
}

// ShowWindow shows or hides a secondary window, making it on first use.
func (d *Desktop) ShowWindow(which Window, show bool) {
	if which == WinPopover {
		if show {
			d.showPopover()
		} else if w := d.layer.popover(false); w != nil {
			w.Hide()
		}
		return
	}
	w := d.wins[which]
	if !show {
		if w != nil {
			w.Hide()
		}
		return
	}
	if w == nil || w.IsDestroyed() {
		w = d.newWindow(which)
		if w == nil {
			return
		}
		d.wins[which] = w
	}
	w.Show()
	w.Focus()
	if which == WinOnboarding {
		d.a.enteredStep(d.a.onb.step)
	}
}

// showPopover shows the tray popover, making it on first use. It calls Show
// alone: the popover is a nonactivating flyout that takes the keyboard as
// it shows (popoverOptions). Focus, or activating the app, would bring the
// hub in front of the app the user is in. Each time it opens it is placed
// on the display the pointer is on (placePopover): MyGo places a tray
// flyout only as it is made.
func (d *Desktop) showPopover() {
	w := d.layer.popover(true)
	if w == nil {
		return
	}
	d.popoverClosing = false
	if !w.IsVisible() {
		cursor, tray, displays := d.layer.popoverAnchor()
		if at, ok := placePopover(cursor, tray, displays, PopoverW, PopoverH); ok {
			w.SetPosition(at.X, at.Y)
		}
	}
	w.Show()
}

// trayClicked is a primary click on the tray icon at now: it opens the
// popover, or closes it, and touches no other window.
func (d *Desktop) trayClicked(now time.Time) {
	w := d.layer.popover(false)
	visible := w != nil && !w.IsDestroyed() && w.IsVisible()
	switch d.popover.click(now, visible) {
	case popoverHide:
		d.ClosePopover(false)
	case popoverShow:
		d.showPopover()
	}
}

// ClosePopover closes the tray popover on purpose: keep leaves the keyboard
// in Sayso, for the hub that shows next; otherwise it goes back to the app
// the user came from.
func (d *Desktop) ClosePopover(keep bool) {
	w := d.layer.popover(false)
	if w == nil || w.IsDestroyed() || !w.IsVisible() {
		return
	}
	exit := exitGiveBack
	if keep {
		exit = exitKeep
	}
	d.popoverClosed(exit, false)
	w.Hide()
}

func (d *Desktop) newWindow(which Window) *mygo.Window {
	opts, ok := d.windowOptions(which)
	if !ok {
		return nil
	}
	w := mygo.NewWindow(opts)
	w.OnClose(func(e *mygo.CloseEvent) {
		if which == WinOnboarding {
			// The step stays saved; the microphone test and a key
			// recording end with the window.
			d.a.stopMeter()
			d.a.cancelRecording()
		}
		if !d.quitting {
			e.PreventDefault()
			w.Hide()
			if which == WinApproval {
				for _, ask := range append([]*approvalAsk(nil), d.a.asks...) {
					d.a.answer(ask, false)
				}
			}
		}
	})
	return w
}

// windowOptions are the options of the secondary window which; false for a
// window that is not one.
func (d *Desktop) windowOptions(which Window) (mygo.WindowOptions, bool) {
	opts := mygo.WindowOptions{TitleBarStyle: mygo.TitleBarHidden, DisableMaximize: true, DisableFullScreen: true}
	switch which {
	case WinOnboarding:
		opts.Title, opts.Width, opts.Height, opts.Content = i18n.T("onboarding.welcome.title"), OnboardingW, OnboardingH, mu.View(d.a.OnboardingView)
		// A fixed size, as the Rust window; the traffic lights sit in the
		// sheet's 52 px top bar, as the Paper artboards place them.
		opts.DisableResize, opts.DisableMinimize = true, false
		opts.TrafficLightPosition = &mygo.Point{X: 28, Y: 30}
	case WinPermissions:
		opts.Title, opts.Width, opts.Height, opts.Content = i18n.T("settings.permissions.title"), PermissionsW, PermissionsH, mu.View(d.a.PermissionsView)
	case WinWhatsNew:
		opts.Title, opts.Width, opts.Height, opts.Content = i18n.T("whats_new.window_title"), WhatsNewW, WhatsNewH, mu.View(d.a.WhatsNewView)
		opts.DisableResize = true
	case WinUpdate:
		opts.Title, opts.Width, opts.Height, opts.Content = i18n.T("update.slip_title"), UpdateW, UpdateH, mu.View(d.a.UpdateView)
	case WinApproval:
		opts.Title, opts.Width, opts.Height, opts.Content = i18n.T("apps.approval.window_title"), ApprovalW, ApprovalH, mu.View(d.a.ApprovalView)
		opts.AlwaysOnTop = true
	default:
		return mygo.WindowOptions{}, false
	}
	return opts, true
}

// newPopover is the themed panel under the tray item.
func (d *Desktop) newPopover() *mygo.Window {
	if d.tray == nil {
		return nil
	}
	w := mygo.NewFlyout(popoverOptions(d.tray, mu.View(d.a.PopoverView)))
	// The flyout's own close, at its start: a press outside it or the loss
	// of the keyboard. OnHide and OnClosed come after its fade, and only
	// matter for a close OnClose did not hear.
	w.OnClose(func(*mygo.CloseEvent) { d.popoverClosed(exitDismissed, d.pressInSayso(w)) })
	w.OnHide(func() { d.popoverClosed(exitDismissed, d.pressInSayso(w)) })
	w.OnClosed(func() { d.popoverClosed(exitDismissed, d.pressInSayso(w)) })
	return w
}

// popoverClosed handles the start of the popover's close, once: it guards
// the tray click after a dismissal, and gives the keyboard back to the app
// the user came from when exit says so (Rust popover.rs hide and
// hide_popover).
func (d *Desktop) popoverClosed(exit popoverExit, pressInSayso bool) {
	if d.popoverClosing {
		return
	}
	d.popoverClosing = true
	d.popover.closing(time.Now(), exit)
	if !popoverGivesBack(exit, pressInSayso) {
		return
	}
	// After the press that dismissed it has reached the window under it.
	delay := time.Duration(0)
	if exit == exitDismissed {
		delay = 120 * time.Millisecond
	}
	d.layer.giveBack(delay)
}

// pressInSayso reports whether the pointer is over the hub or a secondary
// window: a press there keeps Sayso active. The popover does not count,
// nor the overlay, which the user never types in.
func (d *Desktop) pressInSayso(pop *mygo.Window) bool {
	var boxes []mygo.Rectangle
	for _, w := range append([]*mygo.Window{d.hub}, d.windows()...) {
		if w != nil && w != pop && !w.IsDestroyed() && w.IsVisible() {
			boxes = append(boxes, w.Bounds())
		}
	}
	return pointInWindows(mygo.Screen.CursorScreenPoint(), boxes)
}

// pointInWindows reports whether p is in one of boxes.
func pointInWindows(p mygo.Point, boxes []mygo.Rectangle) bool {
	for _, b := range boxes {
		if p.X >= b.X && p.X < b.X+b.Width && p.Y >= b.Y && p.Y < b.Y+b.Height {
			return true
		}
	}
	return false
}

// trayItem is what the tray path asks of the tray icon; *mygo.Tray has it.
type trayItem interface {
	OnClick(fn func()) func()
	OnRightClick(fn func()) func()
	SetMenu(m *mygo.Menu)
	PopUpMenu()
}

// trayMenuAttached: on Linux the menu is attached to the tray icon for good.
// The AppIndicator never reports a right click, and opens the menu it holds
// on a click: GNOME's extension on a single click, and it sends a double
// click to OnClick, which opens the popover; KDE sends a left click there.
const trayMenuAttached = runtime.GOOS == "linux"

func (d *Desktop) newTray() {
	var err error
	d.tray, err = mygo.NewTray(mygo.TrayOptions{ID: "sayso", Icon: trayIcon(d.a.incognito), IconIsTemplate: trayTemplate, ToolTip: "Sayso"})
	if err != nil {
		log.Printf("tray: %v", err)
		return
	}
	d.wireTray(d.tray, trayMenuAttached)
}

// trayMenu is the plain tray menu, in the current interface language.
func (d *Desktop) trayMenu() *mygo.Menu {
	click := func(fn func()) func(*mygo.MenuItem, *mygo.Window) {
		return func(*mygo.MenuItem, *mygo.Window) { fn() }
	}
	return mygo.NewMenu([]*mygo.MenuItem{
		{Label: i18n.T("tray.toggle_dictation"), Click: click(d.a.Toggle)},
		{Label: i18n.T("tray.open_hub"), Click: click(d.ShowHub)},
		mygo.Separator(),
		{Label: i18n.T("tray.quit"), Accelerator: "CmdOrCtrl+Q", Click: click(d.Quit)},
	})
}

// wireTray connects the tray icon t. On macOS and Windows the primary
// click opens the themed popover and the secondary click the plain menu,
// for screen readers and keyboard users. With attached, the menu is on
// the icon from the start.
func (d *Desktop) wireTray(t trayItem, attached bool) {
	d.trayItem, d.trayAttached = t, attached
	if attached {
		t.SetMenu(d.trayMenu())
	}
	t.OnClick(func() { d.trayClicked(time.Now()) })
	t.OnRightClick(func() {
		t.SetMenu(d.trayMenu())
		t.PopUpMenu()
	})
}

// RefreshTrayMenu puts the attached tray menu in the current interface
// language. A menu set only for the secondary click is built as it opens.
func (d *Desktop) RefreshTrayMenu() {
	if d.trayItem != nil && d.trayAttached {
		d.trayItem.SetMenu(d.trayMenu())
	}
}

// SetTrayIncognito dims the tray icon while incognito is on.
func (d *Desktop) SetTrayIncognito(on bool) {
	if d.tray == nil {
		return
	}
	if err := d.tray.SetIcon(trayIcon(on), trayTemplate); err != nil {
		log.Printf("tray: %v", err)
	}
}

// SetThemeMode sets the appearance of the native chrome.
func (d *Desktop) SetThemeMode(m config.ThemeMode) {
	src := mygo.ThemeSystem
	switch m {
	case config.ThemeLight:
		src = mygo.ThemeLight
	case config.ThemeDark:
		src = mygo.ThemeDark
	}
	mygo.Theme.SetSource(src)
}

// SetOverlaySize resizes the overlay window for a size.
func (d *Desktop) SetOverlaySize(config.OverlaySize) {
	if ov := d.layer.overlay(); ov != nil && ov.IsVisible() {
		d.placeOverlay()
	}
}

// OpenPath opens a file or folder with the system.
func (d *Desktop) OpenPath(path string) {
	go func() {
		if err := mygo.Shell.OpenPath(path); err != nil {
			log.Printf("open %s: %v", path, err)
		}
	}()
}

// OpenURL opens a web address.
func (d *Desktop) OpenURL(url string) {
	go func() {
		if err := mygo.Shell.OpenExternal(url); err != nil {
			log.Printf("open %s: %v", url, err)
		}
	}()
}

// Quit quits the app.
func (d *Desktop) Quit() {
	d.quitting = true
	d.stopFocus()
	mygo.App.Quit()
}
