package system

import (
	"os/exec"

	"github.com/watzon/sayso/sdk/go/capture/internal/device"
)

func loopback() device.Backend {
	for _, tool := range []string{"parec", "pw-record"} {
		if _, err := exec.LookPath(tool); err == nil {
			return device.NewMonitor()
		}
	}
	return nil
}
