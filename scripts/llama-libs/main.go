// Command llama-libs gets the pinned llama.cpp libraries of the cleanup
// runtime (internal/llamalib), for the bundle scripts and for a new pin.
//
//	go run ./scripts/llama-libs fetch [-os GOOS] [-arch GOARCH] [-out build/llama-libs]
//	go run ./scripts/llama-libs ffi [-os GOOS] [-arch GOARCH] <folder>
//	go run ./scripts/llama-libs ldflags [-os GOOS]
//	go run ./scripts/llama-libs pin <folder with the archives>
//
// fetch fills <out>/<tag>-<os>-<arch> with the files of the pins, checked
// against their SHA-256, and prints that folder. The archive stays in
// <out>/archives, so a second run downloads nothing.
//
// ffi copies the libffi of the platform (macOS, Windows) from yzma's libffi
// binding into the folder, checked against its SHA-256. ldflags prints the
// -ldflags of the cleanup runtime for the platform (the libffi file on macOS).
//
// pin prints a new internal/llamalib/pins.go from the archives in the folder,
// for the release in the constants below. Download each archive, check it
// against the digest manifest of the release
// (https://github.com/hybridgroup/llama-cpp-builder/releases/download/<tag>/<tag>.json,
// whose SHA-256 is the pin in yzma's download.DefaultVersion), and run pin.
package main

import (
	"archive/tar"
	"archive/zip"
	"compress/gzip"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"go/format"
	"io"
	"os"
	"os/exec"
	"path"
	"path/filepath"
	"regexp"
	"runtime"
	"sort"
	"strings"

	"github.com/watzon/sayso/internal/llamalib"
)

func main() {
	if err := run(os.Args[1:]); err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(1)
	}
}

func run(args []string) error {
	if len(args) == 0 {
		return errors.New("usage: llama-libs fetch|pin ...")
	}
	switch args[0] {
	case "fetch":
		fs := flag.NewFlagSet("fetch", flag.ExitOnError)
		goos := fs.String("os", runtime.GOOS, "GOOS of the libraries")
		goarch := fs.String("arch", runtime.GOARCH, "GOARCH of the libraries")
		out := fs.String("out", "build/llama-libs", "folder of the libraries and the archives")
		fs.Parse(args[1:])
		a, ok := llamalib.For(*goos, *goarch)
		if !ok {
			return fmt.Errorf("no llama.cpp build for %s/%s", *goos, *goarch)
		}
		dest, err := filepath.Abs(filepath.Join(*out, a.Name()))
		if err != nil {
			return err
		}
		if err := llamalib.Fetch(context.Background(), nil, a, dest, filepath.Join(*out, "archives")); err != nil {
			return err
		}
		fmt.Println(dest)
		return nil
	case "ffi":
		fs := flag.NewFlagSet("ffi", flag.ExitOnError)
		goos := fs.String("os", runtime.GOOS, "GOOS")
		goarch := fs.String("arch", runtime.GOARCH, "GOARCH")
		fs.Parse(args[1:])
		if fs.NArg() != 1 {
			return errors.New("usage: llama-libs ffi [-os GOOS] [-arch GOARCH] <folder>")
		}
		f, ok := llamalib.FFIFor(*goos, *goarch)
		if !ok {
			return nil // Linux: the system's libffi
		}
		out, err := exec.Command("go", "mod", "download", "-json", llamalib.FFIModule).Output()
		if err != nil {
			return fmt.Errorf("go mod download %s: %w", llamalib.FFIModule, err)
		}
		var mod struct{ Dir string }
		if err := json.Unmarshal(out, &mod); err != nil || mod.Dir == "" {
			return fmt.Errorf("go mod download %s: no folder", llamalib.FFIModule)
		}
		src := filepath.Join(mod.Dir, "assets", "libffi", *goos+"_"+*goarch, f.Name)
		return llamalib.CopyChecked(src, filepath.Join(fs.Arg(0), f.Name), f.SHA256)
	case "ldflags":
		fs := flag.NewFlagSet("ldflags", flag.ExitOnError)
		goos := fs.String("os", runtime.GOOS, "GOOS")
		fs.Parse(args[1:])
		if *goos == "darwin" {
			fmt.Println("-X " + llamalib.FFIFilenameFlag)
		}
		return nil
	case "pin":
		if len(args) != 2 {
			return errors.New("usage: llama-libs pin <folder with the archives>")
		}
		return pin(args[1])
	}
	return fmt.Errorf("unknown command %q", args[0])
}

// The release, from yzma v1.33.0 (download.DefaultVersion): llama.cpp v1.0.0,
// whose binaries are in the nightly build b12359.
const (
	tag      = "v1.0.0"
	upstream = "b12359"
)

type platform struct {
	goos, goarch, backend, url string
	// keep chooses the regular files; links to them are kept too.
	keep *regexp.Regexp
}

var (
	ggmlOrg = "https://github.com/ggml-org/llama.cpp/releases/download/" + upstream + "/"
	builder = "https://github.com/hybridgroup/llama-cpp-builder/releases/download/" + tag + "/"
	// macOS: llama links ggml-cpu, ggml-blas, and ggml-rpc; Metal is a backend
	// that ggml loads from the folder.
	keepDarwin = regexp.MustCompile(`^(libggml(-base|-cpu|-blas|-rpc|-metal)?|libllama)\.1\.0\.0\.dylib$|^LICENSE$`)
	// Linux: one ggml-cpu library for each CPU level; ggml loads the best one.
	keepLinux = regexp.MustCompile(`^(libggml(-base)?|libllama)\.so\.1\.0\.0$|^libggml-cpu-[a-z0-9._]+\.so$|^LICENSE$`)
	// Windows: the same, and the LLVM OpenMP runtime that ggml links.
	keepWindows = regexp.MustCompile(`^(ggml|ggml-base|llama|libomp)\.dll$|^ggml-cpu-[a-z0-9]+\.dll$|^LICENSE-LLVM-OpenMP$`)
)

