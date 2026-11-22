//go:build unix

package localengine

import (
	"os"

	"golang.org/x/sys/unix"
)

// protocolStdout is the protocol stream: a duplicate of the real stdout.
// Descriptor 1 then points at stderr, so a stray print from a native library
// cannot corrupt the stream. It must run before any library loads.
func protocolStdout() *os.File {
	fd, err := unix.Dup(1)
	if err != nil {
		return os.Stdout
	}
	if err := unix.Dup2(2, 1); err != nil {
		unix.Close(fd)
		return os.Stdout
	}
	unix.CloseOnExec(fd)
	os.Stdout = os.Stderr
	return os.NewFile(uintptr(fd), "protocol-stdout")
}
