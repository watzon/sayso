package platform

// The focus tracker of macOS: where the keyboard focus is, for the overlay
// to show on that display.
//
// Focused asks the app in front, with Accessibility, for its focused window
// and the focused input in it (one attribute each, with the 0.25 s
// Accessibility timeout). Without Accessibility, or when the app does not
// answer, it takes the app's topmost normal window from the window list.
// CoreGraphics and Accessibility give global points with y growing down
// from the top of the primary display, MyGo's screen space, so no
// conversion is needed.
//
// Watch hears app switches from NSWorkspace, and, with Accessibility, the
// focused window and input changes of the app in front from an AXObserver,
// which catches a switch between two windows of one app on two displays.
// The observer lives on a thread of its own with its own run loop: it is
// made, moved to each new app in front, and released there, so no
// Accessibility call ever waits on the main thread.

import (
	"fmt"
	"os"
	"runtime"
	"sync"
	"unsafe"

	"github.com/ebitengine/purego"
	"github.com/ebitengine/purego/objc"
)

var (
	cfArrayGetCount           func(a uintptr) int64
	cfArrayGetValueAtIndex    func(a uintptr, i int64) uintptr
	cfDictionaryGetValue      func(d uintptr, key uintptr) uintptr
	cfNumberGetValue          func(n uintptr, typ int64, out unsafe.Pointer) bool
	cfNumberGetTypeID         func() uint64
	cfDictionaryGetTypeID     func() uint64
	cfRunLoopRunInMode        func(mode uintptr, seconds float64, returnAfterSource bool) int32
	cfRunLoopRemoveSource     func(rl uintptr, src uintptr, mode uintptr)
	cgWindowListCopyInfo      func(option uint32, relativeTo uint32) uintptr
	cgRectFromDictionary      func(dict uintptr, rect *cgRect) bool
	axValueGetValue           func(v uintptr, typ uint32, out unsafe.Pointer) bool
	axValueGetTypeID          func() uint64
	axObserverCreate          func(pid int32, cb uintptr, out *uintptr) int32
	axObserverAddNotification func(obs, el, name, refcon uintptr) int32
	axObserverRemoveNotif     func(obs, el, name uintptr) int32
	axObserverGetRunLoopSrc   func(obs uintptr) uintptr

	kCFRunLoopDefaultMode  uintptr
	nsWorkspaceDidActivate objc.ID

	focusBindOnce sync.Once
	focusBindErr  error

	// The AXObserver callback is made once (purego callbacks are never
	// freed) and calls the running Watch's changed.
	axFocusCallback     uintptr
	axFocusCallbackOnce sync.Once
	axFocusTargetMu     sync.Mutex
	axFocusTarget       func()

	// axFocusNotifications are what the observer hears from the app in front.
	axFocusNotifications = []string{"AXFocusedWindowChanged", "AXFocusedUIElementChanged", "AXMainWindowChanged"}
)

const (
	kCGWindowListOptionOnScreenOnly     = 1 << 0
	kCGWindowListExcludeDesktopElements = 1 << 4
	kCFNumberSInt64Type                 = 4
	kAXValueCGPointType                 = 1
	kAXValueCGSizeType                  = 2
	// focusRunLoopSeconds is how long the observer's run loop runs before
	// it looks again: as good as forever, as poke and stop end it.
	focusRunLoopSeconds = 1e10
)

type cgPoint struct{ X, Y float64 }
type cgSize struct{ W, H float64 }
type cgRect struct {
	Origin cgPoint
	Size   cgSize
}

