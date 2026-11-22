//go:build darwin

package bridge

import (
	"io"
	"net"
)

func passCredentials(*net.UnixListener) error { return nil }

// readerOf reads the socket as it is; the peer is the audit token of its
// last writer.
func readerOf(c *net.UnixConn) (io.Reader, func() (Peer, error)) {
	return c, func() (Peer, error) { return peerOf(c) }
}
