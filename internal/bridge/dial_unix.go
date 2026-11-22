//go:build unix

package bridge

import (
	"net"
	"time"
)

// DialHost connects to the Host API at path, as an app does.
func DialHost(path string) (net.Conn, error) { return net.DialTimeout("unix", path, 5*time.Second) }

func dialHost(path string) (net.Conn, error) { return DialHost(path) }