// bindFocus binds the calls that only the focus tracker makes.
func bindFocus() error {
	if err := loadDarwin(); err != nil {
		return err
	}
	focusBindOnce.Do(func() {
		defer func() {
			// RegisterLibFunc panics on a missing symbol.
			if r := recover(); r != nil {
				focusBindErr = fmt.Errorf("macOS focus calls: %v", r)
			}
		}()
		cf := openLib(pathCoreFoundation)
		purego.RegisterLibFunc(&cfArrayGetCount, cf, "CFArrayGetCount")
		purego.RegisterLibFunc(&cfArrayGetValueAtIndex, cf, "CFArrayGetValueAtIndex")
		purego.RegisterLibFunc(&cfDictionaryGetValue, cf, "CFDictionaryGetValue")
		purego.RegisterLibFunc(&cfNumberGetValue, cf, "CFNumberGetValue")
		purego.RegisterLibFunc(&cfNumberGetTypeID, cf, "CFNumberGetTypeID")
		purego.RegisterLibFunc(&cfDictionaryGetTypeID, cf, "CFDictionaryGetTypeID")
		purego.RegisterLibFunc(&cfRunLoopRunInMode, cf, "CFRunLoopRunInMode")
		purego.RegisterLibFunc(&cfRunLoopRemoveSource, cf, "CFRunLoopRemoveSource")
		kCFRunLoopDefaultMode = global(cf, "kCFRunLoopDefaultMode")
		cg := openLib(pathCoreGraphics)
		purego.RegisterLibFunc(&cgWindowListCopyInfo, cg, "CGWindowListCopyWindowInfo")
		purego.RegisterLibFunc(&cgRectFromDictionary, cg, "CGRectMakeWithDictionaryRepresentation")
		as := openLib(pathApplicationServices)
		purego.RegisterLibFunc(&axValueGetValue, as, "AXValueGetValue")
		purego.RegisterLibFunc(&axValueGetTypeID, as, "AXValueGetTypeID")
		purego.RegisterLibFunc(&axObserverCreate, as, "AXObserverCreate")
		purego.RegisterLibFunc(&axObserverAddNotification, as, "AXObserverAddNotification")
		purego.RegisterLibFunc(&axObserverRemoveNotif, as, "AXObserverRemoveNotification")
		purego.RegisterLibFunc(&axObserverGetRunLoopSrc, as, "AXObserverGetRunLoopSource")
		appKit := openLib(pathAppKit)
		nsWorkspaceDidActivate = objc.ID(global(appKit, "NSWorkspaceDidActivateApplicationNotification"))
	})
	return focusBindErr
}

type macFocus struct{ self int32 }

func newMacFocus() *macFocus { return &macFocus{self: int32(os.Getpid())} }

// frontmostPID is the process of the app in front, 0 when none.
func frontmostPID() int32 {
	var pid int32
	withPool(func() {
		front := class("NSWorkspace").Send(sel("sharedWorkspace")).Send(sel("frontmostApplication"))
		if front != 0 {
			pid = objc.Send[int32](front, sel("processIdentifier"))
		}
	})
	return pid
}

func (m *macFocus) Focused() (FocusFrame, bool) {
	if bindFocus() != nil {
		return FocusFrame{}, false
	}
	pid := frontmostPID()
	if pid <= 0 {
		return FocusFrame{}, false
	}
	// Sayso's own windows come from the list: an Accessibility call into
	// this process is answered by its main thread.
	if pid != m.self && axIsProcessTrusted() {
		if r, answered := axFocusFrame(pid); answered {
			return FocusFrame{Rect: r}, !r.Empty()
		}
	}
	r, ok := frontWindow(cgWindows(), int64(pid))
	return FocusFrame{Rect: r}, ok
}

// axFocusFrame asks pid for its focused window and the focused input in it.
// answered is false when the app did not answer, for the window list to
// take over; an app that answers with no focused window (Finder with only
// its desktop) gives an empty frame.
func axFocusFrame(pid int32) (r Rect, answered bool) {
	app := axUIElementCreateApplication(pid)
	if app == 0 {
		return Rect{}, false
	}
	defer cfRelease(app)
	axUIElementSetMessagingTimeout(app, axTimeoutSeconds)
	window, err := axCopyAttributeErr(app, "AXFocusedWindow")
	if window == 0 {
		return Rect{}, err == kAXErrorNoValue
	}
	defer cfRelease(window)
	frame := axElementFrame(window)
	var input Rect
	if el, _ := axCopyAttributeErr(app, "AXFocusedUIElement"); el != 0 {
		input = axElementFrame(el)
		cfRelease(el)
	}
	r, _ = focusRect(frame, input)
	return r, true
}

// axCopyAttributeErr copies one attribute and the Accessibility error.
func axCopyAttributeErr(element uintptr, name string) (uintptr, int32) {
	attr := cfString(name)
	defer cfRelease(attr)
	var value uintptr
	if e := axUIElementCopyAttributeValue(element, attr, &value); e != kAXErrorSuccess {
		return 0, e
	}
	return value, kAXErrorSuccess
}

// axElementFrame is AXPosition and AXSize of an element, empty when either
// is missing.
func axElementFrame(el uintptr) Rect {
	var p cgPoint
	var s cgSize
	if !axValue(el, "AXPosition", kAXValueCGPointType, unsafe.Pointer(&p)) || !axValue(el, "AXSize", kAXValueCGSizeType, unsafe.Pointer(&s)) {
		return Rect{}
	}
	return rectFromFloats(p.X, p.Y, s.W, s.H)
}

