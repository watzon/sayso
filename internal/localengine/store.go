package localengine

import (
	"crypto/rand"
	"crypto/sha256"
	_ "embed"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode"
)

// The model store: the rules of docs/sdk.md, "Model store" (the Go port of
// sayso_core::model_store).
//
// Each artifact is a folder that never changes. A manifest for each model and
// format names its artifacts. Engines of different programs share one store,
// so each step that changes it holds file locks:
//
//   - locks/meta: shared to read manifests, exclusive to change them or to
//     decide what is garbage. Held for short steps only.
//   - locks/<digest>.get: exclusive while one engine downloads the artifact.
//   - locks/<digest>.use: shared while a model is loaded, exclusive to delete.
//
// The order is get, meta, use, and locks of one kind go in digest order.
// Nobody waits for a lock while it holds a lock that comes later.

const (
	storeEnv       = "SAYSO_MODEL_STORE"
	hookEnv        = "SAYSO_STORE_TEST_HOOK"
	descriptorsEnv = "SAYSO_STORE_TEST_DESCRIPTORS"

	formatSherpaOnnx  = "sherpa-onnx-1"
	formatWhisperCpp  = "whisper-cpp-1"
	formatParakeetCpp = "parakeet-cpp-1"
)

var errWouldBlock = errors.New("the lock is held by another process")

//go:embed catalog/descriptors.json
var builtInDescriptors []byte

// storeFile is one file of an artifact.
type storeFile struct {
	Path   string `json:"path"`
	Size   uint64 `json:"size"`
	SHA256 string `json:"sha256"`
}

// storeSource is where the files of an artifact come from: hugging_face,
// archive, or file.
type storeSource struct {
	Kind     string `json:"kind"`
	Repo     string `json:"repo,omitempty"`
	Revision string `json:"revision,omitempty"`
	Folder   string `json:"folder,omitempty"`
	URL      string `json:"url,omitempty"`
}

// fileURL is the URL of the file p of the artifact. Not ok for an archive.
func (s storeSource) fileURL(p string) (string, bool) {
	switch s.Kind {
	case "hugging_face":
		if s.Folder == "" {
			return "https://huggingface.co/" + s.Repo + "/resolve/" + s.Revision + "/" + p, true
		}
		return "https://huggingface.co/" + s.Repo + "/resolve/" + s.Revision + "/" + s.Folder + "/" + p, true
	case "file":
		return s.URL, true
	}
	return "", false
}

type artifact struct {
	Source storeSource `json:"source"`
	Digest string      `json:"digest"`
	Files  []storeFile `json:"files"`
}

func (a *artifact) size() uint64 {
	var total uint64
	for _, f := range a.Files {
		total += f.Size
	}
	return total
}

type descriptorPart struct {
	Role     string `json:"role"`
	Artifact string `json:"artifact"`
}

type descriptor struct {
	Model    string           `json:"model"`
	Format   string           `json:"format"`
	Revision uint32           `json:"revision"`
	Parts    []descriptorPart `json:"parts"`
}

type descriptors struct {
	Artifacts   map[string]*artifact `json:"artifacts"`
	Descriptors []*descriptor        `json:"descriptors"`
}

func parseDescriptors(data []byte) (*descriptors, error) {
	var d descriptors
	if err := json.Unmarshal(data, &d); err != nil {
		return nil, err
	}
	for _, desc := range d.Descriptors {
		for _, p := range desc.Parts {
			if d.Artifacts[p.Artifact] == nil {
				return nil, fmt.Errorf("the descriptor of %s names no artifact %s", desc.Model, p.Artifact)
			}
		}
	}
	return &d, nil
}

func (d *descriptors) find(model, format string) *descriptor {
	for _, desc := range d.Descriptors {
		if desc.Model == model && desc.Format == format {
			return desc
		}
	}
	return nil
}

// roleArtifact is an artifact of a descriptor with its role.
type roleArtifact struct {
	role     string
	artifact *artifact
}

func (d *descriptors) parts(desc *descriptor) []roleArtifact {
	out := make([]roleArtifact, len(desc.Parts))
	for i, p := range desc.Parts {
		out[i] = roleArtifact{p.Role, d.Artifacts[p.Artifact]}
	}
	return out
}

