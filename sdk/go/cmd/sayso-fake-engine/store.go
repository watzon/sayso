package main

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
)

// The files of a model. With SAYSO_MODEL_STORE, the layout of model store
// v1 (sdk/protocol/paths.md): an artifact with one file, model.bin, under
// its digest, and the manifest models/<id>/fake-1.json. Without it, the
// folder <models>/<id> with model.bin.

const (
	storeFormat = "fake-1"
	modelFile   = "model.bin"
)

type manifest struct {
	Format    string   `json:"format"`
	Model     string   `json:"model"`
	Revision  int      `json:"revision"`
	Artifacts []string `json:"artifacts"`
}

func (e *engine) storeDirs() error {
	for _, d := range []string{"artifacts", "models", "locks", "staging", "trash"} {
		if err := os.MkdirAll(filepath.Join(e.store, d), 0o755); err != nil {
			return err
		}
	}
	return touch(filepath.Join(e.store, "locks", "meta"))
}

func touch(path string) error {
	f, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		return err
	}
	return f.Close()
}

func contentOf(m *model) []byte { return []byte("sayso-fake-engine model " + m.id + "\n") }

// digest is the store digest of an artifact with the one file model.bin.
func digest(content []byte) string {
	file := sha256.Sum256(content)
	list := fmt.Sprintf("%s\x00%d\x00%s\n", modelFile, len(content), hex.EncodeToString(file[:]))
	sum := sha256.Sum256([]byte(list))
	return hex.EncodeToString(sum[:])
}

func (e *engine) manifestPath(m *model) string {
	return filepath.Join(e.store, "models", m.id, storeFormat+".json")
}

func (e *engine) onDisk(m *model) bool {
	if e.store == "" {
		_, err := os.Stat(filepath.Join(e.modelsDir, m.id, modelFile))
		return err == nil
	}
	b, err := os.ReadFile(e.manifestPath(m))
	if err != nil {
		return false
	}
	var man manifest
	if json.Unmarshal(b, &man) != nil || len(man.Artifacts) == 0 {
		return false
	}
	for _, a := range man.Artifacts {
		if _, err := os.Stat(filepath.Join(e.store, "artifacts", a, modelFile)); err != nil {
			return false
		}
	}
	return true
}

func (e *engine) writeFiles(m *model, _ uint64) error {
	content := contentOf(m)
	if e.store == "" {
		dir := filepath.Join(e.modelsDir, m.id)
		if err := os.MkdirAll(dir, 0o755); err != nil {
			return err
		}
		return os.WriteFile(filepath.Join(dir, modelFile), content, 0o644)
	}
	d := digest(content)
	if err := touch(filepath.Join(e.store, "locks", d+".get")); err != nil {
		return err
	}
	if err := touch(filepath.Join(e.store, "locks", d+".use")); err != nil {
		return err
	}
	artifact := filepath.Join(e.store, "artifacts", d)
	if _, err := os.Stat(artifact); errors.Is(err, os.ErrNotExist) {
		var r [4]byte
		rand.Read(r[:])
		stage := filepath.Join(e.store, "staging", fmt.Sprintf("%s.%d.%s", d, os.Getpid(), hex.EncodeToString(r[:])))
		files := filepath.Join(stage, "files")
		if err := os.MkdirAll(files, 0o755); err != nil {
			return err
		}
		if err := os.WriteFile(filepath.Join(stage, "owner"), []byte(fmt.Sprint(os.Getpid())), 0o644); err != nil {
			return err
		}
		if err := os.WriteFile(filepath.Join(files, modelFile), content, 0o644); err != nil {
			return err
		}
		if err := os.Rename(files, artifact); err != nil {
			return err
		}
		os.RemoveAll(stage)
	}
	b, _ := json.Marshal(manifest{Format: storeFormat, Model: m.id, Revision: 1, Artifacts: []string{d}})
	path := e.manifestPath(m)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	tmp := path + ".tmp"
	if err := os.WriteFile(tmp, b, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

func (e *engine) deleteFiles(m *model) {
	if e.store == "" {
		os.RemoveAll(filepath.Join(e.modelsDir, m.id))
		return
	}
	os.Remove(e.manifestPath(m))
	os.Remove(filepath.Dir(e.manifestPath(m)))
}
