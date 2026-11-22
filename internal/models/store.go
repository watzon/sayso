package models

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"log/slog"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"time"
)

// The model store (docs/sdk.md, "Model store"). Each artifact is a folder
// that never changes; a manifest per model and format names its artifacts.
// Engines of different programs share one store, so each step that changes
// it holds file locks:
//
//   - locks/meta: shared to read manifests, exclusive to change them or to
//     decide what is garbage. Held for short steps only.
//   - locks/<digest>.get: exclusive while one engine downloads the artifact.
//   - locks/<digest>.use: shared while a model is loaded, exclusive to delete.
//
// The order is get, meta, use; locks of one kind go in digest order.

const (
	// StoreFolder is the store folder in the models folder.
	StoreFolder = "v1"
	// StoreEnv names a store folder for an engine.
	StoreEnv = "SAYSO_MODEL_STORE"
	// HookEnv makes the store wait at its steps, for conformance tests.
	HookEnv = "SAYSO_STORE_TEST_HOOK"
	// DescriptorsEnv is a file with descriptors in place of the built-in ones.
	DescriptorsEnv = "SAYSO_STORE_TEST_DESCRIPTORS"
)

// ErrVerify marks fetched files that are not the files of the descriptor.
var ErrVerify = errors.New("verify")

// FetchError wraps an error of the fetch function. A later download starts
// the artifact again.
type FetchError struct{ Err error }

func (e *FetchError) Error() string { return e.Err.Error() }
func (e *FetchError) Unwrap() error { return e.Err }

// Fetch gets an artifact, given an empty work folder and the path that must
// hold its files when it returns.
type Fetch func(a Artifact, work, files string) error

// Manifest is models/<model id>/<format>.json.
type Manifest struct {
	Revision uint32         `json:"revision"`
	Parts    []ManifestPart `json:"parts"`
}

type ManifestPart struct {
	Role   string `json:"role"`
	Digest string `json:"digest"`
}

// Lease is a loaded model: the folder of each role and the shared use locks.
// Close it when nothing reads the files.
type Lease struct {
	folders map[string]string
	locks   []*os.File
}

// Folder is the folder of the artifact with this role.
func (l *Lease) Folder(role string) (string, bool) {
	f, ok := l.folders[role]
	return f, ok
}

func (l *Lease) Close() {
	for _, f := range l.locks {
		f.Close()
	}
	l.locks = nil
}

// Store is the store at one folder.
type Store struct {
	root string
	hook string
}

func openLock(path string) (*os.File, error) {
	f, err := os.OpenFile(path, os.O_RDWR|os.O_CREATE, 0o644)
	if err != nil {
		return nil, fmt.Errorf("cannot open the lock %s: %w", path, err)
	}
	return f, nil
}

func lockAt(path string, exclusive bool) (*os.File, error) {
	f, err := openLock(path)
	if err != nil {
		return nil, err
	}
	if _, err := lockFile(f, exclusive, true); err != nil {
		f.Close()
		return nil, fmt.Errorf("cannot lock %s: %w", path, err)
	}
	return f, nil
}

// OpenStore opens the store at root and makes its folders.
func OpenStore(root string) (*Store, error) {
	return openStoreWithHook(root, os.Getenv(HookEnv))
}

func openStoreWithHook(root, hook string) (*Store, error) {
	for _, folder := range []string{"artifacts", "models", "locks", "staging", "trash"} {
		p := filepath.Join(root, folder)
		if err := os.MkdirAll(p, 0o755); err != nil {
			return nil, fmt.Errorf("cannot create %s: %w", p, err)
		}
	}
	return &Store{root: root, hook: hook}, nil
}

func (s *Store) Root() string { return s.root }

// Step is a step of the rules. With the test hook it writes at-<step> in the
// hook folder, then waits while hold-<step> exists.
func (s *Store) Step(name string) {
	if s.hook == "" {
		return
	}
	_ = os.WriteFile(filepath.Join(s.hook, "at-"+name), nil, 0o644)
	hold := filepath.Join(s.hook, "hold-"+name)
	for exists(hold) {
		time.Sleep(5 * time.Millisecond)
	}
}

