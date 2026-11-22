//go:build windows

package localengine

import "golang.org/x/sys/windows"

// writeRawStdout writes to the process stdout handle, as a native DLL whose C
// runtime starts after the guard does.
func writeRawStdout(s string) {
	h, err := windows.GetStdHandle(windows.STD_OUTPUT_HANDLE)
	if err != nil {
		return
	}
	var n uint32
	_ = windows.WriteFile(h, []byte(s), &n, nil)
}
