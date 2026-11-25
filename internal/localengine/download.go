package localengine

import (
	"archive/tar"
	"compress/bzip2"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"path"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

const (
	// defaultBaseURL is the sherpa-onnx release with the speech model archives.
	defaultBaseURL = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models"
	// segmentationRelease holds the segmentation models of diarization.
	segmentationRelease = "speaker-segmentation-models"
	// embeddingRelease holds the speaker embedding models. The tag is spelled so on GitHub.
	embeddingRelease = "speaker-recongition-models"
	// progressInterval is the time between two progress messages.
	progressInterval = 250 * time.Millisecond
	// defaultHFEndpoint is Hugging Face, where GGUF and GGML files come from.
	defaultHFEndpoint = "https://huggingface.co"
	// releasesPrefix starts the URL of a file of a sherpa-onnx release in a descriptor.
	releasesPrefix = "https://github.com/k2-fsa/sherpa-onnx/releases/download/"
	// hfPrefix starts the URL of a Hugging Face file in a descriptor.
	hfPrefix = "https://huggingface.co/"
)

func archiveURL(baseURL, archive string) string {
	return strings.TrimRight(baseURL, "/") + "/" + archive + ".tar.bz2"
}

// fileURL is the URL of a file of a release, such as a VAD model.
func fileURL(baseURL, file string) string {
	return strings.TrimRight(baseURL, "/") + "/" + file
}

// siblingReleaseURL is the release next to the release at baseURL: the same
// place, another last part.
func siblingReleaseURL(baseURL, release string) string {
	trimmed := strings.TrimRight(baseURL, "/")
	if i := strings.LastIndex(trimmed, "/"); i >= 0 {
		return trimmed[:i] + "/" + release
	}
	return release
}

func hfFileURL(endpoint, repo, revision, file string) string {
	return strings.TrimRight(endpoint, "/") + "/" + repo + "/resolve/" + revision + "/" + file
}

// newHTTPClient is the client for model files. It follows redirects (GitHub
// release files redirect). A stalled download holds a store lock that other
// engines wait for, so the whole file has a limit far above what the largest
// model needs.
func newHTTPClient() *http.Client {
	transport := &http.Transport{
		Proxy:                 http.ProxyFromEnvironment,
		DialContext:           (&net.Dialer{Timeout: 30 * time.Second}).DialContext,
		TLSHandshakeTimeout:   30 * time.Second,
		ResponseHeaderTimeout: 60 * time.Second,
	}
	return &http.Client{
		Transport: transport,
		Timeout:   6 * time.Hour,
		CheckRedirect: func(req *http.Request, via []*http.Request) error {
			if len(via) >= 10 {
				return errors.New("too many redirects")
			}
			return nil
		},
	}
}

// fetchArchive downloads the archive at url and extracts it to dest, or with
// file != "", puts the file at url in the folder dest under that name.
// staging is a temporary folder beside dest. m hears the size of each
// response and of each piece that arrives.
func fetchArchive(client *http.Client, url, file, staging, dest string, m meter) error {
	if err := os.RemoveAll(staging); err != nil {
		return fmt.Errorf("cannot remove %s: %w", staging, err)
	}
	if err := os.MkdirAll(staging, 0o755); err != nil {
		return fmt.Errorf("cannot create %s: %w", staging, err)
	}
	var err error
	if file != "" {
		err = downloadToFile(client, url, filepath.Join(staging, file), m)
	} else {
		err = downloadAndExtract(client, url, staging, m)
	}
	if err == nil {
		err = moveIntoPlace(staging, dest)
	}
	if err != nil {
		_ = os.RemoveAll(staging)
	}
	return err
}

// fetchStoreArtifact puts the files of a store artifact at files. urlOf gives
// the URL to use for a URL of a descriptor. work is an empty folder.
func fetchStoreArtifact(client *http.Client, a *artifact, urlOf func(string) string, work, files string, m meter) error {
	if a.Source.Kind == "archive" {
		return fetchArchive(client, urlOf(a.Source.URL), "", filepath.Join(work, "extract"), files, m)
	}
	for _, f := range a.Files {
		if err := checkStorePath(f.Path); err != nil {
			return err
		}
		url, ok := a.Source.fileURL(f.Path)
		if !ok {
			return errors.New("a source without a file URL")
		}
		target := filepath.Join(files, filepath.FromSlash(f.Path))
		if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
			return fmt.Errorf("cannot create %s: %w", filepath.Dir(target), err)
		}
		if err := downloadToFile(client, urlOf(url), target, m); err != nil {
			return err
		}
	}
	return nil
}

