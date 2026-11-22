package models

import (
	"archive/tar"
	"compress/bzip2"
	"context"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"net/http"
	"os"
	"path"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

// ProgressInterval is the time between two progress messages.
const ProgressInterval = 250 * time.Millisecond

// Downloader fetches the files of artifacts over HTTP
// (crates/sayso-engine/src/download.rs).
type Downloader struct {
	Client *http.Client
	// URLOf maps a URL of a descriptor to the URL to use (a mirror, a test
	// server). nil uses the URL as it is.
	URLOf func(string) string
	// OnBytes gets the size of each piece that arrives.
	OnBytes func(n uint64)
	Context context.Context
}

// NewDownloader gives a downloader with redirect following and generous
// timeouts: a stalled download holds a store lock other engines wait for.
func NewDownloader() *Downloader {
	return &Downloader{Client: &http.Client{Timeout: 6 * time.Hour}}
}

func (d *Downloader) url(u string) string {
	if d.URLOf != nil {
		return d.URLOf(u)
	}
	return u
}

// Fetch is a store Fetch function.
func (d *Downloader) Fetch(a Artifact, work, files string) error {
	if a.Source.Kind == SourceArchive {
		return d.fetchArchive(d.url(a.Source.URL), filepath.Join(work, "extract"), files)
	}
	for _, f := range a.Files {
		if err := CheckPath(f.Path); err != nil {
			return err
		}
		u := a.Source.FileURL(f.Path)
		if u == "" {
			return errors.New("a source without a file URL")
		}
		p := filepath.Join(files, filepath.FromSlash(f.Path))
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			return fmt.Errorf("cannot create %s: %w", filepath.Dir(p), err)
		}
		if err := d.downloadToFile(d.url(u), p); err != nil {
			return err
		}
	}
	return nil
}

type counting struct {
	r       io.Reader
	n       uint64
	onBytes func(uint64)
}

func (c *counting) Read(b []byte) (int, error) {
	n, err := c.r.Read(b)
	if n > 0 {
		c.n += uint64(n)
		if c.onBytes != nil {
			c.onBytes(uint64(n))
		}
	}
	return n, err
}

func (d *Downloader) get(u string) (*http.Response, *counting, error) {
	ctx := d.Context
	if ctx == nil {
		ctx = context.Background()
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, u, nil)
	if err != nil {
		return nil, nil, err
	}
	client := d.Client
	if client == nil {
		client = http.DefaultClient
	}
	resp, err := client.Do(req)
	if err != nil {
		return nil, nil, fmt.Errorf("cannot download %s: %w", u, err)
	}
	if resp.StatusCode/100 != 2 {
		resp.Body.Close()
		return nil, nil, fmt.Errorf("cannot download %s: HTTP %d", u, resp.StatusCode)
	}
	return resp, &counting{r: resp.Body, onBytes: d.OnBytes}, nil
}

func checkSize(u string, count uint64, resp *http.Response) error {
	if resp.ContentLength >= 0 && count != uint64(resp.ContentLength) {
		return fmt.Errorf("download of %s is incomplete: %d of %d bytes", u, count, resp.ContentLength)
	}
	return nil
}

func (d *Downloader) downloadToFile(u, p string) error {
	resp, body, err := d.get(u)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	f, err := os.Create(p)
	if err != nil {
		return fmt.Errorf("cannot create %s: %w", p, err)
	}
	if _, err := io.Copy(f, body); err != nil {
		f.Close()
		return fmt.Errorf("cannot download %s: %w", u, err)
	}
	if err := f.Sync(); err != nil {
		f.Close()
		return fmt.Errorf("cannot write %s: %w", p, err)
	}
	f.Close()
	return checkSize(u, body.n, resp)
}

// FetchFile downloads one file into folder `dest` under `name`, through a
// staging folder.
func (d *Downloader) FetchFile(u, name, staging, dest string) error {
	if err := resetDir(staging); err != nil {
		return err
	}
	err := d.downloadToFile(u, filepath.Join(staging, name))
	if err == nil {
		err = moveIntoPlace(staging, dest)
	}
	if err != nil {
		os.RemoveAll(staging)
	}
	return err
}

func (d *Downloader) fetchArchive(u, staging, dest string) error {
	if err := resetDir(staging); err != nil {
		return err
	}
	err := d.downloadAndExtract(u, staging)
	if err == nil {
		err = moveIntoPlace(staging, dest)
	}
	if err != nil {
		os.RemoveAll(staging)
	}
	return err
}

func resetDir(p string) error {
	if err := os.RemoveAll(p); err != nil {
		return fmt.Errorf("cannot remove %s: %w", p, err)
	}
	if err := os.MkdirAll(p, 0o755); err != nil {
		return fmt.Errorf("cannot create %s: %w", p, err)
	}
	return nil
}

