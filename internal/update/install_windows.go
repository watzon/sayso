//go:build windows

package update

import (
	"os"
	"os/exec"
	"path/filepath"
	"syscall"

	"golang.org/x/sys/windows"
	"golang.org/x/sys/windows/registry"
)

// uninstallKey is the uninstall entry that Setup writes, named by the AppId
// of packaging/windows/Sayso.iss with Inno Setup's "_is1".
const uninstallKey = `Software\Microsoft\Windows\CurrentVersion\Uninstall\{6A0D2C3B-4E7F-4B8A-9C1D-5A3E8F2B7C61}_is1`

// DetectInstaller: an install for the current user and a copy that Setup
// did not install (a portable folder, a build from source) update through
// Setup. An install for all users needs administrator rights, so the user
// updates it by hand.
func DetectInstaller() (Installer, Manual, Package) {
	exe, err := os.Executable()
	if err == nil {
		exe, err = filepath.EvalSymlinks(exe)
	}
	if err != nil {
		return nil, ManualUnsupported, PackageNone
	}
	dir := filepath.Dir(exe)
	if loc, ok := installLocation(registry.LOCAL_MACHINE); ok && SameFolder(loc, dir) {
		return nil, ManualNoPermission, PackageNone
	}
	local := os.Getenv("LOCALAPPDATA")
	if local == "" {
		return nil, ManualUnsupported, PackageNone
	}
	kind := WinPortable
	if loc, ok := installLocation(registry.CURRENT_USER); ok && SameFolder(loc, dir) {
		kind = WinPerUser
	}
	w := NewWinInstall(dir, WinStagingDir(local, dir), kind)
	return w, ManualNone, PackageNone
}

func installLocation(root registry.Key) (string, bool) {
	k, err := registry.OpenKey(root, uninstallKey, registry.QUERY_VALUE|registry.WOW64_64KEY)
	if err != nil {
		return "", false
	}
	defer k.Close()
	s, _, err := k.GetStringValue("InstallLocation")
	return s, err == nil && s != ""
}

// installedVersion is the version of the install for the current user.
func installedVersion() (Semver, bool) {
	k, err := registry.OpenKey(registry.CURRENT_USER, uninstallKey, registry.QUERY_VALUE|registry.WOW64_64KEY)
	if err != nil {
		return Semver{}, false
	}
	defer k.Close()
	s, _, err := k.GetStringValue("DisplayVersion")
	if err != nil {
		return Semver{}, false
	}
	v, err := ParseVersion(s)
	return v, err == nil
}

// NewWinInstall is the installer of the copy in dir.
func NewWinInstall(dir, staging string, kind WinKind) *WinInstall {
	return &WinInstall{dir: dir, staging: staging, kind: kind, start: startDetached, installed: installedVersion}
}

// startDetached starts Setup in its own process, with no console, so it
// outlives Sayso.

func startDetached(exe, cmdLine string) error {
	cmd := exec.Command(exe)
	cmd.SysProcAttr = &syscall.SysProcAttr{
		CmdLine:       cmdLine,
		CreationFlags: windows.DETACHED_PROCESS | windows.CREATE_NEW_PROCESS_GROUP,
	}
	if err := cmd.Start(); err != nil {
		return err
	}
	return cmd.Process.Release()
}