// checkStorePath checks a plain path relative to an artifact: parts joined by
// /, no empty part, no . or .., no \, :, or control character.
func checkStorePath(p string) error {
	plain := p != ""
	for _, part := range strings.Split(p, "/") {
		if part == "" || part == "." || part == ".." {
			plain = false
		}
	}
	for _, c := range p {
		if c == '\\' || c == ':' || unicode.IsControl(c) {
			plain = false
		}
	}
	if !plain {
		return fmt.Errorf("the path %q is not a plain relative path in / form", p)
	}
	return nil
}

// manifest is models/<model id>/<format>.json.
type manifest struct {
	Revision uint32         `json:"revision"`
	Parts    []manifestPart `json:"parts"`
}

type manifestPart struct {
	Role   string `json:"role"`
	Digest string `json:"digest"`
}

// verifyError is the reason incomplete_download: the fetched files are not
// the files of the descriptor.
type verifyError struct{ message string }

func (e *verifyError) Error() string { return e.message }

// lease is a loaded model: the folder of each role and the shared use locks.
type lease struct {
	folders map[string]string
	locks   []*os.File
}

func (l *lease) folder(role string) (string, bool) {
	f, ok := l.folders[role]
	return f, ok
}

func (l *lease) release() {
	for _, f := range l.locks {
		f.Close()
	}
	l.locks = nil
}

// modelStore is the store at one folder.
type modelStore struct {
	root string
	hook string
}

func openModelStore(root string) (*modelStore, error) {
	s := &modelStore{root: root, hook: os.Getenv(hookEnv)}
	for _, folder := range []string{"artifacts", "models", "locks", "staging", "trash"} {
		p := filepath.Join(root, folder)
		if err := os.MkdirAll(p, 0o755); err != nil {
			return nil, fmt.Errorf("cannot create %s: %w", p, err)
		}
	}
	return s, nil
}

// step is a step of the rules. With the test hook it writes at-<step> in the
// hook folder, then waits while hold-<step> is there.
func (s *modelStore) step(name string) {
	if s.hook == "" {
		return
	}
	_ = os.WriteFile(filepath.Join(s.hook, "at-"+name), nil, 0o644)
	hold := filepath.Join(s.hook, "hold-"+name)
	for {
		if _, err := os.Stat(hold); err != nil {
			return
		}
		time.Sleep(5 * time.Millisecond)
	}
}

func (s *modelStore) artifactDir(digest string) string {
	return filepath.Join(s.root, "artifacts", digest)
}

func (s *modelStore) lockPath(name string) string { return filepath.Join(s.root, "locks", name) }

func openLock(p string) (*os.File, error) {
	f, err := os.OpenFile(p, os.O_RDWR|os.O_CREATE, 0o644)
	if err != nil {
		return nil, fmt.Errorf("cannot open the lock %s: %w", p, err)
	}
	return f, nil
}

func (s *modelStore) lock(name string, exclusive bool) (*os.File, error) {
	p := s.lockPath(name)
	f, err := openLock(p)
	if err != nil {
		return nil, err
	}
	if exclusive {
		err = lockExclusive(f)
	} else {
		err = lockShared(f)
	}
	if err != nil {
		f.Close()
		return nil, fmt.Errorf("cannot lock %s: %w", p, err)
	}
	return f, nil
}

func plainStoreName(name string) bool {
	return name != "" && !strings.HasPrefix(name, ".") && !strings.ContainsAny(name, `/\:`)
}

func (s *modelStore) manifestPath(model, format string) (string, error) {
	for _, name := range []string{model, format} {
		if !plainStoreName(name) {
			return "", fmt.Errorf("%q is not a plain name", name)
		}
	}
	return filepath.Join(s.root, "models", model, format+".json"), nil
}

// readManifest reads a manifest. Hold meta. A file that does not parse is no manifest.
func (s *modelStore) readManifest(model, format string) (*manifest, error) {
	p, err := s.manifestPath(model, format)
	if err != nil {
		return nil, err
	}
	data, err := os.ReadFile(p)
	if errors.Is(err, fs.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("cannot read %s: %w", p, err)
	}
	var m manifest
	if json.Unmarshal(data, &m) != nil {
		return nil, nil
	}
	return &m, nil
}

// complete: the revision is enough and each artifact exists. Hold meta.
func (s *modelStore) complete(m *manifest, desc *descriptor) bool {
	if m == nil || m.Revision < desc.Revision {
		return false
	}
	for _, p := range m.Parts {
		if !isDir(s.artifactDir(p.Digest)) {
			return false
		}
	}
	return true
}

