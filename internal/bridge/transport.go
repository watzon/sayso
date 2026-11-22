package bridge

import (
	"errors"
	"io"
)

// ErrAddrInUse is the error of a socket that another Sayso serves.
var ErrAddrInUse = errors.New("address in use")

// ErrPermission is the error of a socket folder that is not private.
var ErrPermission = errors.New("permission denied")

// conn is one connection of an app (transport/mod.rs).
type conn struct {
	reader io.Reader
	writer io.Writer
	// closer shuts the connection down in both directions. It can run more
	// than once.
	closer func()
	// release frees the connection after its last use, or is nil.
	release func()
	// peer gives the process that wrote the lines so far.
	peer func() (Peer, error)
}
