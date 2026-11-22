package platform

// Global hotkeys on macOS: one CGEventTap on its own locked OS thread with a
// CFRunLoop. Every decision is in hotkeyEngine.
//
// With Accessibility the tap is active (kCGEventTapOptionDefault): it takes
// the keys of matched chords, Return while Send with Enter records, and Esc
// while "take Esc" records, so the app does not get them. With only Input
// Monitoring the tap listens: hotkeys still fire, but every key also reaches
// the app. Secure Event Input hides regular keys from any tap, so
// modifier-only toggles pause while it is on; solo-modifier push-to-talk keeps
// working.

import (
	"errors"
	"fmt"
	"log"
	"runtime"
	"sync"
	"time"

	"github.com/ebitengine/purego"
)

type macHotkeys struct {
	mu      sync.Mutex
	engine  *hotkeyEngine
	events  chan HotkeyEvent
	tap     *macTap
	handle  uintptr
	stopped chan struct{}
}

type macTap struct {
	runLoop uintptr
	port    uintptr
	active  bool
	done    chan struct{}
}

var (
	tapCallbackOnce sync.Once
	tapCallback     uintptr
	tapOwnersMu     sync.Mutex
	tapOwners       = map[uintptr]*macHotkeys{}
	nextTapOwner    uintptr
)

func newMacHotkeys() *macHotkeys {
	return &macHotkeys{engine: newHotkeyEngine()}
}

func (h *macHotkeys) Configure(cfg HotkeyConfig) error {
	b, err := parseBindings(cfg)
	if err != nil {
		return err
	}
	h.mu.Lock()
	released := h.engine.configure(b)
	h.mu.Unlock()
	h.deliver(released)
	return nil
}

func (h *macHotkeys) SetRecording(recording bool) {
	h.mu.Lock()
	h.engine.setRecording(recording)
	upgrade := recording && h.tap != nil && !h.tap.active
	h.mu.Unlock()
	// Accessibility may have come since the tap started: take keys from now on.
	if upgrade && loadDarwin() == nil && axIsProcessTrusted() {
		go h.restartTap()
	}
}

func (h *macHotkeys) Start(onEvent func(HotkeyEvent)) error {
	if onEvent == nil {
		return errors.New("hotkeys: no event handler")
	}
	if err := loadDarwin(); err != nil {
		return err
	}
	h.mu.Lock()
	if h.events != nil {
		h.mu.Unlock()
		return errors.New("hotkeys: already started")
	}
	// Creating a tap makes the system ask for the permission. Sayso asks in
	// onboarding first, so fail here instead of prompting.
	if !cgPreflightListenEventAccess() && !axIsProcessTrusted() {
		h.mu.Unlock()
		return errors.New("hotkeys: Input Monitoring is not granted")
	}
	h.events = make(chan HotkeyEvent, 256)
	h.stopped = make(chan struct{})
	tapOwnersMu.Lock()
	nextTapOwner++
	h.handle = nextTapOwner
	tapOwners[h.handle] = h
	tapOwnersMu.Unlock()
	events, stopped := h.events, h.stopped
	h.mu.Unlock()

	go func() {
		for {
			select {
			case ev := <-events:
				onEvent(ev)
			case <-stopped:
				return
			}
		}
	}()
	tap, err := startMacTap(h.handle)
	if err != nil {
		_ = h.Stop()
		return err
	}
	h.mu.Lock()
	h.tap = tap
	h.engine.setCanSwallow(tap.active)
	h.mu.Unlock()
	return nil
}

func (h *macHotkeys) Stop() error {
	h.mu.Lock()
	tap := h.tap
	h.tap = nil
	if h.stopped != nil {
		close(h.stopped)
	}
	h.events, h.stopped = nil, nil
	h.engine.setCanSwallow(false)
	h.engine.releaseHeld()
	handle := h.handle
	h.mu.Unlock()
	if tap != nil {
		tap.stop()
	}
	tapOwnersMu.Lock()
	delete(tapOwners, handle)
	tapOwnersMu.Unlock()
	return nil
}

func (h *macHotkeys) restartTap() {
	h.mu.Lock()
	old := h.tap
	h.mu.Unlock()
	if old == nil {
		return
	}
	tap, err := startMacTap(h.handle)
	if err != nil {
		log.Printf("hotkeys: cannot restart the event tap: %v", err)
		return
	}
	h.mu.Lock()
	if h.tap != old {
		// Stopped or restarted meanwhile.
		h.mu.Unlock()
		tap.stop()
		return
	}
	h.tap = tap
	h.engine.setCanSwallow(tap.active)
	released := h.engine.releaseHeld()
	h.mu.Unlock()
	old.stop()
	h.deliver(released)
}

