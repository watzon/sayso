package platform

// A listen-only key stream from /dev/input. It needs read access to the
// event devices (the "input" group), which is the Linux form of Input
// Monitoring. It never grabs a device, so every key also reaches the
// focused app. New devices are picked up by a rescan every few seconds.

import (
	"encoding/binary"
	"os"
	"path/filepath"
	"sync"
	"time"
	"unsafe"

	"golang.org/x/sys/unix"
)

const (
	evSyn = 0x00
	evKey = 0x01
	evRel = 0x02

	relHWheel      = 0x06
	relWheel       = 0x08
	relWheelHiRes  = 0x0b
	relHWheelHiRes = 0x0c

	btnMouseFirst = 0x110 // BTN_LEFT
	btnMouseLast  = 0x117 // BTN_TASK

	inputEventSize = 24 // struct input_event on 64-bit: timeval, type, code, value

	evdevRescan = 3 * time.Second
)

// evdevInput is one event from a device.
type evdevInput struct {
	typ   uint16
	code  uint16
	value int32
}

// evdevTranslator turns device events into engine events. It keeps the held
// modifiers across all keyboards.
type evdevTranslator struct {
	held modifierSet
}

func (t *evdevTranslator) translate(in evdevInput) (rawKeyEvent, bool) {
	switch in.typ {
	case evRel:
		if in.value != 0 && (in.code == relWheel || in.code == relHWheel || in.code == relWheelHiRes || in.code == relHWheelHiRes) {
			return rawKeyEvent{kind: rawPointer}, true
		}
		return rawKeyEvent{}, false
	case evKey:
	default:
		return rawKeyEvent{}, false
	}
	if in.code >= btnMouseFirst && in.code <= btnMouseLast {
		if in.value == 1 {
			return rawKeyEvent{kind: rawPointer}, true
		}
		return rawKeyEvent{}, false
	}
	if m, ok := linuxSoloFromCode(in.code); ok {
		if in.value == 2 {
			return rawKeyEvent{}, false // modifier auto-repeat changes nothing
		}
		if in.value == 1 {
			t.held |= setOf(m)
		} else {
			t.held = t.held.without(m)
		}
		return rawKeyEvent{kind: rawModifiers, code: in.code, held: t.held, mods: t.held.modifiers(), changed: m, hasChanged: true}, true
	}
	ev := rawKeyEvent{code: in.code, key: linuxKeyFromCode[in.code], held: t.held, mods: t.held.modifiers()}
	switch in.value {
	case 0:
		ev.kind = rawKeyUp
	case 1:
		ev.kind = rawKeyDown
	case 2:
		ev.kind = rawKeyDown
		ev.repeat = true
	default:
		return rawKeyEvent{}, false
	}
	return ev, true
}

// evdevReader reads every keyboard and pointer device and hands each event
// to handle, from one goroutine per device. handle must be safe for
// concurrent calls.
type evdevReader struct {
	handle  func(evdevInput)
	mu      sync.Mutex
	devices map[string]*os.File
	stop    chan struct{}
	wg      sync.WaitGroup
}

// evdevReadable is true when at least one event device can be opened.
func evdevReadable() bool {
	paths, _ := filepath.Glob("/dev/input/event*")
	for _, p := range paths {
		if f, err := os.Open(p); err == nil {
			f.Close()
			return true
		}
	}
	return false
}

func startEvdev(handle func(evdevInput)) (*evdevReader, error) {
	r := &evdevReader{handle: handle, devices: map[string]*os.File{}, stop: make(chan struct{})}
	if n := r.scan(); n == 0 {
		return nil, errEvdevUnreadable
	}
	r.wg.Add(1)
	go func() {
		defer r.wg.Done()
		t := time.NewTicker(evdevRescan)
		defer t.Stop()
		for {
			select {
			case <-r.stop:
				return
			case <-t.C:
				r.scan()
			}
		}
	}()
	return r, nil
}

var errEvdevUnreadable = &os.PathError{Op: "open", Path: "/dev/input/event*", Err: os.ErrPermission}

// scan opens new input devices. It returns how many devices are open.
func (r *evdevReader) scan() int {
	paths, _ := filepath.Glob("/dev/input/event*")
	r.mu.Lock()
	defer r.mu.Unlock()
	for _, p := range paths {
		if _, ok := r.devices[p]; ok {
			continue
		}
		f, err := os.Open(p)
		if err != nil {
			continue
		}
		if !isKeyboardOrPointer(f) {
			f.Close()
			continue
		}
		r.devices[p] = f
		r.wg.Add(1)
		go r.read(p, f)
	}
	return len(r.devices)
}

func (r *evdevReader) read(path string, f *os.File) {
	defer r.wg.Done()
	buf := make([]byte, inputEventSize*64)
	for {
		n, err := f.Read(buf)
		if err != nil {
			// Unplugged or closed by stop.
			r.mu.Lock()
			if r.devices[path] == f {
				delete(r.devices, path)
			}
			r.mu.Unlock()
			f.Close()
			return
		}
		for off := 0; off+inputEventSize <= n; off += inputEventSize {
			e := buf[off : off+inputEventSize]
			in := evdevInput{
				typ:   binary.LittleEndian.Uint16(e[16:]),
				code:  binary.LittleEndian.Uint16(e[18:]),
				value: int32(binary.LittleEndian.Uint32(e[20:])),
			}
			if in.typ != evSyn {
				r.handle(in)
			}
		}
	}
}

func (r *evdevReader) close() {
	close(r.stop)
	r.mu.Lock()
	for _, f := range r.devices {
		f.Close()
	}
	r.mu.Unlock()
	r.wg.Wait()
}

// isKeyboardOrPointer checks the key capabilities of a device: a key such as
// A or Esc, or a mouse button.
func isKeyboardOrPointer(f *os.File) bool {
	var bits [96]byte // KEY_MAX (0x2ff) bits
	// Not f.Fd(): it would switch the file to blocking mode, and then Close
	// could not end a pending Read.
	conn, err := f.SyscallConn()
	if err != nil {
		return false
	}
	var ioctlErr error
	if err := conn.Control(func(fd uintptr) { ioctlErr = ioctlEVIOCGBIT(fd, evKey, bits[:]) }); err != nil || ioctlErr != nil {
		return false
	}
	has := func(code uint16) bool { return bits[code/8]&(1<<(code%8)) != 0 }
	return has(evKeyEsc) || has(30 /* KEY_A */) || has(btnMouseFirst)
}

// ioctlEVIOCGBIT reads the event bits of one event type.
func ioctlEVIOCGBIT(fd uintptr, ev uint, buf []byte) error {
	const iocRead = 2
	req := uintptr(iocRead<<30 | len(buf)<<16 | 'E'<<8 | (0x20 + int(ev)))
	_, _, errno := unix.Syscall(unix.SYS_IOCTL, fd, req, uintptr(unsafe.Pointer(&buf[0])))
	if errno != 0 {
		return errno
	}
	return nil
}
