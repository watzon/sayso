package platform

// Text insertion on macOS. Ported from sayso-platform-macos/src/inserter.rs.
//
// Paste: save every pasteboard item with every type, put the text on the
// pasteboard (marked transient, so clipboard managers skip it), post Cmd+V,
// and after the restore delay put the old items back, unless someone copied
// something newer meanwhile. Type: post key events that carry the text as
// Unicode strings, 20 UTF-16 units per event.
//
// The Rust version publishes the text as a pasteboard promise and waits for
// the app to read it. That needs an Objective-C owner served by the main run
// loop; this version waits a fixed delay instead (InsertOptions.RestoreDelay,
// 450 ms when zero).
//
// Both methods need Accessibility to post events.

import (
	"errors"
	"sync"
	"time"
	"unicode/utf16"

	"github.com/ebitengine/purego/objc"
)

const (
	// maxUnitsPerEvent is the most UTF-16 units one key event carries.
	maxUnitsPerEvent    = 20
	typeChunkDelay      = 5 * time.Millisecond
	keyGap              = 8 * time.Millisecond
	defaultRestoreDelay = 450 * time.Millisecond
	// pasteSettle is the wait between Cmd+V and Return, so the app has the
	// text before it sends.
	pasteSettle = 120 * time.Millisecond
	// transientType tells clipboard managers to skip an entry (nspasteboard.org).
	transientType = "org.nspasteboard.TransientType"
)

var errNotTrusted = errors.New("cannot type into other apps: Accessibility is not granted")

type macInserter struct {
	// mu runs one insert at a time, including its clipboard restore.
	mu sync.Mutex
}

// pasteboardSnapshot is every item on the pasteboard, each a list of
// (type, bytes) pairs.
type pasteboardSnapshot [][]pasteboardEntry

type pasteboardEntry struct {
	uti  string
	data []byte
}

func generalPasteboard() objc.ID {
	return class("NSPasteboard").Send(sel("generalPasteboard"))
}

func pasteboardChangeCount(pb objc.ID) int64 {
	return objc.Send[int64](pb, sel("changeCount"))
}

func snapshotPasteboard() (snap pasteboardSnapshot) {
	withPool(func() {
		items := generalPasteboard().Send(sel("pasteboardItems"))
		if items == 0 {
			return
		}
		n := objc.Send[uint64](items, sel("count"))
		for i := uint64(0); i < n; i++ {
			item := items.Send(sel("objectAtIndex:"), i)
			types := item.Send(sel("types"))
			tn := objc.Send[uint64](types, sel("count"))
			var entries []pasteboardEntry
			for j := uint64(0); j < tn; j++ {
				ty := types.Send(sel("objectAtIndex:"), j)
				data := item.Send(sel("dataForType:"), ty)
				if data == 0 {
					continue
				}
				entries = append(entries, pasteboardEntry{uti: goStringFromCF(uintptr(ty)), data: goBytes(data)})
			}
			snap = append(snap, entries)
		}
	})
	return snap
}

// restorePasteboard writes a snapshot back. It returns the new change count.
func restorePasteboard(snap pasteboardSnapshot) (count int64, ok bool) {
	withPool(func() {
		pb := generalPasteboard()
		pb.Send(sel("clearContents"))
		ok = true
		if len(snap) > 0 {
			array := class("NSMutableArray").Send(sel("array"))
			for _, entries := range snap {
				item := class("NSPasteboardItem").Send(sel("alloc")).Send(sel("init"))
				for _, e := range entries {
					objc.Send[bool](item, sel("setData:forType:"), nsData(e.data), nsString(e.uti))
				}
				array.Send(sel("addObject:"), item)
				item.Send(sel("release"))
			}
			ok = objc.Send[bool](pb, sel("writeObjects:"), array)
		}
		count = pasteboardChangeCount(pb)
	})
	return count, ok
}

// writePasteboardText replaces the pasteboard with text. It returns the new
// change count.
func writePasteboardText(text string, transient bool) (count int64, ok bool) {
	withPool(func() {
		pb := generalPasteboard()
		pb.Send(sel("clearContents"))
		ok = objc.Send[bool](pb, sel("setString:forType:"), nsString(text), nsPasteboardTypeString)
		if transient {
			objc.Send[bool](pb, sel("setData:forType:"), nsData(nil), nsString(transientType))
		}
		count = pasteboardChangeCount(pb)
	})
	return count, ok
}

func (*macInserter) ReadClipboard() (string, error) {
	if err := loadDarwin(); err != nil {
		return "", err
	}
	var s string
	withPool(func() {
		s = goStringFromCF(uintptr(generalPasteboard().Send(sel("stringForType:"), nsPasteboardTypeString)))
	})
	return s, nil
}

func (*macInserter) WriteClipboard(text string) error {
	if err := loadDarwin(); err != nil {
		return err
	}
	if _, ok := writePasteboardText(text, false); !ok {
		return errors.New("cannot write the clipboard")
	}
	return nil
}

// macCanReceiveText is false only when Accessibility says that nothing is
// focused and the frontmost app has no focused window. "No value" for the
// focused element alone does not prove it: Electron apps answer that with
// the cursor in a field. Any other error is unknown and must not block.
func macCanReceiveText() bool {
	system := axUIElementCreateSystemWide()
	if system == 0 {
		return true
	}
	element := axAttributeStatus(system, "AXFocusedUIElement")
	cfRelease(system)
	if element != kAXErrorNoValue {
		return true
	}
	var pid int32
	withPool(func() {
		if front := class("NSWorkspace").Send(sel("sharedWorkspace")).Send(sel("frontmostApplication")); front != 0 {
			pid = objc.Send[int32](front, sel("processIdentifier"))
		}
	})
	if pid <= 0 {
		return false
	}
	app := axUIElementCreateApplication(pid)
	if app == 0 {
		return true
	}
	defer cfRelease(app)
	return axAttributeStatus(app, "AXFocusedWindow") != kAXErrorNoValue
}

