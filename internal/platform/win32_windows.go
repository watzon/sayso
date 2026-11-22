package platform

import (
	"syscall"
	"unsafe"

	"golang.org/x/sys/windows"
)

// The Win32 calls of the Windows services that golang.org/x/sys/windows does
// not wrap. Loaded from the system folder only.

var (
	user32   = windows.NewLazySystemDLL("user32.dll")
	kernel32 = windows.NewLazySystemDLL("kernel32.dll")
	winmm    = windows.NewLazySystemDLL("winmm.dll")
	ole32    = windows.NewLazySystemDLL("ole32.dll")
	combase  = windows.NewLazySystemDLL("combase.dll")

	procSetWindowsHookExW        = user32.NewProc("SetWindowsHookExW")
	procUnhookWindowsHookEx      = user32.NewProc("UnhookWindowsHookEx")
	procCallNextHookEx           = user32.NewProc("CallNextHookEx")
	procGetMessageW              = user32.NewProc("GetMessageW")
	procPostThreadMessageW       = user32.NewProc("PostThreadMessageW")
	procPeekMessageW             = user32.NewProc("PeekMessageW")
	procGetAsyncKeyState         = user32.NewProc("GetAsyncKeyState")
	procSendInput                = user32.NewProc("SendInput")
	procOpenClipboard            = user32.NewProc("OpenClipboard")
	procCloseClipboard           = user32.NewProc("CloseClipboard")
	procEmptyClipboard           = user32.NewProc("EmptyClipboard")
	procGetClipboardData         = user32.NewProc("GetClipboardData")
	procSetClipboardData         = user32.NewProc("SetClipboardData")
	procGetClipboardSequenceNum  = user32.NewProc("GetClipboardSequenceNumber")
	procRegisterClipboardFormatW = user32.NewProc("RegisterClipboardFormatW")
	procGetForegroundWindow      = user32.NewProc("GetForegroundWindow")
	procGetClassNameW            = user32.NewProc("GetClassNameW")
	procGetWindowThreadProcessId = user32.NewProc("GetWindowThreadProcessId")

	procGlobalAlloc  = kernel32.NewProc("GlobalAlloc")
	procGlobalFree   = kernel32.NewProc("GlobalFree")
	procGlobalLock   = kernel32.NewProc("GlobalLock")
	procGlobalUnlock = kernel32.NewProc("GlobalUnlock")
	procGlobalSize   = kernel32.NewProc("GlobalSize")

	procPlaySoundW = winmm.NewProc("PlaySoundW")

	procCoInitializeEx   = ole32.NewProc("CoInitializeEx")
	procCoUninitialize   = ole32.NewProc("CoUninitialize")
	procCoCreateInstance = ole32.NewProc("CoCreateInstance")
	procCoTaskMemFree    = ole32.NewProc("CoTaskMemFree")
	procPropVariantClear = ole32.NewProc("PropVariantClear")

	procWindowsCreateString    = combase.NewProc("WindowsCreateString")
	procWindowsDeleteString    = combase.NewProc("WindowsDeleteString")
	procRoGetActivationFactory = combase.NewProc("RoGetActivationFactory")
)

// Window messages and hook codes.
const (
	whKeyboardLL = 13
	whMouseLL    = 14

	wmQuit       = 0x0012
	wmKeyDown    = 0x0100
	wmKeyUp      = 0x0101
	wmSysKeyDown = 0x0104
	wmSysKeyUp   = 0x0105
	wmUser       = 0x0400

	wmLButtonDown = 0x0201
	wmRButtonDown = 0x0204
	wmMButtonDown = 0x0207
	wmMouseWheel  = 0x020A
	wmXButtonDown = 0x020B
	wmMouseHWheel = 0x020E
)

type point struct{ X, Y int32 }

type msg struct {
	Hwnd    uintptr
	Message uint32
	WParam  uintptr
	LParam  uintptr
	Time    uint32
	Pt      point
	_       uint32 // lPrivate
}

type kbdLLHookStruct struct {
	VkCode    uint32
	ScanCode  uint32
	Flags     uint32
	Time      uint32
	ExtraInfo uintptr
}

// Input for SendInput. INPUT is 40 bytes on 64-bit Windows: the type, then
// the largest union member (MOUSEINPUT) at offset 8.
const (
	inputKeyboard     = 1
	keyEventFKeyUp    = 0x0002
	keyEventFUnicode  = 0x0004
	keyEventFExtended = 0x0001
)

type keybdInput struct {
	Vk        uint16
	Scan      uint16
	Flags     uint32
	Time      uint32
	ExtraInfo uintptr
}

type input struct {
	Type uint32
	_    uint32
	Ki   keybdInput
	_    [8]byte // pad KEYBDINPUT (24) to MOUSEINPUT (32)
}

// sayso's own events carry this in dwExtraInfo.
const saysoInputTag = 0x5341_5953 // "SAYS"

func sendInputs(in []input) error {
	if len(in) == 0 {
		return nil
	}
	n, _, err := procSendInput.Call(uintptr(len(in)), uintptr(unsafe.Pointer(&in[0])), unsafe.Sizeof(in[0]))
	if int(n) != len(in) {
		if err == nil || err == windows.ERROR_SUCCESS {
			err = syscall.EINVAL
		}
		return err
	}
	return nil
}

func keyInput(vk uint16, up bool) input {
	var flags uint32
	if up {
		flags |= keyEventFKeyUp
	}
	switch vk {
	case vkRControl, vkRMenu, vkLWin, vkRWin, vkInsert:
		flags |= keyEventFExtended
	}
	return input{Type: inputKeyboard, Ki: keybdInput{Vk: vk, Flags: flags, ExtraInfo: saysoInputTag}}
}

func asyncDown(vk uint16) bool {
	r, _, _ := procGetAsyncKeyState.Call(uintptr(vk))
	return r&0x8000 != 0
}

// utf16Ptr is s as a NUL-terminated UTF-16 string.
func utf16Ptr(s string) *uint16 {
	p, err := windows.UTF16PtrFromString(s)
	if err != nil {
		p, _ = windows.UTF16PtrFromString("")
	}
	return p
}