// onDisk is true when the manifest has the revision of the descriptor or a
// higher one, and each artifact exists.
func (s *modelStore) onDisk(desc *descriptor) (bool, error) {
	meta, err := s.lock("meta", false)
	if err != nil {
		return false, err
	}
	defer meta.Close()
	m, err := s.readManifest(desc.Model, desc.Format)
	if err != nil {
		return false, err
	}
	return s.complete(m, desc), nil
}

// onDiskNow is onDisk without a wait: known is false when another engine has meta now.
func (s *modelStore) onDiskNow(desc *descriptor) (on, known bool, err error) {
	p := s.lockPath("meta")
	meta, err := openLock(p)
	if err != nil {
		return false, false, err
	}
	defer meta.Close()
	if err := tryLockShared(meta); errors.Is(err, errWouldBlock) {
		return false, false, nil
	} else if err != nil {
		return false, false, fmt.Errorf("cannot lock %s: %w", p, err)
	}
	m, err := s.readManifest(desc.Model, desc.Format)
	if err != nil {
		return false, false, err
	}
	return s.complete(m, desc), true, nil
}

// load is "Load", steps 1 to 3: the folders of the model, locked for use. Nil
// when the model is not on disk. It never writes in the store but lock files.
func (s *modelStore) load(desc *descriptor) (*lease, error) {
	meta, err := s.lock("meta", false)
	if err != nil {
		return nil, err
	}
	defer meta.Close()
	m, err := s.readManifest(desc.Model, desc.Format)
	if err != nil || m == nil {
		return nil, err
	}
	s.step("load.manifest_read")
	if m.Revision < desc.Revision {
		return nil, nil
	}
	digests := map[string]bool{}
	for _, p := range m.Parts {
		digests[p.Digest] = true
	}
	sorted := make([]string, 0, len(digests))
	for d := range digests {
		sorted = append(sorted, d)
	}
	sort.Strings(sorted)
	l := &lease{folders: map[string]string{}}
	for _, d := range sorted {
		f, err := s.lock(d+".use", false)
		if err != nil {
			l.release()
			return nil, err
		}
		l.locks = append(l.locks, f)
	}
	s.step("load.use_locked")
	if !s.complete(m, desc) {
		l.release()
		return nil, nil
	}
	for _, p := range m.Parts {
		l.folders[p.Role] = s.artifactDir(p.Digest)
	}
	meta.Close()
	s.step("load.ready")
	return l, nil
}

// fetchFunc gets an artifact, an empty work folder, and the path that must
// hold the files of the artifact when it returns.
type fetchFunc func(a *artifact, work, files string) error

