//go:build !windows

package cleanup

import "os/exec"

func hideWindow(*exec.Cmd) {}
