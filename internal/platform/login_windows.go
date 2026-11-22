package platform

import (
	"errors"
	"fmt"
	"os"
	"strings"

	"golang.org/x/sys/windows/registry"
)

// windowsLoginItem starts Sayso at sign-in with a value under the user's Run
// key. Task Manager's startup switch writes StartupApproved; its first byte
// odd means switched off.

type windowsLoginItem struct {
	// exe is the program to register; empty is this executable.
	exe string
}

const (
	runKey      = `Software\Microsoft\Windows\CurrentVersion\Run`
	approvedKey = `Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run`
	runValue    = "Sayso"
)

func runCommand(exe string) string { return `"` + exe + `"` }

func loginEnabled(run string, haveRun bool, approved []byte) bool {
	registered := haveRun && strings.TrimSpace(run) != ""
	off := len(approved) > 0 && approved[0]&1 == 1
	return registered && !off
}

func (l windowsLoginItem) Enabled() (bool, error) {
	run, haveRun := readString(registry.CURRENT_USER, runKey, runValue)
	var approved []byte
	if k, err := registry.OpenKey(registry.CURRENT_USER, approvedKey, registry.QUERY_VALUE); err == nil {
		approved, _, _ = k.GetBinaryValue(runValue)
		k.Close()
	}
	return loginEnabled(run, haveRun, approved), nil
}

func deleteValue(path, name string) error {
	k, err := registry.OpenKey(registry.CURRENT_USER, path, registry.SET_VALUE)
	if errors.Is(err, registry.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	defer k.Close()
	if err := k.DeleteValue(name); err != nil && !errors.Is(err, registry.ErrNotExist) {
		return err
	}
	return nil
}

func (l windowsLoginItem) SetEnabled(enabled bool) error {
	if !enabled {
		if err := deleteValue(runKey, runValue); err != nil {
			return err
		}
		return deleteValue(approvedKey, runValue)
	}
	exe := l.exe
	if exe == "" {
		var err error
		if exe, err = os.Executable(); err != nil {
			return fmt.Errorf("cannot find the Sayso exe: %w", err)
		}
	}
	k, _, err := registry.CreateKey(registry.CURRENT_USER, runKey, registry.SET_VALUE)
	if err != nil {
		return err
	}
	defer k.Close()
	if err := k.SetStringValue(runValue, runCommand(exe)); err != nil {
		return err
	}
	// An old switch-off in Task Manager would keep the new entry off.
	return deleteValue(approvedKey, runValue)
}
