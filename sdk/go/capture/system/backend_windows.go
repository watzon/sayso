package system

import "github.com/watzon/sayso/sdk/go/capture/internal/device"

func loopback() device.Backend { return device.NewLoopback() }
