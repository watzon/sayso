//go:build !(darwin || linux || freebsd)

package lexicon

import (
	"errors"
	"os"
)

// Open reads the model file at path into memory (no mmap on this system).
func Open(path string) (*Model, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	return FromBytes(b)
}

// OpenFD is only supported where files can be mapped (Android, iOS, macOS, Linux).
func OpenFD(fd int, offset, length int64) (*Model, error) {
	return nil, errors.New("lexicon: OpenFD is not supported on this system")
}
