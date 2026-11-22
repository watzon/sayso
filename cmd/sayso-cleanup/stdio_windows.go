//go:build windows

package main

import (
	"io"
	"os"

	"golang.org/x/sys/windows"
)

// protocolWriter is a private duplicate of the real standard output handle for
// the protocol. Afterwards the standard output handle is the one of stderr.
// The llama.cpp DLLs load later and take their C runtime's descriptor 1 from
// that handle, so their prints go to stderr. It falls back to the plain stdout
// when the duplicate fails.
func protocolWriter() io.Writer {
	proc := windows.CurrentProcess()
	var dup windows.Handle
	if err := windows.DuplicateHandle(proc, windows.Handle(os.Stdout.Fd()), proc, &dup, 0, false, windows.DUPLICATE_SAME_ACCESS); err != nil {
		return os.Stdout
	}
	if stderr, err := windows.GetStdHandle(windows.STD_ERROR_HANDLE); err == nil && stderr != 0 && stderr != windows.InvalidHandle {
		_ = windows.SetStdHandle(windows.STD_OUTPUT_HANDLE, stderr)
	}
	return os.NewFile(uintptr(dup), "protocol")
}
