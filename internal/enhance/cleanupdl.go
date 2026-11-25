package enhance

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"log/slog"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/watzon/sayso/internal/engine"
	"github.com/watzon/sayso/internal/llamalib"
	"github.com/watzon/sayso/internal/update"
)

// Download of cleanup model files.
// A model lives in <models dir>/cleanup/<id>/. The model file streams into
// <file>.partial and resumes with a Range request; after the size and SHA-256
// check it gets its name and ready.json is written.

// DefaultCleanupEndpoint is the default server of the model files.
const DefaultCleanupEndpoint = "https://huggingface.co"

const (
	dlAttempts         = 3
	dlChunk            = 64 * 1024
	dlBodyTimeout      = 60 * time.Second
	dlProgressInterval = 125 * time.Millisecond
	dlRetryPause       = 500 * time.Millisecond
	readyFile          = "ready.json"
)

// DownloadProgress is how far a download is.
type DownloadProgress struct{ BytesDone, BytesTotal uint64 }

// DownloadError is a failed model download. The texts are the Rust ones.
type DownloadError struct {
	Kind     string // cancelled, busy, http, network, wrong_hash, wrong_size, io
	Status   int
	Message  string
	Expected string
	Got      string
}

func (e *DownloadError) Error() string {
	switch e.Kind {
	case "cancelled":
		return "The download was cancelled."
	case "busy":
		return "This model is already downloading or being deleted."
	case "http":
		return fmt.Sprintf("The server answered with status %d.", e.Status)
	case "network":
		return "The network failed: " + e.Message
	case "wrong_hash":
		return fmt.Sprintf("The file is not the expected file. Its hash is %s, but it must be %s.", e.Got, e.Expected)
	case "wrong_size":
		return fmt.Sprintf("The file has %s bytes, but it must have %s.", e.Got, e.Expected)
	}
	return "Cannot use the model folder: " + e.Message
}

func ioErr(err error) *DownloadError { return &DownloadError{Kind: "io", Message: err.Error()} }

// CleanupModelDir is <models dir>/cleanup/<id>.
func CleanupModelDir(modelsDir string, m *CleanupModelInfo) string {
	return filepath.Join(modelsDir, "cleanup", m.ID)
}

// CleanupModelReady is true when ready.json names the catalog revision and
// hash and the model file is there.
func CleanupModelReady(modelsDir string, m *CleanupModelInfo) bool {
	dir := CleanupModelDir(modelsDir, m)
	data, err := os.ReadFile(filepath.Join(dir, readyFile))
	if err != nil {
		return false
	}
	var ready map[string]any
	if json.Unmarshal(data, &ready) != nil {
		return false
	}
	st, err := os.Stat(filepath.Join(dir, m.File))
	return ready["revision"] == m.Revision && ready["sha256"] == m.SHA256 && err == nil && st.Mode().IsRegular()
}

// CleanupModelPath is the model file when ready.
func CleanupModelPath(modelsDir string, m *CleanupModelInfo) (string, bool) {
	if !CleanupModelReady(modelsDir, m) {
		return "", false
	}
	return filepath.Join(CleanupModelDir(modelsDir, m), m.File), true
}

var (
	activeMu   sync.Mutex
	activeDirs = map[string]bool{}
)

func acquireDir(dir string) (func(), error) {
	activeMu.Lock()
	defer activeMu.Unlock()
	if activeDirs[dir] {
		return nil, &DownloadError{Kind: "busy"}
	}
	activeDirs[dir] = true
	return func() {
		activeMu.Lock()
		delete(activeDirs, dir)
		activeMu.Unlock()
	}, nil
}

func isPlainName(n string) bool {
	if n == "" || n == "." || n == ".." {
		return false
	}
	for _, c := range n {
		if !(c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '.' || c == '_' || c == '-') {
			return false
		}
	}
	return true
}

func checkNames(m *CleanupModelInfo) error {
	ok := isPlainName(m.ID) && isPlainName(m.File)
	for _, n := range m.LicenseFiles {
		ok = ok && isPlainName(n)
	}
	if !ok {
		return ioErr(fmt.Errorf("the catalog names a path that is not a plain name (%s)", m.ID))
	}
	return nil
}