func exists(p string) bool { _, err := os.Stat(p); return err == nil }

func isDir(p string) bool {
	fi, err := os.Stat(p)
	return err == nil && fi.IsDir()
}

// ArtifactPath is artifacts/<digest>.
func (s *Store) ArtifactPath(digest string) string { return filepath.Join(s.root, "artifacts", digest) }

func (s *Store) lockPath(name string) string { return filepath.Join(s.root, "locks", name) }

func (s *Store) metaShared() (*os.File, error)    { return lockAt(s.lockPath("meta"), false) }
func (s *Store) metaExclusive() (*os.File, error) { return lockAt(s.lockPath("meta"), true) }

func (s *Store) manifestPath(model, format string) (string, error) {
	for _, name := range []string{model, format} {
		if name == "" || strings.HasPrefix(name, ".") || strings.ContainsAny(name, `/\:`) {
			return "", fmt.Errorf("%q is not a plain name", name)
		}
	}
	return filepath.Join(s.root, "models", model, format+".json"), nil
}

// readManifest: hold meta. A file that does not parse is no manifest.
func (s *Store) readManifest(model, format string) (*Manifest, error) {
	p, err := s.manifestPath(model, format)
	if err != nil {
		return nil, err
	}
	b, err := os.ReadFile(p)
	if errors.Is(err, fs.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("cannot read %s: %w", p, err)
	}
	return parseManifest(b), nil
}

// parseManifest reads a manifest strictly, as serde does: both fields must be
// there. nil when the bytes are no manifest.
func parseManifest(b []byte) *Manifest {
	var raw struct {
		Revision *uint32 `json:"revision"`
		Parts    *[]struct {
			Role   *string `json:"role"`
			Digest *string `json:"digest"`
		} `json:"parts"`
	}
	if json.Unmarshal(b, &raw) != nil || raw.Revision == nil || raw.Parts == nil {
		return nil
	}
	m := &Manifest{Revision: *raw.Revision, Parts: []ManifestPart{}}
	for _, p := range *raw.Parts {
		if p.Role == nil || p.Digest == nil {
			return nil
		}
		m.Parts = append(m.Parts, ManifestPart{*p.Role, *p.Digest})
	}
	return m
}

func (s *Store) complete(m *Manifest, d Descriptor) bool {
	if m == nil || m.Revision < d.Revision {
		return false
	}
	for _, p := range m.Parts {
		if !isDir(s.ArtifactPath(p.Digest)) {
			return false
		}
	}
	return true
}

// OnDisk is true when the manifest has the descriptor's revision or higher
// and each artifact exists.
func (s *Store) OnDisk(d Descriptor) (bool, error) {
	meta, err := s.metaShared()
	if err != nil {
		return false, err
	}
	defer meta.Close()
	m, err := s.readManifest(d.Model, d.Format)
	if err != nil {
		return false, err
	}
	return s.complete(m, d), nil
}

// OnDiskNow is OnDisk without a wait; ok is false when another engine holds meta.
func (s *Store) OnDiskNow(d Descriptor) (onDisk, ok bool, err error) {
	meta, err := openLock(s.lockPath("meta"))
	if err != nil {
		return false, false, err
	}
	defer meta.Close()
	got, err := lockFile(meta, false, false)
	if err != nil || !got {
		return false, false, err
	}
	m, err := s.readManifest(d.Model, d.Format)
	if err != nil {
		return false, false, err
	}
	return s.complete(m, d), true, nil
}

