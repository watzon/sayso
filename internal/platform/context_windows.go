package platform

import (
	"errors"
	"path/filepath"
	"runtime"
	"strings"
	"time"
	"unsafe"

	"golang.org/x/sys/windows"
)

// windowsContext reports the app in front (context.rs of
// sayso-platform-windows): its id is the lowercase exe name, its name the
// description in the exe's version resource, and for a known browser the host
// of the page in its address bar, read with UI Automation.

type windowsContext struct{}

const frameHost = "applicationframehost.exe"

var (
	procEnumChildWindows = user32.NewProc("EnumChildWindows")
	procGetAncestor      = user32.NewProc("GetAncestor")
)

func windowPID(hwnd uintptr) uint32 {
	var pid uint32
	procGetWindowThreadProcessId.Call(hwnd, uintptr(unsafe.Pointer(&pid)))
	return pid
}

func processPath(pid uint32) string {
	h, err := windows.OpenProcess(windows.PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
	if err != nil {
		return ""
	}
	defer windows.CloseHandle(h)
	buf := make([]uint16, windows.MAX_LONG_PATH)
	n := uint32(len(buf))
	if err := windows.QueryFullProcessImageName(h, 0, &buf[0], &n); err != nil {
		return ""
	}
	return windows.UTF16ToString(buf[:n])
}

// appID is the lowercase file name of an exe.
func appID(path string) string { return strings.ToLower(filepath.Base(path)) }

// frameChild: a Store app's top window belongs to ApplicationFrameHost; the
// app is the process of a child window.
var frameChildCallback = windows.NewCallback(func(child, lparam uintptr) uintptr {
	s := *(**frameSearch)(unsafe.Pointer(&lparam))
	if pid := windowPID(child); pid != 0 && pid != s.host {
		s.found = pid
		return 0
	}
	return 1
})

type frameSearch struct{ host, found uint32 }

func windowProcess(hwnd uintptr) (uint32, string) {
	pid := windowPID(hwnd)
	if pid == 0 {
		return 0, ""
	}
	path := processPath(pid)
	if appID(path) == frameHost {
		s := &frameSearch{host: pid}
		procEnumChildWindows.Call(hwnd, frameChildCallback, uintptr(unsafe.Pointer(s)))
		runtime.KeepAlive(s)
		if s.found != 0 {
			if p := processPath(s.found); p != "" {
				return s.found, p
			}
		}
	}
	return pid, path
}

// displayName is the FileDescription of the exe, without a trailing ".exe",
// or the file name without .exe.
func displayName(path string) string {
	fallback := strings.TrimSuffix(filepath.Base(path), filepath.Ext(path))
	size, err := windows.GetFileVersionInfoSize(path, nil)
	if err != nil || size == 0 {
		return fallback
	}
	data := make([]byte, size)
	if err := windows.GetFileVersionInfo(path, 0, size, unsafe.Pointer(&data[0])); err != nil {
		return fallback
	}
	var lookups []string
	var tp unsafe.Pointer
	var tl uint32
	if windows.VerQueryValue(unsafe.Pointer(&data[0]), `\VarFileInfo\Translation`, unsafe.Pointer(&tp), &tl) == nil && tl >= 4 {
		pairs := unsafe.Slice((*uint16)(tp), tl/2)
		for i := 0; i+1 < len(pairs); i += 2 {
			lookups = append(lookups, hex4(pairs[i])+hex4(pairs[i+1]))
		}
	}
	lookups = append(lookups, "040904B0", "040904E4")
	for _, l := range lookups {
		var p unsafe.Pointer
		var n uint32
		if windows.VerQueryValue(unsafe.Pointer(&data[0]), `\StringFileInfo\`+l+`\FileDescription`, unsafe.Pointer(&p), &n) == nil && n > 0 {
			if d := withoutExe(windows.UTF16PtrToString((*uint16)(p))); d != "" {
				return d
			}
		}
	}
	return fallback
}

func hex4(v uint16) string {
	const digits = "0123456789ABCDEF"
	return string([]byte{digits[v>>12], digits[v>>8&15], digits[v>>4&15], digits[v&15]})
}

// withoutExe: some descriptions are just the file name.
func withoutExe(d string) string {
	d = strings.TrimSpace(d)
	if strings.HasSuffix(strings.ToLower(d), ".exe") {
		d = strings.TrimSpace(d[:len(d)-4])
	}
	return d
}

func (windowsContext) ActiveApp() (ActiveApp, error) {
	hwnd, _, _ := procGetForegroundWindow.Call()
	if hwnd == 0 {
		return ActiveApp{}, errors.New("no window is in front")
	}
	pid, path := windowProcess(hwnd)
	if path == "" {
		return ActiveApp{}, errors.New("the app in front cannot be read")
	}
	app := ActiveApp{BundleID: appID(path), Name: displayName(path)}
	if kind, ok := browserKindOf(app.BundleID); ok && windowPID(hwnd) == pid {
		// The find bar, a bubble, or a dialog is a window of its own; the
		// browser window owns it and has the address bar.
		const gaRootOwner = 3
		window := hwnd
		if owner, _, _ := procGetAncestor.Call(hwnd, gaRootOwner); owner != 0 {
			window = owner
		}
		if addr, ok := cleanAddress(addressBarText(window, kind)); ok {
			app.Site, _ = hostOf(addr)
		}
	}
	return app, nil
}

type browserKind int

const (
	// browserChromium: the address bar is the first Edit control.
	browserChromium browserKind = iota
	// browserFirefox: the address bar is the Edit with the id urlbar-input.
	browserFirefox
)

func browserKindOf(id string) (browserKind, bool) {
	switch strings.ToLower(id) {
	case "chrome.exe", "msedge.exe", "brave.exe", "vivaldi.exe", "opera.exe", "arc.exe", "chromium.exe", "thorium.exe":
		return browserChromium, true
	case "firefox.exe", "zen.exe", "librewolf.exe", "waterfox.exe":
		return browserFirefox, true
	}
	return 0, false
}

// UI Automation ids.
const (
	uiaControlTypeProperty  = 30003
	uiaAutomationIDProperty = 30011
	uiaValuePattern         = 10002
	uiaButton               = 50000
	uiaEdit                 = 50004
	uiaImage                = 50006
	uiaMenuBar              = 50010
	uiaMenuItem             = 50011
	uiaTab                  = 50018
	uiaText                 = 50020
	uiaDocument             = 50030
)

type walkStep int

const (
	walkTake walkStep = iota
	walkDescend
	walkSkip
)

// stepFor: a Document is a web page and a Tab the tab strip (hundreds of
// tabs); neither is entered. The other skipped types hold no address bar.
func stepFor(kind browserKind, controlType int32, automationID string) walkStep {
	if controlType == uiaEdit && (kind == browserChromium || automationID == "urlbar-input") {
		return walkTake
	}
	switch controlType {
	case uiaDocument, uiaTab, uiaEdit, uiaButton, uiaMenuBar, uiaMenuItem, uiaText, uiaImage:
		return walkSkip
	}
	return walkDescend
}

// cleanAddress: the trimmed address bar text, when it is not empty and has no
// white space (text with a space is a search being typed).
func cleanAddress(v string) (string, bool) {
	v = strings.TrimSpace(v)
	return v, v != "" && !strings.ContainsAny(v, " \t\r\n")
}

// hostOf is the host of a page address, lowercase and without www., as
// sayso-core's style rules use it.
func hostOf(url string) (string, bool) {
	url = strings.TrimSpace(url)
	rest := url
	if scheme, r, ok := strings.Cut(url, "://"); ok {
		if !strings.EqualFold(scheme, "http") && !strings.EqualFold(scheme, "https") {
			return "", false
		}
		rest = r
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
		return "", false
	}
	for _, c := range host {
		if !(c >= 'a' && c <= 'z' || c >= '0' && c <= '9' || c == '.' || c == '-') {
			return "", false
		}
	}
	return host, true
}

var (
	clsidCUIAutomation8 = mustGUID("{e22ad333-b25f-460c-83d0-0581107395c9}")
	iidIUIAutomation    = mustGUID("{30cbe57d-d9d0-452a-ab13-7ac5ac4825ee}")
	iidIUIAutomation2   = mustGUID("{34723aff-0c9d-49d0-9896-7ab52df8cd8a}")
	iidIUIAValuePattern = mustGUID("{a94cd8b1-0844-4cd6-9d2d-640537ab39e9}")
	uiaTimeoutMs        = uintptr(250)
	uiaWalkBudget       = 250 * time.Millisecond
	uiaWalkMaxControls  = 300
)

// Vtable slots, counted with IUnknown.
const (
	slotElementFromHandle      = 6
	slotControlViewWalker      = 14
	slotCreateCacheRequest     = 20
	slotSetConnectionTimeout   = 61
	slotSetTransactionTimeout  = 63
	slotAddProperty            = 3
	slotFirstChildBuildCache   = 10
	slotNextSiblingBuildCache  = 12
	slotGetCurrentPattern      = 16
	slotCachedControlType      = 53
	slotCachedAutomationID     = 61
	slotValuePatternCurrentVal = 4
)

// addressBarText is the text in the address bar of a browser window, after a
// walk of at most about 250 ms.
func addressBarText(hwnd uintptr, kind browserKind) string {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	done, err := comInit()
	if err != nil {
		return ""
	}
	defer done()
	deadline := time.Now().Add(uiaWalkBudget)
	auto, err := createInstance(&clsidCUIAutomation8, &iidIUIAutomation, clsctxInprocServer)
	if err != nil {
		return ""
	}
	defer auto.release()
	// Without timeouts a browser that hangs holds a call for many seconds.
	if a2, err := auto.query(&iidIUIAutomation2); err == nil {
		a2.call(slotSetConnectionTimeout, uiaTimeoutMs)
		a2.call(slotSetTransactionTimeout, uiaTimeoutMs)
		a2.release()
	}
	var cache, walker, root comObject
	if auto.hr("CreateCacheRequest", slotCreateCacheRequest, uintptr(unsafe.Pointer(&cache))) != nil {
		return ""
	}
	defer cache.release()
	cache.call(slotAddProperty, uiaControlTypeProperty)
	if kind == browserFirefox {
		cache.call(slotAddProperty, uiaAutomationIDProperty)
	}
	if auto.hr("ControlViewWalker", slotControlViewWalker, uintptr(unsafe.Pointer(&walker))) != nil {
		return ""
	}
	defer walker.release()
	if auto.hr("ElementFromHandle", slotElementFromHandle, hwnd, uintptr(unsafe.Pointer(&root))) != nil {
		return ""
	}
	defer root.release()
	bar := findAddressBar(walker, cache, root, kind, deadline)
	if bar == 0 {
		return ""
	}
	defer bar.release()
	var pattern comObject
	if bar.hr("GetCurrentPattern", slotGetCurrentPattern, uiaValuePattern, uintptr(unsafe.Pointer(&pattern))) != nil || pattern == 0 {
		return ""
	}
	defer pattern.release()
	vp, err := pattern.query(&iidIUIAValuePattern)
	if err != nil {
		return ""
	}
	defer vp.release()
	var value uintptr
	if vp.hr("CurrentValue", slotValuePatternCurrentVal, uintptr(unsafe.Pointer(&value))) != nil {
		return ""
	}
	return bstr(value)
}

// findAddressBar walks depth first, so it never touches a control after the
// address bar (the web page is after it). The caller releases the result.
func findAddressBar(walker, cache, root comObject, kind browserKind, deadline time.Time) comObject {
	step := func(slot int, of comObject) comObject {
		var out comObject
		if walker.hr("walk", slot, uintptr(of), uintptr(cache), uintptr(unsafe.Pointer(&out))) != nil {
			return 0
		}
		return out
	}
	var open []comObject
	defer func() {
		for _, o := range open {
			o.release()
		}
	}()
	current := step(slotFirstChildBuildCache, root)
	for seen := 0; ; {
		if time.Now().After(deadline) {
			current.release()
			return 0
		}
		if current == 0 {
			if len(open) == 0 {
				return 0
			}
			parent := open[len(open)-1]
			open = open[:len(open)-1]
			current = step(slotNextSiblingBuildCache, parent)
			parent.release()
			continue
		}
		if seen++; seen > uiaWalkMaxControls {
			current.release()
			return 0
		}
		var ct int32
		current.call(slotCachedControlType, uintptr(unsafe.Pointer(&ct)))
		id := ""
		if kind == browserFirefox {
			var b uintptr
			if current.hr("CachedAutomationId", slotCachedAutomationID, uintptr(unsafe.Pointer(&b))) == nil {
				id = bstr(b)
			}
		}
		switch stepFor(kind, ct, id) {
		case walkTake:
			return current
		case walkDescend:
			open = append(open, current)
			current = step(slotFirstChildBuildCache, current)
		default:
			next := step(slotNextSiblingBuildCache, current)
			current.release()
			current = next
		}
	}
}
