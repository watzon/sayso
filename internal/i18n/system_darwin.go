//go:build darwin

package i18n

import (
	"os/exec"
	"regexp"
)

var quoted = regexp.MustCompile(`"([^"]+)"`)

// platformLocales reads the language list of System Settings.
func platformLocales() []string {
	out, err := exec.Command("defaults", "read", "-g", "AppleLanguages").Output()
	if err != nil {
		return nil
	}
	var langs []string
	for _, m := range quoted.FindAllSubmatch(out, -1) {
		langs = append(langs, string(m[1]))
	}
	return langs
}
