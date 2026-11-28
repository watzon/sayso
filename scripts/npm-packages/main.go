// Command npm-packages makes the folders of the GPL npm packages of a release
// (packaging/npm) from its engine and cleanup release files:
//
//	go run ./scripts/npm-packages -release-dir <dir> -out <dir> [-allow-missing]
//
// For each platform package it checks the release file against its .sha256,
// unpacks it without its top folder, and adds the package.json of
// packaging/npm and the LICENSE of the repository. npm leaves symbolic links
// out of a package, so a link becomes a copy of the file that it names. The
// main packages get their package.json, README.md, and the LICENSE. The
// version of every package must be the version of the app. The command never
// publishes.
package main

import (
	"archive/tar"
	"archive/zip"
	"compress/gzip"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path"
	"path/filepath"
	"regexp"
	"strings"
)

// Platform is an npm platform package and the release file it holds.
type Platform struct {
	NPM, Release string
}

func (p Platform) ext() string {
	if strings.HasPrefix(p.Release, "windows") {
		return "zip"
	}
	return "tar.gz"
}

// Kind is a program with npm packages.
type Kind struct {
	Name      string
	Platforms []Platform
	// Program is the path of the program in the release file, for a platform.
	Program func(Platform) string
}

var kinds = []Kind{
	{
		Name: "engine",
		Platforms: []Platform{
			{"darwin-arm64", "macos-arm64"}, {"darwin-x64", "macos-x86_64"},
			{"linux-x64-gnu", "linux-x86_64"}, {"linux-arm64-gnu", "linux-arm64"},
			{"win32-x64-msvc", "windows-x64"},
		},
		Program: func(p Platform) string {
			if p.ext() == "zip" {
				return "sayso-engine.exe"
			}
			return "sayso-engine"
		},
	},
	{
		Name: "cleanup",
		Platforms: []Platform{
			{"darwin-arm64", "macos-arm64"},
			{"linux-x64-gnu", "linux-x86_64"}, {"linux-arm64-gnu", "linux-arm64"},
			{"win32-x64-msvc", "windows-x64"},
		},
		Program: func(p Platform) string {
			switch {
			case p.ext() == "zip":
				return "sayso-cleanup.exe"
			case strings.HasPrefix(p.Release, "macos"):
				return "bin/sayso-cleanup"
			}
			return "sayso-cleanup"
		},
	},
}

func main() {
	releaseDir := flag.String("release-dir", "", "the folder with the release files")
	out := flag.String("out", "", "the folder for the package folders (must not exist)")
	allowMissing := flag.Bool("allow-missing", false, "skip a platform with no release file")
	flag.Parse()
	if *releaseDir == "" || *out == "" || flag.NArg() != 0 {
		flag.Usage()
		os.Exit(2)
	}
	made, err := Make(".", *releaseDir, *out, *allowMissing)
	for _, m := range made {
		fmt.Println(m)
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(1)
	}
}

var appVersion = regexp.MustCompile(`(?m)^var Version = "([^"]+)"`)

