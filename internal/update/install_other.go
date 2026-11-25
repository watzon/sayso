//go:build !darwin && !windows

package update

import (
	"os"
	"path/filepath"
	"runtime"
)

const (
	packageDir    = "/usr/lib/sayso"
	aurLicenseDir = "/usr/share/licenses/sayso-bin"
)

// DetectInstaller: Linux does not update itself. It names the package that
// updates Sayso instead.
func DetectInstaller() (Installer, Manual, Package) {
	if runtime.GOOS != "linux" {
		return nil, ManualUnsupported, PackageNone
	}

	exe, err := os.Executable()
	if err == nil {
		exe, err = filepath.EvalSymlinks(exe)
	}
	if err != nil {
		return nil, ManualUnsupported, PackageNone
	}
	aur := ""
	if st, err := os.Stat(aurLicenseDir); err == nil && st.IsDir() {
		aur = packageDir
	}
	if p := packageOf(exe, os.Getenv("FLATPAK_ID") != "", os.Getenv("APPDIR"), aur); p != PackageNone {
		return nil, ManualPackaged, p
	}
	return nil, ManualUnsupported, PackageNone
}
