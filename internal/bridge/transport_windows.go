//go:build windows

package bridge

import (
	"errors"
	"fmt"
	"io"
	"log/slog"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"unsafe"

	"github.com/watzon/sayso/internal/paths"
	"golang.org/x/sys/windows"
)

// The named pipe of the Host API (transport/windows.rs): overlapped I/O, so
// a read and a write of one connection run at once, a write gives up after
// 2 s, and a pipe that only the user can open.

const (
	writeTimeoutMs          = 2000
	pipeBuffer              = 64 * 1024
	pipeRejectRemoteClients = 0x00000008
)

var (
	kernel32                        = windows.NewLazySystemDLL("kernel32.dll")
	procGetOverlappedResultEx       = kernel32.NewProc("GetOverlappedResultEx")
	procGetNamedPipeClientProcessID = kernel32.NewProc("GetNamedPipeClientProcessId")
)

type pipe struct {
	handle windows.Handle
	closed atomic.Bool
	once   sync.Once
}

func (p *pipe) close() { p.once.Do(func() { windows.CloseHandle(p.handle) }) }

func isEnd(err error) bool {
	var errno syscall.Errno
	if !errors.As(err, &errno) {
		return false
	}
	switch errno {
	case windows.ERROR_BROKEN_PIPE, windows.ERROR_PIPE_NOT_CONNECTED, windows.ERROR_NO_DATA, windows.ERROR_OPERATION_ABORTED:
		return true
	}
	return false
}

func (p *pipe) io(timeoutMs uint32, start func(*windows.Overlapped) error) (uint32, error) {
	event, err := windows.CreateEvent(nil, 1, 0, nil)
	if err != nil {
		return 0, err
	}
	defer windows.CloseHandle(event)
	var ov windows.Overlapped
	ov.HEvent = event
	if err := start(&ov); err != nil {
		if err != windows.ERROR_IO_PENDING {
			return 0, err
		}
		if p.closed.Load() {
			windows.CancelIoEx(p.handle, &ov)
		}
	}
	var done uint32
	r, _, e := procGetOverlappedResultEx.Call(uintptr(p.handle), uintptr(unsafe.Pointer(&ov)), uintptr(unsafe.Pointer(&done)), uintptr(timeoutMs), 0)
	if r == 0 {
		if e == windows.WAIT_TIMEOUT || e == syscall.Errno(windows.WAIT_TIMEOUT) {
			windows.CancelIoEx(p.handle, &ov)
			windows.GetOverlappedResult(p.handle, &ov, &done, true)
			return 0, errors.New("the other end does not read")
		}
		return 0, e
	}
	return done, nil
}

func (p *pipe) Read(b []byte) (int, error) {
	if p.closed.Load() || len(b) == 0 {
		return 0, io.EOF
	}
	n, err := p.io(windows.INFINITE, func(ov *windows.Overlapped) error { return windows.ReadFile(p.handle, b, nil, ov) })
	if err != nil {
		if isEnd(err) {
			return 0, io.EOF
		}
		return 0, err
	}
	if n == 0 {
		return 0, io.EOF
	}
	return int(n), nil
}

func (p *pipe) Write(b []byte) (int, error) {
	written := 0
	for written < len(b) {
		if p.closed.Load() {
			return written, io.ErrClosedPipe
		}
		n, err := p.io(writeTimeoutMs, func(ov *windows.Overlapped) error { return windows.WriteFile(p.handle, b[written:], nil, ov) })
		if err != nil {
			return written, err
		}
		written += int(n)
	}
	return written, nil
}

func (p *pipe) shutdown() {
	p.closed.Store(true)
	windows.CancelIoEx(p.handle, nil)
}

type listener struct {
	name string
	sddl string
	mu   sync.Mutex
	next *pipe
	all  []*pipe
}

func bindListener(path string) (*listener, error) {
	sid := paths.CurrentUserSID()
	if sid == "" {
		return nil, errors.New("cannot find the SID of the user")
	}
	l := &listener{name: path, sddl: fmt.Sprintf("D:P(A;;GA;;;%s)", sid)}
	first, err := l.instance(true)
	if err != nil {
		if errors.Is(err, windows.ERROR_ACCESS_DENIED) {
			return nil, fmt.Errorf("another Sayso serves extensions at %s: %w", path, ErrAddrInUse)
		}
		return nil, err
	}
	l.next = first
	return l, nil
}

