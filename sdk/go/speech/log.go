package speech

import "log/slog"

// Log gets the log lines of jobs and sessions. It discards them unless the
// app sets it.
var Log = slog.New(slog.DiscardHandler)
