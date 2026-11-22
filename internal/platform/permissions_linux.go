package platform

// Linux has no per-app microphone permission. "Accessibility" is a way to
// send keys in this session, and "Input Monitoring" is read access to
// /dev/input (or an X11 session, where chords are grabbed). Ported from
// sayso-platform-linux/src/permissions.rs. Settings open the Linux guide.

import "os/exec"

const linuxPermissionsGuide = "https://github.com/watzon/sayso/blob/main/docs/linux.md#permissions"

type linuxPermissions struct {
	session  linuxSession
	inserter *linuxInserter
}

func (p linuxPermissions) Status() PermissionStatus {
	return PermissionStatus{
		Microphone:      true,
		Accessibility:   p.inserter.canInject(),
		InputMonitoring: linuxInputMonitoring(p.session),
		NeedsInputPerms: false,
	}
}

func (linuxPermissions) RequestMicrophone() error { return nil }

func (p linuxPermissions) RequestAccessibility() error { return p.OpenSettings("accessibility") }

func (p linuxPermissions) RequestInputMonitoring() error { return p.OpenSettings("input_monitoring") }

func (linuxPermissions) OpenSettings(string) error {
	return exec.Command("xdg-open", linuxPermissionsGuide).Start()
}
