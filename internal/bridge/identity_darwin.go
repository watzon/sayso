//go:build darwin

package bridge

import (
	"errors"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"unsafe"

	"github.com/ebitengine/purego"
	"howett.net/plist"
)

// The code signature of the peer, with Security.framework through purego
// (no cgo), as identity.rs does with security-framework.

const (
	errSecCSUnsigned             = -67062
	kSecCSSigningInformation     = 1 << 1
	kSecCSDoNotValidateResources = 1 << 2
	solLocal                     = 0
	localPeerToken               = 6
	procPidPathInfoMaxSize       = 4 * 1024
	kCFStringEncodingUTF8        = 0x08000100
)

var sec struct {
	once sync.Once
	err  error

	getsockopt          func(fd int32, level, name int32, value unsafe.Pointer, size *uint32) int32
	procPidpathToken    func(token *[8]uint32, buffer unsafe.Pointer, size uint32) int32
	release             func(uintptr)
	dataCreate          func(alloc uintptr, bytes unsafe.Pointer, length int) uintptr
	dictCreate          func(alloc uintptr, keys, values unsafe.Pointer, n int, kcb, vcb uintptr) uintptr
	dictGet             func(dict, key uintptr) uintptr
	strCreate           func(alloc uintptr, s string, enc uint32) uintptr
	strGetCString       func(s uintptr, buf unsafe.Pointer, size int, enc uint32) bool
	strLength           func(s uintptr) int
	arrayCount          func(a uintptr) int
	arrayAt             func(a uintptr, i int) uintptr
	urlFromPath         func(alloc uintptr, path string, length int, dir bool) uintptr
	guestWithAttributes func(host, attrs uintptr, flags uint32, code *uintptr) int32
	requirementCreate   func(text uintptr, flags uint32, req *uintptr) int32
	codeCheck           func(code uintptr, flags uint32, req uintptr) int32
	staticCreate        func(url uintptr, flags uint32, code *uintptr) int32
	staticCheck         func(code uintptr, flags uint32, req uintptr) int32
	signingInfo         func(code uintptr, flags uint32, info *uintptr) int32
	subjectSummary      func(cert uintptr) uintptr

	keyCallBacks, valueCallBacks uintptr
	guestAudit, infoTeam, infoID uintptr
	infoCerts                    uintptr
}

func loadSec() error {
	sec.once.Do(func() {
		sys, err := purego.Dlopen("/usr/lib/libSystem.B.dylib", purego.RTLD_NOW|purego.RTLD_GLOBAL)
		if err != nil {
			sec.err = err
			return
		}
		cf, err := purego.Dlopen("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation", purego.RTLD_NOW|purego.RTLD_GLOBAL)
		if err != nil {
			sec.err = err
			return
		}
		sf, err := purego.Dlopen("/System/Library/Frameworks/Security.framework/Security", purego.RTLD_NOW|purego.RTLD_GLOBAL)
		if err != nil {
			sec.err = err
			return
		}
		purego.RegisterLibFunc(&sec.getsockopt, sys, "getsockopt")
		purego.RegisterLibFunc(&sec.procPidpathToken, sys, "proc_pidpath_audittoken")
		purego.RegisterLibFunc(&sec.release, cf, "CFRelease")
		purego.RegisterLibFunc(&sec.dataCreate, cf, "CFDataCreate")
		purego.RegisterLibFunc(&sec.dictCreate, cf, "CFDictionaryCreate")
		purego.RegisterLibFunc(&sec.dictGet, cf, "CFDictionaryGetValue")
		purego.RegisterLibFunc(&sec.strCreate, cf, "CFStringCreateWithCString")
		purego.RegisterLibFunc(&sec.strGetCString, cf, "CFStringGetCString")
		purego.RegisterLibFunc(&sec.strLength, cf, "CFStringGetLength")
		purego.RegisterLibFunc(&sec.arrayCount, cf, "CFArrayGetCount")
		purego.RegisterLibFunc(&sec.arrayAt, cf, "CFArrayGetValueAtIndex")
		purego.RegisterLibFunc(&sec.urlFromPath, cf, "CFURLCreateFromFileSystemRepresentation")
		purego.RegisterLibFunc(&sec.guestWithAttributes, sf, "SecCodeCopyGuestWithAttributes")
		purego.RegisterLibFunc(&sec.requirementCreate, sf, "SecRequirementCreateWithString")
		purego.RegisterLibFunc(&sec.codeCheck, sf, "SecCodeCheckValidity")
		purego.RegisterLibFunc(&sec.staticCreate, sf, "SecStaticCodeCreateWithPath")
		purego.RegisterLibFunc(&sec.staticCheck, sf, "SecStaticCodeCheckValidity")
		purego.RegisterLibFunc(&sec.signingInfo, sf, "SecCodeCopySigningInformation")
		purego.RegisterLibFunc(&sec.subjectSummary, sf, "SecCertificateCopySubjectSummary")
		sym := func(lib uintptr, name string) uintptr {
			p, e := purego.Dlsym(lib, name)
			if e != nil && sec.err == nil {
				sec.err = e
			}
			return p
		}
		deref := func(lib uintptr, name string) uintptr {
			p := sym(lib, name)
			if p == 0 {
				return 0
			}
			return **(**uintptr)(unsafe.Pointer(&p))
		}
		sec.keyCallBacks = sym(cf, "kCFTypeDictionaryKeyCallBacks")
		sec.valueCallBacks = sym(cf, "kCFTypeDictionaryValueCallBacks")
		sec.guestAudit = deref(sf, "kSecGuestAttributeAudit")
		sec.infoTeam = deref(sf, "kSecCodeInfoTeamIdentifier")
		sec.infoID = deref(sf, "kSecCodeInfoIdentifier")
		sec.infoCerts = deref(sf, "kSecCodeInfoCertificates")
	})
	return sec.err
}

