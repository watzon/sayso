//go:build !darwin

// sayso-engine is the speech engine sidecar for Linux and Windows. It reads
// NDJSON requests on stdin and writes NDJSON replies and events on stdout
// (engine protocol v1, docs/engine-protocol.md).
package main

import (
	"os"

	"github.com/watzon/sayso/internal/localengine"
)

func main() {
	// os.Exit skips the C++ global destructors of the native libraries, which
	// can crash under a load or a final pass that still runs on another thread.
	os.Exit(localengine.Run())
}