// meter follows a download: expect gets the Content-Length of each response
// that has one, add the size of each piece of a body.
type meter interface {
	expect(size int64)
	add(n int64)
}

// countingReader counts the bytes that pass through it.
type countingReader struct {
	r     io.Reader
	count int64
	m     meter
}

func (c *countingReader) Read(p []byte) (int, error) {
	n, err := c.r.Read(p)
	if n > 0 {
		c.count += int64(n)
		c.m.add(int64(n))
	}
	return n, err
}

// get starts a download. The caller closes the body.
func get(client *http.Client, url string, m meter) (io.ReadCloser, *countingReader, int64, error) {
	resp, err := client.Get(url)
	if err != nil {
		return nil, nil, 0, fmt.Errorf("cannot download %s: %w", url, err)
	}
	if resp.StatusCode < 200 || resp.StatusCode > 299 {
		resp.Body.Close()
		return nil, nil, 0, fmt.Errorf("cannot download %s: http status: %d", url, resp.StatusCode)
	}
	if resp.ContentLength > 0 {
		m.expect(resp.ContentLength)
	}
	return resp.Body, &countingReader{r: resp.Body, m: m}, resp.ContentLength, nil
}

func checkSize(url string, count, expected int64) error {
	if expected >= 0 && count != expected {
		return fmt.Errorf("download of %s is incomplete: %d of %d bytes", url, count, expected)
	}
	return nil
}

func downloadToFile(client *http.Client, url, target string, m meter) error {
	body, counted, expected, err := get(client, url, m)
	if err != nil {
		return err
	}
	defer body.Close()
	out, err := os.Create(target)
	if err != nil {
		return fmt.Errorf("cannot create %s: %w", target, err)
	}
	if _, err := io.Copy(out, counted); err != nil {
		out.Close()
		return fmt.Errorf("cannot download %s: %w", url, err)
	}
	if err := out.Sync(); err != nil {
		out.Close()
		return fmt.Errorf("cannot write %s: %w", target, err)
	}
	if err := out.Close(); err != nil {
		return fmt.Errorf("cannot write %s: %w", target, err)
	}
	return checkSize(url, counted.count, expected)
}

func downloadAndExtract(client *http.Client, url, staging string, m meter) error {
	body, counted, expected, err := get(client, url, m)
	if err != nil {
		return err
	}
	defer body.Close()
	if err := extractTarBz2(counted, staging, url); err != nil {
		return err
	}
	// Read to the end, so that the size check counts every byte.
	if _, err := io.Copy(io.Discard, counted); err != nil {
		return fmt.Errorf("cannot download %s: %w", url, err)
	}
	return checkSize(url, counted.count, expected)
}

