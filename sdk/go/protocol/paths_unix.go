//go:build !windows

package protocol

import "os"

func userID() (uint32, bool) { return uint32(os.Getuid()), true }
func userSID() string        { return "" }
