package platform

import (
	"errors"
	"runtime"
	"sync"
	"time"
	"unsafe"

	"golang.org/x/sys/windows"
)

// windowsHotkeys listens with a low-level keyboard hook, and a low-level mouse
// hook for the clicks that cancel a modifier tap, on a thread of its own with a
// message loop. Low-level hooks must answer fast: events go to the callback
// through a queue, never from inside the hook.

type windowsHotkeys struct {
	mu       sync.Mutex
	logic    *hookLogic
	threadID uint32
	running  bool
	done     chan struct{}
	events   chan HotkeyEvent
}

func newWindowsHotkeys() *windowsHotkeys { return &windowsHotkeys{logic: newHookLogic()} }

// The hook procedures are package-wide: Windows calls them on the hook thread,
// and a callback made by windows.NewCallback is never freed.
var (
	activeHooks   *windowsHotkeys
	activeHooksMu sync.Mutex
	keyboardProc  = windows.NewCallback(keyboardHook)
	mouseProc     = windows.NewCallback(mouseHook)
)

func (h *windowsHotkeys) Configure(cfg HotkeyConfig) error {
	h.mu.Lock()
	released, err := h.logic.configure(cfg)
	h.mu.Unlock()
	h.emit(released)
	return err
}

func (h *windowsHotkeys) SetRecording(recording bool) {
	h.mu.Lock()
	h.logic.setRecording(recording)
	h.mu.Unlock()
}

func (h *windowsHotkeys) Start(onEvent func(HotkeyEvent)) error {
	h.mu.Lock()
	if h.running {
		h.mu.Unlock()
		return errors.New("the hotkeys are listening already")
	}
	activeHooksMu.Lock()
	if activeHooks != nil {
		activeHooksMu.Unlock()
		h.mu.Unlock()
		return errors.New("another hotkey listener is running")
	}
	activeHooks = h
	activeHooksMu.Unlock()
	h.running = true
	h.done = make(chan struct{})
	h.events = make(chan HotkeyEvent, 64)
	h.mu.Unlock()

	go func() {
		for ev := range h.events {
			onEvent(ev)
		}
	}()
	started := make(chan error, 1)
	go h.loop(started)
	if err := <-started; err != nil {
		h.finish()
		return err
	}
	return nil
}

func (h *windowsHotkeys) loop(started chan<- error) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	defer close(h.done)
	var m msg
	// Make the message queue before anyone posts WM_QUIT to it.
	procPeekMessageW.Call(uintptr(unsafe.Pointer(&m)), 0, 0, 0, 0)
	h.mu.Lock()
	h.threadID = windows.GetCurrentThreadId()
	h.mu.Unlock()
	kb, _, err := procSetWindowsHookExW.Call(whKeyboardLL, keyboardProc, 0, 0)
	if kb == 0 {
		started <- err
		return
	}
	defer procUnhookWindowsHookEx.Call(kb)
	// Clicks and wheel turns cancel a modifier tap. The mouse hook only reads.
	if mh, _, _ := procSetWindowsHookExW.Call(whMouseLL, mouseProc, 0, 0); mh != 0 {
		defer procUnhookWindowsHookEx.Call(mh)
	}
	started <- nil
	for {
		r, _, _ := procGetMessageW.Call(uintptr(unsafe.Pointer(&m)), 0, 0, 0)
		if int32(r) <= 0 {
			return
		}
	}
}

func (h *windowsHotkeys) Stop() error {
	h.mu.Lock()
	if !h.running {
		h.mu.Unlock()
		return nil
	}
	tid, done := h.threadID, h.done
	h.mu.Unlock()
	procPostThreadMessageW.Call(uintptr(tid), wmQuit, 0, 0)
	select {
	case <-done:
	case <-time.After(2 * time.Second):
	}
	h.mu.Lock()
	released := h.logic.releaseHeld()
	h.mu.Unlock()
	h.emit(released)
	h.finish()
	return nil
}

func (h *windowsHotkeys) finish() {
	activeHooksMu.Lock()
	if activeHooks == h {
		activeHooks = nil
	}
	activeHooksMu.Unlock()
	h.mu.Lock()
	if h.running {
		h.running = false
		close(h.events)
	}
	h.mu.Unlock()
}

// emit queues events for the callback. A full queue drops the event rather
// than stall the hook.
func (h *windowsHotkeys) emit(events []HotkeyEvent) {
	if len(events) == 0 {
		return
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	if !h.running {
		return
	}
	for _, ev := range events {
		select {
		case h.events <- ev:
		default:
		}
	}
}

func currentHooks() *windowsHotkeys {
	activeHooksMu.Lock()
	defer activeHooksMu.Unlock()
	return activeHooks
}

func keyboardHook(code int, wParam, lParam uintptr) uintptr {
	h := currentHooks()
	if code < 0 || h == nil {
		r, _, _ := procCallNextHookEx.Call(0, uintptr(code), wParam, lParam)
		return r
	}
	k := *(**kbdLLHookStruct)(unsafe.Pointer(&lParam)) // lParam points at the system's KBDLLHOOKSTRUCT
	down := wParam == wmKeyDown || wParam == wmSysKeyDown
	vk := sidedVK(uint16(k.VkCode), k.ScanCode, k.Flags&llkhfExtended != 0)
	var held modKeys
	for _, m := range modifierVKs {
		if asyncDown(m) {
			held = held.with(m, true)
		}
	}
	h.mu.Lock()
	if k.Flags&llkhfInjected == 0 {
		h.logic.prune(vk, asyncDown)
	}
	out := h.logic.handle(rawKey{vk: vk, scan: k.ScanCode, flags: k.Flags, down: down, held: held}, time.Now())
	h.mu.Unlock()
	if out.mask {
		// An unassigned key between the Alt or Win down and up: its release
		// then opens no menu.
		_ = sendInputs([]input{keyInput(vkMask, false), keyInput(vkMask, true)})
	}
	h.emit(out.events)
	if out.swallow {
		return 1
	}
	r, _, _ := procCallNextHookEx.Call(0, uintptr(code), wParam, lParam)
	return r
}

func mouseHook(code int, wParam, lParam uintptr) uintptr {
	if h := currentHooks(); code >= 0 && h != nil {
		switch wParam {
		case wmLButtonDown, wmRButtonDown, wmMButtonDown, wmXButtonDown, wmMouseWheel, wmMouseHWheel:
			h.mu.Lock()
			h.logic.cancelModifierTap()
			h.mu.Unlock()
		}
	}
	r, _, _ := procCallNextHookEx.Call(0, uintptr(code), wParam, lParam)
	return r
}
