//go:build windows

package localengine

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"golang.org/x/sys/windows"
)

// openLibrary loads the first library of names that loads: next to the engine
// executable first, then from the system search path. A library next to the
// exe finds the DLLs it needs (onnxruntime.dll, ggml) in its own folder.
func openLibrary(names []string) (uintptr, error) {
	var errs []error
	exe, _ := os.Executable()
	for _, name := range names {
		if exe != "" {
			path := filepath.Join(filepath.Dir(exe), name)
			if _, err := os.Stat(path); err == nil {
				h, err := windows.LoadLibraryEx(path, 0, windows.LOAD_WITH_ALTERED_SEARCH_PATH)
				if err == nil {
					return uintptr(h), nil
				}
				errs = append(errs, fmt.Errorf("%s: %w", path, err))
			}
		}
		h, err := windows.LoadLibraryEx(name, 0, windows.LOAD_LIBRARY_SEARCH_DEFAULT_DIRS)
		if err == nil {
			return uintptr(h), nil
		}
		errs = append(errs, fmt.Errorf("%s: %w", name, err))
	}
	return 0, errors.Join(errs...)
}