// Make writes the package folders into out, and returns their paths.
func Make(root, releaseDir, out string, allowMissing bool) ([]string, error) {
	data, err := os.ReadFile(filepath.Join(root, "internal/update/version.go"))
	if err != nil {
		return nil, err
	}
	m := appVersion.FindSubmatch(data)
	if m == nil {
		return nil, errors.New("internal/update/version.go has no version")
	}
	version := string(m[1])
	if _, err := os.Stat(out); err == nil {
		return nil, fmt.Errorf("%s exists", out)
	}
	license := filepath.Join(root, "LICENSE")
	var made []string
	for _, k := range kinds {
		main := filepath.Join(out, k.Name)
		if err := manifest(root, k.Name, version, main); err != nil {
			return made, err
		}
		for _, f := range []string{"README.md"} {
			if err := copyFile(filepath.Join(root, "packaging/npm", k.Name, f), filepath.Join(main, f), 0o644); err != nil {
				return made, err
			}
		}
		if err := copyFile(license, filepath.Join(main, "LICENSE"), 0o644); err != nil {
			return made, err
		}
		made = append(made, main)
		for _, p := range k.Platforms {
			file := filepath.Join(releaseDir, fmt.Sprintf("sayso-%s-%s-%s.%s", k.Name, version, p.Release, p.ext()))
			if _, err := os.Stat(file); errors.Is(err, fs.ErrNotExist) && allowMissing {
				fmt.Fprintf(os.Stderr, "warning: no %s, so no @justsayso/%s-%s\n", filepath.Base(file), k.Name, p.NPM)
				continue
			}
			if err := checkSum(file); err != nil {
				return made, err
			}
			dir := filepath.Join(out, k.Name+"-"+p.NPM)
			top := fmt.Sprintf("sayso-%s-%s", k.Name, version)
			if p.ext() == "zip" {
				err = unzip(file, top, dir)
			} else {
				err = untar(file, top, dir)
			}
			if err != nil {
				return made, fmt.Errorf("%s: %w", filepath.Base(file), err)
			}
			if fi, err := os.Stat(filepath.Join(dir, filepath.FromSlash(k.Program(p)))); err != nil || !fi.Mode().IsRegular() {
				return made, fmt.Errorf("%s has no %s", filepath.Base(file), k.Program(p))
			}
			// The README of the release file says how to run the program; the package gets the README of npm.
			if err := os.Rename(filepath.Join(dir, "README.md"), filepath.Join(dir, strings.ToUpper(k.Name)+".md")); err != nil && !errors.Is(err, fs.ErrNotExist) {
				return made, err
			}
			if err := manifest(root, k.Name+"-"+p.NPM, version, dir); err != nil {
				return made, err
			}
			if err := os.Remove(filepath.Join(dir, "LICENSE")); err != nil && !errors.Is(err, fs.ErrNotExist) {
				return made, err
			}
			if err := copyFile(license, filepath.Join(dir, "LICENSE"), 0o644); err != nil {
				return made, err
			}
			readme := fmt.Sprintf("# @justsayso/%s-%s\n\nThe %s release file of Sayso %s for %s, for `@justsayso/%s`. See %s.md.\n\nLicense: GPL-3.0-only.\n",
				k.Name, p.NPM, k.Name, version, p.Release, k.Name, strings.ToUpper(k.Name))
			if err := os.WriteFile(filepath.Join(dir, "README.md"), []byte(readme), 0o644); err != nil {
				return made, err
			}
			made = append(made, dir)
		}
	}
	return made, nil
}

// manifest copies packaging/npm/<name>/package.json into dir, after it checks
// its name, license, and versions.
func manifest(root, name, version, dir string) error {
	src := filepath.Join(root, "packaging/npm", name, "package.json")
	data, err := os.ReadFile(src)
	if err != nil {
		return err
	}
	var pkg struct {
		Name, Version, License string
		OptionalDependencies   map[string]string
	}
	if err := json.Unmarshal(data, &pkg); err != nil {
		return fmt.Errorf("%s: %w", src, err)
	}
	switch {
	case pkg.Name != "@justsayso/"+name:
		return fmt.Errorf("%s: the name is %q, not @justsayso/%s", src, pkg.Name, name)
	case pkg.License != "GPL-3.0-only":
		return fmt.Errorf("%s: the license is %q, not GPL-3.0-only", src, pkg.License)
	case pkg.Version != version:
		return fmt.Errorf("%s: the version is %s, but the app is %s", src, pkg.Version, version)
	}
	for dep, v := range pkg.OptionalDependencies {
		if v != version {
			return fmt.Errorf("%s: %s is %s, but the app is %s", src, dep, v, version)
		}
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(dir, "package.json"), data, 0o644)
}

func checkSum(file string) error {
	side, err := os.ReadFile(file + ".sha256")
	if err != nil {
		return fmt.Errorf("%s has no checksum file: %w", filepath.Base(file), err)
	}
	fields := strings.Fields(string(side))
	if len(fields) == 0 {
		return fmt.Errorf("%s.sha256 is empty", filepath.Base(file))
	}
	f, err := os.Open(file)
	if err != nil {
		return err
	}
	defer f.Close()
	h := sha256.New()
	if _, err := io.Copy(h, f); err != nil {
		return err
	}
	if got := hex.EncodeToString(h.Sum(nil)); !strings.EqualFold(got, fields[0]) {
		return fmt.Errorf("%s has the sha256 %s, and its .sha256 file says %s", filepath.Base(file), got, fields[0])
	}
	return nil
}