// download is "Download". waiting runs when another engine downloads an
// artifact that this model needs.
func (s *modelStore) download(data *descriptors, desc *descriptor, fetch fetchFunc, waiting func()) error {
	var artifacts []*artifact
	seen := map[string]bool{}
	for _, p := range data.parts(desc) {
		if !seen[p.artifact.Digest] {
			seen[p.artifact.Digest] = true
			artifacts = append(artifacts, p.artifact)
		}
	}
	sort.Slice(artifacts, func(i, j int) bool { return artifacts[i].Digest < artifacts[j].Digest })
	for {
		// Step 1.
		meta, err := s.lock("meta", false)
		if err != nil {
			return err
		}
		m, err := s.readManifest(desc.Model, desc.Format)
		if err != nil {
			meta.Close()
			return err
		}
		if s.complete(m, desc) {
			meta.Close()
			return nil
		}
		var missing []*artifact
		for _, a := range artifacts {
			if !isDir(s.artifactDir(a.Digest)) {
				missing = append(missing, a)
			}
		}
		meta.Close()
		s.step("download.manifest_read")
		// Step 2. The get locks stay until the artifacts are published.
		var gets []*os.File
		closeGets := func() {
			for _, g := range gets {
				g.Close()
			}
		}
		type staged struct{ digest, stage string }
		var stages []staged
		for _, a := range missing {
			g, err := s.lockGet(a.Digest, waiting)
			if err != nil {
				closeGets()
				return err
			}
			gets = append(gets, g)
			s.step("download.get_locked")
			meta, err := s.lock("meta", false)
			if err != nil {
				closeGets()
				return err
			}
			exists := isDir(s.artifactDir(a.Digest))
			meta.Close()
			if exists {
				continue
			}
			stage, err := s.fetch(a, fetch)
			if err != nil {
				// A retry starts again, so a stage of this attempt is only in the way.
				for _, st := range stages {
					_ = os.RemoveAll(st.stage)
				}
				closeGets()
				return err
			}
			stages = append(stages, staged{a.Digest, stage})
		}
		// Step 3.
		metaEx, err := s.lock("meta", true)
		if err != nil {
			closeGets()
			return err
		}
		s.step("download.meta_locked")
		for _, st := range stages {
			dest := s.artifactDir(st.digest)
			if !isDir(dest) {
				if err := storeRename(filepath.Join(st.stage, "files"), dest); err != nil {
					metaEx.Close()
					closeGets()
					return err
				}
			}
			_ = os.RemoveAll(st.stage)
			s.step("download.artifact_published")
		}
		syncFolder(filepath.Join(s.root, "artifacts"))
		// A cleanup can take an artifact that no manifest named after step 1.
		// Then this goes back to step 1, with what it has published.
		all := true
		for _, a := range artifacts {
			if !isDir(s.artifactDir(a.Digest)) {
				all = false
			}
		}
		if !all {
			metaEx.Close()
			closeGets()
			continue
		}
		if err := s.writeManifest(desc, data); err != nil {
			metaEx.Close()
			closeGets()
			return err
		}
		s.step("download.manifest_written")
		// A manifest with a higher revision stays, and it can name an artifact
		// that a power cut or a person took away.
		m, err = s.readManifest(desc.Model, desc.Format)
		if err == nil && !s.complete(m, desc) {
			err = &verifyError{fmt.Sprintf("the manifest of %s has a higher revision and names a missing artifact; delete the model", desc.Model)}
		}
		metaEx.Close()
		closeGets()
		if err != nil {
			return err
		}
		s.step("download.done")
		return nil
	}
}

func (s *modelStore) lockGet(digest string, waiting func()) (*os.File, error) {
	p := s.lockPath(digest + ".get")
	f, err := openLock(p)
	if err != nil {
		return nil, err
	}
	err = tryLockExclusive(f)
	if errors.Is(err, errWouldBlock) {
		waiting()
		err = lockExclusive(f)
	}
	if err != nil {
		f.Close()
		return nil, fmt.Errorf("cannot lock %s: %w", p, err)
	}
	return f, nil
}

func randomHex() string {
	var b [16]byte
	_, _ = rand.Read(b[:])
	return hex.EncodeToString(b[:])
}

// fetch is step 2 for one artifact: a staging folder, the owner file, the
// files, and the check against the descriptor. The caller holds the get lock.
func (s *modelStore) fetch(a *artifact, fetch fetchFunc) (string, error) {
	pid := strconv.Itoa(os.Getpid())
	stage := filepath.Join(s.root, "staging", a.Digest+"."+pid+"."+randomHex())
	work := filepath.Join(stage, "work")
	if err := os.MkdirAll(work, 0o755); err != nil {
		return "", fmt.Errorf("cannot create %s: %w", work, err)
	}
	owner := filepath.Join(stage, "owner")
	if err := os.WriteFile(owner, []byte(pid+"\n"), 0o644); err != nil {
		return "", fmt.Errorf("cannot write %s: %w", owner, err)
	}
	s.step("download.staged")
	files := filepath.Join(stage, "files")
	err := fetch(a, work, files)
	if err == nil {
		err = verifyArtifact(files, a)
	}
	if err != nil {
		_ = os.RemoveAll(stage)
		return "", err
	}
	s.step("download.fetched")
	return stage, nil
}