// Load gives the folders of the model, locked for use; nil when the model is
// not on disk. It never writes in the store apart from the lock files.
func (s *Store) Load(d Descriptor) (*Lease, error) {
	meta, err := s.metaShared()
	if err != nil {
		return nil, err
	}
	defer func() {
		if meta != nil {
			meta.Close()
		}
	}()
	m, err := s.readManifest(d.Model, d.Format)
	if err != nil || m == nil {
		return nil, err
	}
	s.Step("load.manifest_read")
	if m.Revision < d.Revision {
		return nil, nil
	}
	digests := map[string]bool{}
	for _, p := range m.Parts {
		digests[p.Digest] = true
	}
	sorted := make([]string, 0, len(digests))
	for dg := range digests {
		sorted = append(sorted, dg)
	}
	sort.Strings(sorted)
	lease := &Lease{folders: map[string]string{}}
	for _, dg := range sorted {
		l, err := lockAt(s.lockPath(dg+".use"), false)
		if err != nil {
			lease.Close()
			return nil, err
		}
		lease.locks = append(lease.locks, l)
	}
	s.Step("load.use_locked")
	if !s.complete(m, d) {
		lease.Close()
		return nil, nil
	}
	for _, p := range m.Parts {
		lease.folders[p.Role] = s.ArtifactPath(p.Digest)
	}
	meta.Close()
	meta = nil
	s.Step("load.ready")
	return lease, nil
}

type staged struct{ digest, stage string }

// Download gets the missing artifacts of a descriptor and writes its
// manifest. `waiting` runs when another engine downloads an artifact this
// model needs.
func (s *Store) Download(data *Descriptors, d Descriptor, fetch Fetch, waiting func()) error {
	parts, err := data.Parts(d)
	if err != nil {
		return err
	}
	var artifacts []Artifact
	seen := map[string]bool{}
	for _, p := range parts {
		if !seen[p.Artifact.Digest] {
			seen[p.Artifact.Digest] = true
			artifacts = append(artifacts, p.Artifact)
		}
	}
	sort.Slice(artifacts, func(i, j int) bool { return artifacts[i].Digest < artifacts[j].Digest })
	if waiting == nil {
		waiting = func() {}
	}
	for {
		// Step 1.
		meta, err := s.metaShared()
		if err != nil {
			return err
		}
		m, err := s.readManifest(d.Model, d.Format)
		if err != nil {
			meta.Close()
			return err
		}
		if s.complete(m, d) {
			meta.Close()
			return nil
		}
		var missing []Artifact
		for _, a := range artifacts {
			if !isDir(s.ArtifactPath(a.Digest)) {
				missing = append(missing, a)
			}
		}
		meta.Close()
		s.Step("download.manifest_read")

		// Step 2. The get locks stay until the artifacts are published.
		var gets []*os.File
		closeGets := func() {
			for _, g := range gets {
				g.Close()
			}
		}
		var stages []staged
		for _, a := range missing {
			g, err := s.lockGet(a.Digest, waiting)
			if err != nil {
				closeGets()
				return err
			}
			gets = append(gets, g)
			s.Step("download.get_locked")
			meta, err := s.metaShared()
			if err != nil {
				closeGets()
				return err
			}
			present := isDir(s.ArtifactPath(a.Digest))
			meta.Close()
			if present {
				continue
			}
			stage, err := s.fetch(a, fetch)
			if err != nil {
				// A retry starts again, so a stage of this attempt is only in the way.
				for _, st := range stages {
					os.RemoveAll(st.stage)
				}
				closeGets()
				return err
			}
			stages = append(stages, staged{a.Digest, stage})
		}

		// Step 3.
		metaX, err := s.metaExclusive()
		if err != nil {
			closeGets()
			return err
		}
		s.Step("download.meta_locked")
		for _, st := range stages {
			dest := s.ArtifactPath(st.digest)
			if !isDir(dest) {
				if err := renameRetry(filepath.Join(st.stage, "files"), dest); err != nil {
					metaX.Close()
					closeGets()
					return err
				}
			}
			os.RemoveAll(st.stage)
			s.Step("download.artifact_published")
		}
		syncFolder(filepath.Join(s.root, "artifacts"))
		// A cleanup can take an artifact that no manifest named after step 1;
		// then this goes back to step 1 with what it has published.
		all := true
		for _, a := range artifacts {
			if !isDir(s.ArtifactPath(a.Digest)) {
				all = false
			}
		}
		if all {
			if err := s.writeManifest(d, data); err != nil {
				metaX.Close()
				closeGets()
				return err
			}
			s.Step("download.manifest_written")
			m, err := s.readManifest(d.Model, d.Format)
			if err == nil && !s.complete(m, d) {
				err = fmt.Errorf("%w: the manifest of %s has a higher revision and names a missing artifact; delete the model", ErrVerify, d.Model)
			}
			metaX.Close()
			closeGets()
			if err != nil {
				return err
			}
			s.Step("download.done")
			return nil
		}
		metaX.Close()
		closeGets()
	}
}