// inside is the path of name below top, or an error when name is not in top
// or leaves it.
func inside(name, top string) (string, error) {
	name = strings.TrimSuffix(strings.ReplaceAll(name, `\`, "/"), "/")
	if name == top {
		return "", nil
	}
	rel, ok := strings.CutPrefix(name, top+"/")
	if !ok {
		return "", fmt.Errorf("%s is not in the folder %s", name, top)
	}
	clean := path.Clean(rel)
	if clean == ".." || strings.HasPrefix(clean, "../") || path.IsAbs(clean) {
		return "", fmt.Errorf("%s leaves the folder", name)
	}
	return clean, nil
}

func untar(file, top, dir string) error {
	f, err := os.Open(file)
	if err != nil {
		return err
	}
	defer f.Close()
	gz, err := gzip.NewReader(f)
	if err != nil {
		return err
	}
	tr := tar.NewReader(gz)
	links := map[string]string{}
	for {
		h, err := tr.Next()
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			return err
		}
		rel, err := inside(h.Name, top)
		if err != nil {
			return err
		}
		if rel == "" {
			continue
		}
		dest := filepath.Join(dir, filepath.FromSlash(rel))
		switch h.Typeflag {
		case tar.TypeDir:
			if err := os.MkdirAll(dest, 0o755); err != nil {
				return err
			}
		case tar.TypeReg:
			if err := writeFile(dest, tr, fs.FileMode(h.Mode)&0o755|0o644); err != nil {
				return err
			}
		case tar.TypeSymlink:
			target := path.Join(path.Dir(rel), h.Linkname)
			if _, err := inside(top+"/"+target, top); err != nil || path.IsAbs(h.Linkname) {
				return fmt.Errorf("the link %s leaves the folder", h.Name)
			}
			links[rel] = target
		default:
			return fmt.Errorf("%s is not a file, a folder, or a link", h.Name)
		}
	}
	return copyLinks(dir, links)
}

// copyLinks writes each link as a copy of the file that it names, after
// following the links between them.
func copyLinks(dir string, links map[string]string) error {
	for link := range links {
		target := links[link]
		for range len(links) + 1 {
			next, ok := links[target]
			if !ok {
				break
			}
			target = next
		}
		if _, ok := links[target]; ok {
			return fmt.Errorf("the link %s is a loop", link)
		}
		src := filepath.Join(dir, filepath.FromSlash(target))
		fi, err := os.Stat(src)
		if err != nil || !fi.Mode().IsRegular() {
			return fmt.Errorf("the link %s names %s, which is not a file in the release file", link, target)
		}
		if err := copyFile(src, filepath.Join(dir, filepath.FromSlash(link)), fi.Mode().Perm()); err != nil {
			return err
		}
	}
	return nil
}

func unzip(file, top, dir string) error {
	zr, err := zip.OpenReader(file)
	if err != nil {
		return err
	}
	defer zr.Close()
	for _, zf := range zr.File {
		rel, err := inside(zf.Name, top)
		if err != nil {
			return err
		}
		if rel == "" {
			continue
		}
		dest := filepath.Join(dir, filepath.FromSlash(rel))
		if zf.FileInfo().IsDir() {
			if err := os.MkdirAll(dest, 0o755); err != nil {
				return err
			}
			continue
		}
		if !zf.Mode().IsRegular() && zf.Mode()&fs.ModeType != 0 {
			return fmt.Errorf("%s is not a file or a folder", zf.Name)
		}
		r, err := zf.Open()
		if err != nil {
			return err
		}
		err = writeFile(dest, r, 0o755)
		r.Close()
		if err != nil {
			return err
		}
	}
	return nil
}

func writeFile(dest string, r io.Reader, mode fs.FileMode) error {
	if err := os.MkdirAll(filepath.Dir(dest), 0o755); err != nil {
		return err
	}
	f, err := os.OpenFile(dest, os.O_CREATE|os.O_EXCL|os.O_WRONLY, mode)
	if err != nil {
		return err
	}
	if _, err := io.Copy(f, r); err != nil {
		f.Close()
		return err
	}
	return f.Close()
}

func copyFile(src, dest string, mode fs.FileMode) error {
	f, err := os.Open(src)
	if err != nil {
		return err
	}
	defer f.Close()
	return writeFile(dest, f, mode)
}
