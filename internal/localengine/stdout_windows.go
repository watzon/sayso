//go:build windows

package localengine

import (
	"os"

	"golang.org/x/sys/windows"
)

// protocolStdout is the protocol stream: a duplicate of the real stdout
// handle. The process stdout handle then points at stderr, so a native DLL
// whose C runtime starts later (sherpa-onnx, ONNX Runtime, whisper.cpp) writes
// its prints to stderr. It must run before any DLL loads.
func protocolStdout() *os.File {
	out, err := windows.GetStdHandle(windows.STD_OUTPUT_HANDLE)
	if err != nil || out == 0 || out == windows.InvalidHandle {
		return os.Stdout
	}
	stderr, err := windows.GetStdHandle(windows.STD_ERROR_HANDLE)
	if err != nil || stderr == 0 || stderr == windows.InvalidHandle {
		return os.Stdout
	}
	self := windows.CurrentProcess()
	var protocol windows.Handle
	if err := windows.DuplicateHandle(self, out, self, &protocol, 0, false, windows.DUPLICATE_SAME_ACCESS); err != nil {
		return os.Stdout
	}
	if err := windows.SetStdHandle(windows.STD_OUTPUT_HANDLE, stderr); err != nil {
		windows.CloseHandle(protocol)
		return os.Stdout
	}
	os.Stdout = os.Stderr
	return os.NewFile(uintptr(protocol), "protocol-stdout")
}