// cleanupEndpoint is where model files come from: endpoint, else
// SAYSO_HF_ENDPOINT, else Hugging Face.
func cleanupEndpoint(endpoint string) string {
	if endpoint != "" {
		return endpoint
	}
	if v := os.Getenv("SAYSO_HF_ENDPOINT"); v != "" {
		return v
	}
	return "https://huggingface.co"
}

// DownloadCleanupModel downloads the model and its license files from the
// pinned revision. progress gets about eight calls a second and one at the
// end; the fraction never goes backwards. A cancelled download keeps its
// partial file for a later resume. endpoint "" and cancel nil are allowed.
func DownloadCleanupModel(modelsDir string, m *CleanupModelInfo, endpoint string, cancel *atomic.Bool, progress func(DownloadProgress)) (string, error) {
	if err := checkNames(m); err != nil {
		return "", err
	}
	if cancel == nil {
		cancel = new(atomic.Bool)
	}
	endpoint = cleanupEndpoint(endpoint)
	dir := CleanupModelDir(modelsDir, m)
	release, err := acquireDir(dir)
	if err != nil {
		return "", err
	}
	defer release()
	size := m.SizeBytes
	path := filepath.Join(dir, m.File)
	if CleanupModelReady(modelsDir, m) {
		progress(DownloadProgress{size, size})
		return path, nil
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", ioErr(err)
	}
	url := func(name string) string {
		return strings.TrimRight(endpoint, "/") + "/" + m.Repo + "/resolve/" + m.Revision + "/" + name
	}
	for _, name := range m.LicenseFiles {
		if err := fetchSmall(url(name), filepath.Join(dir, name), cancel); err != nil {
			return "", err
		}
	}
	partial, info := path+".partial", path+".partial.json"
	if err := preparePartial(partial, info, m); err != nil {
		return "", err
	}
	rep := &reporter{total: size}
	if err := fetchModel(url(m.File), partial, m, cancel, rep, progress); err != nil {
		return "", err
	}
	if err := verifyPartial(partial, info, m, cancel); err != nil {
		return "", err
	}
	if err := markReady(partial, info, path, m); err != nil {
		return "", err
	}
	rep.report(size, true, progress)
	return path, nil
}

// DeleteCleanupModel deletes the model folder; Busy while a download runs.
func DeleteCleanupModel(modelsDir string, m *CleanupModelInfo) error {
	if err := checkNames(m); err != nil {
		return err
	}
	dir := CleanupModelDir(modelsDir, m)
	release, err := acquireDir(dir)
	if err != nil {
		return err
	}
	defer release()
	for attempt := 0; ; attempt++ {
		err := os.RemoveAll(dir)
		if err == nil || errors.Is(err, fs.ErrNotExist) {
			return nil
		}
		if runtime.GOOS == "windows" && attempt < 20 {
			time.Sleep(100 * time.Millisecond)
			continue
		}
		return ioErr(err)
	}
}

var dlClient = &http.Client{Transport: func() *http.Transport {
	t := http.DefaultTransport.(*http.Transport).Clone()
	t.ResponseHeaderTimeout = 60 * time.Second
	t.DisableCompression = true
	return t
}()}

