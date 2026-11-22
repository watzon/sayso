//go:build unix

package localengine

import "golang.org/x/sys/unix"

// writeRawStdout writes to descriptor 1, as a native library does.
func writeRawStdout(s string) { _, _ = unix.Write(1, []byte(s)) }
