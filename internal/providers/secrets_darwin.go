//go:build darwin

package providers

import (
	"fmt"
	"log/slog"
	"sync"
	"unsafe"

	"github.com/ebitengine/purego"
)

// The legacy Keychain API, the one security-framework's generic password
// helpers (and so the keyring crate) use.
const errSecItemNotFound = -25300

var (
	secOnce sync.Once
	secErr  error

	secFind   func(keychain uintptr, svcLen uint32, svc string, accLen uint32, acc string, pwLen *uint32, pw *unsafe.Pointer, item *uintptr) int32
	secAdd    func(keychain uintptr, svcLen uint32, svc string, accLen uint32, acc string, pwLen uint32, pw unsafe.Pointer, item *uintptr) int32
	secModify func(item uintptr, attrs uintptr, length uint32, data unsafe.Pointer) int32
	secFree   func(attrs uintptr, data unsafe.Pointer) int32
	secDelete func(item uintptr) int32
	cfRelease func(ref uintptr)
)

func loadSecurity() error {
	secOnce.Do(func() {
		sec, err := purego.Dlopen("/System/Library/Frameworks/Security.framework/Security", purego.RTLD_NOW|purego.RTLD_GLOBAL)
		if err != nil {
			secErr = err
			return
		}
		cf, err := purego.Dlopen("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation", purego.RTLD_NOW|purego.RTLD_GLOBAL)
		if err != nil {
			secErr = err
			return
		}
		purego.RegisterLibFunc(&secFind, sec, "SecKeychainFindGenericPassword")
		purego.RegisterLibFunc(&secAdd, sec, "SecKeychainAddGenericPassword")
		purego.RegisterLibFunc(&secModify, sec, "SecKeychainItemModifyAttributesAndData")
		purego.RegisterLibFunc(&secFree, sec, "SecKeychainItemFreeContent")
		purego.RegisterLibFunc(&secDelete, sec, "SecKeychainItemDelete")
		purego.RegisterLibFunc(&cfRelease, cf, "CFRelease")
	})
	return secErr
}

func osStatus(op string, status int32) error {
	return &SecretError{Msg: fmt.Sprintf("%s: OSStatus %d", op, status)}
}

// find returns the secret and the item (release it), or ok=false.
func find(account string) (secret string, item uintptr, ok bool, err error) {
	if err := loadSecurity(); err != nil {
		return "", 0, false, &SecretError{Msg: err.Error()}
	}
	var n uint32
	var data unsafe.Pointer
	status := secFind(0, uint32(len(KeychainService)), KeychainService, uint32(len(account)), account, &n, &data, &item)
	if status == errSecItemNotFound {
		return "", 0, false, nil
	}
	if status != 0 {
		return "", 0, false, osStatus("find", status)
	}
	if data != nil {
		secret = string(unsafe.Slice((*byte)(data), n))
		secFree(0, data)
	}
	return secret, item, true, nil
}

func osGet(account string) (string, error) {
	secret, item, ok, err := find(account)
	if err != nil || !ok {
		return "", err
	}
	if item != 0 {
		cfRelease(item)
	}
	return secret, nil
}

func osSet(account, secret string) error {
	_, item, ok, err := find(account)
	if err != nil {
		return err
	}
	data := []byte(secret)
	ptr := unsafe.Pointer(unsafe.StringData(secret))
	if len(data) == 0 {
		ptr = nil
	}
	if ok {
		defer cfRelease(item)
		if status := secModify(item, 0, uint32(len(data)), ptr); status != 0 {
			return osStatus("modify", status)
		}
		return nil
	}
	var created uintptr
	if status := secAdd(0, uint32(len(KeychainService)), KeychainService, uint32(len(account)), account, uint32(len(data)), ptr, &created); status != 0 {
		return osStatus("add", status)
	}
	if created != 0 {
		cfRelease(created)
	}
	return nil
}

func osDelete(account string) error {
	_, item, ok, err := find(account)
	if err != nil || !ok {
		return err
	}
	defer cfRelease(item)
	if status := secDelete(item); status != 0 && status != errSecItemNotFound {
		return osStatus("delete", status)
	}
	return nil
}

func logSecretRead(account string, err error) {
	slog.Warn("could not read the Keychain item", "account", account, "error", err)
}