func dlGet(url string, from uint64) (*http.Response, error) {
	req, err := http.NewRequest(http.MethodGet, url, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("User-Agent", "Sayso/"+update.Version)
	req.Header.Set("Accept-Encoding", "identity")
	if from > 0 {
		req.Header.Set("Range", fmt.Sprintf("bytes=%d-", from))
	}
	return dlClient.Do(req)
}

func pause(cancel *atomic.Bool, d time.Duration) error {
	end := time.Now().Add(d)
	for time.Now().Before(end) {
		if cancel.Load() {
			return &DownloadError{Kind: "cancelled"}
		}
		time.Sleep(25 * time.Millisecond)
	}
	return nil
}

func fileLen(p string) uint64 {
	st, err := os.Stat(p)
	if err != nil {
		return 0
	}
	return uint64(st.Size())
}

func removeFile(p string) error {
	if err := os.Remove(p); err != nil && !errors.Is(err, fs.ErrNotExist) {
		return ioErr(err)
	}
	return nil
}

// retryable is an attempt that is worth another try.
type retryable struct{ err *DownloadError }

// readBody copies the body into w in chunks, checking cancel, with a deadline per request.
func readBody(resp *http.Response, w io.Writer, cancel *atomic.Bool, onChunk func(n int) bool) (*retryable, error) {
	timer := time.AfterFunc(dlBodyTimeout, func() { resp.Body.Close() })
	defer timer.Stop()
	buf := make([]byte, dlChunk)
	for {
		if cancel != nil && cancel.Load() {
			return nil, &DownloadError{Kind: "cancelled"}
		}
		n, err := resp.Body.Read(buf)
		if n > 0 {
			if _, werr := w.Write(buf[:n]); werr != nil {
				return nil, ioErr(werr)
			}
			if onChunk != nil && !onChunk(n) {
				return nil, nil
			}
		}
		if err == io.EOF {
			return nil, nil
		}
		if err != nil {
			return &retryable{&DownloadError{Kind: "network", Message: err.Error()}}, nil
		}
	}
}

func fetchSmall(url, dest string, cancel *atomic.Bool) error {
	partial := dest + ".partial"
	failures := 0
	for {
		if cancel.Load() {
			return &DownloadError{Kind: "cancelled"}
		}
		retry, err := smallAttempt(url, partial)
		if err != nil {
			return err
		}
		if retry == nil {
			if err := os.Rename(partial, dest); err != nil {
				return ioErr(err)
			}
			return nil
		}
		failures++
		if failures >= dlAttempts {
			if err := removeFile(partial); err != nil {
				return err
			}
			return retry.err
		}
		slog.Warn("download stopped, trying again", "url", url, "error", retry.err)
		if err := pause(cancel, dlRetryPause*time.Duration(failures)); err != nil {
			return err
		}
	}
}

func smallAttempt(url, partial string) (*retryable, error) {
	resp, err := dlGet(url, 0)
	if err != nil {
		return &retryable{&DownloadError{Kind: "network", Message: err.Error()}}, nil
	}
	defer resp.Body.Close()
	switch {
	case resp.StatusCode == 200:
	case resp.StatusCode >= 500 && resp.StatusCode <= 599:
		return &retryable{&DownloadError{Kind: "http", Status: resp.StatusCode}}, nil
	default:
		return nil, &DownloadError{Kind: "http", Status: resp.StatusCode}
	}
	f, err := os.Create(partial)
	if err != nil {
		return nil, ioErr(err)
	}
	defer f.Close()
	retry, err := readBody(resp, f, nil, nil)
	if retry != nil || err != nil {
		return retry, err
	}
	if err := f.Sync(); err != nil {
		return nil, ioErr(err)
	}
	return nil, nil
}

// partialInfo is the JSON that names the revision and hash of a partial file.
func partialInfoJSON(m *CleanupModelInfo) []byte {
	b, _ := json.Marshal(map[string]string{"revision": m.Revision, "sha256": m.SHA256})
	return b
}

func preparePartial(partial, info string, m *CleanupModelInfo) error {
	var current, wanted map[string]any
	json.Unmarshal(partialInfoJSON(m), &wanted)
	data, err := os.ReadFile(info)
	same := false
	if err == nil && json.Unmarshal(data, &current) == nil && len(current) == len(wanted) {
		same = current["revision"] == wanted["revision"] && current["sha256"] == wanted["sha256"]
	}
	if !same {
		if err := removeFile(partial); err != nil {
			return err
		}
		if err := os.WriteFile(info, partialInfoJSON(m), 0o644); err != nil {
			return ioErr(err)
		}
	}
	return nil
}

type reporter struct {
	total, highest uint64
	last           time.Time
}

func (r *reporter) report(done uint64, force bool, progress func(DownloadProgress)) {
	r.highest = max(r.highest, min(done, r.total))
	if force || r.last.IsZero() || time.Since(r.last) >= dlProgressInterval {
		r.last = time.Now()
		progress(DownloadProgress{r.highest, r.total})
	}
}

func fetchModel(url, partial string, m *CleanupModelInfo, cancel *atomic.Bool, rep *reporter, progress func(DownloadProgress)) error {
	best := fileLen(partial)
	failures := 0
	for {
		if cancel.Load() {
			return &DownloadError{Kind: "cancelled"}
		}
		retry, err := modelAttempt(url, partial, m.SizeBytes, cancel, rep, progress)
		if err != nil {
			return err
		}
		if retry == nil {
			return nil
		}
		if now := fileLen(partial); now > best {
			best, failures = now, 0
		} else {
			failures++
		}
		if failures >= dlAttempts {
			return retry.err
		}
		slog.Warn("download stopped, resuming", "model", m.ID, "error", retry.err)
		if err := pause(cancel, dlRetryPause*time.Duration(max(failures, 1))); err != nil {
			return err
		}
	}
}

func modelAttempt(url, partial string, size uint64, cancel *atomic.Bool, rep *reporter, progress func(DownloadProgress)) (*retryable, error) {
	have := fileLen(partial)
	if have > size {
		if err := removeFile(partial); err != nil {
			return nil, err
		}
		have = 0
	}
	resp, err := dlGet(url, have)
	if err != nil {
		return &retryable{&DownloadError{Kind: "network", Message: err.Error()}}, nil
	}
	defer resp.Body.Close()
	flags := os.O_CREATE | os.O_WRONLY
	switch st := resp.StatusCode; {
	case st == 206:
		rng := resp.Header.Get("Content-Range")
		start, ok := contentRangeStart(rng)
		if have == 0 || !ok || start != have {
			if err := removeFile(partial); err != nil {
				return nil, err
			}
			return &retryable{&DownloadError{Kind: "network", Message: fmt.Sprintf("the server resumed at the wrong place (%q)", rng)}}, nil
		}
		flags |= os.O_APPEND
	case st == 200:
		have = 0
		flags |= os.O_TRUNC
	case st == 416 && have == size && have > 0:
		return nil, nil
	case st == 416:
		if err := removeFile(partial); err != nil {
			return nil, err
		}
		return &retryable{&DownloadError{Kind: "http", Status: st}}, nil
	case st >= 500 && st <= 599:
		return &retryable{&DownloadError{Kind: "http", Status: st}}, nil
	default:
		return nil, &DownloadError{Kind: "http", Status: st}
	}
	f, err := os.OpenFile(partial, flags, 0o644)
	if err != nil {
		return nil, ioErr(err)
	}
	defer f.Close()
	rep.report(have, false, progress)
	retry, err := readBody(resp, f, cancel, func(n int) bool {
		have += uint64(n)
		if have > size {
			return false // too long; the size check reports it
		}
		rep.report(have, false, progress)
		return true
	})
	if err != nil {
		return nil, err
	}
	if retry != nil {
		f.Sync()
		return retry, nil
	}
	if err := f.Sync(); err != nil {
		return nil, ioErr(err)
	}
	if have < size {
		return &retryable{&DownloadError{Kind: "network", Message: fmt.Sprintf("the connection closed after %d of %d bytes", have, size)}}, nil
	}
	return nil, nil
}

// contentRangeStart reads "bytes 100-199/200" as 100.
func contentRangeStart(v string) (uint64, bool) {
	rng, ok := strings.CutPrefix(strings.TrimSpace(v), "bytes ")
	if !ok {
		return 0, false
	}
	start, _, ok := strings.Cut(rng, "-")
	if !ok {
		return 0, false
	}
	n, err := strconv.ParseUint(strings.TrimSpace(start), 10, 64)
	return n, err == nil
}

func verifyPartial(partial, info string, m *CleanupModelInfo, cancel *atomic.Bool) error {
	if got := fileLen(partial); got != m.SizeBytes {
		removeFile(partial)
		removeFile(info)
		return &DownloadError{Kind: "wrong_size", Expected: strconv.FormatUint(m.SizeBytes, 10), Got: strconv.FormatUint(got, 10)}
	}
	f, err := os.Open(partial)
	if err != nil {
		return ioErr(err)
	}
	h := sha256.New()
	buf := make([]byte, dlChunk)
	for {
		if cancel.Load() {
			f.Close()
			return &DownloadError{Kind: "cancelled"}
		}
		n, err := f.Read(buf)
		h.Write(buf[:n])
		if err == io.EOF {
			break
		}
		if err != nil {
			f.Close()
			return ioErr(err)
		}
	}
	f.Close()
	got := hex.EncodeToString(h.Sum(nil))
	if !strings.EqualFold(got, m.SHA256) {
		removeFile(partial)
		removeFile(info)
		return &DownloadError{Kind: "wrong_hash", Expected: m.SHA256, Got: got}
	}
	return nil
}

// markReady: the file gets its name, then ready.json appears through a rename.
func markReady(partial, info, path string, m *CleanupModelInfo) error {
	if err := os.Rename(partial, path); err != nil {
		return ioErr(err)
	}
	if err := removeFile(info); err != nil {
		return err
	}
	data, _ := json.Marshal(map[string]string{"revision": m.Revision, "sha256": m.SHA256, "file": m.File})
	readyPath := filepath.Join(filepath.Dir(path), readyFile)
	temp := readyPath + ".tmp"
	f, err := os.Create(temp)
	if err != nil {
		return ioErr(err)
	}
	if _, err := f.Write(data); err != nil {
		f.Close()
		return ioErr(err)
	}
	if err := f.Sync(); err != nil {
		f.Close()
		return ioErr(err)
	}
	f.Close()
	if err := os.Rename(temp, readyPath); err != nil {
		return ioErr(err)
	}
	return nil
}

// ---- The cleaner on the cleanup runtime ----

// TimePerToken is the time a cleanup gets per text token, on top of the style timeout.
const TimePerToken = 15 * time.Millisecond

// libsDownloadLimit bounds a download of the llama.cpp libraries.
const libsDownloadLimit = 10 * time.Minute

// RuntimeCleaner runs cleanup models in the sayso-cleanup runtime.
type RuntimeCleaner struct {
	Runtime   *engine.CleanupClient
	ModelsDir string

	// fetching is true while the llama.cpp libraries download.
	fetching atomic.Bool
	// fetchLibs downloads the libraries; tests replace it.
	fetchLibs func(ctx context.Context) error
}

// fetchLibraries starts a download of the llama.cpp libraries into the user
// cache (one at a time), where the runtime finds them at its next start. The
// runtime says in its hello that it has no libraries. A package has them, so
// this is for development builds and broken installs. The runtime could
// download them itself, but only within the load limit of 15 s.
func (c *RuntimeCleaner) fetchLibraries() {
	if !c.fetching.CompareAndSwap(false, true) {
		return
	}
	go func() {
		defer c.fetching.Store(false)
		ctx, cancel := context.WithTimeout(context.Background(), libsDownloadLimit)
		defer cancel()
		fetch := c.fetchLibs
		if fetch == nil {
			fetch = func(ctx context.Context) error {
				a, ok := llamalib.Current()
				if !ok {
					return errors.New("no llama.cpp build for this system")
				}
				return llamalib.Fetch(ctx, nil, a, llamalib.UserCacheDir(a), "")
			}
		}
		if err := fetch(ctx); err != nil {
			slog.Warn("could not download the llama.cpp libraries of the cleanup runtime", "error", err)
			return
		}
		slog.Info("downloaded the llama.cpp libraries of the cleanup runtime")
	}()
}

// Preload loads model while the microphone records. Errors are logged.
func (c *RuntimeCleaner) Preload(model string) {
	go func() { _ = c.Warm(model) }()
}

// Warm loads model ahead and waits for the load. A model that is not
// downloaded, and a failed load, are errors (logged).
func (c *RuntimeCleaner) Warm(model string) error {
	info, ok := FindCleanupModel(model)
	if !ok {
		return fmt.Errorf("no cleanup model %s", model)
	}
	path, ok := CleanupModelPath(c.ModelsDir, info)
	if !ok {
		return fmt.Errorf("%s is not downloaded", info.Name)
	}
	err := c.Runtime.EnsureLoaded(info.ID, path, NCtx)
	if errors.Is(err, engine.ErrCleanupNoLibraries) {
		c.fetchLibraries()
	}
	if err != nil {
		slog.Warn("could not load the cleanup model ahead", "model", info.ID, "error", err)
	}
	return err
}

// Clean splits the text, cleans each part, checks each answer, and joins them.
func (c *RuntimeCleaner) Clean(r CleanupRequest) (CleanupResponse, error) {
	started := time.Now()
	info, ok := FindCleanupModel(r.Model)
	if !ok {
		return CleanupResponse{}, &CleanError{Kind: "not_configured", Message: "no cleanup model " + r.Model}
	}
	path, ok := CleanupModelPath(c.ModelsDir, info)
	if !ok {
		return CleanupResponse{}, &CleanError{Kind: "not_configured", Message: info.Name + " is not downloaded"}
	}
	var resp CleanupResponse
	var check error
	called := time.Now()
	err := c.Runtime.WithContext(r.Context, info.ID, path, NCtx, func(s *engine.CleanupSession) error {
		resp.LoadMs = uint64(time.Since(called).Milliseconds())
		begun := time.Now()
		left := func(until time.Time) (time.Duration, error) {
			if until.IsZero() {
				return 0, nil
			}
			d := time.Until(until)
			if d <= 0 {
				return 0, engine.ErrCleanupTimeout
			}
			return d, nil
		}
		var planUntil time.Time
		if r.Timeout > 0 {
			planUntil = begun.Add(r.Timeout)
		}
		count := func(text string) (uint32, error) {
			d, err := left(planUntil)
			if err != nil {
				return 0, err
			}
			return s.Count(text, d)
		}
		parts, err := PlanParts(r.Text, MaxPartTokens, count)
		if err != nil {
			return err
		}
		counts := make([]uint32, len(parts))
		var total uint32
		for i, p := range parts {
			if counts[i], err = count(p); err != nil {
				return err
			}
			total += counts[i]
		}
		var deadline time.Time
		if r.Timeout > 0 {
			deadline = begun.Add(r.Timeout + TimePerToken*time.Duration(total))
		}
		answers := make([]string, 0, len(parts))
		for i, p := range parts {
			prompt := Render(info, p, r.Settings, r.App, r.Site, counts[i])
			d, err := left(deadline)
			if err != nil {
				return err
			}
			done, err := s.Complete(prompt.Text, prompt.End, prompt.MaxTokens, d)
			if err != nil {
				return err
			}
			resp.TokensIn += done.TokensIn
			resp.TokensOut += done.TokensOut
			answer, err := CheckAnswer(p, done.Stop, done.Text)
			if err != nil {
				check = err // one failed part fails the whole text
				return nil
			}
			answers = append(answers, answer)
		}
		resp.Text = JoinAnswers(parts, answers)
		resp.Parts = uint32(len(parts))
		return nil
	})
	if errors.Is(err, engine.ErrCleanupNoLibraries) {
		c.fetchLibraries()
	}
	if err != nil {
		return CleanupResponse{}, toCleanError(err, r)
	}
	if check != nil {
		return CleanupResponse{}, &CleanError{Kind: "invalid_output", Message: check.Error()}
	}
	resp.Model = info.ID
	resp.ElapsedMs = uint64(time.Since(started).Milliseconds())
	return resp, nil
}

func toCleanError(err error, r CleanupRequest) error {
	var ce *engine.CleanupError
	switch {
	case errors.Is(err, engine.ErrCleanupTimeout):
		return &CleanError{Kind: "timeout", Ms: uint64(r.Timeout.Milliseconds())}
	case errors.Is(err, engine.ErrCleanupCancelled):
		return &CleanError{Kind: "runtime", Message: err.Error()}
	case errors.Is(err, engine.ErrCleanupDisabled), errors.Is(err, engine.ErrStopped), errors.Is(err, engine.ErrCleanupNotInstalled),
		errors.Is(err, engine.ErrCleanupNoLibraries):
		return &CleanError{Kind: "not_configured", Message: err.Error()}
	case errors.As(err, &ce):
		return &CleanError{Kind: "runtime", Message: err.Error()}
	}
	return &CleanError{Kind: "runtime", Message: err.Error()}
}