// extractTarBz2 extracts a .tar.bz2 stream into staging. Only plain files and
// folders with plain relative paths are taken; a link or an unsafe path fails.
func extractTarBz2(r io.Reader, staging, url string) error {
	archive := tar.NewReader(bzip2.NewReader(r))
	for {
		header, err := archive.Next()
		if errors.Is(err, io.EOF) {
			return nil
		}
		if err != nil {
			return fmt.Errorf("cannot extract %s: %w", url, err)
		}
		switch header.Typeflag {
		case tar.TypeXHeader, tar.TypeXGlobalHeader:
			continue
		}
		name := header.Name
		if !plainTarPath(name) || !(header.Typeflag == tar.TypeReg || header.Typeflag == tar.TypeDir || header.Typeflag == tar.TypeGNUSparse || header.Typeflag == tar.TypeRegA) {
			return fmt.Errorf("the archive %s has an entry that is not a plain file or folder: %s", url, name)
		}
		target := filepath.Join(staging, filepath.FromSlash(path.Clean(name)))
		if header.Typeflag == tar.TypeDir {
			if err := os.MkdirAll(target, 0o755); err != nil {
				return fmt.Errorf("cannot extract %s: %w", url, err)
			}
			continue
		}
		if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
			return fmt.Errorf("cannot extract %s: %w", url, err)
		}
		out, err := os.Create(target)
		if err != nil {
			return fmt.Errorf("cannot extract %s: %w", url, err)
		}
		_, err = io.Copy(out, archive)
		if cerr := out.Close(); err == nil {
			err = cerr
		}
		if err != nil {
			return fmt.Errorf("cannot extract %s: %w", url, err)
		}
	}
}

// plainTarPath is true for a relative path of normal parts and ".".
func plainTarPath(name string) bool {
	if name == "" || strings.HasPrefix(name, "/") || strings.Contains(name, `\`) || filepath.IsAbs(name) {
		return false
	}
	for _, part := range strings.Split(strings.TrimSuffix(name, "/"), "/") {
		if part == ".." || part == "" || strings.Contains(part, ":") {
			return false
		}
	}
	return true
}

// moveIntoPlace moves the extracted files to dest. An archive usually holds
// one top folder. That folder becomes dest.
func moveIntoPlace(staging, dest string) error {
	entries, err := os.ReadDir(staging)
	if err != nil {
		return err
	}
	if err := os.RemoveAll(dest); err != nil {
		return fmt.Errorf("cannot remove %s: %w", dest, err)
	}
	switch {
	case len(entries) == 1 && entries[0].IsDir():
		if err := storeRename(filepath.Join(staging, entries[0].Name()), dest); err != nil {
			return err
		}
		if err := os.Remove(staging); err != nil {
			return fmt.Errorf("cannot remove %s: %w", staging, err)
		}
	case len(entries) == 0:
		return errors.New("the archive is empty")
	default:
		if err := storeRename(staging, dest); err != nil {
			return err
		}
	}
	return nil
}

// progress is the download progress over all archives of a model. The
// fraction never goes backwards and stays below 1 until the end. A model
// whose size is not known takes its total from the Content-Length of the
// responses as they start.
type progress struct {
	mu       sync.Mutex
	sized    bool
	total    int64
	done     int64
	highest  float64
	lastEmit time.Time
}

func newProgress(total int64) *progress { return &progress{total: total, sized: total > 0} }

func (p *progress) expect(size int64) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if !p.sized && size > 0 {
		p.total += size
	}
}

func (p *progress) add(n int64) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.done += n
	if p.total > 0 {
		p.highest = max(p.highest, min(float64(p.done)/float64(p.total), 0.99))
	}
}

func (p *progress) snapshot() (fraction float64, done, total int64) {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.highest, p.done, max(p.total, p.done)
}

// knownTotal is false while no size is known: bytes_total is then 0.
func (p *progress) knownTotal() bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.total > 0
}

// due is true when a progress message is due at now. It then counts as sent.
func (p *progress) due(now time.Time) bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.lastEmit.IsZero() || now.Sub(p.lastEmit) >= progressInterval {
		p.lastEmit = now
		return true
	}
	return false
}

// reporting is a meter that sends progress messages when they are due.
type reporting struct {
	p      *progress
	report func()
}

func (r reporting) expect(size int64) { r.p.expect(size) }

func (r reporting) add(n int64) {
	r.p.add(n)
	if r.p.due(time.Now()) {
		r.report()
	}
}
