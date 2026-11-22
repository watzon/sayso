package platform

// Launch at login with SMAppService.mainAppService (macOS 13 and later).
// Ported from sayso-platform-macos/src/login_item.rs.
//
// Only a bundled app can register. For an unbundled binary the status is
// NotFound, which reads as disabled, and registering returns an error.

import (
	"errors"
	"unsafe"

	"github.com/ebitengine/purego/objc"
)

// SMAppServiceStatus values.
const (
	smNotRegistered    = 0
	smEnabled          = 1
	smRequiresApproval = 2
	smNotFound         = 3
)

type macLoginItem struct{}

func mainAppService() objc.ID {
	return class("SMAppService").Send(sel("mainAppService"))
}

func loginStatus() int64 {
	var status int64
	withPool(func() {
		if svc := mainAppService(); svc != 0 {
			status = objc.Send[int64](svc, sel("status"))
		} else {
			status = smNotFound
		}
	})
	return status
}

// Enabled is true only for Enabled. RequiresApproval means that the user
// must allow Sayso in System Settings > General > Login Items first.
func (macLoginItem) Enabled() (bool, error) {
	if err := loadDarwin(); err != nil {
		return false, err
	}
	return loginStatus() == smEnabled, nil
}

func (macLoginItem) SetEnabled(enabled bool) error {
	if err := loadDarwin(); err != nil {
		return err
	}
	var ok bool
	var message string
	withPool(func() {
		svc := mainAppService()
		if svc == 0 {
			message = "SMAppService is not available"
			return
		}
		var nsErr objc.ID
		name := "unregisterAndReturnError:"
		if enabled {
			name = "registerAndReturnError:"
		}
		ok = objc.Send[bool](svc, sel(name), unsafe.Pointer(&nsErr))
		if !ok && nsErr != 0 {
			message = goStringFromCF(uintptr(nsErr.Send(sel("localizedDescription"))))
		}
	})
	if ok {
		return nil
	}
	// Registering twice, or unregistering what is not registered, is not a failure.
	status := loginStatus()
	if enabled && (status == smEnabled || status == smRequiresApproval) || !enabled && status != smEnabled && status != smRequiresApproval {
		return nil
	}
	if message == "" {
		message = "the login item call failed"
	}
	return errors.New("login item: " + message)
}
