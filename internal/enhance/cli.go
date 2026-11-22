package enhance

// Run a command line tool that talks in lines, with a hard deadline, for the
// Claude provider (sayso-enhance cli.rs).

import (
	"bufio"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"os/exec"
	"strings"
	"sync"
	"time"
)

// invocation is one tool run.
type invocation struct {
	program string
	args    []string
	// envs are extra variables, "NAME=value". The rest of the environment is
	// inherited, because the tool needs HOME to find its own login.
	envs []string
	// dir is an empty folder: the tool must not see a project.
	dir     string
	timeout time.Duration
}

// stderrKeep is how much error output a session keeps.
const stderrKeep = 4096

// cliSession is a running tool: write a line, read lines until one matches.
// Every read stops at the deadline. close kills the tool and waits for it, so
// no path leaves a process behind.
type cliSession struct {
	cmd    *exec.Cmd
	stdin  io.WriteCloser
	lines  chan string
	exited chan struct{}

	stderrMu   sync.Mutex
	stderr     []byte
	stderrDone chan struct{}

	deadline time.Time
	timeout  time.Duration
	closed   sync.Once
	// onClose runs after the tool has stopped (removes its folder).
	onClose func()
}

// startSession starts the tool. The deadline is inv.timeout from now.
func startSession(inv invocation) (*cliSession, error) {
	var lastErr error
	for attempt := 0; attempt < 20; attempt++ {
		s, err := trySession(inv)
		if err == nil {
			return s, nil
		}
		lastErr = err
		// ETXTBSY: another goroutine still holds a freshly written script open
		// for writing. It clears within milliseconds.
		if !isTextBusy(err) {
			break
		}
		time.Sleep(25 * time.Millisecond)
	}
	msg := fmt.Sprintf("could not start %s: %v", inv.program, lastErr)
	if errors.Is(lastErr, exec.ErrNotFound) || errors.Is(lastErr, fs.ErrNotExist) || errors.Is(lastErr, fs.ErrPermission) {
		return nil, notConfigured(msg)
	}
	return nil, &Error{Kind: ErrCLI, Message: msg}
}

func trySession(inv invocation) (*cliSession, error) {
	cmd := newToolCommand(inv.program, inv.args)
	cmd.Env = append(os.Environ(), inv.envs...)
	cmd.Dir = inv.dir
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	// Own pipes rather than StdoutPipe, so Wait in the exit watcher never
	// closes a pipe before its reader has read all of it.
	outR, outW, err := os.Pipe()
	if err != nil {
		return nil, err
	}
	errR, errW, err := os.Pipe()
	if err != nil {
		outR.Close()
		outW.Close()
		return nil, err
	}
	cmd.Stdout, cmd.Stderr = outW, errW
	if err := cmd.Start(); err != nil {
		for _, f := range []*os.File{outR, outW, errR, errW} {
			f.Close()
		}
		return nil, err
	}
	outW.Close()
	errW.Close()
	s := &cliSession{cmd: cmd, stdin: stdin, lines: make(chan string, 64), exited: make(chan struct{}),
		stderrDone: make(chan struct{}), deadline: time.Now().Add(inv.timeout), timeout: inv.timeout}
	go func() {
		_ = cmd.Wait()
		close(s.exited)
	}()
	go func() {
		defer close(s.lines)
		defer outR.Close()
		r := bufio.NewReaderSize(outR, 64*1024)
		for {
			line, err := r.ReadString('\n')
			if line != "" {
				s.lines <- strings.TrimRight(line, "\r\n")
			}
			if err != nil {
				return
			}
		}
	}()
	go func() {
		defer close(s.stderrDone)
		defer errR.Close()
		buf := make([]byte, 4096)
		for {
			n, err := errR.Read(buf)
			if n > 0 {
				s.stderrMu.Lock()
				s.stderr = append(s.stderr, buf[:n]...)
				if len(s.stderr) > stderrKeep {
					s.stderr = append([]byte(nil), s.stderr[len(s.stderr)-stderrKeep:]...)
				}
				s.stderrMu.Unlock()
			}
			if err != nil {
				return
			}
		}
	}()
	return s, nil
}

// restartDeadline starts a new deadline, left from now, for a tool that was
// started before the request that uses it. A timeout error names total.
func (s *cliSession) restartDeadline(left, total time.Duration) {
	s.deadline = time.Now().Add(left)
	s.timeout = total
}

// running is true while the tool runs.
func (s *cliSession) running() bool {
	select {
	case <-s.exited:
		return false
	default:
		return true
	}
}

// send writes one line. The input stays open afterwards.
func (s *cliSession) send(line string) error {
	if _, err := io.WriteString(s.stdin, line+"\n"); err != nil {
		return &Error{Kind: ErrCLI, Message: "could not write to the tool: " + err.Error()}
	}
	return nil
}

// errClosedOutput is the message of a tool that stopped before it answered.
const errClosedOutput = "the tool closed its output before it answered"

// readUntil reads lines until pick returns true. Lines it rejects are skipped.
func (s *cliSession) readUntil(pick func(line string) bool) error {
	timer := time.NewTimer(max(time.Until(s.deadline), 0))
	defer timer.Stop()
	for {
		select {
		case line, ok := <-s.lines:
			if !ok {
				return &Error{Kind: ErrCLI, Message: errClosedOutput}
			}
			if pick(line) {
				return nil
			}
		case <-timer.C:
			return &Error{Kind: ErrTimeout, Ms: uint64(s.timeout.Milliseconds())}
		}
	}
}

// stderrTail is the last lines of the error output. It waits a moment for a
// tool that just stopped to finish writing it.
func (s *cliSession) stderrTail() string {
	select {
	case <-s.stderrDone:
	case <-time.After(300 * time.Millisecond):
	}
	s.stderrMu.Lock()
	defer s.stderrMu.Unlock()
	return tail(string(s.stderr))
}

// close kills the tool and waits for it.
func (s *cliSession) close() {
	s.closed.Do(func() {
		_ = s.stdin.Close()
		if s.cmd.Process != nil {
			_ = s.cmd.Process.Kill()
		}
		<-s.exited
		if s.onClose != nil {
			s.onClose()
		}
	})
}

// looksLikeLoginProblem is true when a tool's message says the user is not signed in.
func looksLikeLoginProblem(message string) bool {
	lower := strings.ToLower(message)
	for _, needle := range []string{"/login", "not logged in", "log in", "sign in", "signed in", "api key", "unauthorized", "authenticat"} {
		if strings.Contains(lower, needle) {
			return true
		}
	}
	return false
}

// tail is the last few lines of tool output, for an error message.
func tail(text string) string {
	lines := strings.Split(strings.ReplaceAll(text, "\r\n", "\n"), "\n")
	var kept []string
	for i := len(lines) - 1; i >= 0 && len(kept) < 4; i-- {
		if lines[i] == "" && len(kept) == 0 {
			continue
		}
		kept = append([]string{lines[i]}, kept...)
	}
	joined := []rune(strings.Join(kept, " | "))
	if len(joined) > 400 {
		joined = joined[:400]
	}
	return string(joined)
}
