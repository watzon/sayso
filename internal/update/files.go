package update

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync/atomic"
	"time"
)

// writeAtomic writes <path>.tmp, syncs it, and renames it over path.
func writeAtomic(path string, data []byte) error {
	tmp := path + ".tmp"
	f, err := os.Create(tmp)
	if err != nil {
		return err
	}
	if _, err := f.Write(data); err != nil {
		f.Close()
		return err
	}
	if err := f.Sync(); err != nil {
		f.Close()
		return err
	}
	f.Close()
	if err := os.Rename(tmp, path); err != nil {
		return err
	}
	syncDir(filepath.Dir(path))
	return nil
}

func syncDir(dir string) {
	if runtime.GOOS == "windows" {
		return
	}
	if d, err := os.Open(dir); err == nil {
		d.Sync()
		d.Close()
	}
}

// prettyJSON is serde_json's to_vec_pretty: two spaces, no HTML escaping.
func prettyJSON(v any) ([]byte, error) {
	var b bytes.Buffer
	e := json.NewEncoder(&b)
	e.SetEscapeHTML(false)
	e.SetIndent("", "  ")
	if err := e.Encode(v); err != nil {
		return nil, err
	}
	return bytes.TrimRight(b.Bytes(), "\n"), nil
}

// ---- Failure ----

// Failure is why an update did not happen. It is saved in update.json as
// {"kind": ..., "detail": ...}; its text is the Rust Display text.
type Failure struct {
	Kind   string // snake_case, as serde names the variant
	Detail string // the string payload, or the version of Replayed
	Back   string // RelaunchAndRestore: the restore error (Detail is the relaunch error)
}

func (f Failure) Error() string {
	switch f.Kind {
	case "recorded":
		return f.Detail
	case "busy":
		return "Another Sayso runs from this app. Quit it, then try again."
	case "cannot_update_itself":
		return "This install cannot update itself."
	case "nothing_staged":
		return "No update is ready to install."
	case "not_installed":
		return "Sayso could not install the update. You still have the version from before."
	case "bad_signature":
		return "The update record does not have a signature of Sayso."
	case "moved":
		return "Sayso moved to another folder after the update was prepared."
	case "not_newer":
		return "The prepared update is not newer than this version."
	case "staging_gone":
		return "The prepared update is gone."
	case "verify":
		return f.Detail
	case "write_record":
		return "Sayso could not write the update record: " + f.Detail + "."
	case "exchange":
		return "Sayso could not replace the app: " + f.Detail
	case "relaunch":
		return "Sayso could not start the new version: " + f.Detail + ". You still have the version from before."
	case "relaunch_and_restore":
		return "Sayso could not start the new version: " + f.Detail + ". It could not put the old version back: " + f.Back + ". Install Sayso again from the download page."
	case "unreachable":
		return "Sayso could not reach the update server. Check your connection, then try again. (" + f.Detail + ")"
	case "check_failed":
		return "Sayso could not get the update information: " + f.Detail + ". Try again later."
	case "not_accepted":
		return "Sayso did not accept the update information: " + f.Detail + "."
	case "replayed":
		return "The update server offered version " + f.Detail + ", which is older than a version that Sayso saw before. Try again later."
	case "mismatch":
		return "The file that Sayso downloaded is not the file of the release. Try again later."
	case "disk":
		return "Sayso could not write the file: " + f.Detail + ". Make sure that the disk has free space, then try again."
	case "download":
		return "Sayso could not download the update: " + f.Detail + ". Check your connection, then try again."
	case "prepare":
		return "Sayso could not prepare the update: " + f.Detail + "."
	}
	return "The update failed."
}

var unitFailures = map[string]bool{"unknown": true, "busy": true, "cannot_update_itself": true, "nothing_staged": true, "not_installed": true,
	"bad_signature": true, "moved": true, "not_newer": true, "staging_gone": true, "mismatch": true}

func (f Failure) MarshalJSON() ([]byte, error) {
	switch {
	case unitFailures[f.Kind]:
		return json.Marshal(map[string]string{"kind": f.Kind})
	case f.Kind == "relaunch_and_restore":
		return json.Marshal(map[string]any{"kind": f.Kind, "detail": map[string]string{"error": f.Detail, "back": f.Back}})
	}
	return json.Marshal(map[string]string{"kind": f.Kind, "detail": f.Detail})
}

