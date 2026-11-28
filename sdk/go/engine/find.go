package engine

import (
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"

	"golang.org/x/sys/cpu"
)

// Program is the file name of the engine in a release folder.
func Program() string {
	if runtime.GOOS == "windows" {
		return "sayso-engine.exe"
	}
	return "sayso-engine"
}

const folderPrefix = "sayso-engine-"

type build struct {
	name string
	runs func() bool
}

// builds are the builds of a release folder for x86-64, best first: the
// whisper.cpp of each is compiled for one instruction set.
var builds = []build{
	{"sayso-engine", runsAVX2},
	{"sayso-engine-avx", runsAVX},
	{"sayso-engine-baseline", func() bool { return true }},
}

func runsAVX() bool {
	if runtime.GOARCH != "amd64" {
		return false
	}
	return cpu.X86.HasAVX && cpu.X86.HasSSE42
}

func runsAVX2() bool {
	if runtime.GOARCH != "amd64" {
		return true
	}
	return runsAVX() && cpu.X86.HasAVX2 && cpu.X86.HasFMA && cpu.X86.HasBMI2 && cpu.X86.HasF16C
}

func isFile(path string) bool {
	fi, err := os.Stat(path)
	return err == nil && fi.Mode().IsRegular()
}

func bestBuild(dir string) string {
	exe := ""
	if runtime.GOOS == "windows" {
		exe = ".exe"
	}
	for _, b := range builds {
		if p := filepath.Join(dir, b.name+exe); b.runs() && isFile(p) {
			return p
		}
	}
	return filepath.Join(dir, Program())
}

// Find is the engine program in folder, where a release file of the engine
// was unpacked (sdk/protocol/paths.md, "Engine program"). folder holds the
// program, or folders sayso-engine-<version> that hold it; then the folder
// whose name sorts last wins. On x86-64 it picks the best build that this
// processor runs.
func Find(folder string) (string, error) {
	if isFile(filepath.Join(folder, Program())) {
		return bestBuild(folder), nil
	}
	entries, err := os.ReadDir(folder)
	if err != nil {
		return "", fmt.Errorf("cannot read %s: %w", folder, err)
	}
	var versions []string
	for _, e := range entries {
		dir := filepath.Join(folder, e.Name())
		if strings.HasPrefix(e.Name(), folderPrefix) && isFile(filepath.Join(dir, Program())) {
			versions = append(versions, dir)
		}
	}
	if len(versions) == 0 {
		return "", fmt.Errorf("there is no %s in %s, and no %s<version> folder with one. Unpack a release file of the engine there: %w", Program(), folder, folderPrefix, fs.ErrNotExist)
	}
	sort.Strings(versions)
	return bestBuild(versions[len(versions)-1]), nil
}
