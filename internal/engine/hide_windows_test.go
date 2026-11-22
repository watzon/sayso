//go:build windows

package engine

import (
	"os/exec"
	"testing"
)

// A GUI release build flashed a terminal window each time it started the
// engine or the cleanup runtime.
func TestHelpersStartWithoutAConsoleWindow(t *testing.T) {
	cmd := exec.Command("sayso-engine.exe")
	hideConsoleWindow(cmd)
	if cmd.SysProcAttr == nil || cmd.SysProcAttr.CreationFlags&createNoWindow == 0 {
		t.Fatalf("CREATE_NO_WINDOW is not set: %+v", cmd.SysProcAttr)
	}
}

func TestTheEngineProcessIsStartedWithoutAConsoleWindow(t *testing.T) {
	c, _ := startFake(t, nil)
	c.mu.Lock()
	p := c.proc
	c.mu.Unlock()
	if p == nil || p.cmd.SysProcAttr == nil || p.cmd.SysProcAttr.CreationFlags&createNoWindow == 0 {
		t.Fatal("the engine was started with a console window")
	}
}
