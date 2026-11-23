//go:build darwin || linux || freebsd

package lexicon

import (
	"fmt"
	"os"
	"syscall"
)

// Open maps the model file at path read-only.
func Open(path string) (*Model, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil {
		return nil, err
	}
	return OpenFD(int(f.Fd()), 0, st.Size())
}

// OpenFD maps length bytes at offset of an open file, such as an
// uncompressed asset inside an Android APK. The descriptor may be closed
// once OpenFD returns.
func OpenFD(fd int, offset, length int64) (*Model, error) {
	if offset < 0 || length <= 0 || length > 1<<31 {
		return nil, fmt.Errorf("lexicon: bad range %d+%d", offset, length)
	}
	page := int64(os.Getpagesize())
	base := offset - offset%page
	delta := offset - base
	mapped, err := syscall.Mmap(fd, base, int(length+delta), syscall.PROT_READ, syscall.MAP_SHARED)
	if err != nil {
		return nil, fmt.Errorf("lexicon: mmap: %w", err)
	}
	m, err := FromBytes(mapped[delta : delta+length])
	if err != nil {
		syscall.Munmap(mapped)
		return nil, err
	}
	if &m.data[0] != &mapped[delta] {
		// FromBytes copied a misaligned view; the mapping is not needed.
		syscall.Munmap(mapped)
		return m, nil
	}
	m.release = func() error { return syscall.Munmap(mapped) }
	return m, nil
}
