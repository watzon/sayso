package main

import (
	"strings"
	"testing"

	"github.com/watzon/sayso/internal/paths"
	"github.com/watzon/sayso/internal/update"
)

// Sayso starts Setup with /UPDATE=1 and quits; Setup must know that switch,
// wait for the mutex that Sayso holds, and start the new version even in
// silent mode.
func TestTheWindowsInstallerTakesTheUpdateThatSaysoStarts(t *testing.T) {
	iss := repoFile(t, "packaging/windows/Sayso.iss")
	cmdLine := update.SetupCommandLine(`C:\s\setup.exe`, `C:\p\Sayso`, `C:\s\log.txt`, update.WinPerUser)
	if !strings.Contains(cmdLine, " /UPDATE=1") || !strings.Contains(iss, `ExpandConstant('{param:UPDATE|0}') = '1'`) {
		t.Errorf("Setup does not read the /UPDATE=1 of %s", cmdLine)
	}
	if !strings.Contains(iss, `#define InstanceMutex "`+paths.InstanceMutex+`"`) || !strings.Contains(iss, "AppMutex={#InstanceMutex}") {
		t.Errorf("Sayso.iss does not wait for the mutex %s", paths.InstanceMutex)
	}
	if !strings.Contains(iss, "function InitializeSetup: Boolean;") || !strings.Contains(iss, "CheckForMutexes(InstanceMutex)") {
		t.Error("Setup does not wait for Sayso to quit before its AppMutex check")
	}
	if !strings.Contains(iss, `Filename: "{app}\Sayso.exe"; Flags: nowait; Check: IsUpdating`) {
		t.Error("Setup does not start the new version after an update")
	}
}
