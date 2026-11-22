package platform

import (
	"errors"
	"fmt"
	"runtime"
	"strings"
	"time"
	"unicode/utf16"
	"unsafe"

	"golang.org/x/sys/windows"
)

// windowsInserter puts text into the focused field: it saves the clipboard,
// puts the text on it, sends Ctrl+V (Shift+Insert to terminals that paste
// with it), and puts the old clipboard back when the clipboard still holds
// the text. Without a paste it types the text as Unicode key events.

const (
	cfUnicodeText = 13
	gmemMoveable  = 0x0002

	// maxUnitsPerEvent: SendInput calls of at most this many UTF-16 units.
	maxUnitsPerEvent = 128
	chunkDelay       = 5 * time.Millisecond
	keyGap           = 8 * time.Millisecond
	openTries        = 10
	openRetry        = 15 * time.Millisecond
	// defaultRestoreDelay gives the app time to read the clipboard.
	defaultRestoreDelay = 250 * time.Millisecond
)

var procEnumClipboardFormats = user32.NewProc("EnumClipboardFormats")

// skipFormats tell clipboard history, the cloud clipboard, and clipboard
// managers to skip an entry. Each holds a DWORD 0.
var skipFormats = []string{
	"ExcludeClipboardContentFromMonitorProcessing",
	"CanIncludeInClipboardHistory",
	"CanUploadToCloudClipboard",
	"Clipboard Viewer Ignore",
}

type windowsInserter struct{}

// clipEntry is one clipboard format and its bytes.
type clipEntry struct {
	format uint32
	data   []byte
}

// isMemoryFormat: a format whose handle is an HGLOBAL that can be copied as
// bytes. Bitmaps, metafiles, palettes, owner-display, and private formats hold
// other handles or need the owner. Windows makes CF_BITMAP from CF_DIB again.
func isMemoryFormat(f uint32) bool {
	switch {
	case f == 0 || f == 2 || f == 3 || f == 9 || f == 14:
		return false
	case (f >= 0x80 && f <= 0x83) || f == 0x8E:
		return false
	case f >= 0x200 && f <= 0x3FF:
		return false
	}
	return true
}

// withClipboard opens the clipboard on one OS thread, runs f, and closes it.
func withClipboard(f func() error) error {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	opened := false
	for i := 0; i < openTries; i++ {
		if r, _, _ := procOpenClipboard.Call(0); r != 0 {
			opened = true
			break
		}
		time.Sleep(openRetry)
	}
	if !opened {
		return errors.New("another app holds the clipboard")
	}
	defer procCloseClipboard.Call()
	return f()
}

func globalBytes(h uintptr) []byte {
	size, _, _ := procGlobalSize.Call(h)
	p, _, _ := procGlobalLock.Call(h)
	if p == 0 {
		return nil
	}
	defer procGlobalUnlock.Call(h)
	return append([]byte(nil), unsafe.Slice((*byte)(cPtr(p)), int(size))...)
}

// cPtr is memory that Windows owns, from a handle value.
func cPtr(p uintptr) unsafe.Pointer { return *(*unsafe.Pointer)(unsafe.Pointer(&p)) }

// setData puts bytes on the open clipboard. The clipboard owns the memory after it.
func setData(format uint32, data []byte) bool {
	n := max(len(data), 1)
	h, _, _ := procGlobalAlloc.Call(gmemMoveable, uintptr(n))
	if h == 0 {
		return false
	}
	p, _, _ := procGlobalLock.Call(h)
	if p == 0 {
		procGlobalFree.Call(h)
		return false
	}
	copy(unsafe.Slice((*byte)(cPtr(p)), n), data)
	procGlobalUnlock.Call(h)
	if r, _, _ := procSetClipboardData.Call(uintptr(format), h); r == 0 {
		procGlobalFree.Call(h)
		return false
	}
	return true
}

func markSkipped(except []uint32) {
	for _, name := range skipFormats {
		f, _, _ := procRegisterClipboardFormatW.Call(uintptr(unsafe.Pointer(utf16Ptr(name))))
		if f == 0 || containsU32(except, uint32(f)) {
			continue
		}
		setData(uint32(f), []byte{0, 0, 0, 0})
	}
}

func containsU32(list []uint32, v uint32) bool {
	for _, x := range list {
		if x == v {
			return true
		}
	}
	return false
}

