package testbin

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"

	"sync/atomic"
	"testing"
	"time"
)

// Host is a running sayso-testhost.
type Host struct {
	t      testing.TB
	Socket string
	cmd    *exec.Cmd
	stdin  io.WriteCloser
	lines  chan map[string]any
	exited chan struct{}
}

var hostCount atomic.Int64

// SocketPath is a new socket path (a pipe name on Windows) for a test.
func SocketPath(t testing.TB) string {
	t.Helper()
	n := hostCount.Add(1)
	if runtime.GOOS == "windows" {
		return fmt.Sprintf(`\\.\pipe\sayso-sdk-test-%d-%d`, os.Getpid(), n)
	}
	dir, err := os.MkdirTemp("/tmp", "ssdk")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.RemoveAll(dir) })
	return filepath.Join(dir, "s", "host.sock")
}

// StartHost starts the test host with the fake engine and args, and waits
// until it is ready. The host stops at the end of the test.
func StartHost(t testing.TB, args ...string) *Host {
	t.Helper()
	return StartHostAt(t, SocketPath(t), args...)
}

// StartHostAt starts the test host on socket.
func StartHostAt(t testing.TB, socket string, args ...string) *Host {
	t.Helper()
	program := Path(t, TestHost)
	engine := Path(t, FakeEngine)
	all := append([]string{"--socket", socket, "--engine", engine, "--data", t.TempDir(), "--engine-env", `SAYSO_FAKE_SCRIPT={"accept_any":true}`}, args...)
	cmd := exec.Command(program, all...)
	stdin, err := cmd.StdinPipe()
	if err != nil {
		t.Fatal(err)
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		t.Fatal(err)
	}
	if testing.Verbose() {
		cmd.Stderr = os.Stderr
	}
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	h := &Host{t: t, Socket: socket, cmd: cmd, stdin: stdin, lines: make(chan map[string]any, 256), exited: make(chan struct{})}
	go func() {
		s := bufio.NewScanner(stdout)
		for s.Scan() {
			var m map[string]any
			if json.Unmarshal(s.Bytes(), &m) == nil {
				h.lines <- m
			}
		}
		cmd.Wait()
		close(h.exited)
	}()
	t.Cleanup(h.Stop)
	h.Await("ready")
	return h
}

// Await waits for a line on stdout with the key (and the value, when given).
func (h *Host) Await(key string, value ...string) map[string]any {
	h.t.Helper()
	timeout := time.After(20 * time.Second)
	for {
		select {
		case m := <-h.lines:
			v, ok := m[key]
			if ok && (len(value) == 0 || fmt.Sprint(v) == value[0]) {
				return m
			}
		case <-h.exited:
			h.t.Fatalf("the test host exited while the test waited for %s %v", key, value)
		case <-timeout:
			h.t.Fatalf("the test host printed no %s %v", key, value)
		}
	}
}

// Send writes a command on stdin.
func (h *Host) Send(cmd map[string]any) {
	h.t.Helper()
	b, _ := json.Marshal(cmd)
	if _, err := h.stdin.Write(append(b, '\n')); err != nil {
		h.t.Fatal(err)
	}
}

// Exited closes when the host exited.
func (h *Host) Exited() <-chan struct{} { return h.exited }

// Stop closes the host like Sayso quits, and kills it after 10 s.
func (h *Host) Stop() {
	select {
	case <-h.exited:
		return
	default:
	}
	h.stdin.Write([]byte(`{"close":true}` + "\n"))
	select {
	case <-h.exited:
	case <-time.After(10 * time.Second):
		h.cmd.Process.Kill()
		<-h.exited
	}
}

// Env is SAYSO_HOST_SOCKET for the socket.
func (h *Host) Env() string { return "SAYSO_HOST_SOCKET=" + h.Socket }
