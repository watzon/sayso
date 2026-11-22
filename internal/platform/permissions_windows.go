package platform

import (
	"strings"

	"golang.org/x/sys/windows"
	"golang.org/x/sys/windows/registry"
)

// windowsPermissions: Windows asks for no accessibility or input monitoring
// permission. The microphone is off when any of the privacy switches says
// Deny: the device-wide one, the user's, or the one for desktop apps.

type windowsPermissions struct{}

const consentKey = `Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone`

func readString(root registry.Key, path, name string) (string, bool) {
	k, err := registry.OpenKey(root, path, registry.QUERY_VALUE)
	if err != nil {
		return "", false
	}
	defer k.Close()
	v, _, err := k.GetStringValue(name)
	return v, err == nil
}

// micAllowed: the switches that are set; none of them may say Deny.
func micAllowed(switches []string) bool {
	for _, v := range switches {
		if strings.EqualFold(strings.TrimSpace(v), "deny") {
			return false
		}
	}
	return true
}

func microphoneAllowed() bool {
	var switches []string
	for _, s := range []struct {
		root registry.Key
		path string
	}{{registry.LOCAL_MACHINE, consentKey}, {registry.CURRENT_USER, consentKey}, {registry.CURRENT_USER, consentKey + `\NonPackaged`}} {
		if v, ok := readString(s.root, s.path, "Value"); ok {
			switches = append(switches, v)
		}
	}
	return micAllowed(switches)
}

func (windowsPermissions) Status() PermissionStatus {
	return PermissionStatus{Microphone: microphoneAllowed(), Accessibility: true, InputMonitoring: true, NeedsInputPerms: false}
}

func openURI(uri string) error {
	return windows.ShellExecute(0, utf16Ptr("open"), utf16Ptr(uri), nil, nil, windows.SW_SHOWNORMAL)
}

func (p windowsPermissions) RequestMicrophone() error {
	if microphoneAllowed() {
		return nil
	}
	return p.OpenSettings("microphone")
}

func (windowsPermissions) RequestAccessibility() error   { return nil }
func (windowsPermissions) RequestInputMonitoring() error { return nil }

// OpenSettings opens the microphone privacy page. Windows has no page for
// the other kinds.
func (windowsPermissions) OpenSettings(kind string) error {
	if kind == "microphone" {
		return openURI("ms-settings:privacy-microphone")
	}
	return nil
}