func (f *Failure) UnmarshalJSON(b []byte) error {
	var raw struct {
		Kind   string          `json:"kind"`
		Detail json.RawMessage `json:"detail"`
	}
	if err := json.Unmarshal(b, &raw); err != nil {
		return err
	}
	f.Kind = raw.Kind
	if raw.Kind == "relaunch_and_restore" {
		var d struct{ Error, Back string }
		if err := json.Unmarshal(raw.Detail, &d); err != nil {
			return err
		}
		f.Detail, f.Back = d.Error, d.Back
		return nil
	}
	if len(raw.Detail) > 0 {
		return json.Unmarshal(raw.Detail, &f.Detail)
	}
	return nil
}

// ---- State and record ----

// UpdateState is <data>/update-state.json.
type UpdateState struct {
	HighestSeen *Semver `json:"highest_seen"`
	LastCheck   *uint64 `json:"last_check"`
	Notice      *Semver `json:"notice"`
}

// LoadState reads the state; a missing or broken file gives the default.
func LoadState(path string) UpdateState {
	var s UpdateState
	data, err := os.ReadFile(path)
	if err != nil {
		return s
	}
	if err := json.Unmarshal(data, &s); err != nil {
		slog.Warn("update: the state is not readable", "path", path, "error", err)
		return UpdateState{}
	}
	return s
}

// Save writes the state atomically.
func (s UpdateState) Save(path string) error {
	data, err := prettyJSON(s)
	if err != nil {
		return err
	}
	return writeAtomic(path, data)
}

// Saw records a version from a check; the highest seen never goes down.
func (s *UpdateState) Saw(v Semver, now uint64) {
	if s.HighestSeen == nil || v.CmpPrecedence(*s.HighestSeen) > 0 {
		c := v
		s.HighestSeen = &c
	}
	s.LastCheck = &now
}

// Phase is how far a prepared update is.
type Phase string

const (
	PhaseStaged   Phase = "staged"
	PhaseSwapping Phase = "swapping"
	PhaseSwapped  Phase = "swapped"
	PhaseFailed   Phase = "failed"
)

// Record is update.json beside the prepared update.
type Record struct {
	Envelope    string   `json:"envelope"`
	FromVersion Semver   `json:"from_version"`
	ToVersion   Semver   `json:"to_version"`
	InstallPath string   `json:"install_path"`
	Phase       Phase    `json:"phase"`
	Attempts    uint32   `json:"attempts"`
	Error       *string  `json:"error"`
	Failure     *Failure `json:"failure,omitempty"`
}

// LoadRecord reads update.json, or nil.
func LoadRecord(path string) *Record {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil
	}
	var r Record
	if err := json.Unmarshal(data, &r); err != nil {
		slog.Warn("update: the record is not readable", "path", path, "error", err)
		return nil
	}
	return &r
}

// Save writes update.json atomically.
func (r *Record) Save(path string) error {
	data, err := prettyJSON(r)
	if err != nil {
		return err
	}
	return writeAtomic(path, data)
}

// DeleteRecord removes update.json.
func DeleteRecord(path string) { os.Remove(path) }

// Fail marks the record failed and saves it.
func (r Record) Fail(path string, f Failure) Record {
	r.Phase = PhaseFailed
	s := f.Error()
	r.Error = &s
	r.Failure = &f
	if err := r.Save(path); err != nil {
		slog.Error("update: could not write the record", "path", path, "error", err)
	}
	return r
}

// ---- Check and download ----

// UserAgent is "Sayso/<version> (<os>; <arch>)" as Rust's consts name them.
func UserAgent() string {
	p := strings.SplitN(Platform(), "-", 2)
	return fmt.Sprintf("Sayso/%s (%s; %s)", Version, p[0], p[1])
}

// CheckError is a failed fetch of latest.json.
type CheckError struct {
	Kind   string // not_found, status, network
	Status int
	Msg    string
}

func (e *CheckError) Error() string {
	switch e.Kind {
	case "not_found":
		return "the newest release has no update information"
	case "status":
		return fmt.Sprintf("the server answered with HTTP %d", e.Status)
	}
	return e.Msg
}

func httpClient(httpsOnly bool, redirects int, timeout time.Duration) *http.Client {
	return &http.Client{Timeout: timeout, CheckRedirect: func(req *http.Request, via []*http.Request) error {
		if len(via) >= redirects {
			return errors.New("too many redirects")
		}
		if httpsOnly && req.URL.Scheme != "https" {
			return errors.New("a redirect left https")
		}
		return nil
	}}
}