func (d *Downloader) downloadAndExtract(u, staging string) error {
	resp, body, err := d.get(u)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if err := ExtractTarBz2(body, staging); err != nil {
		return fmt.Errorf("cannot extract %s: %w", u, err)
	}
	// Read to the end, so that the size check counts every byte.
	if _, err := io.Copy(io.Discard, body); err != nil {
		return fmt.Errorf("cannot download %s: %w", u, err)
	}
	return checkSize(u, body.n, resp)
}

// ExtractTarBz2 extracts a .tar.bz2 stream (also multi-stream bzip2) into
// dest. A link or a path that leaves dest is refused.
func ExtractTarBz2(r io.Reader, dest string) error {
	tr := tar.NewReader(bzip2.NewReader(r))
	for {
		h, err := tr.Next()
		if errors.Is(err, io.EOF) {
			return nil
		}
		if err != nil {
			return err
		}
		if h.Typeflag == tar.TypeXGlobalHeader || h.Typeflag == tar.TypeXHeader {
			continue
		}
		name := h.Name
		plain := name != "" && !path.IsAbs(name) && !strings.Contains(name, "\\")
		for _, part := range strings.Split(strings.TrimSuffix(name, "/"), "/") {
			if part == ".." {
				plain = false
			}
		}
		isFile := h.Typeflag == tar.TypeReg || h.Typeflag == tar.TypeRegA || h.Typeflag == tar.TypeGNUSparse
		if !plain || !(isFile || h.Typeflag == tar.TypeDir) {
			return fmt.Errorf("the archive has an entry that is not a plain file or folder: %s", name)
		}
		target := filepath.Join(dest, filepath.FromSlash(path.Clean(name)))
		if h.Typeflag == tar.TypeDir {
			if err := os.MkdirAll(target, 0o755); err != nil {
				return err
			}
			continue
		}
		if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
			return err
		}
		f, err := os.OpenFile(target, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, 0o644)
		if err != nil {
			return err
		}
		if _, err := io.Copy(f, tr); err != nil {
			f.Close()
			return err
		}
		if err := f.Close(); err != nil {
			return err
		}
	}
}

// moveIntoPlace moves the extracted files to dest. An archive usually holds
// one top folder; that folder becomes dest.
func moveIntoPlace(staging, dest string) error {
	entries, err := os.ReadDir(staging)
	if err != nil {
		return err
	}
	if err := os.RemoveAll(dest); err != nil && !errors.Is(err, fs.ErrNotExist) {
		return fmt.Errorf("cannot remove %s: %w", dest, err)
	}
	switch {
	case len(entries) == 0:
		return errors.New("the archive is empty")
	case len(entries) == 1 && entries[0].IsDir():
		if err := os.Rename(filepath.Join(staging, entries[0].Name()), dest); err != nil {
			return fmt.Errorf("cannot move files to %s: %w", dest, err)
		}
		if err := os.Remove(staging); err != nil {
			return fmt.Errorf("cannot remove %s: %w", staging, err)
		}
	default:
		if err := os.Rename(staging, dest); err != nil {
			return fmt.Errorf("cannot move files to %s: %w", dest, err)
		}
	}
	return nil
}

// Progress is download progress over all artifacts of a model. The fraction
// never goes backwards and stays below 1 until the download is complete.
type Progress struct {
	mu       sync.Mutex
	total    uint64
	done     uint64
	highest  float64
	lastEmit time.Time
}

// NewProgress takes the expected size; 0 means unknown.
func NewProgress(total uint64) *Progress { return &Progress{total: total} }

func (p *Progress) Add(n uint64) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.done += n
	if p.total > 0 {
		f := min(float64(p.done)/float64(p.total), 0.99)
		p.highest = max(p.highest, f)
	}
}

func (p *Progress) Fraction() float64 { p.mu.Lock(); defer p.mu.Unlock(); return p.highest }
func (p *Progress) BytesDone() uint64 { p.mu.Lock(); defer p.mu.Unlock(); return p.done }

// BytesTotal is the expected size, or the bytes so far when larger.
func (p *Progress) BytesTotal() uint64 {
	p.mu.Lock()
	defer p.mu.Unlock()
	return max(p.total, p.done)
}

// Due is true when a progress message is due at now; it then counts as sent.
func (p *Progress) Due(now time.Time) bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.lastEmit.IsZero() || now.Sub(p.lastEmit) >= ProgressInterval {
		p.lastEmit = now
		return true
	}
	return false
}

// DescriptorSize is the size of all artifacts of a descriptor.
func DescriptorSize(data *Descriptors, d Descriptor) uint64 {
	parts, err := data.Parts(d)
	if err != nil {
		return 0
	}
	var n uint64
	seen := map[string]bool{}
	for _, p := range parts {
		if !seen[p.Artifact.Digest] {
			seen[p.Artifact.Digest] = true
			n += p.Artifact.Size()
		}
	}
	return n
}
