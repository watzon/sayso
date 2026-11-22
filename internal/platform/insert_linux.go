package platform

// Text insertion on X11 and Wayland.
//
// Paste: save the text of the clipboard and the primary selection, put our
// text on both (Shift+Insert pastes the primary selection in some X11 apps
// and the clipboard in others), send the paste key, and after the restore
// delay put the old text back, unless someone copied something newer.
// Type: send the text as key presses.
//
//	Session              Clipboard               Keys
//	X11                  xclip, else xsel        XTest, typing with xdotool
//	Wayland (wlroots)    wl-copy / wl-paste      wtype, else ydotool
//	Wayland (GNOME, KDE) wl-copy / wl-paste      ydotool (needs /dev/uinput)
//
// The paste key is Shift+Insert; SAYSO_PASTE_KEY=ctrl+v or ctrl+shift+v
// changes it, as in the Rust version. Only text is saved and restored; an
// image on the clipboard is lost by a paste with restore. The Rust version
// serves the selections itself and also uses the remote desktop portal and
// the virtual keyboard protocol; those need native Wayland code and are not
// ported. Linux cannot see a missing text field, so the result is never
// "not_inserted" for that reason.

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"sync"
	"time"
)

type pasteKey uint8

const (
	pasteShiftInsert pasteKey = iota
	pasteCtrlV
	pasteCtrlShiftV
)

// parsePasteKey reads SAYSO_PASTE_KEY: shift+insert, ctrl+v, or ctrl+shift+v,
// in any order and case.
func parsePasteKey(value string) (pasteKey, bool) {
	parts := map[string]bool{}
	for _, p := range strings.Split(strings.ToLower(value), "+") {
		if p = strings.TrimSpace(p); p != "" {
			parts[p] = true
		}
	}
	switch {
	case len(parts) == 2 && parts["shift"] && parts["insert"]:
		return pasteShiftInsert, true
	case len(parts) == 2 && parts["ctrl"] && parts["v"]:
		return pasteCtrlV, true
	case len(parts) == 3 && parts["ctrl"] && parts["shift"] && parts["v"]:
		return pasteCtrlShiftV, true
	}
	return pasteShiftInsert, false
}

// evdevCodes are the keys of the paste combination, modifiers first.
func (k pasteKey) evdevCodes() []uint16 {
	switch k {
	case pasteCtrlV:
		return []uint16{evKeyLeftCtrl, evKeyV}
	case pasteCtrlShiftV:
		return []uint16{evKeyLeftCtrl, evKeyLeftShift, evKeyV}
	}
	return []uint16{evKeyLeftShift, evKeyInsert}
}

// xdotoolName and wtype arguments for the paste combination.
func (k pasteKey) xdotoolName() string {
	return map[pasteKey]string{pasteShiftInsert: "shift+Insert", pasteCtrlV: "ctrl+v", pasteCtrlShiftV: "ctrl+shift+v"}[k]
}

func (k pasteKey) wtypeArgs() []string {
	switch k {
	case pasteCtrlV:
		return []string{"-M", "ctrl", "-k", "v", "-m", "ctrl"}
	case pasteCtrlShiftV:
		return []string{"-M", "ctrl", "-M", "shift", "-k", "v", "-m", "shift", "-m", "ctrl"}
	}
	return []string{"-M", "shift", "-k", "Insert", "-m", "shift"}
}

// ydotoolArgs presses the codes in order and releases them in reverse.
func ydotoolArgs(codes []uint16) []string {
	args := []string{"key"}
	for _, c := range codes {
		args = append(args, fmt.Sprintf("%d:1", c))
	}
	for i := len(codes) - 1; i >= 0; i-- {
		args = append(args, fmt.Sprintf("%d:0", codes[i]))
	}
	return args
}

// selection is the clipboard or the primary selection.
type selection uint8

const (
	selClipboard selection = iota
	selPrimary
)

type linuxInserter struct {
	session  linuxSession
	lookPath func(string) (string, error)
	mu       sync.Mutex
}

