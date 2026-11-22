//go:build linux

package bridge

import (
	"bufio"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"syscall"

	"golang.org/x/sys/unix"
)

// The identity of a peer on Linux (crates/sayso-bridge/src/identity.rs):
// the executable of a process in this PID namespace, or the Flatpak app of
// a process in a sandbox.

func systemName(executable string) string { return fileName(executable, false) }

func systemIdentify(peer Peer) (Caller, error) {
	who, err := identifyPID(int(peer.PID))
	if err != nil {
		return Caller{}, err
	}
	if who.flatpak != "" {
		return CallerFromFlatpak(who.flatpak), nil
	}
	return CallerFromPath(who.path), nil
}

type who struct{ path, flatpak string }

func deletedLink(link string) bool { return strings.HasSuffix(link, " (deleted)") }

func identifyPID(pid int) (who, error) {
	namespace := func(p string) (string, error) {
		link, err := os.Readlink("/proc/" + p + "/ns/pid")
		if err != nil {
			return "", fmt.Errorf("no PID namespace of %s: %v", p, err)
		}
		return link, nil
	}
	theirs, err := namespace(strconv.Itoa(pid))
	if err != nil {
		return who{}, err
	}
	ours, err := namespace("self")
	if err != nil {
		return who{}, err
	}
	if theirs == ours {
		return inThisNamespace(pid)
	}
	return inASandbox(pid)
}

func inThisNamespace(pid int) (who, error) {
	exe := fmt.Sprintf("/proc/%d/exe", pid)
	link, err := os.Readlink(exe)
	if err != nil {
		return who{}, fmt.Errorf("no %s: %v", exe, err)
	}
	if deletedLink(link) {
		return who{}, fmt.Errorf("%s was deleted or replaced after it started", link)
	}
	a, err := fileOf(exe)
	if err != nil {
		return who{}, err
	}
	b, err := fileOf(link)
	if err != nil {
		return who{}, err
	}
	if a != b {
		return who{}, fmt.Errorf("process %d does not run the file at %s", pid, link)
	}
	return who{path: link}, nil
}

func fileOf(path string) ([2]uint64, error) {
	var st unix.Stat_t
	if err := unix.Stat(path, &st); err != nil {
		return [2]uint64{}, fmt.Errorf("cannot read %s: %v", path, err)
	}
	return [2]uint64{uint64(st.Dev), st.Ino}, nil
}

func inASandbox(pid int) (who, error) {
	fd, err := unix.Open(fmt.Sprintf("/proc/%d", pid), unix.O_PATH|unix.O_DIRECTORY|unix.O_CLOEXEC, 0)
	if err != nil {
		return who{}, fmt.Errorf("cannot open process %d: %v", pid, err)
	}
	defer unix.Close(fd)
	at := func(name string) string { return fmt.Sprintf("/proc/self/fd/%d/%s", fd, name) }
	status, err := os.ReadFile(at("status"))
	if err != nil {
		return who{}, fmt.Errorf("cannot read the status of process %d: %v", pid, err)
	}
	if err := checkStatus(string(status)); err != nil {
		return who{}, fmt.Errorf("process %d is in another PID namespace, and %v", pid, err)
	}
	text, err := readInfo(at("root/.flatpak-info"))
	if err != nil {
		return who{}, fmt.Errorf("process %d is in another PID namespace, and it has no Flatpak info: %v", pid, err)
	}
	info, err := ParseFlatpakInfo(text)
	if err != nil {
		return who{}, fmt.Errorf("process %d: %v", pid, err)
	}
	roots := installRoots(systemInstallEnv{})
	app, err := deployed(roots, "app", info.AppID, info.AppPath)
	if err != nil {
		return who{}, fmt.Errorf("process %d says that it is %s, and %v", pid, info.AppID, err)
	}
	rt, err := deployed(roots, "runtime", "", info.RuntimePath)
	if err != nil {
		return who{}, fmt.Errorf("process %d says that it is %s, and %v", pid, info.AppID, err)
	}
	a, err := fileOf(at("root/app"))
	if err != nil {
		return who{}, err
	}
	b, err := fileOf(app)
	if err != nil {
		return who{}, err
	}
	if a != b {
		return who{}, fmt.Errorf("process %d says that it is %s, and its /app is another folder", pid, info.AppID)
	}
	link, err := os.Readlink(at("exe"))
	if err != nil {
		return who{}, fmt.Errorf("no executable of process %d: %v", pid, err)
	}
	if deletedLink(link) {
		return who{}, fmt.Errorf("%s was deleted or replaced after it started", link)
	}
	onHost, ok := fileOnHost(link, app, rt)
	if !ok {
		return who{}, fmt.Errorf("process %d says that it is %s, and it runs %s, which is not a file of the app or its runtime", pid, info.AppID, link)
	}
	a, err = fileOf(at("exe"))
	if err != nil {
		return who{}, err
	}
	if b, err = fileOf(onHost); err != nil {
		return who{}, err
	}
	if a != b {
		return who{}, fmt.Errorf("process %d says that it is %s, and it does not run the file at %s", pid, info.AppID, onHost)
	}
	return who{flatpak: info.AppID}, nil
}

