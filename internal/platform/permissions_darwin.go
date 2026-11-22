package platform

// Microphone (AVFoundation), Accessibility, and Input Monitoring, plus the
// System Settings deep links. Ported from sayso-platform-macos/src/permissions.rs.
//
// macOS gives no way to tell "denied" from "not asked yet" for Accessibility
// and Input Monitoring, so both read as false until granted.

import (
	"errors"
	"fmt"
	"os/exec"

	"github.com/ebitengine/purego/objc"
)

type micAuthorization int

// AVAuthorizationStatus values.
const (
	micNotDetermined micAuthorization = 0
	micRestricted    micAuthorization = 1
	micDenied        micAuthorization = 2
	micAuthorized    micAuthorization = 3
)

// Settings kinds for OpenSettings.
const (
	settingsMicrophone      = "microphone"
	settingsAccessibility   = "accessibility"
	settingsInputMonitoring = "input_monitoring"
)

func macSettingsURL(kind string) (string, error) {
	const base = "x-apple.systempreferences:com.apple.preference.security?"
	switch kind {
	case settingsMicrophone:
		return base + "Privacy_Microphone", nil
	case settingsAccessibility:
		return base + "Privacy_Accessibility", nil
	case settingsInputMonitoring, "input-monitoring", "inputmonitoring":
		return base + "Privacy_ListenEvent", nil
	}
	return "", fmt.Errorf("unknown settings pane %q", kind)
}

type macPermissions struct{}

func microphoneAuthorization() micAuthorization {
	if loadDarwin() != nil || avMediaTypeAudio == 0 {
		return micNotDetermined
	}
	var status int64
	withPool(func() {
		status = objc.Send[int64](class("AVCaptureDevice"), sel("authorizationStatusForMediaType:"), avMediaTypeAudio)
	})
	return micAuthorization(status)
}

func (macPermissions) Status() PermissionStatus {
	if loadDarwin() != nil {
		return PermissionStatus{NeedsInputPerms: true}
	}
	return PermissionStatus{
		Microphone:      microphoneAuthorization() == micAuthorized,
		Accessibility:   axIsProcessTrusted(),
		InputMonitoring: cgPreflightListenEventAccess(),
		NeedsInputPerms: true,
	}
}

// hasMicrophoneUsageDescription: macOS kills a process that asks for the
// microphone without NSMicrophoneUsageDescription in its Info.plist.
func hasMicrophoneUsageDescription() bool {
	var ok bool
	withPool(func() {
		bundle := class("NSBundle").Send(sel("mainBundle"))
		ok = bundle != 0 && bundle.Send(sel("objectForInfoDictionaryKey:"), nsString("NSMicrophoneUsageDescription")) != 0
	})
	return ok
}

func (p macPermissions) RequestMicrophone() error {
	if err := loadDarwin(); err != nil {
		return err
	}
	switch microphoneAuthorization() {
	case micAuthorized:
		return nil
	case micDenied, micRestricted:
		return p.OpenSettings(settingsMicrophone)
	}
	if !hasMicrophoneUsageDescription() {
		return errors.New("microphone: this program has no NSMicrophoneUsageDescription, so macOS would stop it if it asked")
	}
	withPool(func() {
		var handler objc.Block
		handler = objc.NewBlock(func(_ objc.Block, granted bool) {
			defer handler.Release()
		})
		class("AVCaptureDevice").Send(sel("requestAccessForMediaType:completionHandler:"), avMediaTypeAudio, handler)
	})
	return nil
}

func (macPermissions) RequestAccessibility() error {
	if err := loadDarwin(); err != nil {
		return err
	}
	options := cfBoolDict("AXTrustedCheckOptionPrompt")
	defer cfRelease(options)
	axIsProcessTrustedWithOptions(options)
	return nil
}

func (macPermissions) RequestInputMonitoring() error {
	if err := loadDarwin(); err != nil {
		return err
	}
	cgRequestListenEventAccess()
	return nil
}

func (macPermissions) OpenSettings(kind string) error {
	url, err := macSettingsURL(kind)
	if err != nil {
		return err
	}
	return exec.Command("/usr/bin/open", url).Run()
}
