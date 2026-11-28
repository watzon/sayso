package cleanup

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

// Download of cleanup model files.
// A model lives in <models dir>/cleanup/<id>/. The model file streams into
// <file>.partial and resumes with a Range request; after the size and SHA-256
// check it gets its name and ready.json is written.

// DefaultEndpoint is the default server of the model files.
const DefaultEndpoint = "https://huggingface.co"

// UserAgent is the User-Agent of the downloads.
var UserAgent = "sayso-sdk-go"

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

// ModelDir is <models dir>/cleanup/<id>.
func ModelDir(modelsDir string, m *ModelInfo) string {
	return filepath.Join(modelsDir, "cleanup", m.ID)
}

// IsReady is true when ready.json names the catalog revision and
// hash and the model file is there.
func IsReady(modelsDir string, m *ModelInfo) bool {
	dir := ModelDir(modelsDir, m)
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

// ModelPath is the model file when ready.
func ModelPath(modelsDir string, m *ModelInfo) (string, bool) {
	if !IsReady(modelsDir, m) {
		return "", false
	}
	return filepath.Join(ModelDir(modelsDir, m), m.File), true
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

func checkNames(m *ModelInfo) error {
	ok := isPlainName(m.ID) && isPlainName(m.File)
	for _, n := range m.LicenseFiles {
		ok = ok && isPlainName(n)
	}
	if !ok {
		return ioErr(fmt.Errorf("the catalog names a path that is not a plain name (%s)", m.ID))
	}
	return nil
}

// endpointOf is where model files come from: endpoint, else
// SAYSO_HF_ENDPOINT, else Hugging Face.
func endpointOf(endpoint string) string {
	if endpoint != "" {
		return endpoint
	}
	if v := os.Getenv("SAYSO_HF_ENDPOINT"); v != "" {
		return v
	}
	return "https://huggingface.co"
}

// Download downloads the model and its license files from the
// pinned revision. progress gets about eight calls a second and one at the
// end; the fraction never goes backwards. A cancelled download keeps its
// partial file for a later resume. endpoint "" and cancel nil are allowed.
func Download(modelsDir string, m *ModelInfo, endpoint string, cancel *atomic.Bool, progress func(DownloadProgress)) (string, error) {
	if err := checkNames(m); err != nil {
		return "", err
	}
	if cancel == nil {
		cancel = new(atomic.Bool)
	}
	endpoint = endpointOf(endpoint)
	dir := ModelDir(modelsDir, m)
	release, err := acquireDir(dir)
	if err != nil {
		return "", err
	}
	defer release()
	size := m.SizeBytes
	path := filepath.Join(dir, m.File)
	if IsReady(modelsDir, m) {
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

// Delete deletes the model folder; Busy while a download runs.
func Delete(modelsDir string, m *ModelInfo) error {
	if err := checkNames(m); err != nil {
		return err
	}
	dir := ModelDir(modelsDir, m)
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
	req.Header.Set("User-Agent", UserAgent)
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
		Log.Warn("download stopped, trying again", "url", url, "error", retry.err)
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
func partialInfoJSON(m *ModelInfo) []byte {
	b, _ := json.Marshal(map[string]string{"revision": m.Revision, "sha256": m.SHA256})
	return b
}

func preparePartial(partial, info string, m *ModelInfo) error {
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

func fetchModel(url, partial string, m *ModelInfo, cancel *atomic.Bool, rep *reporter, progress func(DownloadProgress)) error {
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
		Log.Warn("download stopped, resuming", "model", m.ID, "error", retry.err)
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

func verifyPartial(partial, info string, m *ModelInfo, cancel *atomic.Bool) error {
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
func markReady(partial, info, path string, m *ModelInfo) error {
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