func checkStatus(status string) error {
	field := func(name string) (string, bool) {
		for _, line := range strings.Split(status, "\n") {
			if rest, ok := strings.CutPrefix(line, name); ok {
				if rest, ok := strings.CutPrefix(rest, ":"); ok {
					return strings.TrimSpace(rest), true
				}
			}
		}
		return "", false
	}
	levels := 0
	if pids, ok := field("NSpid"); ok {
		levels = len(strings.Fields(pids))
	}
	if levels != 2 {
		return fmt.Errorf("it is %d PID namespaces below this one", max(levels-1, 0))
	}
	noCapability := func(name string) bool {
		mask, ok := field(name)
		return ok && mask != "" && strings.Trim(mask, "0") == ""
	}
	if !noCapability("CapEff") || !noCapability("CapBnd") {
		return errors.New("it has capabilities")
	}
	if v, _ := field("NoNewPrivs"); v != "1" {
		return errors.New("it has no seccomp filter")
	}
	if v, _ := field("Seccomp"); v != "2" {
		return errors.New("it has no seccomp filter")
	}
	return nil
}

const maxInfo = 1024 * 1024

func readInfo(path string) (string, error) {
	f, err := os.OpenFile(path, os.O_RDONLY|syscall.O_NOFOLLOW|syscall.O_NONBLOCK|syscall.O_NOCTTY, 0)
	if err != nil {
		return "", err
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil {
		return "", err
	}
	if !st.Mode().IsRegular() {
		return "", errors.New("it is not a regular file")
	}
	b, err := io.ReadAll(io.LimitReader(f, maxInfo))
	return string(b), err
}

// FlatpakInfo is what the host needs of /.flatpak-info.
type FlatpakInfo struct{ AppID, AppPath, RuntimePath string }

// ParseFlatpakInfo reads the keyfile of a Flatpak instance.
func ParseFlatpakInfo(text string) (FlatpakInfo, error) {
	values := map[[2]string]string{}
	group := ""
	sc := bufio.NewScanner(strings.NewReader(text))
	sc.Buffer(make([]byte, 64*1024), maxInfo+1)
	for sc.Scan() {
		line := strings.TrimSuffix(sc.Text(), "\r")
		if strings.HasPrefix(line, "[") && strings.HasSuffix(line, "]") && len(line) >= 2 {
			group = line[1 : len(line)-1]
		} else if k, v, ok := strings.Cut(line, "="); ok {
			values[[2]string{group, k}] = v
		}
	}
	value := func(g, k string) (string, error) {
		v, ok := values[[2]string{g, k}]
		if !ok {
			return "", fmt.Errorf("the Flatpak info has no %s", k)
		}
		return v, nil
	}
	id, err := value("Application", "name")
	if err != nil {
		return FlatpakInfo{}, err
	}
	if !ValidAppID(id) {
		return FlatpakInfo{}, fmt.Errorf("the Flatpak info has the app id %q, which is not valid", id)
	}
	if _, ok := values[[2]string{"Instance", "original-app-path"}]; ok {
		return FlatpakInfo{}, fmt.Errorf("%s runs with another app folder", id)
	}
	app, err := value("Instance", "app-path")
	if err != nil {
		return FlatpakInfo{}, err
	}
	rt, err := value("Instance", "runtime-path")
	if err != nil {
		return FlatpakInfo{}, err
	}
	return FlatpakInfo{AppID: id, AppPath: app, RuntimePath: rt}, nil
}

// ValidAppID is true for an app id that Flatpak allows.
func ValidAppID(id string) bool {
	parts := strings.Split(id, ".")
	if len(id) > 255 || len(parts) < 3 {
		return false
	}
	for _, part := range parts {
		if part == "" {
			return false
		}
		first := part[0]
		if !(first >= 'a' && first <= 'z' || first >= 'A' && first <= 'Z' || first == '_') {
			return false
		}
		for _, c := range part {
			if !(c < 128 && (c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '_' || c == '-')) {
				return false
			}
		}
	}
	return true
}

type installEnv interface {
	Var(key string) string
	Installations() []string
}

type systemInstallEnv struct{}

func (systemInstallEnv) Var(key string) string { return os.Getenv(key) }
func (systemInstallEnv) Installations() []string {
	entries, err := os.ReadDir("/etc/flatpak/installations.d")
	if err != nil {
		return nil
	}
	var out []string
	for _, e := range entries {
		if b, err := os.ReadFile(filepath.Join("/etc/flatpak/installations.d", e.Name())); err == nil {
			out = append(out, string(b))
		}
	}
	return out
}

func installRoots(env installEnv) []string {
	var roots []string
	if v := env.Var("FLATPAK_USER_DIR"); v != "" {
		roots = append(roots, v)
	}
	if v := env.Var("XDG_DATA_HOME"); v != "" {
		roots = append(roots, filepath.Join(v, "flatpak"))
	}
	if v := env.Var("HOME"); v != "" {
		roots = append(roots, filepath.Join(v, ".local/share/flatpak"))
	}
	if v := env.Var("FLATPAK_SYSTEM_DIR"); v != "" {
		roots = append(roots, v)
	}
	roots = append(roots, "/var/lib/flatpak")
	for _, text := range env.Installations() {
		for _, line := range strings.Split(text, "\n") {
			if p, ok := strings.CutPrefix(line, "Path="); ok {
				roots = append(roots, p)
			}
		}
	}
	return roots
}

func canonical(path string) (string, error) {
	p, err := filepath.EvalSymlinks(path)
	if err != nil {
		return "", err
	}
	return filepath.Abs(p)
}

func deployed(roots []string, kind, id, path string) (string, error) {
	c, err := canonical(path)
	if err != nil {
		return "", fmt.Errorf("cannot read %s: %v", path, err)
	}
	var croots []string
	for _, r := range roots {
		if cr, err := canonical(r); err == nil {
			croots = append(croots, cr)
		}
	}
	if underRoots(croots, kind, id, c) {
		return c, nil
	}
	return "", fmt.Errorf("%s is not in a folder where Flatpak installs", path)
}

func underRoots(roots []string, kind, id, path string) bool {
	for _, root := range roots {
		base := filepath.Join(root, kind) + "/"
		rest, ok := strings.CutPrefix(path, base)
		if !ok {
			continue
		}
		parts := strings.Split(rest, "/")
		bad := false
		for _, p := range parts {
			if p == "" || p == "." || p == ".." {
				bad = true
			}
		}
		if !bad && len(parts) == 5 && parts[4] == "files" && (id == "" || parts[0] == id) {
			return true
		}
	}
	return false
}

func fileOnHost(link, app, rt string) (string, bool) {
	if rest, ok := cutDir(link, "/app"); ok {
		return filepath.Join(app, rest), true
	}
	if rest, ok := cutDir(link, "/usr"); ok {
		return filepath.Join(rt, rest), true
	}
	return "", false
}

func cutDir(path, dir string) (string, bool) {
	if path == dir {
		return "", true
	}
	return strings.CutPrefix(path, dir+"/")
}

// credReader reads a Unix socket with SCM_CREDENTIALS and keeps the pids of
// the processes that wrote (credentials.rs).
type credReader struct {
	conn  *unixConn
	mu    sync.Mutex
	pid   int32
	other bool
}

func (r *credReader) add(pid int32, ok bool) {
	r.mu.Lock()
	defer r.mu.Unlock()
	switch {
	case ok && pid > 0 && r.pid == 0:
		r.pid = pid
	case ok && pid > 0 && pid == r.pid:
	default:
		r.other = true
	}
}

func (r *credReader) writer() (Peer, error) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.pid == 0 || r.other {
		return Peer{}, errors.New("more than one process wrote to this connection")
	}
	return Peer{PID: uint32(r.pid)}, nil
}

func (r *credReader) Read(p []byte) (int, error) {
	oob := make([]byte, unix.CmsgSpace(unix.SizeofUcred))
	n, oobn, _, _, err := r.conn.ReadMsgUnix(p, oob)
	if err != nil {
		if n == 0 {
			return 0, err
		}
	}
	if n > 0 {
		pid, ok := int32(0), false
		if msgs, perr := unix.ParseSocketControlMessage(oob[:oobn]); perr == nil {
			for _, m := range msgs {
				if m.Header.Level == unix.SOL_SOCKET && m.Header.Type == unix.SCM_CREDENTIALS {
					if cred, cerr := unix.ParseUnixCredentials(&m); cerr == nil {
						pid, ok = cred.Pid, true
						break
					}
				}
			}
		}
		r.add(pid, ok)
		return n, nil
	}
	return 0, io.EOF
}
