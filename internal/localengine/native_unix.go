//go:build unix

package localengine

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"github.com/ebitengine/purego"
)

// openLibrary loads the first library of names that loads: next to the engine
// executable first, then from the system search path (LD_LIBRARY_PATH, the
// RPATH of nothing, and ldconfig).
func openLibrary(names []string) (uintptr, error) {
	var errs []error
	exe, _ := os.Executable()
	for _, name := range names {
		if exe != "" {
			path := filepath.Join(filepath.Dir(exe), name)
			if _, err := os.Stat(path); err == nil {
				h, err := purego.Dlopen(path, purego.RTLD_NOW|purego.RTLD_GLOBAL)
				if err == nil {
					return h, nil
				}
				errs = append(errs, fmt.Errorf("%s: %w", path, err))
			}
		}
		h, err := purego.Dlopen(name, purego.RTLD_NOW|purego.RTLD_GLOBAL)
		if err == nil {
			return h, nil
		}
		errs = append(errs, fmt.Errorf("%s: %w", name, err))
	}
	return 0, errors.Join(errs...)
}