// deliver queues events for the handler without blocking the tap thread.
func (h *macHotkeys) deliver(events []HotkeyEvent) {
	if len(events) == 0 {
		return
	}
	h.mu.Lock()
	ch := h.events
	h.mu.Unlock()
	if ch == nil {
		return
	}
	for _, ev := range events {
		select {
		case ch <- ev:
		default:
			log.Printf("hotkeys: event queue full, dropped %v", ev.Kind)
		}
	}
}

// onTapEvent runs on the tap thread. It returns true to take the event.
func (h *macHotkeys) onTapEvent(etype uint32, event uintptr) bool {
	if etype == kCGEventTapDisabledByTimeout || etype == kCGEventTapDisabledByUserInput {
		h.mu.Lock()
		if h.tap != nil {
			cgEventTapEnable(h.tap.port, true)
		}
		// Key-up events may have been missed while the tap was off.
		released := h.engine.releaseHeld()
		h.mu.Unlock()
		h.deliver(released)
		return false
	}
	code := uint16(cgEventGetIntegerValueField(event, kCGKeyboardEventKeycode))
	flags := cgEventGetFlags(event)
	repeat := cgEventGetIntegerValueField(event, kCGKeyboardEventAutorepeat) != 0
	secure := etype == kCGEventFlagsChanged && isSecureEventInputEnabled()
	raw, ok := macRawEvent(etype, code, flags, repeat, secure)
	if !ok {
		return false
	}
	h.mu.Lock()
	if raw.kind == rawModifiers {
		// Reconcile before gating the gesture, also for the first event after
		// Secure Event Input ended.
		h.engine.reconcilePressed(func(c uint16) bool {
			return cgEventSourceKeyState(kCGEventSourceStateHIDSystemState, c)
		})
	}
	d := h.engine.handle(raw, time.Now())
	h.mu.Unlock()
	h.deliver(d.events)
	return d.swallow
}

func tapTrampoline(proxy uintptr, etype uint32, event uintptr, user uintptr) uintptr {
	tapOwnersMu.Lock()
	h := tapOwners[user]
	tapOwnersMu.Unlock()
	if h != nil && h.onTapEvent(etype, event) {
		return 0 // A null return takes the event from the app.
	}
	return event
}

// startMacTap starts a tap thread and waits until it runs. The tap is active
// when Accessibility allows it, else listen-only.
func startMacTap(owner uintptr) (*macTap, error) {
	tapCallbackOnce.Do(func() { tapCallback = purego.NewCallback(tapTrampoline) })
	ready := make(chan error, 1)
	t := &macTap{done: make(chan struct{})}
	go func() {
		runtime.LockOSThread()
		defer runtime.UnlockOSThread()
		defer close(t.done)
		mask := macEventMask()
		var port uintptr
		if axIsProcessTrusted() {
			port = cgEventTapCreate(kCGSessionEventTap, kCGHeadInsertEventTap, kCGEventTapOptionDefault, mask, tapCallback, owner)
			t.active = port != 0
		}
		if port == 0 {
			port = cgEventTapCreate(kCGSessionEventTap, kCGHeadInsertEventTap, kCGEventTapOptionListenOnly, mask, tapCallback, owner)
		}
		if port == 0 {
			ready <- fmt.Errorf("hotkeys: CGEventTapCreate failed (Input Monitoring granted: %v, Accessibility: %v)",
				cgPreflightListenEventAccess(), axIsProcessTrusted())
			return
		}
		source := cfMachPortCreateRunLoopSource(0, port, 0)
		t.port = port
		t.runLoop = cfRunLoopGetCurrent()
		cfRunLoopAddSource(t.runLoop, source, kCFRunLoopCommonModes)
		cgEventTapEnable(port, true)
		ready <- nil
		cfRunLoopRun()
		cgEventTapEnable(port, false)
		cfMachPortInvalidate(port)
		cfRelease(source)
		cfRelease(port)
	}()
	if err := <-ready; err != nil {
		<-t.done
		return nil, err
	}
	return t, nil
}

func (t *macTap) stop() {
	stopRunLoop(t.runLoop, t.done)
}

// stopRunLoop stops a run loop of another thread and waits for the thread.
// CFRunLoopStop is safe from any thread, but it does nothing to a loop that
// has not entered CFRunLoopRun yet, so it repeats until the thread is done.
func stopRunLoop(runLoop uintptr, done <-chan struct{}) {
	for {
		cfRunLoopStop(runLoop)
		select {
		case <-done:
			return
		case <-time.After(20 * time.Millisecond):
		}
	}
}