// snapshotClipboard copies every memory format on the clipboard.
func snapshotClipboard() ([]clipEntry, error) {
	var out []clipEntry
	err := withClipboard(func() error {
		var f uintptr
		for {
			f, _, _ = procEnumClipboardFormats.Call(f)
			if f == 0 {
				return nil
			}
			if !isMemoryFormat(uint32(f)) {
				continue
			}
			if h, _, _ := procGetClipboardData.Call(f); h != 0 {
				if b := globalBytes(h); b != nil {
					out = append(out, clipEntry{uint32(f), b})
				}
			}
		}
	})
	return out, err
}

// restoreClipboard puts a snapshot back. The old content is in clipboard
// history already, so it is marked to be skipped.
func restoreClipboard(saved []clipEntry) bool {
	ok := false
	_ = withClipboard(func() error {
		if r, _, _ := procEmptyClipboard.Call(); r == 0 {
			return nil
		}
		ok = true
		var present []uint32
		for _, e := range saved {
			ok = setData(e.format, e.data) && ok
			present = append(present, e.format)
		}
		if len(saved) > 0 {
			markSkipped(present)
		}
		return nil
	})
	return ok
}

func utf16Bytes(s string) []byte {
	units := utf16.Encode([]rune(s + "\x00"))
	return unsafe.Slice((*byte)(unsafe.Pointer(&units[0])), len(units)*2)
}

// putText puts text on the clipboard and returns the sequence number after
// it. skipHistory keeps a transient paste out of clipboard history.
func putText(text string, skipHistory bool) (uint32, error) {
	err := withClipboard(func() error {
		if r, _, e := procEmptyClipboard.Call(); r == 0 {
			return fmt.Errorf("cannot empty the clipboard: %w", e)
		}
		if !setData(cfUnicodeText, utf16Bytes(text)) {
			return errors.New("cannot put the text on the clipboard")
		}
		if skipHistory {
			markSkipped(nil)
		}
		return nil
	})
	seq, _, _ := procGetClipboardSequenceNum.Call()
	return uint32(seq), err
}

func (windowsInserter) ReadClipboard() (string, error) {
	var text string
	err := withClipboard(func() error {
		h, _, _ := procGetClipboardData.Call(cfUnicodeText)
		if h == 0 {
			return nil
		}
		b := globalBytes(h)
		units := unsafe.Slice((*uint16)(unsafe.Pointer(unsafe.SliceData(b))), len(b)/2)
		text = windows.UTF16ToString(units)
		return nil
	})
	return text, err
}

func (windowsInserter) WriteClipboard(text string) error {
	_, err := putText(text, false)
	return err
}

// utf16Chunks splits text into chunks of at most max UTF-16 units without
// cutting a surrogate pair.
func utf16Chunks(text string, max int) [][]uint16 {
	var chunks [][]uint16
	var cur []uint16
	for _, r := range text {
		units := utf16.Encode([]rune{r})
		if len(cur)+len(units) > max {
			chunks = append(chunks, cur)
			cur = nil
		}
		cur = append(cur, units...)
	}
	if len(cur) > 0 {
		chunks = append(chunks, cur)
	}
	return chunks
}

func unicodeInput(unit uint16, up bool) input {
	flags := uint32(keyEventFUnicode)
	if up {
		flags |= keyEventFKeyUp
	}
	return input{Type: inputKeyboard, Ki: keybdInput{Scan: unit, Flags: flags, ExtraInfo: saysoInputTag}}
}

// textInputs types units: a Return for each line break, a Unicode key event
// pair for anything else.
func textInputs(units []uint16) []input {
	out := make([]input, 0, len(units)*2)
	for _, u := range units {
		if u == '\n' || u == '\r' {
			out = append(out, keyInput(vkReturn, false), keyInput(vkReturn, true))
			continue
		}
		out = append(out, unicodeInput(u, false), unicodeInput(u, true))
	}
	return out
}

// heldModifiers are the modifier keys the user still holds (Ctrl optional).
func heldModifiers(withCtrl bool) []uint16 {
	var held []uint16
	for _, vk := range modifierVKs {
		if !withCtrl && (vk == vkLControl || vk == vkRControl) {
			continue
		}
		if asyncDown(vk) {
			held = append(held, vk)
		}
	}
	return held
}

// releaseInputs lets go of held modifiers, after the mask key so a lone Alt
// or Win release opens no menu.
func releaseInputs(held []uint16) []input {
	if len(held) == 0 {
		return nil
	}
	out := []input{keyInput(vkMask, false), keyInput(vkMask, true)}
	for _, vk := range held {
		out = append(out, keyInput(vk, true))
	}
	return out
}