func axAttributeStatus(element uintptr, name string) int32 {
	attr := cfString(name)
	defer cfRelease(attr)
	var value uintptr
	status := axUIElementCopyAttributeValue(element, attr, &value)
	if status == kAXErrorSuccess && value != 0 {
		cfRelease(value)
	}
	return status
}

// utf16Chunks splits text into chunks of at most max UTF-16 units without
// cutting a surrogate pair. max must be at least 2.
func utf16Chunks(text string, max int) [][]uint16 {
	var chunks [][]uint16
	var current []uint16
	for _, r := range text {
		units := utf16.Encode([]rune{r})
		if len(current)+len(units) > max {
			chunks = append(chunks, current)
			current = nil
		}
		current = append(current, units...)
	}
	if len(current) > 0 {
		chunks = append(chunks, current)
	}
	return chunks
}

// postKey posts one key event from a private source, so keys that the user
// still holds (the Right Option of push-to-talk) are not added.
func postKey(source uintptr, code uint16, down bool, flags uint64, unicode []uint16) error {
	ev := cgEventCreateKeyboardEvent(source, code, down)
	if ev == 0 {
		return errors.New("cannot create a keyboard event")
	}
	cgEventSetFlags(ev, flags)
	if len(unicode) > 0 {
		cgEventKeyboardSetUnicodeStr(ev, uint64(len(unicode)), &unicode[0])
	}
	cgEventPost(kCGHIDEventTap, ev)
	cfRelease(ev)
	return nil
}

func pressKey(code uint16, flags uint64) error {
	source := cgEventSourceCreate(kCGEventSourceStatePrivate)
	if source != 0 {
		defer cfRelease(source)
	}
	if err := postKey(source, code, true, flags, nil); err != nil {
		return err
	}
	time.Sleep(keyGap)
	return postKey(source, code, false, flags, nil)
}

func typeText(text string) error {
	source := cgEventSourceCreate(kCGEventSourceStatePrivate)
	if source != 0 {
		defer cfRelease(source)
	}
	for _, chunk := range utf16Chunks(text, maxUnitsPerEvent) {
		if err := postKey(source, 0, true, 0, chunk); err != nil {
			return err
		}
		if err := postKey(source, 0, false, 0, chunk); err != nil {
			return err
		}
		time.Sleep(typeChunkDelay)
	}
	return nil
}

func (m *macInserter) Insert(text string, opts InsertOptions) InsertResult {
	start := time.Now()
	result := m.insert(text, opts)
	result.Backend = "macos"
	result.Elapsed = time.Since(start)
	return result
}

func (m *macInserter) insert(text string, opts InsertOptions) InsertResult {
	if err := loadDarwin(); err != nil {
		return InsertResult{Method: "failed", Err: err}
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	if !axIsProcessTrusted() {
		return InsertResult{Method: "not_inserted", Err: errNotTrusted}
	}
	if !macCanReceiveText() {
		return InsertResult{Method: "not_inserted", Err: errors.New("no focused text field")}
	}
	if opts.TrailingSpace {
		text += " "
	}
	result, err := m.paste(text, opts)
	if err == nil {
		return result
	}
	if !opts.FallbackToType {
		return InsertResult{Method: "failed", Err: err}
	}
	if terr := typeText(text); terr != nil {
		return InsertResult{Method: "failed", Err: errors.Join(err, terr)}
	}
	m.pressEnterAfter(opts)
	return InsertResult{Method: "typed"}
}

func (m *macInserter) paste(text string, opts InsertOptions) (InsertResult, error) {
	var saved pasteboardSnapshot
	if opts.RestoreClipboard {
		saved = snapshotPasteboard()
	}
	ours, ok := writePasteboardText(text, opts.RestoreClipboard)
	if !ok {
		return InsertResult{}, errors.New("cannot write the clipboard")
	}
	if err := pressKey(kcV, cgFlagCommand); err != nil {
		if opts.RestoreClipboard {
			restorePasteboard(saved)
		}
		return InsertResult{}, err
	}
	// Return goes out while the text is still on the clipboard: some apps
	// read the pasteboard late.
	if opts.PressEnterAfter {
		time.Sleep(pasteSettle)
		m.pressEnterAfter(opts)
	}
	if !opts.RestoreClipboard {
		return InsertResult{Method: "pasted"}, nil
	}
	delay := opts.RestoreDelay
	if delay <= 0 {
		delay = defaultRestoreDelay
	}
	time.Sleep(delay)
	restored := false
	var count int64
	withPool(func() { count = pasteboardChangeCount(generalPasteboard()) })
	// Someone copied something newer: that content stays.
	if count == ours {
		_, restored = restorePasteboard(saved)
	}
	return InsertResult{Method: "pasted", ClipboardRestored: restored}, nil
}

// pressEnterAfter presses Return without the modifiers the user may hold.
func (m *macInserter) pressEnterAfter(opts InsertOptions) {
	if opts.PressEnterAfter {
		_ = pressKey(kcReturn, 0)
	}
}
