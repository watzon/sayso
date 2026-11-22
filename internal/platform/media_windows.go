package platform

import (
	"errors"
	"fmt"
	"runtime"
	"sync"
	"time"
	"unsafe"

	"golang.org/x/sys/windows"
)

// windowsMedia pauses the media sessions that play (Global System Media
// Transport Controls, as media.rs does) and resumes the ones that are still
// paused afterwards. A session that the user stopped, or that plays again,
// stays as it is. WinRT objects live on one COM thread of their own, which
// keeps the paused sessions between Pause and Resume.

const (
	gsmtcClass = "Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager"

	statusPlaying = 4
	statusPaused  = 5

	asyncStarted   = 0
	asyncCompleted = 1
	asyncTimeout   = 3 * time.Second

	slotRequestAsync    = 6
	slotGetSessions     = 7
	slotGetPlaybackInfo = 9
	slotTryPlayAsync    = 10
	slotTryPauseAsync   = 11
	slotInfoControls    = 6
	slotInfoStatus      = 7
	slotIsPauseEnabled  = 7
	slotAsyncGetResults = 8
	slotAsyncInfoStatus = 7
	slotVectorViewGetAt = 6
	slotVectorViewSize  = 7
)

var (
	iidGSMTCManagerStatics = mustGUID("{2050c4ee-11a0-57de-aed7-c97c70338245}")
	iidIAsyncInfo          = mustGUID("{00000036-0000-0000-C000-000000000046}")
)

type windowsMedia struct {
	once  sync.Once
	tasks chan func()
	// paused is only touched on the media thread.
	paused []comObject
}

// run runs f on the media thread and waits for it.
func (m *windowsMedia) run(f func() error) error {
	m.once.Do(func() {
		m.tasks = make(chan func())
		ready := make(chan struct{})
		go func() {
			runtime.LockOSThread()
			done, err := comInit()
			close(ready)
			if err == nil {
				defer done()
			}
			for t := range m.tasks {
				t()
			}
		}()
		<-ready
	})
	errc := make(chan error, 1)
	m.tasks <- func() { errc <- f() }
	return <-errc
}

func hstring(s string) (uintptr, error) {
	u, err := windows.UTF16FromString(s)
	if err != nil {
		return 0, err
	}
	var h uintptr
	r, _, _ := procWindowsCreateString.Call(uintptr(unsafe.Pointer(&u[0])), uintptr(len(u)-1), uintptr(unsafe.Pointer(&h)))
	if int32(r) < 0 {
		return 0, fmt.Errorf("WindowsCreateString: HRESULT %#x", uint32(r))
	}
	return h, nil
}

// await waits for a WinRT async operation and returns its result (a pointer,
// or a bool in the low byte).
func await(op comObject) (uintptr, error) {
	defer op.release()
	info, err := op.query(&iidIAsyncInfo)
	if err != nil {
		return 0, err
	}
	defer info.release()
	deadline := time.Now().Add(asyncTimeout)
	for {
		var status int32
		if err := info.hr("get_Status", slotAsyncInfoStatus, uintptr(unsafe.Pointer(&status))); err != nil {
			return 0, err
		}
		if status == asyncCompleted {
			break
		}
		if status != asyncStarted {
			return 0, fmt.Errorf("the media request ended with status %d", status)
		}
		if time.Now().After(deadline) {
			return 0, errors.New("the media request timed out")
		}
		time.Sleep(5 * time.Millisecond)
	}
	var result uintptr
	err = op.hr("GetResults", slotAsyncGetResults, uintptr(unsafe.Pointer(&result)))
	return result, err
}

func gsmtcSessions() ([]comObject, error) {
	cls, err := hstring(gsmtcClass)
	if err != nil {
		return nil, err
	}
	defer procWindowsDeleteString.Call(cls)
	var statics comObject
	if r, _, _ := procRoGetActivationFactory.Call(cls, uintptr(unsafe.Pointer(&iidGSMTCManagerStatics)), uintptr(unsafe.Pointer(&statics))); int32(r) < 0 {
		return nil, fmt.Errorf("the media controls are not available: HRESULT %#x", uint32(r))
	}
	defer statics.release()
	var op comObject
	if err := statics.hr("RequestAsync", slotRequestAsync, uintptr(unsafe.Pointer(&op))); err != nil {
		return nil, err
	}
	mgr, err := await(op)
	if err != nil {
		return nil, err
	}
	manager := comObject(mgr)
	defer manager.release()
	var view comObject
	if err := manager.hr("GetSessions", slotGetSessions, uintptr(unsafe.Pointer(&view))); err != nil {
		return nil, err
	}
	defer view.release()
	var n uint32
	if err := view.hr("Size", slotVectorViewSize, uintptr(unsafe.Pointer(&n))); err != nil {
		return nil, err
	}
	var out []comObject
	for i := uint32(0); i < n; i++ {
		var s comObject
		if view.hr("GetAt", slotVectorViewGetAt, uintptr(i), uintptr(unsafe.Pointer(&s))) == nil && s != 0 {
			out = append(out, s)
		}
	}
	return out, nil
}

// sessionState is the playback status and whether pause is allowed.
func sessionState(s comObject) (status int32, canPause bool) {
	var info comObject
	if s.hr("GetPlaybackInfo", slotGetPlaybackInfo, uintptr(unsafe.Pointer(&info))) != nil {
		return -1, false
	}
	defer info.release()
	status = -1
	info.call(slotInfoStatus, uintptr(unsafe.Pointer(&status)))
	var controls comObject
	if info.hr("Controls", slotInfoControls, uintptr(unsafe.Pointer(&controls))) == nil {
		var b byte
		controls.call(slotIsPauseEnabled, uintptr(unsafe.Pointer(&b)))
		canPause = b != 0
		controls.release()
	}
	return status, canPause
}

func trySession(s comObject, slot int) bool {
	var op comObject
	if s.hr("Try", slot, uintptr(unsafe.Pointer(&op))) != nil {
		return false
	}
	r, err := await(op)
	return err == nil && byte(r) != 0
}

func (m *windowsMedia) Pause() (bool, error) {
	var any bool
	err := m.run(func() error {
		sessions, err := gsmtcSessions()
		if err != nil {
			return err
		}
		for _, s := range sessions {
			if st, can := sessionState(s); st == statusPlaying && can && trySession(s, slotTryPauseAsync) {
				m.paused = append(m.paused, s)
				any = true
				continue
			}
			s.release()
		}
		return nil
	})
	return any, err
}

func (m *windowsMedia) Resume() error {
	return m.run(func() error {
		var failed int
		for _, s := range m.paused {
			// A closed session gives an error here and stays closed.
			if st, _ := sessionState(s); st == statusPaused && !trySession(s, slotTryPlayAsync) {
				failed++
			}
			s.release()
		}
		m.paused = nil
		if failed > 0 {
			return fmt.Errorf("%d media sessions did not play again", failed)
		}
		return nil
	})
}