func (l *listener) instance(first bool) (*pipe, error) {
	sd, err := windows.SecurityDescriptorFromString(l.sddl)
	if err != nil {
		return nil, err
	}
	sa := windows.SecurityAttributes{Length: uint32(unsafe.Sizeof(windows.SecurityAttributes{})), SecurityDescriptor: sd}
	mode := uint32(windows.PIPE_ACCESS_DUPLEX | windows.FILE_FLAG_OVERLAPPED)
	if first {
		mode |= windows.FILE_FLAG_FIRST_PIPE_INSTANCE
	}
	name, err := windows.UTF16PtrFromString(l.name)
	if err != nil {
		return nil, err
	}
	h, err := windows.CreateNamedPipe(name, mode, windows.PIPE_TYPE_BYTE|windows.PIPE_READMODE_BYTE|windows.PIPE_WAIT|pipeRejectRemoteClients, windows.PIPE_UNLIMITED_INSTANCES, pipeBuffer, pipeBuffer, 0, &sa)
	if err != nil {
		return nil, err
	}
	return &pipe{handle: h}, nil
}

func (l *listener) accept() (*conn, error) {
	l.mu.Lock()
	p := l.next
	l.next = nil
	l.mu.Unlock()
	if p == nil {
		var err error
		if p, err = l.instance(false); err != nil {
			return nil, err
		}
	}
	if _, err := p.io(windows.INFINITE, func(ov *windows.Overlapped) error { return windows.ConnectNamedPipe(p.handle, ov) }); err != nil && err != windows.ERROR_PIPE_CONNECTED {
		p.close()
		return nil, err
	}
	if next, err := l.instance(false); err == nil {
		l.mu.Lock()
		l.next = next
		l.mu.Unlock()
	} else {
		slog.Warn("host: cannot make the next instance", "pipe", l.name, "err", err)
	}
	return &conn{
		reader:  p,
		writer:  p,
		closer:  func() { p.shutdown() },
		release: p.close,
		peer:    func() (Peer, error) { return pipeClient(p) },
	}, nil
}

func (l *listener) wake() bool {
	name, err := windows.UTF16PtrFromString(l.name)
	if err != nil {
		return false
	}
	h, err := windows.CreateFile(name, windows.GENERIC_READ|windows.GENERIC_WRITE, 0, nil, windows.OPEN_EXISTING, 0, 0)
	if err != nil {
		return false
	}
	windows.CloseHandle(h)
	return true
}

func (l *listener) remove() {
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.next != nil {
		l.next.close()
		l.next = nil
	}
}

func pipeClient(p *pipe) (Peer, error) {
	var pid uint32
	r, _, e := procGetNamedPipeClientProcessID.Call(uintptr(p.handle), uintptr(unsafe.Pointer(&pid)))
	if r == 0 {
		return Peer{}, fmt.Errorf("no client process: %v", e)
	}
	return Peer{PID: pid}, nil
}

func systemName(executable string) string { return fileName(executable, true) }

func systemIdentify(peer Peer) (Caller, error) {
	image, err := imagePath(peer.PID)
	if err != nil {
		return Caller{}, err
	}
	path, err := finalPath(image)
	if err != nil {
		return Caller{}, err
	}
	return CallerFromPath(path), nil
}

func imagePath(pid uint32) (string, error) {
	h, err := windows.OpenProcess(windows.PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
	if err != nil {
		return "", fmt.Errorf("cannot open process %d: %v", pid, err)
	}
	defer windows.CloseHandle(h)
	buf := make([]uint16, 32*1024)
	size := uint32(len(buf))
	if err := windows.QueryFullProcessImageName(h, 0, &buf[0], &size); err != nil {
		return "", fmt.Errorf("no image path of process %d: %v", pid, err)
	}
	return windows.UTF16ToString(buf[:size]), nil
}

func finalPath(path string) (string, error) {
	name, err := windows.UTF16PtrFromString(path)
	if err != nil {
		return "", err
	}
	h, err := windows.CreateFile(name, windows.GENERIC_READ, windows.FILE_SHARE_READ|windows.FILE_SHARE_WRITE|windows.FILE_SHARE_DELETE, nil, windows.OPEN_EXISTING, windows.FILE_FLAG_BACKUP_SEMANTICS, 0)
	if err != nil {
		return "", fmt.Errorf("cannot open %s: %v", path, err)
	}
	defer windows.CloseHandle(h)
	buf := make([]uint16, 32*1024)
	n, err := windows.GetFinalPathNameByHandle(h, &buf[0], uint32(len(buf)), 0 /* FILE_NAME_NORMALIZED */)
	if err != nil || n == 0 || int(n) >= len(buf) {
		return "", fmt.Errorf("no final path of %s: %v", path, err)
	}
	text := windows.UTF16ToString(buf[:n])
	if rest, ok := strings.CutPrefix(text, `\\?\UNC\`); ok {
		return `\\` + rest, nil
	}
	return strings.TrimPrefix(text, `\\?\`), nil
}
