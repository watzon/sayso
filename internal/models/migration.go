package models

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"log/slog"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

// The move of the models from the old folders into the store (docs/sdk.md,
// "Migration and downgrade"). It runs before Sayso starts its engine and
// holds locks/meta for the whole run.
// migration.json is the journal; each step can run twice with the same result.

// Journal is the name of the journal in the store.
const Journal = "migration.json"

// stagesFolder is not staging/, because another program's cleanup deletes a
// stage whose get lock nobody holds.
const stagesFolder = "migrating"

const whisperCppFolder = "main"

// OldPart is the old place of one artifact: folders relative to the models
// folder in / form; the first that has a file gives it.
type OldPart struct {
	Role    string   `json:"role"`
	Folders []string `json:"folders"`
}

// OldModel is the old files of one model for one format.
type OldModel struct {
	Model  string    `json:"model"`
	Format string    `json:"format"`
	Parts  []OldPart `json:"parts"`
	Remove []string  `json:"remove"`
}

// MigrationReport is what a run did.
type MigrationReport struct {
	Moved []string
	Kept  [][2]string // model, reason
}

type journalEntry struct {
	OldModel
	State string `json:"state"` // pending, done, kept
	Why   string `json:"why,omitempty"`
}

type journal struct {
	Finished bool           `json:"finished"`
	Removing bool           `json:"removing"`
	Models   []journalEntry `json:"models"`
}

// Links says how a file gets into a staging folder.
type Links int

const (
	// LinksHard makes a hard link, and a copy where the volume has none.
	LinksHard Links = iota
	// LinksCopyOnly copies, for tests.
	LinksCopyOnly
)

// RunMigration moves the models of this build from the old folders in
// modelsDir into the store in modelsDir/v1.
func RunMigration(modelsDir string) (MigrationReport, error) {
	store, err := OpenStore(filepath.Join(modelsDir, StoreFolder))
	if err != nil {
		return MigrationReport{}, err
	}
	return RunMigrationWith(store, modelsDir, AllDescriptors(), OldModels(CurrentSystem()), LinksHard)
}

// StoreInUse, after an error of RunMigration: true when the engine must use
// the store anyway, because old folders of moved models can be gone.
func StoreInUse(modelsDir string) bool {
	b, err := os.ReadFile(filepath.Join(modelsDir, StoreFolder, Journal))
	if err != nil {
		return false
	}
	var j journal
	if json.Unmarshal(b, &j) != nil {
		return false
	}
	return j.Finished || j.Removing
}

// ModelStoreEnv runs the migration and gives the environment that tells the
// engine to use the store.
// After an error the engine runs on the old folders when they are all there.
func ModelStoreEnv(modelsDir string) []string {
	storeEnv := []string{StoreEnv + "=" + filepath.Join(modelsDir, StoreFolder)}
	report, err := RunMigration(modelsDir)
	if err != nil {
		slog.Error("the migration of the models failed", "err", err)
		if StoreInUse(modelsDir) {
			return storeEnv
		}
		return nil
	}
	if len(report.Moved) > 0 {
		slog.Info("moved to the model store", "models", strings.Join(report.Moved, ", "))
	}
	for _, k := range report.Kept {
		slog.Info("a model is not in the model store, so its old files stay", "model", k[0], "reason", k[1])
	}
	return storeEnv
}

func readJournal(path string) *journal {
	b, err := os.ReadFile(path)
	if err != nil {
		return nil
	}
	var raw map[string]json.RawMessage
	if json.Unmarshal(b, &raw) != nil {
		return nil
	}
	if _, ok := raw["finished"]; !ok {
		return nil
	}
	if _, ok := raw["models"]; !ok {
		return nil
	}
	var j journal
	if json.Unmarshal(b, &j) != nil {
		return nil
	}
	return &j
}

