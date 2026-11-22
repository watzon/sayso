//go:build darwin

package update

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"

	"golang.org/x/sys/unix"
)

// The self-update of a macOS app bundle (sayso-update install/macos.rs):
// the new Sayso.app is copied out of the disk image into .Sayso-update/
// beside the app, its signature is checked, and renamex_np(RENAME_SWAP)
// exchanges the two bundles in one step.

const codeRequirement = "=anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"MB5789APU7\" and identifier \"dev.sayso.Sayso\""

const imageApp = "Sayso.app"

var passEnv = []string{"XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "SAYSO_UPDATE_URL", "SAYSO_UPDATE_KEY"}

// MacInstall is a Sayso.app in a folder the user can write.
type MacInstall struct{ app string }

// DetectInstaller finds the install of this executable.
func DetectInstaller() (Installer, Manual, Package) {
	exe, err := os.Executable()
	if err == nil {
		exe, err = filepath.EvalSymlinks(exe)
	}
	if err != nil {
		return nil, ManualWrongFolder, PackageNone
	}
	m, manual := macForExecutable(exe)
	if manual != ManualNone {
		return nil, manual, PackageNone
	}
	return m, ManualNone, PackageNone
}

func macForExecutable(exe string) (*MacInstall, Manual) {
	app := filepath.Dir(filepath.Dir(filepath.Dir(exe)))
	if filepath.Ext(app) != ".app" || strings.Contains(app, "/AppTranslocation/") {
		return nil, ManualWrongFolder
	}
	parent := filepath.Dir(app)
	var fs unix.Statfs_t
	if unix.Statfs(parent, &fs) != nil || fs.Flags&unix.MNT_RDONLY != 0 {
		return nil, ManualWrongFolder
	}
	if unix.Access(parent, unix.W_OK) != nil {
		return nil, ManualNoPermission
	}
	m := &MacInstall{app: app}
	if m.makeStagingDir() != nil {
		return nil, ManualNoPermission
	}
	ok := m.canExchange()
	os.Remove(m.stagingDir())
	if !ok {
		return nil, ManualNoExchange
	}
	return m, ManualNone
}

func (m *MacInstall) stagingDir() string {
	stem := strings.TrimSuffix(filepath.Base(m.app), filepath.Ext(m.app))
	return filepath.Join(filepath.Dir(m.app), "."+stem+"-update")
}

func (m *MacInstall) stagedApp() string { return filepath.Join(m.stagingDir(), filepath.Base(m.app)) }

func (m *MacInstall) makeStagingDir() error {
	dir := m.stagingDir()
	if st, err := os.Lstat(dir); err == nil {
		if sys, ok := st.Sys().(*syscall.Stat_t); ok && st.IsDir() && int(sys.Uid) == os.Getuid() {
			return nil
		}
		return fmt.Errorf("%s is not a folder of this user", dir)
	}
	return os.Mkdir(dir, 0o700)
}

func (m *MacInstall) canExchange() bool {
	a, b := filepath.Join(m.stagingDir(), ".probe-a"), filepath.Join(m.stagingDir(), ".probe-b")
	ok := os.Mkdir(a, 0o755) == nil && os.Mkdir(b, 0o755) == nil && renameSwap(a, b) == nil
	os.Remove(a)
	os.Remove(b)
	return ok
}

func renameSwap(a, b string) error { return unix.RenamexNp(a, b, unix.RENAME_SWAP) }

func runTool(name string, args ...string) (string, error) {
	out, err := exec.Command(name, args...).Output()
	if err != nil {
		msg := err.Error()
		if ee, ok := err.(*exec.ExitError); ok {
			msg = strings.TrimSpace(string(ee.Stderr))
		}
		return "", fmt.Errorf("%s: %s", name, msg)
	}
	return string(out), nil
}

func (m *MacInstall) copyFromImage(image string) error {
	root := filepath.Join(m.stagingDir(), ".mount")
	os.RemoveAll(root)
	if err := os.Mkdir(root, 0o755); err != nil {
		return fmt.Errorf("cannot make %s: %v", root, err)
	}
	defer os.RemoveAll(root)
	if _, err := runTool("/usr/bin/hdiutil", "attach", "-nobrowse", "-readonly", "-noautoopen", "-mountroot", root, image); err != nil {
		return err
	}
	entries, _ := os.ReadDir(root)
	if len(entries) == 0 {
		return fmt.Errorf("the disk image has no %s", imageApp)
	}
	volume := filepath.Join(root, entries[0].Name())
	defer runTool("/usr/bin/hdiutil", "detach", "-force", volume)
	src := filepath.Join(volume, imageApp)
	if st, err := os.Stat(src); err != nil || !st.IsDir() {
		return fmt.Errorf("the disk image has no %s", imageApp)
	}
	_, err := runTool("/usr/bin/ditto", src, m.stagedApp())
	return err
}

func bundleVersion(app string) (string, error) {
	return runTool("/usr/libexec/PlistBuddy", "-c", "Print :CFBundleShortVersionString", filepath.Join(app, "Contents/Info.plist"))
}

func (m *MacInstall) InstallPath() string { return m.app }

func (m *MacInstall) LockPath() string {
	home := os.Getenv("HOME")
	if home == "" {
		home = "/tmp"
	}
	name := strings.Map(func(c rune) rune {
		if c < 128 && (c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9') {
			return c
		}
		return '_'
	}, m.app)
	return filepath.Join(home, "Library/Caches/dev.sayso.Sayso/install-locks", name+".lock")
}

func (m *MacInstall) RecordPath() string { return filepath.Join(m.stagingDir(), "update.json") }

func (m *MacInstall) InstalledVersion() (Semver, bool) {
	s, err := bundleVersion(m.app)
	if err != nil {
		return Semver{}, false
	}
	v, err := ParseVersion(strings.TrimSpace(s))
	return v, err == nil
}

func (m *MacInstall) Stage(file string, version Semver) error {
	m.RemoveStaging()
	if err := m.makeStagingDir(); err != nil {
		return err
	}
	err := m.copyFromImage(file)
	if err == nil {
		err = m.VerifyStaged()
	}
	if err == nil {
		var found string
		if found, err = bundleVersion(m.stagedApp()); err == nil && strings.TrimSpace(found) != version.String() {
			err = fmt.Errorf("the disk image has version %s, and the update information says %s", strings.TrimSpace(found), version)
		}
	}
	if err != nil {
		m.RemoveStaging()
	}
	return err
}

func (m *MacInstall) StagingExists() bool {
	st, err := os.Stat(m.stagedApp())
	return err == nil && st.IsDir()
}

func (m *MacInstall) VerifyStaged() error {
	app := m.stagedApp()
	if _, err := runTool("/usr/bin/codesign", "--verify", "--deep", "--strict", "-R", codeRequirement, app); err != nil {
		return fmt.Errorf("the signature of the new version is not the signature of Sayso (%v)", err)
	}
	if _, err := runTool("/usr/sbin/spctl", "--assess", "--type", "execute", app); err != nil {
		return fmt.Errorf("macOS did not accept the new version (%v)", err)
	}
	return nil
}

func (m *MacInstall) Exchange() error {
	if err := renameSwap(m.app, m.stagedApp()); err != nil {
		return err
	}
	syncDir(filepath.Dir(m.app))
	return nil
}

// Relaunch starts the new app after this process exits.
func (m *MacInstall) Relaunch() error {
	args := []string{"-c", `pid=$1; shift; while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done; exec /usr/bin/open -n "$@"`, "sayso-relaunch", strconv.Itoa(os.Getpid())}
	for _, name := range passEnv {
		if v, ok := os.LookupEnv(name); ok {
			args = append(args, "--env", name+"="+v)
		}
	}
	args = append(args, m.app)
	cmd := exec.Command("/bin/sh", args...)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	return cmd.Start()
}

func (m *MacInstall) RemoveStaging() {
	dir := m.stagingDir()
	if st, err := os.Lstat(dir); err != nil || !st.IsDir() {
		return
	}
	os.RemoveAll(m.stagedApp())
	os.RemoveAll(filepath.Join(dir, ".mount"))
	os.Remove(dir)
}