func axValue(el uintptr, name string, typ uint32, out unsafe.Pointer) bool {
	v, _ := axCopyAttributeErr(el, name)
	if v == 0 {
		return false
	}
	defer cfRelease(v)
	return cfGetTypeID(v) == axValueGetTypeID() && axValueGetValue(v, typ, out)
}

// cgWindows is the on-screen window list, front to back, without the
// desktop.
func cgWindows() []sysWindow {
	arr := cgWindowListCopyInfo(kCGWindowListOptionOnScreenOnly|kCGWindowListExcludeDesktopElements, 0)
	if arr == 0 {
		return nil
	}
	defer cfRelease(arr)
	kPID, kLayer, kBounds, kName := cfString("kCGWindowOwnerPID"), cfString("kCGWindowLayer"), cfString("kCGWindowBounds"), cfString("kCGWindowName")
	defer func() { cfRelease(kPID); cfRelease(kLayer); cfRelease(kBounds); cfRelease(kName) }()
	n := cfArrayGetCount(arr)
	out := make([]sysWindow, 0, n)
	for i := range n {
		d := cfArrayGetValueAtIndex(arr, i)
		if d == 0 || cfGetTypeID(d) != cfDictionaryGetTypeID() {
			continue
		}
		w := sysWindow{pid: cfInt(cfDictionaryGetValue(d, kPID)), layer: cfInt(cfDictionaryGetValue(d, kLayer))}
		if b := cfDictionaryGetValue(d, kBounds); b != 0 && cfGetTypeID(b) == cfDictionaryGetTypeID() {
			var r cgRect
			if cgRectFromDictionary(b, &r) {
				w.bounds = rectFromFloats(r.Origin.X, r.Origin.Y, r.Size.W, r.Size.H)
			}
		}
		// Names of other apps' windows need Screen Recording; Sayso's own
		// are always there, which is all frontWindow needs.
		if name := cfDictionaryGetValue(d, kName); name != 0 && cfGetTypeID(name) == cfStringGetTypeID() {
			w.name = goStringFromCF(name)
		}
		out = append(out, w)
	}
	return out
}

// cfInt reads a CFNumber; -1 when it is not one.
func cfInt(n uintptr) int64 {
	if n == 0 || cfGetTypeID(n) != cfNumberGetTypeID() {
		return -1
	}
	var v int64
	if !cfNumberGetValue(n, kCFNumberSInt64Type, unsafe.Pointer(&v)) {
		return -1
	}
	return v
}

// Watch hears NSWorkspace's app switches on the main thread and runs the
// AXObserver of the app in front on a thread of its own.
func (m *macFocus) Watch(changed func()) (func(), error) {
	if err := bindFocus(); err != nil {
		return nil, err
	}
	axFocusCallbackOnce.Do(func() {
		axFocusCallback = purego.NewCallback(func(_, _, _, _ uintptr) uintptr {
			axFocusTargetMu.Lock()
			fn := axFocusTarget
			axFocusTargetMu.Unlock()
			if fn != nil {
				fn()
			}
			return 0
		})
	})
	w := &axWatch{self: m.self, changed: changed, kick: make(chan struct{}, 1), quit: make(chan struct{}), done: make(chan struct{})}
	axFocusTargetMu.Lock()
	axFocusTarget = changed
	axFocusTargetMu.Unlock()
	w.block = objc.NewBlock(func(objc.Block, objc.ID) {
		w.poke()
		changed()
	})
	withPool(func() {
		center := class("NSWorkspace").Send(sel("sharedWorkspace")).Send(sel("notificationCenter"))
		w.token = center.Send(sel("addObserverForName:object:queue:usingBlock:"), nsWorkspaceDidActivate, objc.ID(0), objc.ID(0), w.block)
		w.token.Send(sel("retain"))
	})
	started := make(chan struct{})
	go w.loop(started)
	<-started
	return w.stop, nil
}

// axWatch is one Watch: the NSWorkspace observer and the thread that holds
// the AXObserver of the app in front.
type axWatch struct {
	self    int32
	changed func()
	block   objc.Block
	token   objc.ID
	kick    chan struct{}
	quit    chan struct{}
	done    chan struct{}
	once    sync.Once

	mu      sync.Mutex
	runLoop uintptr // the thread's run loop while it runs an observer

	pid int32   // the app the observer watches
	obs uintptr // AXObserverRef
	app uintptr // AXUIElementRef of pid
}

