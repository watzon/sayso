package platform

// The frontmost app (NSWorkspace) and the host of the page in its focused
// window. Ported from sayso-platform-macos/src/context.rs.
//
// The page address is the AXDocument attribute of the focused window, the
// same for every app: Safari and the Chromium browsers (Chrome, Arc, Brave,
// Edge, Dia) answer it; Electron apps answer an empty string. It reads one
// attribute on the app and one on the window with a 0.25 s Accessibility
// timeout, and never walks the tree (a walk took 18 s on Mail). It needs
// Accessibility; without it Site stays empty.

import (
	"errors"
	"strings"

	"github.com/ebitengine/purego/objc"
)

// axTimeoutSeconds is how long one Accessibility call may wait for the app.
const axTimeoutSeconds = 0.25

type macContext struct{}

func (macContext) ActiveApp() (ActiveApp, error) {
	if err := loadDarwin(); err != nil {
		return ActiveApp{}, err
	}
	var app ActiveApp
	var pid int32
	found := false
	withPool(func() {
		front := class("NSWorkspace").Send(sel("sharedWorkspace")).Send(sel("frontmostApplication"))
		if front == 0 {
			return
		}
		found = true
		app.BundleID = goStringFromCF(uintptr(front.Send(sel("bundleIdentifier"))))
		app.Name = goStringFromCF(uintptr(front.Send(sel("localizedName"))))
		pid = objc.Send[int32](front, sel("processIdentifier"))
	})
	if !found {
		return ActiveApp{}, errors.New("no frontmost app")
	}
	if axIsProcessTrusted() {
		if url := focusedWindowDocument(pid); url != "" {
			app.Site = hostOf(url)
		}
	}
	return app, nil
}

// axCopyAttribute copies one attribute. The caller releases the result.
func axCopyAttribute(element uintptr, name string) uintptr {
	attr := cfString(name)
	defer cfRelease(attr)
	var value uintptr
	if axUIElementCopyAttributeValue(element, attr, &value) != kAXErrorSuccess {
		return 0
	}
	return value
}

// focusedWindowDocument is the AXDocument of the focused window of pid.
func focusedWindowDocument(pid int32) string {
	if pid <= 0 {
		return ""
	}
	app := axUIElementCreateApplication(pid)
	if app == 0 {
		return ""
	}
	defer cfRelease(app)
	axUIElementSetMessagingTimeout(app, axTimeoutSeconds)
	window := axCopyAttribute(app, "AXFocusedWindow")
	if window == 0 {
		return ""
	}
	defer cfRelease(window)
	doc := axCopyAttribute(window, "AXDocument")
	if doc == 0 {
		return ""
	}
	defer cfRelease(doc)
	if cfGetTypeID(doc) != cfStringGetTypeID() {
		return ""
	}
	return goStringFromCF(doc)
}

// hostOf is the host of an http or https address, lowercase and without
// "www.", as sayso-core's style_rules::host_of. Empty when it is not a site.
func hostOf(url string) string {
	rest := strings.TrimSpace(url)
	if scheme, after, ok := strings.Cut(rest, "://"); ok {
		if !strings.EqualFold(scheme, "http") && !strings.EqualFold(scheme, "https") {
			return ""
		}
		rest = after
	}
	if i := strings.IndexAny(rest, "?#"); i >= 0 {
		rest = rest[:i]
	}
	if i := strings.IndexByte(rest, '/'); i >= 0 {
		rest = rest[:i]
	}
	if i := strings.LastIndexByte(rest, '@'); i >= 0 {
		rest = rest[i+1:]
	}
	if i := strings.IndexByte(rest, ':'); i >= 0 {
		rest = rest[:i]
	}
	host := strings.TrimPrefix(strings.ToLower(rest), "www.")
	if !strings.Contains(host, ".") || strings.HasPrefix(host, ".") || strings.HasSuffix(host, ".") {
		return ""
	}
	for _, c := range host {
		if !(c >= 'a' && c <= 'z' || c >= '0' && c <= '9' || c == '.' || c == '-') {
			return ""
		}
	}
	return host
}
