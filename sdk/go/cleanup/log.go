package cleanup

import (
	"errors"
	"log/slog"
)

// Log gets the log lines of downloads and the runtime. It discards them
// unless the app sets it.
var Log = slog.New(slog.DiscardHandler)

// ErrStopped is a request after the runtime stopped.
var ErrStopped = errors.New("cleanup runtime: stopped")
