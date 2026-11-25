// Command engine-libs gets the pinned prebuilt libraries that the speech
// engine (internal/localengine) loads on Windows and Linux, for the bundle
// scripts (SAYSO_NATIVE_LIBS_DIR).
//
//	go run ./scripts/engine-libs fetch [-os GOOS] [-arch GOARCH] [-out build/engine-libs]
//
// fetch fills <out>/<os>-<arch> with the pinned files, each checked against its
// SHA-256, and prints that folder. Each archive is checked against its SHA-256
// too and stays in <out>/archives, so a second run downloads nothing.
//
// Linux also needs libparakeet.so, which has no prebuilt build:
// scripts/build-parakeet.sh builds it from pinned sources.
//
// A new pin: download the archive, compare its SHA-256 with the digest that
// GitHub shows for the release asset, and pin the archive and each file here.
package main

import (
	"archive/tar"
	"archive/zip"
	"compress/bzip2"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"os"
	"path"
	"path/filepath"
	"runtime"
	"strings"
)

type file struct {
	path   string // in the archive
	sha256 string
	size   int64
}

type archive struct {
	url    string
	sha256 string
	size   int64
	files  []file
}

const (
	sherpa  = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.8/"
	whisper = "https://github.com/ggml-org/whisper.cpp/releases/download/v1.8.3/"
)

// sherpa-onnx 1.13.8 (the C API of internal/localengine/sherpa_capi.go) with
// its ONNX Runtime, and on Windows whisper.cpp v1.8.3 with its ggml
// (whisper_windows.go). The Windows sherpa-onnx is the /MT build that the Rust
// engine shipped.
var pins = map[string][]archive{
	"linux/amd64": {{
		url:    sherpa + "sherpa-onnx-v1.13.8-linux-x64-shared-lib.tar.bz2",
		sha256: "3892d184be41027e18165e67f549cd4e4cdd8dcd73ac5579e97afd55e14e30b6", size: 9816899,
		files: []file{
			{"sherpa-onnx-v1.13.8-linux-x64-shared-lib/lib/libsherpa-onnx-c-api.so", "b8351ca1632571ac108adbb317bcc4bf7cfe84b72690e3017316b0da3e1e344f", 5124192},
			{"sherpa-onnx-v1.13.8-linux-x64-shared-lib/lib/libonnxruntime.so", "4b3607aebd1784b26b6f9b20e4bd974c7ab8287043e4d095cb7d2cb40b5e566e", 27026609},
		},
	}},
	"linux/arm64": {{
		url:    sherpa + "sherpa-onnx-v1.13.8-linux-aarch64-shared-cpu-lib.tar.bz2",
		sha256: "fb98b80628a383909bf83626daca2e0d0ffc93bf1f7d222eaaa4282b388f072f", size: 12595068,
		files: []file{
			{"sherpa-onnx-v1.13.8-linux-aarch64-shared-cpu-lib/lib/libsherpa-onnx-c-api.so", "11378e1aea169aecb59a4d41ca8da6f5ee4f3614c34a4b908dd78de7646a518d", 4613032},
			{"sherpa-onnx-v1.13.8-linux-aarch64-shared-cpu-lib/lib/libonnxruntime.so", "cfc800d4438f15e8d9017521326776ca7117d8783cb847ccccc2fc61ac4d540f", 34606912},
		},
	}},
	"windows/amd64": {
		{
			url:    sherpa + "sherpa-onnx-v1.13.8-win-x64-shared-MT-Release-lib.tar.bz2",
			sha256: "b8eedf41bd6d3779218887b48367bb7a3ece5aaa7667f01f69ee823a12b0a9e7", size: 8032957,
			files: []file{
				{"sherpa-onnx-v1.13.8-win-x64-shared-MT-Release-lib/lib/sherpa-onnx-c-api.dll", "300e0c88400903fc4cfc88be88a8ae587a24cc658653e0e1fd1c2b8dbbc68557", 4605952},
				{"sherpa-onnx-v1.13.8-win-x64-shared-MT-Release-lib/lib/onnxruntime.dll", "7f66f939a881baf4f46a2216496798edf4a1429878b646d12674aa62f27d8a25", 17799168},
				{"sherpa-onnx-v1.13.8-win-x64-shared-MT-Release-lib/lib/onnxruntime_providers_shared.dll", "551d0e1fe4c227d8542314ba718d52f4379e0c7bfe729a37c59833a884e27b4d", 104960},
			},
		},
		{
			url:    whisper + "whisper-bin-x64.zip",
			sha256: "d824b1e37599f882b396e73f1ee0bfd5d0529f700314c48311dcbd00b803321d", size: 3968674,
			files: []file{
				{"Release/whisper.dll", "4037a6567fbb08fc7efda18e4d128a95df9c31ba171af20439d1a93b785d007e", 483840},
				{"Release/ggml.dll", "bcfeaf6e5b59c315fc36e290715aa7a84b22461e968543c13dfcf57df7b1b81d", 67584},
				{"Release/ggml-base.dll", "61c9c57f49f380896ed54ed3f71735e10a2d986f482c2b0a55e72b47f731b193", 536064},
				{"Release/ggml-cpu.dll", "eb2f1d023e51c40b27463b5ab165600eb48cb60323740f400b1b62e1c8706c56", 683008},
			},
		},
	},
}

func main() {
	if err := run(os.Args[1:]); err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(1)
	}
}

