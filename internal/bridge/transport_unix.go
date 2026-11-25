//go:build unix

package bridge

import (
	"errors"
	"fmt"
	"io/fs"
	"net"
	"os"
	"path/filepath"
	"syscall"
	"time"
)

// The Unix socket of the Host API.

const writeTimeout = 2 * time.Second

type unixConn = net.UnixConn

type listener struct {
	ln   *net.UnixListener
	path string
	file [2]uint64
}

// privateDir makes dir with mode 0700, or checks that it is a folder of
// this user that nobody else can open.
func privateDir(dir string) error {
	err := os.Mkdir(dir, 0o700)
	if err == nil {
		return nil
	}
	if !errors.Is(err, fs.ErrExist) {
		return fmt.Errorf("cannot make %s: %w", dir, err)
	}
	st, err := os.Lstat(dir)
	if err != nil {
		return err
	}
	sys, ok := st.Sys().(*syscall.Stat_t)
	if st.IsDir() && ok && int(sys.Uid) == os.Getuid() && st.Mode().Perm()&0o077 == 0 {
		return nil
	}
	return fmt.Errorf("%s is not a private folder of this user: %w", dir, ErrPermission)
}

func bindListener(path string) (*listener, error) {
	parent := filepath.Dir(path)
	if parent == "" || parent == "." && !filepath.IsAbs(path) && filepath.Base(path) == path {
		return nil, fmt.Errorf("%s has no folder", path)
	}
	if err := privateDir(parent); err != nil {
		return nil, err
	}
	if c, err := net.DialTimeout("unix", path, time.Second); err == nil {
		c.Close()
		return nil, fmt.Errorf("another Sayso serves extensions at %s: %w", path, ErrAddrInUse)
	}
	_ = os.Remove(path)
	ln, err := net.ListenUnix("unix", &net.UnixAddr{Name: path, Net: "unix"})
	if err != nil {
		return nil, err
	}
	ln.SetUnlinkOnClose(false)
	if err := passCredentials(ln); err != nil {
		ln.Close()
		return nil, err
	}
	st, err := os.Lstat(path)
	if err != nil {
		ln.Close()
		return nil, err
	}
	l := &listener{ln: ln, path: path}
	if sys, ok := st.Sys().(*syscall.Stat_t); ok {
		l.file = [2]uint64{uint64(sys.Dev), uint64(sys.Ino)}
	}
	return l, nil
}

func (l *listener) accept() (*conn, error) {
	c, err := l.ln.AcceptUnix()
	if err != nil {
		return nil, err
	}
	reader, peer := readerOf(c)
	return &conn{
		reader: reader,
		writer: deadlineWriter{c},
		closer: func() { _ = c.Close() },
		peer:   peer,
	}, nil
}

// deadlineWriter gives up a write that the other end does not read in 2 s.
type deadlineWriter struct{ c *net.UnixConn }

func (w deadlineWriter) Write(b []byte) (int, error) {
	_ = w.c.SetWriteDeadline(time.Now().Add(writeTimeout))
	return w.c.Write(b)
}

// wake connects once, so that a blocked accept returns.
func (l *listener) wake() bool {
	c, err := net.DialTimeout("unix", l.path, time.Second)
	if err != nil {
		return false
	}
	c.Close()
	return true
}

// remove deletes the socket file, if it is still the file of this listener.
func (l *listener) remove() {
	_ = l.ln.Close()
	st, err := os.Lstat(l.path)
	if err != nil {
		return
	}
	if sys, ok := st.Sys().(*syscall.Stat_t); ok && [2]uint64{uint64(sys.Dev), uint64(sys.Ino)} == l.file {
		_ = os.Remove(l.path)
	}
}
