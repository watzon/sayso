package platform

// The Dock icon: NSApplication activation policy Regular (in the Dock) or
// Accessory (menu bar only). AppKit wants this on the main thread, so a call
// from another thread goes to the main queue.

import (
	"sync"

	"github.com/ebitengine/purego"
	"github.com/ebitengine/purego/objc"
)

const (
	nsActivationPolicyRegular   = 0
	nsActivationPolicyAccessory = 1
)

type macDock struct{}

var (
	dispatchOnce    sync.Once
	dispatchAsyncF  func(queue uintptr, ctx uintptr, fn uintptr)
	dispatchMainQ   uintptr
	dockCallback    uintptr
	pendingPolicyMu sync.Mutex
	pendingPolicy   int64
)

func setActivationPolicy(policy int64) {
	withPool(func() {
		app := class("NSApplication").Send(sel("sharedApplication"))
		objc.Send[bool](app, sel("setActivationPolicy:"), policy)
	})
}

func (macDock) SetShowInDock(show bool) error {
	if err := loadDarwin(); err != nil {
		return err
	}
	policy := int64(nsActivationPolicyAccessory)
	if show {
		policy = nsActivationPolicyRegular
	}
	var onMain bool
	withPool(func() { onMain = objc.Send[bool](class("NSThread"), sel("isMainThread")) })
	if onMain {
		setActivationPolicy(policy)
		return nil
	}
	dispatchOnce.Do(func() {
		lib := openLib("/usr/lib/libSystem.B.dylib")
		purego.RegisterLibFunc(&dispatchAsyncF, lib, "dispatch_async_f")
		// The main queue is the struct _dispatch_main_q itself.
		dispatchMainQ = symbol(lib, "_dispatch_main_q")
		dockCallback = purego.NewCallback(func(uintptr) uintptr {
			pendingPolicyMu.Lock()
			p := pendingPolicy
			pendingPolicyMu.Unlock()
			setActivationPolicy(p)
			return 0
		})
	})
	pendingPolicyMu.Lock()
	pendingPolicy = policy
	pendingPolicyMu.Unlock()
	dispatchAsyncF(dispatchMainQ, 0, dockCallback)
	return nil
}

// DeactivateApp gives the keyboard back to the app the user came from, as
// the Rust popover did when it closed. Call it on the main thread.
func DeactivateApp() {
	if loadDarwin() != nil {
		return
	}
	withPool(func() { class("NSApplication").Send(sel("sharedApplication")).Send(sel("deactivate")) })
}