// poke wakes the thread to move the observer to the app now in front.
func (w *axWatch) poke() {
	select {
	case w.kick <- struct{}{}:
	default:
	}
	w.mu.Lock()
	if w.runLoop != 0 {
		cfRunLoopStop(w.runLoop)
	}
	w.mu.Unlock()
}

func (w *axWatch) loop(started chan<- struct{}) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	defer close(w.done)
	rl := cfRunLoopGetCurrent()
	w.attach()
	close(started)
	for {
		select {
		case <-w.quit:
			w.detach()
			return
		case <-w.kick:
			w.attach()
			continue
		default:
		}
		if w.obs == 0 {
			// No observer: nothing for the run loop to run, so wait here.
			select {
			case <-w.quit:
				w.detach()
				return
			case <-w.kick:
				w.attach()
			}
			continue
		}
		w.mu.Lock()
		w.runLoop = rl
		w.mu.Unlock()
		// poke and stop send first, then stop the run loop if it is set:
		// one that came before runLoop was set left its kick or quit,
		// seen here; one after stops the run loop, which keeps a stop
		// that comes before it runs.
		if len(w.kick) > 0 || isClosed(w.quit) {
			w.mu.Lock()
			w.runLoop = 0
			w.mu.Unlock()
			continue
		}
		// Runs the observer's callbacks until poke or stop stops it.
		cfRunLoopRunInMode(kCFRunLoopDefaultMode, focusRunLoopSeconds, false)
		w.mu.Lock()
		w.runLoop = 0
		w.mu.Unlock()
	}
}

// attach moves the observer to the app in front, with Accessibility. Not
// to Sayso itself, whose main thread answers its Accessibility calls.
func (w *axWatch) attach() {
	pid := frontmostPID()
	if !axIsProcessTrusted() || pid <= 0 || pid == w.self {
		w.detach()
		return
	}
	if pid == w.pid && w.obs != 0 {
		return
	}
	w.detach()
	var obs uintptr
	if axObserverCreate(pid, axFocusCallback, &obs) != kAXErrorSuccess || obs == 0 {
		return
	}
	app := axUIElementCreateApplication(pid)
	if app == 0 {
		cfRelease(obs)
		return
	}
	axUIElementSetMessagingTimeout(app, axTimeoutSeconds)
	added := 0
	for _, n := range axFocusNotifications {
		name := cfString(n)
		if axObserverAddNotification(obs, app, name, 0) == kAXErrorSuccess {
			added++
		}
		cfRelease(name)
	}
	if added == 0 {
		cfRelease(app)
		cfRelease(obs)
		return
	}
	cfRunLoopAddSource(cfRunLoopGetCurrent(), axObserverGetRunLoopSrc(obs), kCFRunLoopDefaultMode)
	w.pid, w.obs, w.app = pid, obs, app
}

// detach removes and releases the observer. On the watch's thread.
func (w *axWatch) detach() {
	if w.obs == 0 {
		return
	}
	cfRunLoopRemoveSource(cfRunLoopGetCurrent(), axObserverGetRunLoopSrc(w.obs), kCFRunLoopDefaultMode)
	for _, n := range axFocusNotifications {
		name := cfString(n)
		axObserverRemoveNotif(w.obs, w.app, name)
		cfRelease(name)
	}
	cfRelease(w.app)
	cfRelease(w.obs)
	w.pid, w.obs, w.app = 0, 0, 0
}

func (w *axWatch) stop() {
	w.once.Do(func() {
		withPool(func() {
			center := class("NSWorkspace").Send(sel("sharedWorkspace")).Send(sel("notificationCenter"))
			center.Send(sel("removeObserver:"), w.token)
			w.token.Send(sel("release"))
		})
		w.block.Release()
		axFocusTargetMu.Lock()
		axFocusTarget = nil
		axFocusTargetMu.Unlock()
		close(w.quit)
		w.mu.Lock()
		if w.runLoop != 0 {
			cfRunLoopStop(w.runLoop)
		}
		w.mu.Unlock()
		<-w.done
	})
}

func isClosed(c <-chan struct{}) bool {
	select {
	case <-c:
		return true
	default:
		return false
	}
}

var _ FocusTracker = (*macFocus)(nil)
