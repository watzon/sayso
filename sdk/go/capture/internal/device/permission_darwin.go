package device

import (
	"time"

	"github.com/ebitengine/purego/objc"
)

// AVAuthorizationStatus values.
type micAuthorization int

const (
	micNotDetermined micAuthorization = 0
	micRestricted    micAuthorization = 1
	micDenied        micAuthorization = 2
	micAuthorized    micAuthorization = 3
)

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

func stateOf(a micAuthorization) State {
	switch a {
	case micAuthorized:
		return Granted
	case micDenied, micRestricted:
		return Denied
	}
	return Unknown
}

func permission() State { return stateOf(microphoneAuthorization()) }

// hasUsageDescription: macOS kills a process that asks for the microphone
// without NSMicrophoneUsageDescription in its Info.plist. A program with no
// bundle has its permission from the app that started it (the terminal).
func hasUsageDescription() bool {
	var ok bool
	withPool(func() {
		bundle := class("NSBundle").Send(sel("mainBundle"))
		ok = bundle != 0 && bundle.Send(sel("objectForInfoDictionaryKey:"), nsString("NSMicrophoneUsageDescription")) != 0
	})
	return ok
}

func requestPermission() State {
	if microphoneAuthorization() != micNotDetermined || !hasUsageDescription() {
		return permission()
	}
	answer := make(chan bool, 1)
	withPool(func() {
		var handler objc.Block
		handler = objc.NewBlock(func(_ objc.Block, granted bool) {
			defer handler.Release()
			answer <- granted
		})
		class("AVCaptureDevice").Send(sel("requestAccessForMediaType:completionHandler:"), avMediaTypeAudio, handler)
	})
	select {
	case <-answer:
	case <-time.After(10 * time.Minute):
	}
	return permission()
}
