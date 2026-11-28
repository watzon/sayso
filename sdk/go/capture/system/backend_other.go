//go:build !windows && !linux

package system

import "github.com/watzon/sayso/sdk/go/capture/internal/device"

// The Core Audio process tap of macOS 14.2 is not in this version.
func loopback() device.Backend { return nil }
