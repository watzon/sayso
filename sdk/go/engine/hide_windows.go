//go:build windows

package engine

import (
	"os/exec"
	"syscall"
)

// createNoWindow keeps a console program from opening a window.
const createNoWindow = 0x08000000

func hideWindow(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{CreationFlags: createNoWindow}
}
