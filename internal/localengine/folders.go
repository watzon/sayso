package localengine

import (
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"time"
)

// The model folders under SAYSO_MODELS_DIR, for a model without a store
// descriptor:
//
//	<model id>/
//	  <archive>/            the extracted archive (or a GGUF commit), renamed into place when complete
//	  <preview archive>/    the streaming archive of a model with a live preview
//	  .partial-<archive>/   an extraction in progress
//	  .complete             the archive names, one per line, written last
//
// A model is downloaded only when .complete names exactly its archives, so a
// download that stopped half way never looks complete. The Windows engine
// before 0.12 kept a model's files directly in its folder with the marker
// .sayso-downloaded; its Whisper files move into this layout (adoptLegacy).

const (
	completeMarker = ".complete"
	partialPrefix  = ".partial-"
	legacyMarker   = ".sayso-downloaded"
)

// legacyState is what the files of the Windows engine before 0.12 are.
type legacyState int

const (
	legacyNone legacyState = iota
	legacyAdopted
	legacyFormatChanged
)

type modelFolders struct {
	root string
}

// folder is the folder of a model. The id must be one plain path part.
func (m *modelFolders) folder(model string) (string, error) {
	plain := model != "" && !strings.HasPrefix(model, ".") && !strings.ContainsAny(model, `/\:`) && model != ".."
	if !plain {
		return "", fmt.Errorf("invalid model id %q", model)
	}
	return filepath.Join(m.root, model), nil
}

func (m *modelFolders) archiveDir(model, archive string) (string, error) {
	folder, err := m.folder(model)
	if err != nil {
		return "", err
	}
	return filepath.Join(folder, archive), nil
}

func (m *modelFolders) stagingDir(model, archive string) (string, error) {
	folder, err := m.folder(model)
	if err != nil {
		return "", err
	}
	return filepath.Join(folder, partialPrefix+archive), nil
}

func isDir(path string) bool {
	info, err := os.Stat(path)
	return err == nil && info.IsDir()
}

func isFile(path string) bool {
	info, err := os.Stat(path)
	return err == nil && info.Mode().IsRegular()
}

// isDownloaded is true when every archive of the model is extracted and marked complete.
func (m *modelFolders) isDownloaded(model string, spec *modelSpec) bool {
	folder, err := m.folder(model)
	if err != nil {
		return false
	}
	marker, err := os.ReadFile(filepath.Join(folder, completeMarker))
	if err != nil {
		return false
	}
	var listed []string
	for _, line := range strings.Split(string(marker), "\n") {
		if line = strings.TrimSuffix(line, "\r"); line != "" {
			listed = append(listed, line)
		}
	}
	archives := spec.archives()
	if !slices.Equal(listed, archives) {
		return false
	}
	for _, a := range archives {
		if !isDir(filepath.Join(folder, a)) {
			return false
		}
	}
	return true
}

// markDownloaded writes the marker. Call it after every archive is in place.
func (m *modelFolders) markDownloaded(model string, spec *modelSpec) error {
	folder, err := m.folder(model)
	if err != nil {
		return err
	}
	temp := filepath.Join(folder, completeMarker+".tmp")
	if err := os.WriteFile(temp, []byte(strings.Join(spec.archives(), "\n")+"\n"), 0o644); err != nil {
		return fmt.Errorf("cannot write %s: %w", temp, err)
	}
	if err := os.Rename(temp, filepath.Join(folder, completeMarker)); err != nil {
		return fmt.Errorf("cannot write the marker in %s: %w", folder, err)
	}
	return nil
}

// prepareDownload removes the marker and everything that is not a complete
// archive of spec: stopped extractions, archives of an older version, and
// files of the Windows engine before 0.12.
func (m *modelFolders) prepareDownload(model string, spec *modelSpec) error {
	folder, err := m.folder(model)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(folder, 0o755); err != nil {
		return fmt.Errorf("cannot create %s: %w", folder, err)
	}
	if err := removeFileIfExists(filepath.Join(folder, completeMarker)); err != nil {
		return err
	}
	entries, err := os.ReadDir(folder)
	if err != nil {
		return err
	}
	archives := spec.archives()
	for _, e := range entries {
		if e.IsDir() && slices.Contains(archives, e.Name()) {
			continue
		}
		path := filepath.Join(folder, e.Name())
		if err := os.RemoveAll(path); err != nil {
			return fmt.Errorf("cannot remove %s: %w", path, err)
		}
	}
	return nil
}

// adoptLegacy moves the files of the Windows engine before 0.12 into this
// layout. A Whisper GGML file moves into its archive folder. Other files stay,
// and a download of the model removes them.
func (m *modelFolders) adoptLegacy(model string, spec *modelSpec) (legacyState, error) {
	folder, err := m.folder(model)
	if err != nil {
		return legacyNone, err
	}
	marker := filepath.Join(folder, legacyMarker)
	if !isFile(marker) {
		return legacyNone, nil
	}
	if spec.source.kind != sourceHuggingFace || spec.recipe != recipeWhisperCpp || !isFile(filepath.Join(folder, spec.source.file)) {
		return legacyFormatChanged, nil
	}
	file := spec.source.file
	dest, err := m.archiveDir(model, spec.archive)
	if err != nil {
		return legacyNone, err
	}
	if err := os.MkdirAll(dest, 0o755); err != nil {
		return legacyNone, fmt.Errorf("cannot create %s: %w", dest, err)
	}
	if err := os.Rename(filepath.Join(folder, file), filepath.Join(dest, file)); err != nil {
		return legacyNone, fmt.Errorf("cannot move %s into %s: %w", file, dest, err)
	}
	if err := m.markDownloaded(model, spec); err != nil {
		return legacyNone, err
	}
	if err := removeFileIfExists(marker); err != nil {
		return legacyNone, err
	}
	return legacyAdopted, nil
}

// delete removes every file of a model. Windows can refuse for a moment while
// a file is still open, so there it tries again for about two seconds.
func (m *modelFolders) delete(model string) error {
	folder, err := m.folder(model)
	if err != nil {
		return err
	}
	return retryOnWindows(func() error { return os.RemoveAll(folder) }, func(err error) error {
		return fmt.Errorf("cannot delete %s: %w", folder, err)
	})
}

// retryOnWindows runs op, and on Windows tries again for about two seconds
// while it fails (a virus scanner or a closing file holds it).
func retryOnWindows(op func() error, wrap func(error) error) error {
	attempts := 1
	if runtime.GOOS == "windows" {
		attempts = 20
	}
	var err error
	for attempt := 1; attempt <= attempts; attempt++ {
		if err = op(); err == nil {
			return nil
		}
		if attempt < attempts {
			time.Sleep(100 * time.Millisecond)
		}
	}
	return wrap(err)
}

func removeFileIfExists(path string) error {
	if err := os.Remove(path); err != nil && !errors.Is(err, fs.ErrNotExist) {
		return fmt.Errorf("cannot remove %s: %w", path, err)
	}
	return nil
}
