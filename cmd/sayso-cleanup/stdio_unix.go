//go:build unix

package main

import (
	"io"
	"os"
	"syscall"

	"golang.org/x/sys/unix"
)

// protocolWriter is a private duplicate of the real stdout for the protocol.
// Afterwards descriptor 1 goes to stderr, so a print from llama.cpp, ggml, or
// Metal cannot corrupt the stream. It falls back to the plain stdout when the
// duplicate fails.
func protocolWriter() io.Writer {
	fd, err := syscall.Dup(1)
	if err != nil {
		return os.Stdout
	}
	syscall.CloseOnExec(fd)
	// If this fails, stdout stays as it was; the protocol still works.
	_ = unix.Dup2(2, 1)
	return os.NewFile(uintptr(fd), "protocol")
}