// RunMigrationWith runs with other descriptors and another list of old models.
func RunMigrationWith(store *Store, modelsDir string, data *Descriptors, old []OldModel, links Links) (MigrationReport, error) {
	var report MigrationReport
	meta, err := store.metaExclusive()
	if err != nil {
		return report, err
	}
	metaHeld := true
	defer func() {
		if metaHeld {
			meta.Close()
		}
	}()
	store.Step("migration.meta_locked")
	stages := filepath.Join(store.root, stagesFolder)
	os.RemoveAll(stages)
	path := filepath.Join(store.root, Journal)
	stored := readJournal(path)
	first := stored == nil
	resume := stored != nil && !stored.Finished
	var j journal
	if stored != nil && !stored.Finished {
		// An engine can have run on the old folders since then.
		for i := range stored.Models {
			if stored.Models[i].State == "kept" {
				stored.Models[i].State = "pending"
				stored.Models[i].Why = ""
			}
		}
		j = plan(modelsDir, old, stored.Models)
		j.Removing = stored.Removing
	} else {
		var prev []journalEntry
		if stored != nil {
			prev = stored.Models
		}
		j = plan(modelsDir, old, prev)
	}
	work := first || resume
	for _, e := range j.Models {
		if e.State == "pending" {
			work = true
		}
	}
	if work {
		j.Finished = false
		if err := writeJournal(path, &j); err != nil {
			return report, err
		}
	}
	store.Step("migration.planned")

	for i := range j.Models {
		e := &j.Models[i]
		if e.State != "pending" {
			continue
		}
		name := e.Model
		why := "no descriptor"
		if d, ok := data.Find(name, e.Format); ok {
			why, err = migrateModel(store, modelsDir, data, d, e.OldModel, links)
			if err != nil {
				return report, err
			}
		}
		if why == "" {
			e.State = "done"
			report.Moved = append(report.Moved, name)
		} else {
			slog.Warn("the model is not moved into the store", "model", name, "why", why)
			e.State, e.Why = "kept", why
		}
		if err := writeJournal(path, &j); err != nil {
			return report, err
		}
		store.Step("migration.marked")
	}

	// A kept model that is in the store now gives its folders up.
	for i := range j.Models {
		e := &j.Models[i]
		if e.State != "kept" {
			continue
		}
		if d, ok := data.Find(e.Model, e.Format); ok {
			if m, err := store.readManifest(d.Model, d.Format); err == nil && store.complete(m, d) {
				e.State = "done"
			}
		}
	}
	var kept []journalEntry
	keep := map[string]bool{}
	remove := map[string]bool{}
	removed := false
	for _, e := range j.Models {
		for _, r := range e.Remove {
			remove[r] = true
		}
		if e.State == "done" {
			removed = true
		}
		if e.State == "kept" {
			kept = append(kept, e)
			for _, r := range e.Remove {
				keep[r] = true
			}
			for _, p := range e.Parts {
				for _, f := range p.Folders {
					keep[f] = true
				}
			}
		}
	}
	if work {
		for _, m := range old {
			for _, r := range m.Remove {
				remove[r] = true
			}
		}
		j.Removing = true
		if err := writeJournal(path, &j); err != nil {
			return report, err
		}
	}
	var toRemove []string
	for r := range remove {
		if !keep[r] {
			toRemove = append(toRemove, r)
		}
	}
	sort.Strings(toRemove)
	for _, folder := range toRemove {
		removeOldFolder(modelsDir, folder)
		store.Step("migration.folder_removed")
	}
	for _, e := range kept {
		report.Kept = append(report.Kept, [2]string{e.Model, e.Why})
	}
	if work || removed {
		if kept == nil {
			kept = []journalEntry{}
		}
		if err := writeJournal(path, &journal{Finished: true, Models: kept}); err != nil {
			return report, err
		}
	}
	os.RemoveAll(stages)
	store.Step("migration.finished")
	meta.Close()
	metaHeld = false
	if work {
		if err := store.Cleanup(); err != nil {
			slog.Warn("the cleanup of the model store failed", "err", err)
		}
	}
	return report, nil
}

func plan(modelsDir string, old []OldModel, previous []journalEntry) journal {
	models := previous
	for _, m := range old {
		found := false
		for _, e := range models {
			if e.Model == m.Model && e.Format == m.Format {
				found = true
				break
			}
		}
		if found || len(m.Parts) == 0 {
			continue
		}
		there := false
		for _, f := range m.Parts[0].Folders {
			if isDir(filepath.Join(modelsDir, filepath.FromSlash(f))) {
				there = true
			}
		}
		if there {
			models = append(models, journalEntry{OldModel: m, State: "pending"})
		}
	}
	if models == nil {
		models = []journalEntry{}
	}
	return journal{Finished: true, Models: models}
}

