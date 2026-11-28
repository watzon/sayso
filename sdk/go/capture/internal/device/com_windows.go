package device

import (
	"fmt"
	"syscall"
	"unsafe"

	"golang.org/x/sys/windows"
)

// COM through vtables, with no cgo. A COM object is a pointer to a pointer to
// its table of methods; slot 0 to 2 are IUnknown (QueryInterface, AddRef,
// Release). The slot numbers come from the windows 0.62 crate that the Rust
// app uses, counted with the IUnknown slots.

type comObject uintptr

const (
	clsctxAll          = 0x17
	clsctxInprocServer = 0x1
	coinitMultithread  = 0x0
	rpcEChangedMode    = 0x80010106
)

// call calls method slot of the object with the object as the first argument.
func (o comObject) call(slot int, args ...uintptr) uintptr {
	vtbl := *(*uintptr)(cPtr(uintptr(o)))
	fn := *(*uintptr)(cPtr(vtbl + uintptr(slot)*unsafe.Sizeof(uintptr(0))))
	r, _, _ := syscall.SyscallN(fn, append([]uintptr{uintptr(o)}, args...)...)
	return r
}

// hr calls a method that returns an HRESULT.
func (o comObject) hr(what string, slot int, args ...uintptr) error {
	if r := int32(o.call(slot, args...)); r < 0 {
		return fmt.Errorf("%s: HRESULT %#x", what, uint32(r))
	}
	return nil
}

func (o comObject) release() {
	if o != 0 {
		o.call(2)
	}
}

func (o comObject) query(iid *windows.GUID) (comObject, error) {
	var out comObject
	err := o.hr("QueryInterface", 0, uintptr(unsafe.Pointer(iid)), uintptr(unsafe.Pointer(&out)))
	return out, err
}

func createInstance(clsid, iid *windows.GUID, ctx uintptr) (comObject, error) {
	var out comObject
	r, _, _ := procCoCreateInstance.Call(uintptr(unsafe.Pointer(clsid)), 0, ctx, uintptr(unsafe.Pointer(iid)), uintptr(unsafe.Pointer(&out)))
	if int32(r) < 0 {
		return 0, fmt.Errorf("CoCreateInstance: HRESULT %#x", uint32(r))
	}
	return out, nil
}

// comInit starts COM (multithreaded) on the calling thread, which must be
// locked. done gives the count back when it was ours.
func comInit() (done func(), err error) {
	r, _, _ := procCoInitializeEx.Call(0, coinitMultithread)
	switch {
	case int32(r) >= 0:
		return func() { procCoUninitialize.Call() }, nil
	case uint32(r) == rpcEChangedMode:
		return func() {}, nil
	}
	return nil, fmt.Errorf("CoInitializeEx: HRESULT %#x", uint32(r))
}

var (
	oleaut32          = windows.NewLazySystemDLL("oleaut32.dll")
	procSysFreeString = oleaut32.NewProc("SysFreeString")
	procSysStringLen  = oleaut32.NewProc("SysStringLen")
)

// bstr reads and frees a BSTR.
func bstr(p uintptr) string {
	if p == 0 {
		return ""
	}
	defer procSysFreeString.Call(p)
	n, _, _ := procSysStringLen.Call(p)
	return windows.UTF16ToString(unsafe.Slice((*uint16)(cPtr(p)), int(n)))
}

func mustGUID(s string) windows.GUID {
	g, err := windows.GUIDFromString(s)
	if err != nil {
		panic(err)
	}
	return g
}
