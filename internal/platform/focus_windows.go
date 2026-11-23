package platform

// The focus tracker of Windows: the monitor of the focused control of the
// foreground window, else of the window.
//
// MyGo's Display.ID on Windows is the HMONITOR, and its DIP space has its
// own mapping for monitors of mixed scales, so the tracker names the
// monitor (MonitorFromWindow) instead of converting pixels. Watch hears
// foreground, focus and move-end events from SetWinEventHook on a thread
// of its own with a message loop.

import (
	"errors"
	"os"
	"runtime"
	"sync"
	"time"
	"unsafe"

	"golang.org/x/sys/windows"
)

var (
	procGetGUIThreadInfo  = user32.NewProc("GetGUIThreadInfo")
	procMonitorFromWindow = user32.NewProc("MonitorFromWindow")
	procGetWindowLongPtrW = user32.NewProc("GetWindowLongPtrW")
	procGetWindowTextW    = user32.NewProc("GetWindowTextW")
	procIsIconic          = user32.NewProc("IsIconic")
	procIsWindowVisible   = user32.NewProc("IsWindowVisible")
	procSetWinEventHook   = user32.NewProc("SetWinEventHook")
	procUnhookWinEvent    = user32.NewProc("UnhookWinEvent")
)

const (
	eventSystemForeground  = 0x0003
	eventSystemMoveSizeEnd = 0x000B
	eventObjectFocus       = 0x8005
	winEventOutOfContext   = 0x0000
	monitorDefaultToNull   = 0
	gwlExStyle             = -20
	wsExToolWindow         = 0x00000080
	wsExNoActivate         = 0x08000000
)

// guiThreadInfo is GUITHREADINFO.
type guiThreadInfo struct {
	Size      uint32
	Flags     uint32
	Active    uintptr
	Focus     uintptr
	Capture   uintptr
	MenuOwner uintptr
	MoveSize  uintptr
	Caret     uintptr
	CaretRect [4]int32
}

// shellClasses are the desktop and the taskbars: no focus for dictation.
var shellClasses = map[string]bool{
	"Progman": true, "WorkerW": true, "Shell_TrayWnd": true, "Shell_SecondaryTrayWnd": true, "NotifyIconOverflowWindow": true,
}

type windowsFocus struct{ self uint32 }

func newWindowsFocus() *windowsFocus { return &windowsFocus{self: uint32(os.Getpid())} }

func (f *windowsFocus) Focused() (FocusFrame, bool) {
	hwnd, _, _ := procGetForegroundWindow.Call()
	if hwnd == 0 || skipForeground(hwnd, windowPID(hwnd) == f.self) {
		return FocusFrame{}, false
	}
	target := hwnd
	tid, _ := windowThread(hwnd)
	gti := guiThreadInfo{Size: uint32(unsafe.Sizeof(guiThreadInfo{}))}
	if r, _, _ := procGetGUIThreadInfo.Call(uintptr(tid), uintptr(unsafe.Pointer(&gti))); r != 0 && gti.Focus != 0 {
		if v, _, _ := procIsWindowVisible.Call(gti.Focus); v != 0 {
			target = gti.Focus
		}
	}
	mon, _, _ := procMonitorFromWindow.Call(target, monitorDefaultToNull)
	if mon == 0 {
		return FocusFrame{}, false
	}
	return FocusFrame{Display: int64(mon)}, true
}

// skipForeground is true for a foreground window that holds no focus for
// dictation: the desktop, a taskbar, a minimized window, or Sayso's overlay
// and tray popover (tool windows that do not activate). Sayso's hub counts.
func skipForeground(hwnd uintptr, own bool) bool {
	var class [64]uint16
	n, _, _ := procGetClassNameW.Call(hwnd, uintptr(unsafe.Pointer(&class[0])), uintptr(len(class)))
	if shellClasses[windows.UTF16ToString(class[:n])] {
		return true
	}
	if r, _, _ := procIsIconic.Call(hwnd); r != 0 {
		return true
	}
	if !own {
		return false
	}
	index := gwlExStyle // a negative index: through a variable to wrap
	ex, _, _ := procGetWindowLongPtrW.Call(hwnd, uintptr(index))
	var title [64]uint16
	t, _, _ := procGetWindowTextW.Call(hwnd, uintptr(unsafe.Pointer(&title[0])), uintptr(len(title)))
	return ownFloating(ex, windows.UTF16ToString(title[:t]))
}