var platforms = []platform{
	{"darwin", "arm64", "metal", ggmlOrg + "llama-" + upstream + "-bin-macos-arm64.tar.gz", keepDarwin},
	{"darwin", "amd64", "cpu", ggmlOrg + "llama-" + upstream + "-bin-macos-x64.tar.gz", keepDarwin},
	{"linux", "amd64", "cpu", ggmlOrg + "llama-" + upstream + "-bin-ubuntu-x64.tar.gz", keepLinux},
	{"linux", "arm64", "cpu", builder + "llama-" + tag + "-bin-ubuntu-cpu-arm64.tar.gz", keepLinux},
	{"windows", "amd64", "cpu", ggmlOrg + "llama-" + upstream + "-bin-win-cpu-x64.zip", keepWindows},
}

type entry struct {
	name, link string // link: the target of a symbolic link
	sum        string
	size       int64
}

func pin(dir string) error {
	var b strings.Builder
	b.WriteString("// Code generated by scripts/llama-libs pin; DO NOT EDIT.\n\npackage llamalib\n\n")
	fmt.Fprintf(&b, "// Tag is the llama.cpp release of the libraries, and Upstream the nightly\n// build that holds its binaries.\nconst (\n\tTag      = %q\n\tUpstream = %q\n)\n\n", tag, upstream)
	b.WriteString("var assets = []Asset{\n")
	for _, p := range platforms {
		archive := filepath.Join(dir, path.Base(p.url))
		sum, size, err := hashFile(archive)
		if err != nil {
			return err
		}
		entries, err := list(archive)
		if err != nil {
			return err
		}
		root := ""
		for _, e := range entries {
			if i := strings.Index(e.name, "/"); i > 0 {
				root = e.name[:i+1]
				break
			}
		}
		kept := map[string]bool{}
		var files, links []entry
		for _, e := range entries {
			name := strings.TrimPrefix(e.name, root)
			if e.link == "" && p.keep.MatchString(name) {
				e.name = name
				files = append(files, e)
				kept[name] = true
			}
		}
		// Links to kept files, and links to those links.
		for changed := true; changed; {
			changed = false
			for _, e := range entries {
				name := strings.TrimPrefix(e.name, root)
				if e.link != "" && !kept[name] && kept[e.link] {
					links = append(links, entry{name: name, link: e.link})
					kept[name] = true
					changed = true
				}
			}
		}
		sort.Slice(files, func(i, j int) bool { return files[i].name < files[j].name })
		sort.Slice(links, func(i, j int) bool { return links[i].name < links[j].name })
		fmt.Fprintf(&b, "\t{\n\t\tGOOS: %q, GOARCH: %q, Backend: %q,\n\t\tURL:    %q,\n\t\tSHA256: %q, Size: %d,\n\t\tRoot:   %q,\n\t\tFiles: []File{\n",
			p.goos, p.goarch, p.backend, p.url, sum, size, root)
		for _, f := range files {
			fmt.Fprintf(&b, "\t\t\t{Name: %q, SHA256: %q, Size: %d},\n", f.name, f.sum, f.size)
		}
		b.WriteString("\t\t},\n")
		if len(links) > 0 {
			b.WriteString("\t\tLinks: []Link{\n")
			for _, l := range links {
				fmt.Fprintf(&b, "\t\t\t{Name: %q, Target: %q},\n", l.name, l.link)
			}
			b.WriteString("\t\t},\n")
		}
		b.WriteString("\t},\n")
	}
	b.WriteString("}\n")
	src, err := format.Source([]byte(b.String()))
	if err != nil {
		return err
	}
	_, err = os.Stdout.Write(src)
	return err
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

func hashReader(r io.Reader) (string, int64, error) {
	h := sha256.New()
	n, err := io.Copy(h, r)
	return hex.EncodeToString(h.Sum(nil)), n, err
}

func list(archive string) ([]entry, error) {
	var out []entry
	if strings.HasSuffix(archive, ".zip") {
		zr, err := zip.OpenReader(archive)
		if err != nil {
			return nil, err
		}
		defer zr.Close()
		for _, zf := range zr.File {
			if zf.FileInfo().IsDir() {
				continue
			}
			r, err := zf.Open()
			if err != nil {
				return nil, err
			}
			sum, n, err := hashReader(r)
			r.Close()
			if err != nil {
				return nil, err
			}
			out = append(out, entry{name: zf.Name, sum: sum, size: n})
		}
		return out, nil
	}
	f, err := os.Open(archive)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	gz, err := gzip.NewReader(f)
	if err != nil {
		return nil, err
	}
	tr := tar.NewReader(gz)
	for {
		h, err := tr.Next()
		if errors.Is(err, io.EOF) {
			return out, nil
		}
		if err != nil {
			return nil, err
		}
		switch h.Typeflag {
		case tar.TypeReg:
			sum, n, err := hashReader(tr)
			if err != nil {
				return nil, err
			}
			out = append(out, entry{name: h.Name, sum: sum, size: n})
		case tar.TypeSymlink:
			out = append(out, entry{name: h.Name, link: h.Linkname})
		}
	}
}