// migrateModel: hold meta exclusive. A non-empty string says why the model
// does not move; an error stops the run.
func migrateModel(store *Store, modelsDir string, data *Descriptors, d Descriptor, old OldModel, links Links) (string, error) {
	if m, err := store.readManifest(d.Model, d.Format); err != nil {
		return "", err
	} else if store.complete(m, d) {
		return "", nil
	}
	parts, err := data.Parts(d)
	if err != nil {
		return "", err
	}
	type st struct{ digest, stage string }
	var stagedList []st
	why := ""
	for _, ra := range parts {
		if isDir(store.ArtifactPath(ra.Artifact.Digest)) {
			continue
		}
		dup := false
		for _, s := range stagedList {
			if s.digest == ra.Artifact.Digest {
				dup = true
			}
		}
		if dup {
			continue
		}
		var part *OldPart
		for i := range old.Parts {
			if old.Parts[i].Role == ra.Role {
				part = &old.Parts[i]
				break
			}
		}
		if part == nil {
			why = "no old folder for the role " + ra.Role
			break
		}
		stage, reason, err := stageArtifact(store, modelsDir, ra.Artifact, *part, links)
		if err != nil {
			return "", err
		}
		if reason != "" {
			why = reason
			break
		}
		stagedList = append(stagedList, st{ra.Artifact.Digest, stage})
	}
	if why != "" {
		for _, s := range stagedList {
			os.RemoveAll(s.stage)
		}
		return why, nil
	}
	for _, s := range stagedList {
		dest := store.ArtifactPath(s.digest)
		if !isDir(dest) {
			if err := renameRetry(filepath.Join(s.stage, "files"), dest); err != nil {
				return "", err
			}
		}
		os.RemoveAll(s.stage)
		store.Step("migration.published")
	}
	syncFolder(filepath.Join(store.root, "artifacts"))
	if err := store.writeManifest(d, data); err != nil {
		return "", err
	}
	store.Step("migration.manifest_written")
	m, err := store.readManifest(d.Model, d.Format)
	if err != nil {
		return "", err
	}
	if !store.complete(m, d) {
		return "a manifest with a higher revision names a missing artifact", nil
	}
	return "", nil
}

func stageArtifact(store *Store, modelsDir string, a Artifact, part OldPart, links Links) (string, string, error) {
	stage := filepath.Join(store.root, stagesFolder, a.Digest)
	files := filepath.Join(stage, "files")
	if err := os.MkdirAll(files, 0o755); err != nil {
		return "", "", fmt.Errorf("cannot create %s: %w", files, err)
	}
	refuse := func(why string) (string, string, error) {
		os.RemoveAll(stage)
		return "", why, nil
	}
	for _, f := range a.Files {
		from := ""
		for _, folder := range part.Folders {
			p := filepath.Join(modelsDir, filepath.FromSlash(folder), filepath.FromSlash(f.Path))
			if fi, err := os.Lstat(p); err == nil && fi.Mode().IsRegular() {
				from = p
				break
			}
		}
		if from == "" {
			return refuse("the file " + f.Path + " is missing")
		}
		to := filepath.Join(files, filepath.FromSlash(f.Path))
		if err := os.MkdirAll(filepath.Dir(to), 0o755); err != nil {
			return "", "", fmt.Errorf("cannot create %s: %w", filepath.Dir(to), err)
		}
		if err := linkOrCopy(from, to, links); err != nil {
			// A file this user cannot read keeps its model out; another error stops the run.
			if errors.Is(err, fs.ErrPermission) {
				return refuse(fmt.Sprintf("cannot copy %s: %v", from, err))
			}
			return "", "", fmt.Errorf("cannot copy %s: %w", from, err)
		}
	}
	store.Step("migration.linked")
	if err := Verify(files, a); err != nil {
		if errors.Is(err, ErrVerify) || errors.Is(err, fs.ErrPermission) {
			return refuse(err.Error())
		}
		return "", "", err
	}
	store.Step("migration.verified")
	return stage, "", nil
}

func linkOrCopy(from, to string, links Links) error {
	if links == LinksHard && os.Link(from, to) == nil {
		return nil
	}
	src, err := os.Open(from)
	if err != nil {
		return err
	}
	defer src.Close()
	dst, err := os.Create(to)
	if err != nil {
		return err
	}
	if _, err := io.Copy(dst, src); err != nil {
		dst.Close()
		return err
	}
	return dst.Close()
}

// removeOldFolder deletes an old folder and the parents that are empty then.
func removeOldFolder(modelsDir, folder string) {
	p := filepath.Join(modelsDir, filepath.FromSlash(folder))
	// A link to a volume that is not there at this start keeps its place.
	if fi, err := os.Lstat(p); err == nil && fi.Mode()&os.ModeSymlink != 0 {
		if _, err := os.Stat(p); err != nil {
			return
		}
	}
	if err := os.RemoveAll(p); err != nil && !errors.Is(err, fs.ErrNotExist) {
		slog.Warn("cannot delete an old model folder", "path", p, "err", err)
		return
	}
	root := filepath.Clean(modelsDir)
	for parent := filepath.Dir(p); parent != root && strings.HasPrefix(parent, root+string(filepath.Separator)); parent = filepath.Dir(parent) {
		if os.Remove(parent) != nil {
			break
		}
	}
}

func writeJournal(path string, j *journal) error {
	folder := filepath.Dir(path)
	tmp := filepath.Join(folder, fmt.Sprintf(".%s.%s.tmp", Journal, randomHex()))
	b, _ := json.MarshalIndent(j, "", "  ")
	if err := writeSynced(tmp, b); err != nil {
		return fmt.Errorf("cannot write %s: %w", tmp, err)
	}
	if err := renameRetry(tmp, path); err != nil {
		return err
	}
	syncFolder(folder)
	return nil
}