func run(args []string) error {
	if len(args) == 0 || args[0] != "fetch" {
		return errors.New("usage: engine-libs fetch [-os GOOS] [-arch GOARCH] [-out build/engine-libs]")
	}
	fs := flag.NewFlagSet("fetch", flag.ExitOnError)
	goos := fs.String("os", runtime.GOOS, "GOOS of the libraries")
	goarch := fs.String("arch", runtime.GOARCH, "GOARCH of the libraries")
	out := fs.String("out", "build/engine-libs", "folder of the libraries and the archives")
	fs.Parse(args[1:])
	archives, ok := pins[*goos+"/"+*goarch]
	if !ok {
		return fmt.Errorf("no engine libraries for %s/%s", *goos, *goarch)
	}
	dest, err := filepath.Abs(filepath.Join(*out, *goos+"-"+*goarch))
	if err != nil {
		return err
	}
	if err := fetch(context.Background(), http.DefaultClient, archives, dest, filepath.Join(*out, "archives")); err != nil {
		return err
	}
	fmt.Println(dest)
	return nil
}

// fetch puts the pinned files of archives in dest, which appears in one
// rename, so a folder that exists is whole. A dest that verifies is kept.
func fetch(ctx context.Context, client *http.Client, archives []archive, dest, cache string) error {
	if verify(dest, archives) == nil {
		return nil
	}
	parent := filepath.Dir(dest)
	if err := os.MkdirAll(parent, 0o755); err != nil {
		return err
	}
	work, err := os.MkdirTemp(parent, "."+filepath.Base(dest)+".partial-")
	if err != nil {
		return err
	}
	defer os.RemoveAll(work)
	for _, a := range archives {
		p, err := cached(ctx, client, a, cache)
		if err != nil {
			return err
		}
		if err := extract(p, a, work); err != nil {
			return err
		}
	}
	if err := verify(work, archives); err != nil {
		return err
	}
	_ = os.RemoveAll(dest)
	return os.Rename(work, dest)
}

// verify checks that dir has exactly the pinned files, each with its SHA-256.
func verify(dir string, archives []archive) error {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return err
	}
	want := map[string]file{}
	for _, a := range archives {
		for _, f := range a.files {
			want[path.Base(f.path)] = f
		}
	}
	for _, e := range entries {
		if _, ok := want[e.Name()]; !ok {
			return fmt.Errorf("%s is not a pinned file", e.Name())
		}
	}
	for name, f := range want {
		sum, _, err := hashFile(filepath.Join(dir, name))
		if err != nil {
			return err
		}
		if sum != f.sha256 {
			return fmt.Errorf("%s: the SHA-256 is %s, not %s", name, sum, f.sha256)
		}
	}
	return nil
}

// cached is the path of the verified archive in cache, downloaded when it is
// missing or wrong.
func cached(ctx context.Context, client *http.Client, a archive, cache string) (string, error) {
	p := filepath.Join(cache, path.Base(a.url))
	if sum, n, err := hashFile(p); err == nil && sum == a.sha256 && n == a.size {
		return p, nil
	}
	if err := os.MkdirAll(cache, 0o755); err != nil {
		return "", err
	}
	f, err := os.CreateTemp(cache, "."+path.Base(a.url)+".partial-")
	if err != nil {
		return "", err
	}
	temp := f.Name()
	err = download(ctx, client, a, f)
	if cerr := f.Close(); err == nil {
		err = cerr
	}
	if err == nil {
		err = os.Rename(temp, p)
	}
	if err != nil {
		os.Remove(temp)
		return "", err
	}
	return p, nil
}

func download(ctx context.Context, client *http.Client, a archive, w io.Writer) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, a.url, nil)
	if err != nil {
		return err
	}
	resp, err := client.Do(req)
	if err != nil {
		return fmt.Errorf("download of %s: %w", a.url, err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("download of %s: %s", a.url, resp.Status)
	}
	h := sha256.New()
	n, err := io.Copy(io.MultiWriter(w, h), io.LimitReader(resp.Body, a.size+1))
	if err != nil {
		return fmt.Errorf("download of %s: %w", a.url, err)
	}
	if n != a.size {
		return fmt.Errorf("download of %s: %d bytes, not %d", a.url, n, a.size)
	}
	if got := hex.EncodeToString(h.Sum(nil)); got != a.sha256 {
		return fmt.Errorf("download of %s: the SHA-256 is %s, not %s", a.url, got, a.sha256)
	}
	return nil
}

// extract writes the pinned files of a into dir under their base names. Every
// other entry is skipped, so a name in the archive never chooses a path.
func extract(p string, a archive, dir string) error {
	want := map[string]file{}
	for _, f := range a.files {
		want[f.path] = f
	}
	write := func(name string, r io.Reader) error {
		f, ok := want[name]
		if !ok {
			return nil
		}
		delete(want, name)
		out, err := os.OpenFile(filepath.Join(dir, path.Base(f.path)), os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o755)
		if err != nil {
			return err
		}
		_, err = io.Copy(out, io.LimitReader(r, f.size+1))
		if cerr := out.Close(); err == nil {
			err = cerr
		}
		return err
	}
	var err error
	if strings.HasSuffix(a.url, ".zip") {
		err = extractZip(p, write)
	} else {
		err = extractTarBz2(p, write)
	}
	if err != nil {
		return err
	}
	for name := range want {
		return fmt.Errorf("%s is not in %s", name, path.Base(a.url))
	}
	return nil
}

func extractTarBz2(p string, write func(string, io.Reader) error) error {
	f, err := os.Open(p)
	if err != nil {
		return err
	}
	defer f.Close()
	tr := tar.NewReader(bzip2.NewReader(f))
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

func extractZip(p string, write func(string, io.Reader) error) error {
	zr, err := zip.OpenReader(p)
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

func hashFile(p string) (string, int64, error) {
	f, err := os.Open(p)
	if err != nil {
		return "", 0, err
	}
	defer f.Close()
	h := sha256.New()
	n, err := io.Copy(h, f)
	return hex.EncodeToString(h.Sum(nil)), n, err
}