func cfString(s string) uintptr { return sec.strCreate(0, s+"\x00", kCFStringEncodingUTF8) }

func goString(ref uintptr) (string, bool) {
	if ref == 0 {
		return "", false
	}
	size := sec.strLength(ref)*4 + 1
	buf := make([]byte, size)
	if !sec.strGetCString(ref, unsafe.Pointer(&buf[0]), size, kCFStringEncodingUTF8) {
		return "", false
	}
	if i := strings.IndexByte(string(buf), 0); i >= 0 {
		buf = buf[:i]
	}
	return string(buf), true
}

// peerOf reads the audit token of the last writer of a Unix socket
// (LOCAL_PEERTOKEN).
func peerOf(conn *net.UnixConn) (Peer, error) {
	if err := loadSec(); err != nil {
		return Peer{}, fmt.Errorf("no audit token: %v", err)
	}
	raw, err := conn.SyscallConn()
	if err != nil {
		return Peer{}, fmt.Errorf("no audit token: %v", err)
	}
	var token [8]uint32
	var result int32
	var errno error
	if cerr := raw.Control(func(fd uintptr) {
		size := uint32(unsafe.Sizeof(token))
		result = sec.getsockopt(int32(fd), solLocal, localPeerToken, unsafe.Pointer(&token), &size)
		if result != 0 {
			errno = errors.New("getsockopt failed")
		}
	}); cerr != nil {
		return Peer{}, fmt.Errorf("no audit token: %v", cerr)
	}
	if result != 0 {
		return Peer{}, fmt.Errorf("no audit token: %v", errno)
	}
	return Peer{PID: token[5], Token: token}, nil
}

func executableOf(token [8]uint32) (string, error) {
	pid := token[5]
	buf := make([]byte, procPidPathInfoMaxSize)
	n := sec.procPidpathToken(&token, unsafe.Pointer(&buf[0]), uint32(len(buf)))
	if n <= 0 {
		return "", fmt.Errorf("no path for pid %d", pid)
	}
	path, err := filepath.EvalSymlinks(string(buf[:n]))
	if err != nil {
		return "", fmt.Errorf("no file for pid %d: %v", pid, err)
	}
	if abs, err := filepath.Abs(path); err == nil {
		path = abs
	}
	return path, nil
}