func (s *Store) lockGet(digest string, waiting func()) (*os.File, error) {
	p := s.lockPath(digest + ".get")
	f, err := openLock(p)
	if err != nil {
		return nil, err
	}
	got, err := lockFile(f, true, false)
	if err != nil {
		f.Close()
		return nil, fmt.Errorf("cannot lock %s: %w", p, err)
	}
	if !got {
		waiting()
		if _, err := lockFile(f, true, true); err != nil {
			f.Close()
			return nil, fmt.Errorf("cannot lock %s: %w", p, err)
		}
	}
	return f, nil
}

func randomHex() string {
	var b [16]byte
	_, _ = rand.Read(b[:])
	return hex.EncodeToString(b[:])
}

// fetch is step 2 for one artifact: a staging folder with an owner file, the
// files, and the check. The caller holds the get lock.
func (s *Store) fetch(a Artifact, fetch Fetch) (string, error) {
	stage := filepath.Join(s.root, "staging", fmt.Sprintf("%s.%d.%s", a.Digest, os.Getpid(), randomHex()))
	work := filepath.Join(stage, "work")
	if err := os.MkdirAll(work, 0o755); err != nil {
		return "", fmt.Errorf("cannot create %s: %w", work, err)
	}
	owner := filepath.Join(stage, "owner")
	if err := os.WriteFile(owner, []byte(fmt.Sprintf("%d\n", os.Getpid())), 0o644); err != nil {
		return "", fmt.Errorf("cannot write %s: %w", owner, err)
	}
	s.Step("download.staged")
	files := filepath.Join(stage, "files")
	err := fetch(a, work, files)
	if err != nil {
		err = &FetchError{err}
	} else {
		err = Verify(files, a)
	}
	if err != nil {
		os.RemoveAll(stage)
		return "", err
	}
	s.Step("download.fetched")
	return stage, nil
}

// writeManifest: hold meta exclusive. A manifest never goes back to a lower revision.
func (s *Store) writeManifest(d Descriptor, data *Descriptors) error {
	existing, err := s.readManifest(d.Model, d.Format)
	if err != nil {
		return err
	}
	if existing != nil && existing.Revision > d.Revision {
		return nil
	}
	parts, err := data.Parts(d)
	if err != nil {
		return err
	}
	m := Manifest{Revision: d.Revision, Parts: make([]ManifestPart, 0, len(parts))}
	for _, p := range parts {
		m.Parts = append(m.Parts, ManifestPart{p.Role, p.Artifact.Digest})
	}
	p, err := s.manifestPath(d.Model, d.Format)
	if err != nil {
		return err
	}
	folder := filepath.Dir(p)
	if err := os.MkdirAll(folder, 0o755); err != nil {
		return fmt.Errorf("cannot create %s: %w", folder, err)
	}
	b, _ := json.MarshalIndent(m, "", "  ")
	tmp := filepath.Join(folder, fmt.Sprintf(".%d.%s.tmp", os.Getpid(), randomHex()))
	if err := writeSynced(tmp, b); err != nil {
		return fmt.Errorf("cannot write %s: %w", tmp, err)
	}
	if err := renameRetry(tmp, p); err != nil {
		return err
	}
	syncFolder(folder)
	return nil
}

func writeSynced(path string, b []byte) error {
	f, err := os.Create(path)
	if err != nil {
		return err
	}
	if _, err := f.Write(b); err != nil {
		f.Close()
		return err
	}
	if err := f.Sync(); err != nil {
		f.Close()
		return err
	}
	return f.Close()
}

