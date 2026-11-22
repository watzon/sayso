package llamalib

import (
	"archive/tar"
	"archive/zip"
	"compress/gzip"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"strings"
)

// Fetch downloads the archive of a, checks its SHA-256, takes the pinned files
// out of it, checks each of them, and puts them in dest. dest appears in one
// rename, so a folder that exists is whole. An archive that is already in
// archiveCache (a folder, or "") with the right SHA-256 is not downloaded
// again. A dest that is already there and verifies is left as it is.
func Fetch(ctx context.Context, client *http.Client, a *Asset, dest, archiveCache string) error {
	if HasLibraries(dest, a.GOOS) && Verify(dest, a) == nil {
		return nil
	}
	if client == nil {
		client = http.DefaultClient
	}
	parent := filepath.Dir(dest)
	if err := os.MkdirAll(parent, 0o755); err != nil {
		return err
	}
	archive, cleanup, err := archiveFile(ctx, client, a, archiveCache, parent)
	if err != nil {
		return err
	}
	defer cleanup()
	work, err := os.MkdirTemp(parent, "."+filepath.Base(dest)+".partial-")
	if err != nil {
		return err
	}
	defer os.RemoveAll(work)
	if err := extract(archive, a, work); err != nil {
		return err
	}
	if err := Verify(work, a); err != nil {
		return err
	}
	// Another process may have filled dest in the meantime.
	if HasLibraries(dest, a.GOOS) && Verify(dest, a) == nil {
		return nil
	}
	_ = os.RemoveAll(dest)
	return os.Rename(work, dest)
}

// archiveFile is a path to the verified archive, from the cache or downloaded.
func archiveFile(ctx context.Context, client *http.Client, a *Asset, archiveCache, tempDir string) (string, func(), error) {
	none := func() {}
	name := filepath.Base(a.URL)
	if archiveCache != "" {
		cached := filepath.Join(archiveCache, name)
		if sum, err := fileSHA256(cached); err == nil && sum == a.SHA256 {
			return cached, none, nil
		}
		if err := os.MkdirAll(archiveCache, 0o755); err != nil {
			return "", none, err
		}
		tempDir = archiveCache
	}
	f, err := os.CreateTemp(tempDir, "."+name+".partial-")
	if err != nil {
		return "", none, err
	}
	temp := f.Name()
	drop := func() { os.Remove(temp) }
	err = download(ctx, client, a, f)
	if cerr := f.Close(); err == nil {
		err = cerr
	}
	if err != nil {
		drop()
		return "", none, err
	}
	if archiveCache == "" {
		return temp, drop, nil
	}
	cached := filepath.Join(archiveCache, name)
	if err := os.Rename(temp, cached); err != nil {
		drop()
		return "", none, err
	}
	return cached, none, nil
}

// download writes the archive to w and checks its size and SHA-256.
func download(ctx context.Context, client *http.Client, a *Asset, w io.Writer) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, a.URL, nil)
	if err != nil {
		return err
	}
	resp, err := client.Do(req)
	if err != nil {
		return fmt.Errorf("download of %s: %w", a.URL, err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("download of %s: %s", a.URL, resp.Status)
	}
	h := sha256.New()
	n, err := io.Copy(io.MultiWriter(w, h), io.LimitReader(resp.Body, a.Size+1))
	if err != nil {
		return fmt.Errorf("download of %s: %w", a.URL, err)
	}
	if n != a.Size {
		return fmt.Errorf("download of %s: %d bytes, not %d", a.URL, n, a.Size)
	}
	if got := hex.EncodeToString(h.Sum(nil)); got != a.SHA256 {
		return fmt.Errorf("download of %s: the SHA-256 is %s, not %s", a.URL, got, a.SHA256)
	}
	return nil
}

// extract writes the pinned files of a from the archive into dir, and makes
// the pinned links. Every other entry of the archive is skipped, so a name in
// the archive never chooses a path.
func extract(archive string, a *Asset, dir string) error {
	want := map[string]File{}
	for _, f := range a.Files {
		want[a.Root+f.Name] = f
	}
	write := func(name string, r io.Reader) error {
		f, ok := want[name]
		if !ok {
			return nil
		}
		delete(want, name)
		out, err := os.OpenFile(filepath.Join(dir, f.Name), os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o755)
		if err != nil {
			return err
		}
		_, err = io.Copy(out, io.LimitReader(r, f.Size+1))
		if cerr := out.Close(); err == nil {
			err = cerr
		}
		return err
	}
	var err error
	if strings.HasSuffix(a.URL, ".zip") {
		err = extractZip(archive, write)
	} else {
		err = extractTarGz(archive, write)
	}
	if err != nil {
		return err
	}
	for name := range want {
		return fmt.Errorf("%s is not in %s", name, filepath.Base(a.URL))
	}
	if runtime.GOOS == "windows" {
		return nil
	}
	for _, l := range a.Links {
		if err := os.Symlink(l.Target, filepath.Join(dir, l.Name)); err != nil {
			return err
		}
	}
	return nil
}

func extractTarGz(archive string, write func(string, io.Reader) error) error {
	f, err := os.Open(archive)
	if err != nil {
		return err
	}
	defer f.Close()
	gz, err := gzip.NewReader(f)
	if err != nil {
		return err
	}
	tr := tar.NewReader(gz)
	for {
		h, err := tr.Next()
		if errors.Is(err, io.EOF) {
			return nil
		}
		if err != nil {
			return err
		}
		if h.Typeflag != tar.TypeReg {
			continue
		}
		if err := write(h.Name, tr); err != nil {
			return err
		}
	}
}

func extractZip(archive string, write func(string, io.Reader) error) error {
	zr, err := zip.OpenReader(archive)
	if err != nil {
		return err
	}
	defer zr.Close()
	for _, zf := range zr.File {
		if zf.FileInfo().IsDir() {
			continue
		}
		r, err := zf.Open()
		if err != nil {
			return err
		}
		err = write(zf.Name, r)
		r.Close()
		if err != nil {
			return err
		}
	}
	return nil
}