func systemIdentify(peer Peer) (Caller, error) {
	if err := loadSec(); err != nil {
		return Caller{}, fmt.Errorf("cannot load Security.framework: %v", err)
	}
	executable, err := executableOf(peer.Token)
	if err != nil {
		return Caller{}, err
	}
	data := sec.dataCreate(0, unsafe.Pointer(&peer.Token[0]), int(unsafe.Sizeof(peer.Token)))
	defer sec.release(data)
	keys, values := []uintptr{sec.guestAudit}, []uintptr{data}
	attrs := sec.dictCreate(0, unsafe.Pointer(&keys[0]), unsafe.Pointer(&values[0]), 1, sec.keyCallBacks, sec.valueCallBacks)
	defer sec.release(attrs)
	var code uintptr
	if status := sec.guestWithAttributes(0, attrs, 0, &code); status != 0 || code == 0 {
		return Caller{}, fmt.Errorf("cannot find the code of %s: %d", executable, status)
	}
	defer sec.release(code)
	any, err := requirement("always")
	if err != nil {
		return Caller{}, err
	}
	defer sec.release(any)
	switch status := sec.codeCheck(code, 0, any); status {
	case 0:
	case errSecCSUnsigned:
		return CallerFromPath(executable), nil
	default:
		return Caller{}, fmt.Errorf("the signature of %s is not valid: %d", executable, status)
	}
	url := sec.urlFromPath(0, executable, len(executable), false)
	if url == 0 {
		return Caller{}, fmt.Errorf("no URL for %s", executable)
	}
	defer sec.release(url)
	var onDisk uintptr
	if status := sec.staticCreate(url, 0, &onDisk); status != 0 || onDisk == 0 {
		return Caller{}, fmt.Errorf("cannot read the file of %s: %d", executable, status)
	}
	defer sec.release(onDisk)
	if status := sec.staticCheck(onDisk, kSecCSDoNotValidateResources, any); status != 0 {
		return Caller{}, fmt.Errorf("the signature of %s is not valid: %d", executable, status)
	}
	var info uintptr
	if status := sec.signingInfo(code, kSecCSSigningInformation, &info); status != 0 || info == 0 {
		return Caller{}, fmt.Errorf("no signing information: %d", status)
	}
	defer sec.release(info)
	teamID, _ := goString(sec.dictGet(info, sec.infoTeam))
	identifier, hasID := goString(sec.dictGet(info, sec.infoID))
	if teamID == "" || !hasID {
		return CallerFromPath(executable), nil
	}
	if !signedBy(code, teamID, identifier) {
		return Caller{}, fmt.Errorf("%s says that it has the team %s and the identifier %s, and its signature does not prove it", executable, teamID, identifier)
	}
	var signer string
	if certs := sec.dictGet(info, sec.infoCerts); certs != 0 && sec.arrayCount(certs) > 0 {
		if summary := sec.subjectSummary(sec.arrayAt(certs, 0)); summary != 0 {
			if text, ok := goString(summary); ok {
				signer, _ = signerName(text, teamID)
			}
			sec.release(summary)
		}
	}
	return Caller{Name: displayName(executable), Interpreter: isInterpreter(executable), Identity: Identity{Kind: IdentitySigned, TeamID: teamID, Identifier: identifier}, Signer: signer}, nil
}

func requirement(text string) (uintptr, error) {
	s := cfString(text)
	defer sec.release(s)
	var req uintptr
	if status := sec.requirementCreate(s, 0, &req); status != 0 || req == 0 {
		return 0, fmt.Errorf("bad requirement %q: %d", text, status)
	}
	return req, nil
}

func signedBy(code uintptr, teamID, identifier string) bool {
	plain := func(id string) bool {
		if id == "" {
			return false
		}
		for _, c := range id {
			if !(c < 128 && (c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '.' || c == '-' || c == '_')) {
				return false
			}
		}
		return true
	}
	if !plain(teamID) || !plain(identifier) {
		return false
	}
	req, err := requirement(fmt.Sprintf("anchor apple generic and certificate leaf[subject.OU] = \"%s\" and identifier \"%s\"", teamID, identifier))
	if err != nil {
		return false
	}
	defer sec.release(req)
	return sec.codeCheck(code, 0, req) == 0
}

// displayName is the name of the app bundle that holds the executable, or
// its file name.
func displayName(executable string) string {
	for dir := filepath.Dir(executable); dir != "/" && dir != "."; dir = filepath.Dir(dir) {
		if filepath.Ext(dir) == ".app" {
			if name, ok := bundleName(dir); ok {
				return name
			}
			break
		}
	}
	return fileName(executable, false)
}

func bundleName(folder string) (string, bool) {
	if b, err := os.ReadFile(filepath.Join(folder, "Contents", "Info.plist")); err == nil {
		var info map[string]any
		if _, err := plist.Unmarshal(b, &info); err == nil {
			for _, key := range []string{"CFBundleDisplayName", "CFBundleName"} {
				if s, ok := info[key].(string); ok {
					if name, ok := CleanName(s); ok {
						return name, true
					}
				}
			}
		}
	}
	return CleanName(strings.TrimSuffix(filepath.Base(folder), filepath.Ext(folder)))
}

func systemName(executable string) string { return displayName(executable) }
