//go:build linux

package bridge

import (
	"io"
	"net"

	"golang.org/x/sys/unix"
)

// passCredentials asks the kernel for the credentials of each message
// (SO_PASSCRED). Accepted sockets inherit it.
func passCredentials(ln *net.UnixListener) error {
	raw, err := ln.SyscallConn()
	if err != nil {
		return err
	}
	var serr error
	if err := raw.Control(func(fd uintptr) { serr = unix.SetsockoptInt(int(fd), unix.SOL_SOCKET, unix.SO_PASSCRED, 1) }); err != nil {
		return err
	}
	return serr
}

// readerOf reads with the credentials of each writer; the peer is the one
// process that wrote, or an error when more than one did.
func readerOf(c *net.UnixConn) (io.Reader, func() (Peer, error)) {
	r := &credReader{conn: c}
	return r, r.writer
}