// Fetch downloads latest.json (at most ManifestMaxBytes + 1 bytes).
func Fetch(url string, httpsOnly bool) ([]byte, error) {
	if httpsOnly && !strings.HasPrefix(url, "https://") {
		return nil, &CheckError{Kind: "network", Msg: "the address is not https"}
	}
	req, _ := http.NewRequest(http.MethodGet, url, nil)
	req.Header.Set("User-Agent", UserAgent())
	resp, err := httpClient(httpsOnly, 5, 30*time.Second).Do(req)
	if err != nil {
		return nil, &CheckError{Kind: "network", Msg: err.Error()}
	}
	defer resp.Body.Close()
	switch resp.StatusCode {
	case 200:
	case 404:
		return nil, &CheckError{Kind: "not_found"}
	default:
		return nil, &CheckError{Kind: "status", Status: resp.StatusCode}
	}
	data, err := io.ReadAll(io.LimitReader(resp.Body, ManifestMaxBytes+1))
	if err != nil {
		return nil, &CheckError{Kind: "network", Msg: err.Error()}
	}
	return data, nil
}

// DownloadError is a failed download of an update.
type DownloadError struct {
	Kind   string // cancelled, mismatch, status, network, disk
	Status int
	Msg    string
}

func (e *DownloadError) Error() string {
	switch e.Kind {
	case "cancelled":
		return "the download was cancelled"
	case "mismatch":
		return "the file is not the file that the update information names"
	case "status":
		return fmt.Sprintf("the server answered with HTTP %d", e.Status)
	case "disk":
		return "Sayso could not write the file: " + e.Msg
	}
	return e.Msg
}

// DownloadFileName is a safe file name from the asset URL.
func DownloadFileName(a Asset) string {
	last := a.URL[strings.LastIndexByte(a.URL, '/')+1:]
	var b strings.Builder
	for _, c := range last {
		if c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '.' || c == '-' || c == '_' {
			b.WriteRune(c)
		}
	}
	if strings.Trim(b.String(), ".") == "" {
		return "update"
	}
	return b.String()
}

// Download fetches an asset into dir and checks its size and hash.
func Download(a Asset, dir string, httpsOnly bool, cancel *atomic.Bool, onProgress func(uint64)) (string, error) {
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", &DownloadError{Kind: "disk", Msg: err.Error()}
	}
	dest := filepath.Join(dir, DownloadFileName(a))
	part := dest + ".part"
	if err := fetchAsset(a, part, httpsOnly, cancel, onProgress); err != nil {
		os.Remove(part)
		return "", err
	}
	if err := os.Rename(part, dest); err != nil {
		return "", &DownloadError{Kind: "disk", Msg: err.Error()}
	}
	return dest, nil
}

func fetchAsset(a Asset, part string, httpsOnly bool, cancel *atomic.Bool, onProgress func(uint64)) error {
	if httpsOnly && !strings.HasPrefix(a.URL, "https://") {
		return &DownloadError{Kind: "network", Msg: "the address is not https"}
	}
	req, _ := http.NewRequest(http.MethodGet, a.URL, nil)
	req.Header.Set("User-Agent", UserAgent())
	resp, err := httpClient(httpsOnly, 10, 20*time.Minute).Do(req)
	if err != nil {
		return &DownloadError{Kind: "network", Msg: err.Error()}
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		return &DownloadError{Kind: "status", Status: resp.StatusCode}
	}
	f, err := os.Create(part)
	if err != nil {
		return &DownloadError{Kind: "disk", Msg: err.Error()}
	}
	defer f.Close()
	h := sha256.New()
	var received uint64
	buf := make([]byte, 64*1024)
	for {
		if cancel.Load() {
			return &DownloadError{Kind: "cancelled"}
		}
		n, rerr := resp.Body.Read(buf)
		if n > 0 {
			received += uint64(n)
			if received > a.Size {
				return &DownloadError{Kind: "mismatch"}
			}
			h.Write(buf[:n])
			if _, err := f.Write(buf[:n]); err != nil {
				return &DownloadError{Kind: "disk", Msg: err.Error()}
			}
			onProgress(received)
		}
		if rerr == io.EOF {
			break
		}
		if rerr != nil {
			if cancel.Load() {
				return &DownloadError{Kind: "cancelled"}
			}
			return &DownloadError{Kind: "network", Msg: rerr.Error()}
		}
	}
	if err := f.Sync(); err != nil {
		return &DownloadError{Kind: "disk", Msg: err.Error()}
	}
	if received != a.Size || hex.EncodeToString(h.Sum(nil)) != a.SHA256 {
		return &DownloadError{Kind: "mismatch"}
	}
	return nil
}