// writeManifest writes the manifest of desc. Hold meta exclusive. A manifest
// never goes back to a lower revision.
func (s *modelStore) writeManifest(desc *descriptor, data *descriptors) error {
	existing, err := s.readManifest(desc.Model, desc.Format)
	if err != nil {
		return err
	}
	if existing != nil && existing.Revision > desc.Revision {
		return nil
	}
	m := manifest{Revision: desc.Revision}
	for _, p := range data.parts(desc) {
		m.Parts = append(m.Parts, manifestPart{Role: p.role, Digest: p.artifact.Digest})
	}
	p, err := s.manifestPath(desc.Model, desc.Format)
	if err != nil {
		return err
	}
	folder := filepath.Dir(p)
	if err := os.MkdirAll(folder, 0o755); err != nil {
		return fmt.Errorf("cannot create %s: %w", folder, err)
	}
	temp := filepath.Join(folder, "."+strconv.Itoa(os.Getpid())+"."+randomHex()+".tmp")
	bytes, _ := json.MarshalIndent(m, "", "  ")
	f, err := os.Create(temp)
	if err == nil {
		_, err = f.Write(bytes)
		if err == nil {
			err = f.Sync()
		}
		if cerr := f.Close(); err == nil {
			err = cerr
		}
	}
	if err != nil {
		return fmt.Errorf("cannot write %s: %w", temp, err)
	}
	if err := storeRename(temp, p); err != nil {
		return err
	}
	syncFolder(folder)
	return nil
}

// delete is "Delete a model", then "Clean up". It never fails because of
// another engine: one that has the model loaded keeps it until it unloads.
func (s *modelStore) delete(model, format string) error {
	p, err := s.manifestPath(model, format)
	if err != nil {
		return err
	}
	meta, err := s.lock("meta", true)
	if err != nil {
		return err
	}
	s.step("delete.meta_locked")
	if err := os.Remove(p); err != nil && !errors.Is(err, fs.ErrNotExist) {
		meta.Close()
		return fmt.Errorf("cannot delete %s: %w", p, err)
	}
	s.step("delete.manifest_removed")
	meta.Close()
	// The model is gone now. What a cleanup cannot do stays for the next one.
	_ = s.cleanup()
	return nil
}

// cleanup is "Clean up". Run it at the start of an engine and after each
// delete. What it cannot remove now stays for the next cleanup.
func (s *modelStore) cleanup() error {
	meta, err := s.lock("meta", true)
	if err != nil {
		return err
	}
	s.step("cleanup.meta_locked")
	named, known, err := s.namedDigests()
	if err != nil {
		meta.Close()
		return err
	}
	if known {
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
			use, err := openLock(s.lockPath(digest + ".use"))
			if err != nil {
				meta.Close()
				return err
			}
			if tryLockExclusive(use) != nil {
				use.Close()
				continue
			}
			s.step("cleanup.use_locked")
			trash := filepath.Join(s.root, "trash", digest+"."+randomHex())
			_ = storeRename(filepath.Join(s.root, "artifacts", digest), trash)
			s.step("cleanup.trashed")
			use.Close()
		}
	}
	meta.Close()
	s.step("cleanup.meta_released")
	trash, err := os.ReadDir(filepath.Join(s.root, "trash"))
	if err != nil {
		return fmt.Errorf("cannot read %s: %w", filepath.Join(s.root, "trash"), err)
	}
	for _, e := range trash {
		_ = os.RemoveAll(filepath.Join(s.root, "trash", e.Name()))
	}
	s.step("cleanup.trash_emptied")
	stages, err := folders(filepath.Join(s.root, "staging"))
	if err != nil {
		return err
	}
	for _, e := range stages {
		digest, _, _ := strings.Cut(e.Name(), ".")
		get, err := openLock(s.lockPath(digest + ".get"))
		if err != nil {
			return err
		}
		if tryLockExclusive(get) == nil {
			s.step("cleanup.staging_locked")
			_ = os.RemoveAll(filepath.Join(s.root, "staging", e.Name()))
		}
		get.Close()
	}
	s.step("cleanup.done")
	return nil
}

// namedDigests are the digests that the manifests name. Hold meta exclusive.
// It removes manifest files that a crash left half written, and the folder of
// a model without manifests. known is false when a manifest does not parse.
func (s *modelStore) namedDigests() (named map[string]bool, known bool, err error) {
	named = map[string]bool{}
	models, err := folders(filepath.Join(s.root, "models"))
	if err != nil {
		return nil, false, err
	}
	for _, model := range models {
		dir := filepath.Join(s.root, "models", model.Name())
		files, err := os.ReadDir(dir)
		if err != nil {
			return nil, false, fmt.Errorf("cannot read %s: %w", dir, err)
		}
		for _, f := range files {
			p := filepath.Join(dir, f.Name())
			if strings.HasPrefix(f.Name(), ".") {
				_ = os.Remove(p)
				continue
			}
			data, err := os.ReadFile(p)
			if err != nil {
				return nil, false, fmt.Errorf("cannot read %s: %w", p, err)
			}
			var m manifest
			if json.Unmarshal(data, &m) != nil {
				return nil, false, nil
			}
			for _, part := range m.Parts {
				named[part.Digest] = true
			}
		}
		_ = os.Remove(dir)
	}
	return named, true, nil
}

