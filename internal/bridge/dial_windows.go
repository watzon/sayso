//go:build windows

package bridge

import (
	"net"
	"time"

	"github.com/Microsoft/go-winio"
)

// DialHost connects to the Host API at path, as an app does.
func DialHost(path string) (net.Conn, error) {
	timeout := 5 * time.Second
	return winio.DialPipe(path, &timeout)
}

func dialHost(path string) (net.Conn, error) { return DialHost(path) }
