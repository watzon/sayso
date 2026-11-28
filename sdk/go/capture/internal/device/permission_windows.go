package device

import (
	"strings"

	"golang.org/x/sys/windows/registry"
)

// The microphone is off when any of the privacy switches says Deny: the
// device-wide one, the one of the user, or the one for desktop apps.
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

func micAllowed(switches []string) bool {
	for _, v := range switches {
		if strings.EqualFold(strings.TrimSpace(v), "deny") {
			return false
		}
	}
	return true
}

func permission() State {
	var switches []string
	for _, s := range []struct {
		root registry.Key
		path string
	}{{registry.LOCAL_MACHINE, consentKey}, {registry.CURRENT_USER, consentKey}, {registry.CURRENT_USER, consentKey + `\NonPackaged`}} {
		if v, ok := readString(s.root, s.path, "Value"); ok {
			switches = append(switches, v)
		}
	}
	if micAllowed(switches) {
		return Granted
	}
	return Denied
}

// Windows asks nothing: the user turns the switches on in Settings.
func requestPermission() State { return permission() }