func newLinuxInserter(s linuxSession) *linuxInserter {
	return &linuxInserter{session: s, lookPath: exec.LookPath}
}

func (l *linuxInserter) has(tool string) bool {
	_, err := l.lookPath(tool)
	return err == nil
}

// clipboardTool is the command that reads or writes a selection, or empty
// when no tool is installed.
func (l *linuxInserter) clipboardCommand(sel selection, write bool) []string {
	if l.session.kind == sessionWayland && l.has("wl-copy") {
		var args []string
		if write {
			args = []string{"wl-copy"}
		} else {
			args = []string{"wl-paste", "--no-newline"}
		}
		if sel == selPrimary {
			args = append(args, "--primary")
		}
		return args
	}
	if l.session.hasX11 {
		name := map[selection]string{selClipboard: "clipboard", selPrimary: "primary"}[sel]
		if l.has("xclip") {
			if write {
				return []string{"xclip", "-selection", name, "-in"}
			}
			return []string{"xclip", "-selection", name, "-out"}
		}
		if l.has("xsel") {
			if write {
				return []string{"xsel", "--" + name, "--input"}
			}
			return []string{"xsel", "--" + name, "--output"}
		}
	}
	return nil
}

var errNoClipboardTool = errors.New("no clipboard tool: install wl-clipboard (Wayland) or xclip (X11)")

// runTool runs a helper program. With capture, it returns the output.
// Without, the output is not a pipe: wl-copy and xclip stay behind to serve
// the selection, and a pipe they hold open would block the wait.
func runTool(args []string, stdin string, capture bool) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, args[0], args[1:]...)
	cmd.Stdin = strings.NewReader(stdin)
	var out, errOut bytes.Buffer
	if capture {
		cmd.Stdout, cmd.Stderr = &out, &errOut
	}
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("%s: %v %s", args[0], err, strings.TrimSpace(errOut.String()))
	}
	return out.String(), nil
}

func (l *linuxInserter) readSelection(sel selection) (string, error) {
	args := l.clipboardCommand(sel, false)
	if args == nil {
		return "", errNoClipboardTool
	}
	out, err := runTool(args, "", true)
	if err != nil {
		// An empty selection is an error for wl-paste and xclip.
		return "", nil
	}
	return out, nil
}

func (l *linuxInserter) writeSelection(sel selection, text string) error {
	args := l.clipboardCommand(sel, true)
	if args == nil {
		return errNoClipboardTool
	}
	if text == "" && args[0] == "wl-copy" {
		args = append(args, "--clear")
	}
	_, err := runTool(args, text, false)
	return err
}

func (l *linuxInserter) ReadClipboard() (string, error) {
	return l.readSelection(selClipboard)
}

func (l *linuxInserter) WriteClipboard(text string) error {
	return l.writeSelection(selClipboard, text)
}

// pressKeys sends a key combination of evdev codes.
func (l *linuxInserter) pressKeys(codes []uint16, xdotool string, wtype []string) error {
	switch {
	case l.session.kind == sessionX11:
		if err := x11PressKeys(codes...); err == nil {
			return nil
		}
		if l.has("xdotool") {
			_, err := runTool([]string{"xdotool", "key", "--clearmodifiers", xdotool}, "", true)
			return err
		}
	case l.session.kind == sessionWayland:
		if l.session.desktop != desktopGNOME && l.session.desktop != desktopKDE && l.has("wtype") {
			_, err := runTool(append([]string{"wtype"}, wtype...), "", true)
			return err
		}
		if l.has("ydotool") {
			_, err := runTool(append([]string{"ydotool"}, ydotoolArgs(codes)...), "", true)
			return err
		}
		if l.session.hasX11 {
			// XWayland reaches X11 apps only, but that is better than nothing.
			return x11PressKeys(codes...)
		}
	}
	return errors.New("no way to send keys: install xdotool (X11), wtype (wlroots), or ydotool")
}

