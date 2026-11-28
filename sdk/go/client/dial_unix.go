//go:build !windows

package client

import (
	"context"
	"net"
)

func dial(ctx context.Context, socket string) (net.Conn, error) {
	var d net.Dialer
	return d.DialContext(ctx, "unix", socket)
}