// ownFloating is true for Sayso's overlay or tray popover, by their
// extended style or title.
func ownFloating(exStyle uintptr, title string) bool {
	return title == overlayTitle || exStyle&wsExToolWindow != 0 || exStyle&wsExNoActivate != 0
}

func windowThread(hwnd uintptr) (tid, pid uint32) {
	r, _, _ := procGetWindowThreadProcessId.Call(hwnd, uintptr(unsafe.Pointer(&pid)))
	return uint32(r), pid
}

// The WinEvent callback is made once and calls the running Watch.
var (
	winEventCallback     uintptr
	winEventCallbackOnce sync.Once
	winEventTargetMu     sync.Mutex
	winEventTarget       func()
)

func (f *windowsFocus) Watch(changed func()) (func(), error) {
	winEventCallbackOnce.Do(func() {
		winEventCallback = windows.NewCallback(func(hook, event, hwnd, idObject, idChild, thread, at uintptr) uintptr {
			winEventTargetMu.Lock()
			fn := winEventTarget
			winEventTargetMu.Unlock()
			if fn != nil {
				fn()
			}
			return 0
		})
	})
	winEventTargetMu.Lock()
	winEventTarget = changed
	winEventTargetMu.Unlock()
	w := &winEventWatch{done: make(chan struct{})}
	started := make(chan error, 1)
	go w.loop(started)
	if err := <-started; err != nil {
		w.clearTarget()
		return nil, err
	}
	return w.stop, nil
}

type winEventWatch struct {
	mu       sync.Mutex
	threadID uint32
	done     chan struct{}
	once     sync.Once
}

func (w *winEventWatch) loop(started chan<- error) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	defer close(w.done)
	var m msg
	// Make the message queue before stop posts WM_QUIT to it.
	procPeekMessageW.Call(uintptr(unsafe.Pointer(&m)), 0, 0, 0, 0)
	w.mu.Lock()
	w.threadID = windows.GetCurrentThreadId()
	w.mu.Unlock()
	var hooks []uintptr
	for _, ev := range [][2]uintptr{{eventSystemForeground, eventSystemForeground}, {eventSystemMoveSizeEnd, eventSystemMoveSizeEnd}, {eventObjectFocus, eventObjectFocus}} {
		if h, _, _ := procSetWinEventHook.Call(ev[0], ev[1], 0, winEventCallback, 0, 0, winEventOutOfContext); h != 0 {
			hooks = append(hooks, h)
		}
	}
	defer func() {
		for _, h := range hooks {
			procUnhookWinEvent.Call(h)
		}
	}()
	if len(hooks) == 0 {
		started <- errors.New("SetWinEventHook failed")
		return
	}
	started <- nil
	for {
		r, _, _ := procGetMessageW.Call(uintptr(unsafe.Pointer(&m)), 0, 0, 0)
		if int32(r) <= 0 {
			return
		}
	}
}

func (w *winEventWatch) clearTarget() {
	winEventTargetMu.Lock()
	winEventTarget = nil
	winEventTargetMu.Unlock()
}

func (w *winEventWatch) stop() {
	w.once.Do(func() {
		w.clearTarget()
		w.mu.Lock()
		tid := w.threadID
		w.mu.Unlock()
		procPostThreadMessageW.Call(uintptr(tid), wmQuit, 0, 0)
		select {
		case <-w.done:
		case <-time.After(2 * time.Second):
		}
	})
}

var _ FocusTracker = (*windowsFocus)(nil)
