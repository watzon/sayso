//go:build unix && !darwin && !linux

package bridge

import (
	"errors"
	"io"
	"net"
)

func passCredentials(*net.UnixListener) error { return nil }

func readerOf(c *net.UnixConn) (io.Reader, func() (Peer, error)) {
	return c, func() (Peer, error) { return Peer{}, nil }
}

func systemName(executable string) string { return fileName(executable, false) }

func systemIdentify(Peer) (Caller, error) {
	return Caller{}, errors.New("the host cannot find the peer on this system")
}