// pasteInputs is the paste shortcut in two steps: the keys down, then up.
// Held modifiers other than the shortcut's go up first.
func pasteInputs(held []uint16, terminal bool) (first, second []input) {
	mod, key := uint16(vkLControl), uint16(vkV)
	if terminal {
		mod, key = vkLShift, vkInsert
	}
	first = append(first, keyInput(mod, false))
	for _, vk := range held {
		if vk != mod {
			first = append(first, keyInput(vk, true))
		}
	}
	first = append(first, keyInput(key, false))
	second = []input{keyInput(key, true), keyInput(mod, true)}
	return first, second
}

// terminalClasses paste with Shift+Insert: Ctrl+V reaches the shell there.
var terminalClasses = []string{"mintty", "PuTTY", "VirtualConsoleClass", "ConsoleWindowClass"}

func isTerminalClass(class string) bool {
	for _, c := range terminalClasses {
		if strings.EqualFold(class, c) {
			return true
		}
	}
	return false
}

func windowClass(hwnd uintptr) string {
	buf := make([]uint16, 256)
	n, _, _ := procGetClassNameW.Call(hwnd, uintptr(unsafe.Pointer(&buf[0])), uintptr(len(buf)))
	return windows.UTF16ToString(buf[:n])
}

func typeText(text string) error {
	if err := sendInputs(releaseInputs(heldModifiers(true))); err != nil {
		return err
	}
	for i, chunk := range utf16Chunks(strings.ReplaceAll(text, "\r\n", "\n"), maxUnitsPerEvent) {
		if i > 0 {
			time.Sleep(chunkDelay)
		}
		if err := sendInputs(textInputs(chunk)); err != nil {
			return fmt.Errorf("Windows took fewer synthetic keys than sent: %w", err)
		}
	}
	return nil
}

func (w windowsInserter) Insert(text string, opts InsertOptions) InsertResult {
	start := time.Now()
	res := InsertResult{Backend: "sendinput"}
	done := func() InsertResult { res.Elapsed = time.Since(start); return res }
	hwnd, _, _ := procGetForegroundWindow.Call()
	if hwnd == 0 {
		res.Method, res.Err = "not_inserted", errors.New("no window is in front")
		return done()
	}
	if opts.TrailingSpace && text != "" && !strings.HasSuffix(text, " ") && !strings.HasSuffix(text, "\n") {
		text += " "
	}
	if text == "" {
		res.Method = "not_inserted"
		return done()
	}
	pasted, err := w.paste(text, opts, isTerminalClass(windowClass(hwnd)), &res)
	switch {
	case pasted:
		res.Method = "pasted"
	case opts.FallbackToType:
		if terr := typeText(text); terr != nil {
			res.Method, res.Err = "failed", errors.Join(err, terr)
			return done()
		}
		res.Method, res.Backend = "typed", "unicode"
	default:
		res.Method, res.Err = "failed", err
		return done()
	}
	if opts.PressEnterAfter {
		time.Sleep(keyGap)
		if err := typeText("\n"); err != nil {
			res.Err = err
		}
	}
	return done()
}

func (windowsInserter) paste(text string, opts InsertOptions, terminal bool, res *InsertResult) (bool, error) {
	var saved []clipEntry
	if opts.RestoreClipboard {
		var err error
		if saved, err = snapshotClipboard(); err != nil {
			return false, err
		}
	}
	seq, err := putText(text, opts.RestoreClipboard)
	if err != nil {
		return false, err
	}
	first, second := pasteInputs(heldModifiers(false), terminal)
	if err := sendInputs(first); err != nil {
		_ = sendInputs(second)
		if opts.RestoreClipboard {
			res.ClipboardRestored = restoreClipboard(saved)
		}
		return false, err
	}
	time.Sleep(keyGap)
	if err := sendInputs(second); err != nil {
		return false, err
	}
	if opts.RestoreClipboard {
		delay := opts.RestoreDelay
		if delay <= 0 {
			delay = defaultRestoreDelay
		}
		time.Sleep(delay)
		// Something else copied in the meantime: its content stays.
		if now, _, _ := procGetClipboardSequenceNum.Call(); uint32(now) == seq {
			res.ClipboardRestored = restoreClipboard(saved)
		}
	}
	return true, nil
}
