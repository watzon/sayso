//go:build !windows

package engine

import "os/exec"

// hideConsoleWindow does nothing outside Windows: there a child has no
// console window of its own.
func hideConsoleWindow(*exec.Cmd) {}