// Delete removes a model's manifest, then cleans up. It never fails because
// of another engine: an engine with the model loaded keeps it until it unloads.
func (s *Store) Delete(model, format string) error {
	p, err := s.manifestPath(model, format)
	if err != nil {
		return err
	}
	meta, err := s.metaExclusive()
	if err != nil {
		return err
	}
	s.Step("delete.meta_locked")
	if err := os.Remove(p); err != nil && !errors.Is(err, fs.ErrNotExist) {
		meta.Close()
		return fmt.Errorf("cannot delete %s: %w", p, err)
	}
	s.Step("delete.manifest_removed")
	meta.Close()
	if err := s.Cleanup(); err != nil {
		slog.Warn("the cleanup of the model store failed", "err", err)
	}
	return nil
}

// Cleanup removes artifacts no manifest names, empties the trash, and removes
// abandoned staging folders. What it cannot remove now stays for the next one.
func (s *Store) Cleanup() error {
	meta, err := s.metaExclusive()
	if err != nil {
		return err
	}
	s.Step("cleanup.meta_locked")
	named, err := s.namedDigests()
	if err != nil {
		meta.Close()
		return err
	}
	if named != nil {
		entries, err := folders(filepath.Join(s.root, "artifacts"))
		if err != nil {
			meta.Close()
			return err
		}
		for _, e := range entries {
			digest := e.Name()
			if named[digest] {
				continue
			}
			l, err := openLock(s.lockPath(digest + ".use"))
			if err != nil {
				meta.Close()
				return err
			}
			if got, _ := lockFile(l, true, false); !got {
				l.Close()
				continue
			}
			s.Step("cleanup.use_locked")
			trash := filepath.Join(s.root, "trash", digest+"."+randomHex())
			_ = renameRetry(filepath.Join(s.root, "artifacts", digest), trash)
			s.Step("cleanup.trashed")
			l.Close()
		}
	}
	meta.Close()
	s.Step("cleanup.meta_released")
	trashEntries, err := os.ReadDir(filepath.Join(s.root, "trash"))
	if err != nil {
		return fmt.Errorf("cannot read trash: %w", err)
	}
	for _, e := range trashEntries {
		os.RemoveAll(filepath.Join(s.root, "trash", e.Name()))
	}
	s.Step("cleanup.trash_emptied")
	stagingEntries, err := folders(filepath.Join(s.root, "staging"))
	if err != nil {
		return err
	}
	for _, e := range stagingEntries {
		digest, _, _ := strings.Cut(e.Name(), ".")
		l, err := openLock(s.lockPath(digest + ".get"))
		if err != nil {
			return err
		}
		if got, _ := lockFile(l, true, false); got {
			s.Step("cleanup.staging_locked")
			os.RemoveAll(filepath.Join(s.root, "staging", e.Name()))
		}
		l.Close()
	}
	s.Step("cleanup.done")
	return nil
}

// namedDigests: hold meta exclusive. Removes half-written manifest files
// (dot files) and empty model folders. nil when a manifest does not parse.
func (s *Store) namedDigests() (map[string]bool, error) {
	named := map[string]bool{}
	models, err := folders(filepath.Join(s.root, "models"))
	if err != nil {
		return nil, err
	}
	for _, model := range models {
		dir := filepath.Join(s.root, "models", model.Name())
		files, err := os.ReadDir(dir)
		if err != nil {
			return nil, fmt.Errorf("cannot read %s: %w", dir, err)
		}
		for _, f := range files {
			p := filepath.Join(dir, f.Name())
			if strings.HasPrefix(f.Name(), ".") {
				os.Remove(p)
				continue
			}
			b, err := os.ReadFile(p)
			if err != nil {
				return nil, fmt.Errorf("cannot read %s: %w", p, err)
			}
			m := parseManifest(b)
			if m == nil {
				return nil, nil
			}
			for _, part := range m.Parts {
				named[part.Digest] = true
			}
		}
		os.Remove(dir)
	}
	return named, nil
}

