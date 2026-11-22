package platform

// Commands from the shell: `sayso --toggle` and the others reach the running
// Sayso through a Unix socket, one word per line. A desktop without global
// shortcuts for apps can bind these commands to keys in its own settings.
// Ported from sayso-platform-linux/src/ipc.rs (the command part).

import (
	"bufio"
	"errors"
	"fmt"
	"log"
	"net"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"time"
)

// commandEvents maps a command word to the hotkey events it sends.
func commandEvents(word string) ([]HotkeyEvent, bool) {
	word = strings.TrimSpace(word)
	if id, ok := strings.CutPrefix(word, "style="); ok {
		if id == "" || strings.Contains(id, "\n") {
			return nil, false
		}
		return []HotkeyEvent{{Kind: HotkeyStyle, StyleID: id}}, true
	}
	switch word {
	case "toggle":
		return []HotkeyEvent{{Kind: HotkeyToggle}}, true
	case "cancel":
		return []HotkeyEvent{{Kind: HotkeyCancel}}, true
	case "paste-last":
		return []HotkeyEvent{{Kind: HotkeyPasteLast}}, true
	case "cycle-style":
		return []HotkeyEvent{{Kind: HotkeyCycleStyle}}, true
	case "incognito":
		return []HotkeyEvent{{Kind: HotkeyToggleIncognito}}, true
	case "push-to-talk-down":
		return []HotkeyEvent{{Kind: HotkeyPushToTalkDown}}, true
	case "push-to-talk-up":
		return []HotkeyEvent{{Kind: HotkeyPushToTalkUp}}, true
	case "push-to-talk-up-submit":
		// Release, then Enter after the insert: sends a chat message.
		return []HotkeyEvent{{Kind: HotkeyEnterSend}, {Kind: HotkeyPushToTalkUp}}, true
	case "open":
		// Opening the Hub is the app's job; it carries no hotkey event.
		return nil, true
	}
	return nil, false
}

// commandSocketPath is $XDG_RUNTIME_DIR/sayso.sock ($XDG_RUNTIME_DIR/app/<id>
// in a Flatpak), or sayso.sock in a private folder of this user in the temp
// folder. Anyone can write the temp folder, so the socket is not there directly.
func commandSocketPath() (string, error) {
	runtime := os.Getenv("XDG_RUNTIME_DIR")
	flatpak := os.Getenv("FLATPAK_ID")
	switch {
	case runtime != "" && flatpak != "":
		return filepath.Join(runtime, "app", flatpak, "sayso.sock"), nil
	case runtime != "":
		return filepath.Join(runtime, "sayso.sock"), nil
	}
	dir := filepath.Join(os.TempDir(), fmt.Sprintf("sayso-%d", os.Getuid()))
	if err := privateDir(dir); err != nil {
		return "", err
	}
	return filepath.Join(dir, "sayso.sock"), nil
}

// privateDir makes dir for this user only. A folder that is there must be a
// real folder that this user owns and nobody else can open.
func privateDir(dir string) error {
	err := os.Mkdir(dir, 0o700)
	if err == nil || !errors.Is(err, os.ErrExist) {
		return err
	}
	st, err := os.Lstat(dir)
	if err != nil {
		return err
	}
	sys, ok := st.Sys().(*syscall.Stat_t)
	if !st.IsDir() || !ok || int(sys.Uid) != os.Getuid() || st.Mode().Perm()&0o077 != 0 {
		return fmt.Errorf("%s is not a private folder of this user", dir)
	}
	return nil
}

// SendCommand sends one command word ("toggle", "push-to-talk-down",
// "style=<id>", ...) to the running Sayso. The command line flags use it.
func SendCommand(word string) error {
	if _, ok := commandEvents(word); !ok {
		return fmt.Errorf("unknown command %q", word)
	}
	path, err := commandSocketPath()
	if err != nil {
		return err
	}
	conn, err := net.DialTimeout("unix", path, 2*time.Second)
	if err != nil {
		return err
	}
	defer conn.Close()
	_ = conn.SetWriteDeadline(time.Now().Add(2 * time.Second))
	_, err = conn.Write([]byte(word + "\n"))
	return err
}

type commandListener struct {
	ln   net.Listener
	path string
	done chan struct{}
}

// listenCommands serves the socket. When another Sayso answers on it, the
// commands go there, and this returns an error.
func listenCommands(emit func(HotkeyEvent)) (*commandListener, error) {
	path, err := commandSocketPath()
	if err != nil {
		return nil, err
	}
	if c, err := net.DialTimeout("unix", path, 500*time.Millisecond); err == nil {
		c.Close()
		return nil, fmt.Errorf("another Sayso listens on %s", path)
	}
	// A socket nobody answers is left from a crash.
	_ = os.Remove(path)
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return nil, err
	}
	ln, err := net.Listen("unix", path)
	if err != nil {
		return nil, err
	}
	_ = os.Chmod(path, 0o600)
	l := &commandListener{ln: ln, path: path, done: make(chan struct{})}
	go func() {
		defer close(l.done)
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			go serveCommands(conn, emit)
		}
	}()
	return l, nil
}

func serveCommands(conn net.Conn, emit func(HotkeyEvent)) {
	defer conn.Close()
	_ = conn.SetReadDeadline(time.Now().Add(5 * time.Second))
	scanner := bufio.NewScanner(conn)
	scanner.Buffer(make([]byte, 1024), 4096)
	for scanner.Scan() {
		events, ok := commandEvents(scanner.Text())
		if !ok {
			log.Printf("hotkeys: unknown command %q on the socket", scanner.Text())
			continue
		}
		for _, ev := range events {
			emit(ev)
		}
	}
}

func (l *commandListener) close() {
	l.ln.Close()
	<-l.done
	_ = os.Remove(l.path)
}
