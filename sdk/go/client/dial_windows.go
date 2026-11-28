//go:build windows

package client

import (
	"context"
	"net"

	"github.com/Microsoft/go-winio"
)

func dial(ctx context.Context, pipe string) (net.Conn, error) {
	return winio.DialPipeContext(ctx, pipe)
}
