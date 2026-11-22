package i18n

import (
	"os"
	"strings"
)

// SystemLocales returns the languages of the system in the order of
// preference.
func SystemLocales() []string {
	if l := platformLocales(); len(l) > 0 {
		return l
	}
	return envLocales()
}

// envLocales reads the POSIX language variables.
func envLocales() []string {
	var out []string
	if v := os.Getenv("LANGUAGE"); v != "" {
		out = append(out, strings.Split(v, ":")...)
	}
	for _, name := range []string{"LC_ALL", "LC_MESSAGES", "LANG"} {
		if v := os.Getenv(name); v != "" {
			out = append(out, v)
		}
	}
	return out
}
