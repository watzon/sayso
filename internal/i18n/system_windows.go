//go:build windows

package i18n

import (
	"syscall"
	"unsafe"
)

var getUserPreferredUILanguages = syscall.NewLazyDLL("kernel32.dll").NewProc("GetUserPreferredUILanguages")

const muiLanguageName = 0x8

// platformLocales reads the preferred UI languages of the user.
func platformLocales() []string {
	var count, size uint32
	r, _, _ := getUserPreferredUILanguages.Call(muiLanguageName, uintptr(unsafe.Pointer(&count)), 0, uintptr(unsafe.Pointer(&size)))
	if r == 0 || size == 0 {
		return nil
	}
	buf := make([]uint16, size)
	r, _, _ = getUserPreferredUILanguages.Call(muiLanguageName, uintptr(unsafe.Pointer(&count)), uintptr(unsafe.Pointer(&buf[0])), uintptr(unsafe.Pointer(&size)))
	if r == 0 {
		return nil
	}
	var out []string
	start := 0
	for i, c := range buf {
		if c == 0 {
			if i > start {
				out = append(out, syscall.UTF16ToString(buf[start:i]))
			}
			start = i + 1
		}
	}
	return out
}