// folders are the folders in p. The store ignores other files there.
func folders(p string) ([]fs.DirEntry, error) {
	entries, err := os.ReadDir(p)
	if err != nil {
		return nil, fmt.Errorf("cannot read %s: %w", p, err)
	}
	out := entries[:0]
	for _, e := range entries {
		if e.IsDir() {
			out = append(out, e)
		}
	}
	return out, nil
}

// storeRename is a rename that Windows retries for about two seconds.
func storeRename(from, to string) error {
	return retryOnWindows(func() error { return os.Rename(from, to) }, func(err error) error {
		return fmt.Errorf("cannot move %s: %w", from, err)
	})
}

// verifyArtifact checks that folder holds the files of a and nothing else.
func verifyArtifact(folder string, a *artifact) error {
	var found []string
	if err := walkFiles(folder, "", &found); err != nil {
		return err
	}
	sort.Strings(found)
	wanted := make([]string, len(a.Files))
	for i, f := range a.Files {
		wanted[i] = f.Path
	}
	sort.Strings(wanted)
	if strings.Join(found, "\x00") != strings.Join(wanted, "\x00") {
		extra, missing := fileSetDiff(found, wanted)
		return &verifyError{fmt.Sprintf("the files of %s are not the files of its descriptor: not in the descriptor %q, missing %q", a.Digest, extra, missing)}
	}
	for _, f := range a.Files {
		p := filepath.Join(folder, filepath.FromSlash(f.Path))
		info, err := os.Stat(p)
		if err != nil {
			return fmt.Errorf("cannot read %s: %w", p, err)
		}
		if uint64(info.Size()) != f.Size {
			return &verifyError{fmt.Sprintf("%s has %d bytes, not %d", f.Path, info.Size(), f.Size)}
		}
		sum, err := hashFile(p)
		if err != nil {
			return err
		}
		if sum != f.SHA256 {
			return &verifyError{fmt.Sprintf("%s does not have the hash of its descriptor", f.Path)}
		}
		// After a power cut, a published artifact must not have short files.
		// Windows can only flush a handle that can write.
		h, err := os.OpenFile(p, os.O_RDWR, 0)
		if err != nil {
			h, err = os.Open(p)
		}
		if err == nil {
			err = h.Sync()
			h.Close()
		}
		if err != nil {
			return fmt.Errorf("cannot write %s: %w", p, err)
		}
	}
	return nil
}

// fileSetDiff is the paths of found that are not in wanted, and the reverse.
func fileSetDiff(found, wanted []string) (extra, missing []string) {
	extra, missing = []string{}, []string{}
	for _, p := range found {
		if !slices.Contains(wanted, p) {
			extra = append(extra, p)
		}
	}
	for _, p := range wanted {
		if !slices.Contains(found, p) {
			missing = append(missing, p)
		}
	}
	return extra, missing
}

// walkFiles lists the files under folder in / form. A link is an error.
func walkFiles(folder, prefix string, found *[]string) error {
	entries, err := os.ReadDir(folder)
	if err != nil {
		return fmt.Errorf("cannot read %s: %w", folder, err)
	}
	for _, e := range entries {
		name := prefix + e.Name()
		switch {
		case e.Type().IsDir():
			if err := walkFiles(filepath.Join(folder, e.Name()), name+"/", found); err != nil {
				return err
			}
		case e.Type().IsRegular():
			*found = append(*found, name)
		default:
			return &verifyError{name + " is not a file or a folder"}
		}
	}
	return nil
}

// hashFile is the SHA-256 of a file in lowercase hex.
func hashFile(p string) (string, error) {
	f, err := os.Open(p)
	if err != nil {
		return "", fmt.Errorf("cannot read %s: %w", p, err)
	}
	defer f.Close()
	h := sha256.New()
	if _, err := io.Copy(h, f); err != nil {
		return "", fmt.Errorf("cannot read %s: %w", p, err)
	}
	return hex.EncodeToString(h.Sum(nil)), nil
}