func (l *linuxInserter) typeText(text string) error {
	switch {
	case l.session.kind == sessionX11 && l.has("xdotool"):
		_, err := runTool([]string{"xdotool", "type", "--clearmodifiers", "--", text}, "", true)
		return err
	case l.session.kind == sessionWayland && l.session.desktop != desktopGNOME && l.session.desktop != desktopKDE && l.has("wtype"):
		_, err := runTool([]string{"wtype", "--", text}, "", true)
		return err
	case l.has("ydotool"):
		_, err := runTool([]string{"ydotool", "type", "--", text}, "", true)
		return err
	}
	return errors.New("no way to type: install xdotool (X11), wtype (wlroots), or ydotool")
}

func (l *linuxInserter) pressEnter() error {
	return l.pressKeys([]uint16{evKeyEnter}, "Return", []string{"-k", "Return"})
}

// canInject: some way to send keys exists in this session.
func (l *linuxInserter) canInject() bool {
	switch l.session.kind {
	case sessionX11:
		return true // XTest is part of every X server we target
	case sessionWayland:
		return l.has("wtype") || l.has("ydotool") || l.session.hasX11
	}
	return false
}

func (l *linuxInserter) Insert(text string, opts InsertOptions) InsertResult {
	start := time.Now()
	r := l.insert(text, opts)
	r.Backend = map[sessionKind]string{sessionX11: "x11", sessionWayland: "wayland", sessionNone: "none"}[l.session.kind]
	r.Elapsed = time.Since(start)
	return r
}

func (l *linuxInserter) insert(text string, opts InsertOptions) InsertResult {
	l.mu.Lock()
	defer l.mu.Unlock()
	if !l.canInject() {
		return InsertResult{Method: "not_inserted", Err: errors.New("no way to send keys in this session")}
	}
	if opts.TrailingSpace {
		text += " "
	}
	result, err := l.paste(text, opts)
	if err == nil {
		return result
	}
	if !opts.FallbackToType {
		return InsertResult{Method: "failed", Err: err}
	}
	if terr := l.typeText(text); terr != nil {
		return InsertResult{Method: "failed", Err: errors.Join(err, terr)}
	}
	if opts.PressEnterAfter {
		_ = l.pressEnter()
	}
	return InsertResult{Method: "typed"}
}

func (l *linuxInserter) paste(text string, opts InsertOptions) (InsertResult, error) {
	key := pasteShiftInsert
	if v := os.Getenv("SAYSO_PASTE_KEY"); v != "" {
		key, _ = parsePasteKey(v)
	}
	var savedClip, savedPrimary string
	if opts.RestoreClipboard {
		var err error
		if savedClip, err = l.readSelection(selClipboard); err != nil {
			return InsertResult{}, err
		}
		savedPrimary, _ = l.readSelection(selPrimary)
	}
	if err := l.writeSelection(selClipboard, text); err != nil {
		return InsertResult{}, err
	}
	_ = l.writeSelection(selPrimary, text)
	// Clipboard managers read a new clipboard at once; let them finish before
	// the paste key, so the app reads our text.
	time.Sleep(50 * time.Millisecond)
	if err := l.pressKeys(key.evdevCodes(), key.xdotoolName(), key.wtypeArgs()); err != nil {
		return InsertResult{}, err
	}
	if opts.PressEnterAfter {
		time.Sleep(pasteSettleLinux)
		_ = l.pressEnter()
	}
	if !opts.RestoreClipboard {
		return InsertResult{Method: "pasted"}, nil
	}
	delay := opts.RestoreDelay
	if delay <= 0 {
		delay = 450 * time.Millisecond
	}
	time.Sleep(delay)
	restored := false
	// Someone copied something newer: that content stays.
	if now, err := l.readSelection(selClipboard); err == nil && now == text {
		restored = l.writeSelection(selClipboard, savedClip) == nil
	}
	if now, err := l.readSelection(selPrimary); err == nil && now == text {
		_ = l.writeSelection(selPrimary, savedPrimary)
	}
	return InsertResult{Method: "pasted", ClipboardRestored: restored}, nil
}

// pasteSettleLinux is the wait between the paste key and Return.
const pasteSettleLinux = 120 * time.Millisecond