// folders lists the folders in path; the store ignores other files.
func folders(path string) ([]fs.DirEntry, error) {
	entries, err := os.ReadDir(path)
	if err != nil {
		return nil, fmt.Errorf("cannot read %s: %w", path, err)
	}
	out := entries[:0]
	for _, e := range entries {
		if e.IsDir() {
			out = append(out, e)
		}
	}
	return out, nil
}

// syncFolder makes a rename in folder last through a power cut, where the
// system can do that.
func syncFolder(folder string) {
	if runtime.GOOS == "windows" {
		return
	}
	if f, err := os.Open(folder); err == nil {
		_ = f.Sync()
		f.Close()
	}
}

// renameRetry renames. Windows can refuse for a moment while another program
// has a file open, so there it retries for about two seconds.
func renameRetry(from, to string) error {
	attempts := 1
	if runtime.GOOS == "windows" {
		attempts = 20
	}
	var err error
	for i := 1; i <= attempts; i++ {
		if err = os.Rename(from, to); err == nil {
			return nil
		}
		if i < attempts {
			time.Sleep(100 * time.Millisecond)
		}
	}
	return fmt.Errorf("cannot move %s: %w", from, err)
}

// Verify checks that folder holds the files of the artifact and nothing else,
// with their sizes and hashes, and flushes each file.
func Verify(folder string, a Artifact) error {
	var found []string
	if err := walk(folder, "", &found); err != nil {
		return err
	}
	sort.Strings(found)
	wanted := make([]string, len(a.Files))
	for i, f := range a.Files {
		wanted[i] = f.Path
	}
	sort.Strings(wanted)
	if strings.Join(found, "\x00") != strings.Join(wanted, "\x00") {
		return fmt.Errorf("%w: the files of %s are not the files of its descriptor: %q", ErrVerify, a.Digest, found)
	}
	for _, f := range a.Files {
		p := filepath.Join(folder, filepath.FromSlash(f.Path))
		fi, err := os.Stat(p)
		if err != nil {
			return fmt.Errorf("cannot read %s: %w", p, err)
		}
		if uint64(fi.Size()) != f.Size {
			return fmt.Errorf("%w: %s has %d bytes, not %d", ErrVerify, f.Path, fi.Size(), f.Size)
		}
		h, err := HashFile(p)
		if err != nil {
			return err
		}
		if h != f.SHA256 {
			return fmt.Errorf("%w: %s does not have the hash of its descriptor", ErrVerify, f.Path)
		}
		// Windows can only flush a handle that can write.
		flag := os.O_RDONLY
		if runtime.GOOS == "windows" {
			flag = os.O_WRONLY
		}
		if fh, err := os.OpenFile(p, flag, 0); err == nil {
			err = fh.Sync()
			fh.Close()
			if err != nil && runtime.GOOS != "windows" {
				return fmt.Errorf("cannot write %s: %w", p, err)
			}
		} else {
			return fmt.Errorf("cannot write %s: %w", p, err)
		}
	}
	return nil
}

func walk(folder, prefix string, found *[]string) error {
	entries, err := os.ReadDir(folder)
	if err != nil {
		return fmt.Errorf("cannot read %s: %w", folder, err)
	}
	for _, e := range entries {
		p := prefix + e.Name()
		switch {
		case e.Type().IsDir():
			if err := walk(filepath.Join(folder, e.Name()), p+"/", found); err != nil {
				return err
			}
		case e.Type().IsRegular():
			*found = append(*found, p)
		default:
			return fmt.Errorf("%w: %s is not a file or a folder", ErrVerify, p)
		}
	}
	return nil
}

// HashFile is the SHA-256 of a file in lowercase hex.
func HashFile(path string) (string, error) {
	f, err := os.Open(path)
	if err != nil {
		return "", fmt.Errorf("cannot read %s: %w", path, err)
	}
	defer f.Close()
	h := sha256.New()
	if _, err := io.CopyBuffer(h, f, make([]byte, 1<<20)); err != nil {
		return "", fmt.Errorf("cannot read %s: %w", path, err)
	}
	return hex.EncodeToString(h.Sum(nil)), nil
}
