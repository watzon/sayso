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
	popover popoverToggle
	// popoverClosing is set once the popover's close was handled: Sayso
	// closing it, or the flyout's dismissal, of which OnHide and OnClosed
	// hear again.
	popoverClosing bool
	quitting       bool
	// region is the overlay's card, the part of its window that takes the mouse.
	region mu.Rect
}

var (
	_ Shell        = (*Desktop)(nil)
	_ overlayInput = (*Desktop)(nil)
)

// NewDesktop makes the shell of a and makes it a's shell. Call Start from
// mygo.App.WhenReady.
func NewDesktop(a *App) *Desktop {
	d := &Desktop{a: a, wins: map[Window]*mygo.Window{}}
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
	// The system title bar is a Linux choice (crates/sayso-app/src/chrome.rs);
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
	mygo.App.OnBeforeQuit(func(*mygo.QuitEvent) { d.quitting = true })
	mygo.App.OnWindowAllClosed(func() {})
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

// placeOverlay puts the pill at the bottom center (or the notch at the
// top center) of the display with the pointer.
func (d *Desktop) placeOverlay() {
	w, h := d.overlaySize()
	d.overlay.SetSize(w, h)
	disp := mygo.Screen.PrimaryDisplay()
	if d.a.cfg.Overlay.FollowDisplay {
		disp = mygo.Screen.DisplayNearestPoint(mygo.Screen.CursorScreenPoint())
	}
	if d.a.cfg.Overlay.Style == config.OverlayNotch {
		b := disp.Bounds
		d.overlay.SetPosition(b.X+(b.Width-w)/2, b.Y)
	} else {
		// The card's bottom edge is overlayMargin inside the window, and
		// sits that far above the bottom of the work area (Rust placement
		// DEFAULT_BOTTOM).
		wa := disp.WorkArea
		d.overlay.SetPosition(wa.X+(wa.Width-w)/2, wa.Y+wa.Height-h)
	}
	d.notePillHalf()
}

func (d *Desktop) notePillHalf() {
	x, y := d.overlay.Position()
	disp := mygo.Screen.DisplayNearestPoint(mygo.Point{X: x, Y: y})
	d.a.SetPreviewBelow(y < disp.Bounds.Y+disp.Bounds.Height/2)
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

// ShowHub shows and focuses the hub.
func (d *Desktop) ShowHub() {
	if d.a.cfg.General.DockIconWithHub && runtime.GOOS == "darwin" {
		mygo.App.SetActivationPolicy(mygo.ActivationPolicyRegular)
	}
	d.hub.Show()
	d.hub.Focus()
}

// ShowOverlay shows the overlay without taking the keyboard, or hides it.
func (d *Desktop) ShowOverlay(show bool) {
	if d.overlay == nil {
		return
	}
	if !show {
		d.overlay.Hide()
		return
	}
	if !d.overlay.IsVisible() {
		d.placeOverlay()
	}
	d.overlay.SetIgnoreMouseEvents(d.a.OverlayClickThrough())
	d.applyInputRegion()
	d.overlay.ShowInactive()
}

// SetOverlayInputRegion makes only r, the visible card, take the mouse:
// the rest of the overlay window lets it through to what is under it.
func (d *Desktop) SetOverlayInputRegion(r mu.Rect) {
	d.region = r
	d.applyInputRegion()
}

func (d *Desktop) applyInputRegion() {
	if d.overlay == nil {
		return
	}
	r := d.region
	if r.W <= 0 || r.H <= 0 {
		d.overlay.SetInputRegion([]mygo.Rectangle{})
		return
	}
	x0, y0 := int(math.Floor(float64(r.X))), int(math.Floor(float64(r.Y)))
	x1, y1 := int(math.Ceil(float64(r.X+r.W))), int(math.Ceil(float64(r.Y+r.H)))
	d.overlay.SetInputRegion([]mygo.Rectangle{{X: x0, Y: y0, Width: x1 - x0, Height: y1 - y0}})
}

// ShowWindow shows or hides a secondary window, making it on first use.
func (d *Desktop) ShowWindow(which Window, show bool) {
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
	if which == WinPopover {
		d.popoverClosing = false
	}
	w.Show()
	w.Focus()
	if which == WinOnboarding {
		d.a.enteredStep(d.a.onb.step)
	}
}

// ClosePopover closes the tray popover on purpose: keep leaves the keyboard
// in Sayso, for the hub that shows next; otherwise it goes back to the app
// the user came from.
func (d *Desktop) ClosePopover(keep bool) {
	w := d.wins[WinPopover]
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
	case WinUpdate:
		opts.Title, opts.Width, opts.Height, opts.Content = i18n.T("update.slip_title"), UpdateW, UpdateH, mu.View(d.a.UpdateView)
	case WinApproval:
		opts.Title, opts.Width, opts.Height, opts.Content = i18n.T("apps.approval.window_title"), ApprovalW, ApprovalH, mu.View(d.a.ApprovalView)
		opts.AlwaysOnTop = true
	case WinPopover:
		return d.newPopover()
	default:
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

// newPopover is the themed panel under the tray item.
func (d *Desktop) newPopover() *mygo.Window {
	if d.tray == nil {
		return nil
	}
	// Focusable: it closes as a menu does, on a press outside it (in
	// another app, the desktop or a window of Sayso), when another window
	// takes the keyboard or the app is deactivated, and on Escape (Rust
	// popover.rs). A press on the tray icon only closes it.
	w := mygo.NewFlyout(mygo.FlyoutOptions{
		Tray: d.tray, Placement: mygo.PlacementBottom, Gap: 6, Width: PopoverW, Height: PopoverH,
		Shadow: true, Popover: true, Focusable: true, CornerRadius: 12, Content: mu.View(d.a.PopoverView),
	})
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
	time.AfterFunc(delay, func() {
		mygo.RunOnMain(func() {
			if mygo.App.IsActive() {
				platform.DeactivateApp()
			}
		})
	})
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

func (d *Desktop) newTray() {
	click := func(fn func()) func(*mygo.MenuItem, *mygo.Window) {
		return func(*mygo.MenuItem, *mygo.Window) { fn() }
	}
	menu := mygo.NewMenu([]*mygo.MenuItem{
		{Label: i18n.T("tray.toggle_dictation"), Click: click(d.a.Toggle)},
		{Label: i18n.T("tray.open_hub"), Click: click(d.ShowHub)},
		mygo.Separator(),
		{Label: i18n.T("tray.quit"), Accelerator: "CmdOrCtrl+Q", Click: click(d.Quit)},
	})
	var err error
	d.tray, err = mygo.NewTray(mygo.TrayOptions{ID: "sayso", Icon: trayIcon(d.a.incognito), IconIsTemplate: trayTemplate, ToolTip: "Sayso"})
	if err != nil {
		log.Printf("tray: %v", err)
		return
	}
	// The primary click opens the themed popover; the secondary click the
	// plain menu, for screen readers and keyboard users.
	d.tray.OnClick(func() {
		w := d.wins[WinPopover]
		visible := w != nil && !w.IsDestroyed() && w.IsVisible()
		switch d.popover.click(time.Now(), visible) {
		case popoverHide:
			d.ClosePopover(false)
		case popoverShow:
			d.ShowWindow(WinPopover, true)
		}
	})
	d.tray.OnRightClick(func() {
		d.tray.SetMenu(menu)
		d.tray.PopUpMenu()
	})
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
	if d.overlay != nil && d.overlay.IsVisible() {
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
	mygo.App.Quit()
}
