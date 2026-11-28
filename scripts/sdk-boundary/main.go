// Command sdk-boundary checks the rules of the MIT SDK tree (sdk/README.md):
// all Rust is one crate in sdk/rust, sdk/go imports nothing of the app, each
// SDK has the same MIT license, the versions agree, and no commit since the
// base carries AI attribution.
//
//	go run ./scripts/sdk-boundary [-base main] [-skip-deps]
package main

import (
	"bytes"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

func main() {
	base := flag.String("base", "main", "the commits after this ref get the attribution check; empty skips it")
	skipDeps := flag.Bool("skip-deps", false, "skip `go list -deps` in sdk/go")
	flag.Parse()
	root, err := gitOutput(".", "rev-parse", "--show-toplevel")
	if err != nil {
		fail(err)
	}
	root = strings.TrimSpace(root)
	repo, err := openGitRepo(root)
	if err != nil {
		fail(err)
	}
	var violations []Violation
	violations = append(violations, CheckRust(repo)...)
	var deps []string
	if !*skipDeps {
		deps, err = goDeps(filepath.Join(root, "sdk", "go"))
		if err != nil {
			fail(err)
		}
	}
	violations = append(violations, CheckGo(repo, deps)...)
	violations = append(violations, CheckMIT(repo)...)
	violations = append(violations, CheckVersions(repo)...)
	violations = append(violations, CheckGPLPackages(repo)...)
	if *base != "" {
		commits, err := commitsSince(root, *base)
		if err != nil {
			fail(err)
		}
		violations = append(violations, CheckAttribution(commits)...)
	}
	for _, v := range violations {
		fmt.Println(v)
	}
	if len(violations) > 0 {
		fmt.Printf("sdk-boundary: %d violations\n", len(violations))
		os.Exit(1)
	}
	fmt.Printf("sdk-boundary: ok (%d files, %d sdk/go packages)\n", len(repo.Files()), len(deps))
}

func fail(err error) {
	fmt.Fprintln(os.Stderr, "sdk-boundary:", err)
	os.Exit(2)
}

func gitOutput(dir string, args ...string) (string, error) {
	cmd := exec.Command("git", args...)
	cmd.Dir = dir
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	out, err := cmd.Output()
	if err != nil {
		return "", fmt.Errorf("git %s: %v: %s", strings.Join(args, " "), err, strings.TrimSpace(stderr.String()))
	}
	return string(out), nil
}

type gitRepo struct {
	root  string
	files []string
}

// openGitRepo lists the tracked files and the new files that git does not
// ignore, so the check also sees work that is not committed yet.
func openGitRepo(root string) (*gitRepo, error) {
	out, err := gitOutput(root, "ls-files", "-z", "--cached", "--others", "--exclude-standard")
	if err != nil {
		return nil, err
	}
	var files []string
	for _, f := range strings.Split(out, "\x00") {
		if f == "" {
			continue
		}
		if _, err := os.Lstat(filepath.Join(root, filepath.FromSlash(f))); err == nil {
			files = append(files, f)
		}
	}
	return &gitRepo{root: root, files: files}, nil
}

func (g *gitRepo) Files() []string { return g.files }

func (g *gitRepo) Read(name string) ([]byte, error) {
	return os.ReadFile(filepath.Join(g.root, filepath.FromSlash(name)))
}

// goDeps lists the packages that sdk/go and its tests use, for each system.
func goDeps(dir string) ([]string, error) {
	seen := map[string]bool{}
	var out []string
	for _, goos := range []string{"darwin", "linux", "windows"} {
		cmd := exec.Command("go", "list", "-deps", "-test", "./...")
		cmd.Dir = dir
		cmd.Env = append(os.Environ(), "GOOS="+goos, "CGO_ENABLED=0", "GOFLAGS=-mod=mod")
		var stderr bytes.Buffer
		cmd.Stderr = &stderr
		list, err := cmd.Output()
		if err != nil {
			return nil, fmt.Errorf("go list in sdk/go (GOOS=%s): %v: %s", goos, err, strings.TrimSpace(stderr.String()))
		}
		for _, p := range strings.Fields(string(list)) {
			if !seen[p] {
				seen[p] = true
				out = append(out, p)
			}
		}
	}
	return out, nil
}

func commitsSince(root, base string) ([]Commit, error) {
	out, err := gitOutput(root, "log", "--format=%H%x00%B%x1e", base+"..HEAD")
	if err != nil {
		return nil, err
	}
	var commits []Commit
	for _, rec := range strings.Split(out, "\x1e") {
		rec = strings.TrimLeft(rec, "\n")
		hash, msg, ok := strings.Cut(rec, "\x00")
		if ok {
			commits = append(commits, Commit{Hash: hash, Message: msg})
		}
	}
	return commits, nil
}