// OldModels lists the old files of the models of a system's catalog. The
// folder names of the Swift engine come from FluidAudio and WhisperKit.
func OldModels(s System) []OldModel {
	var out []OldModel
	for _, m := range LocalModelsFor(s) {
		format := FormatOf(m.Engine)
		if format == "" {
			continue
		}
		var parts []OldPart
		var remove []string
		var ok bool
		if s == MacOS {
			parts, remove, ok = swiftFolders(m.Engine)
		} else {
			parts, remove, ok = rustFolders(string(m.ID), m.Engine)
		}
		if ok {
			out = append(out, OldModel{Model: string(m.ID), Format: format, Parts: parts, Remove: remove})
		}
	}
	return out
}

func part(role string, folders ...string) OldPart { return OldPart{Role: role, Folders: folders} }

func swiftFolders(e EngineKind) ([]OldPart, []string, bool) {
	one := func(folder string) ([]OldPart, []string, bool) {
		return []OldPart{part("model", folder)}, []string{folder}, true
	}
	switch e.Kind {
	case KindParakeetUnified:
		model, ctc := "parakeet-unified-en-0.6b", "parakeet-ctc-110m-coreml"
		return []OldPart{part("model", model), part("ctc", ctc)}, []string{model, ctc}, true
	case KindParakeetEou:
		return one("parakeet-eou-streaming/160ms")
	case KindParakeetTdt:
		folder, ok := map[string]string{
			"v2": "parakeet-tdt-0.6b-v2", "v3": "parakeet-tdt-0.6b-v3", "ultra": "parakeet-ultra",
			"redux": "parakeet-redux", "phonon2": "phonon-2", "tdt_ctc_110m": "parakeet-tdt-ctc-110m",
			"ja": "parakeet-ja", "fluid_mini": "fluid-speech-mini-coreml",
		}[e.Version]
		if !ok {
			panic("no old folder for the Parakeet TDT version " + e.Version)
		}
		return one(folder)
	case KindNemotron:
		return one(fmt.Sprintf("nemotron-streaming/%dms", e.ChunkMs))
	case KindNemotronMultilingual:
		return one(fmt.Sprintf("nemotron-multilingual/multilingual/%dms", e.ChunkMs))
	case KindCohere:
		folder := "cohere-transcribe/q8"
		return []OldPart{part("model", folder), part("vocabulary", folder)}, []string{folder}, true
	case KindCanary:
		return one("canary-1b-v2")
	case KindSenseVoice:
		return one("sensevoice-small")
	case KindParaformer:
		return one("paraformer-large-zh")
	case KindFluidVad:
		return one("silero-vad")
	case KindFluidDiarization:
		return one("speaker-diarization")
	case KindWhisper:
		model := "models/argmaxinc/whisperkit-coreml/" + e.Variant
		tokenizer := "models/" + WhisperTokenizer(e.Variant)
		cache := "models/argmaxinc/whisperkit-coreml/.cache"
		return []OldPart{part("model", model), part("tokenizer", tokenizer)}, []string{model, tokenizer, cache}, true
	}
	return nil, nil, false
}

func rustFolders(id string, e EngineKind) ([]OldPart, []string, bool) {
	archive := func(name string) string { return id + "/" + name }
	stem := func(f string) string { return strings.TrimSuffix(f, ".onnx") }
	var parts []OldPart
	switch e.Kind {
	case KindSherpa:
		parts = []OldPart{part("main", archive(e.Archive))}
		if e.PreviewArchive != nil {
			parts = append(parts, part("preview", archive(*e.PreviewArchive)))
		}
	case KindSherpaVad:
		parts = []OldPart{part("main", archive(stem(e.File)))}
	case KindSherpaDiarization:
		parts = []OldPart{part("segmentation", archive(e.Segmentation)), part("embedding", archive(stem(e.Embedding)))}
	case KindWhisperCpp:
		parts = []OldPart{part("main", archive(whisperCppFolder), id)}
	case KindParakeetCpp:
		parts = []OldPart{part("main", archive(e.Revision))}
		if e.PreviewArchive != nil {
			parts = append(parts, part("preview", archive(*e.PreviewArchive)))
		}
	default:
		return nil, nil, false
	}
	return parts, []string{id}, true
}

// WhisperTokenizer is the tokenizer repository of a WhisperKit variant.
// Distil-Whisper large-v3 and turbo use the large-v3 tokenizer.
func WhisperTokenizer(variant string) string {
	name := strings.ToLower(variant)
	for _, size := range []string{"large-v3", "large-v2", "medium", "small", "base", "tiny"} {
		if strings.Contains(name, size) {
			english := ""
			if strings.Contains(name, size+".en") {
				english = ".en"
			}
			return "openai/whisper-" + size + english
		}
	}
	panic("the WhisperKit variant " + variant + " has no known tokenizer")
}
