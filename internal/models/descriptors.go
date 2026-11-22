package models

import (
	"crypto/sha256"
	_ "embed"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"sort"
	"strconv"
	"strings"
	"sync"
)

// Formats of the files of a model (engine_catalog/descriptors.rs).
const (
	FormatFluidAudio  = "fluidaudio-coreml-1"
	FormatWhisperKit  = "whisperkit-coreml-1"
	FormatSherpaOnnx  = "sherpa-onnx-1"
	FormatWhisperCpp  = "whisper-cpp-1"
	FormatParakeetCpp = "parakeet-cpp-1"
)

// FormatOf is the format of the files of a model of `engine`. "" for Apple
// Speech (macOS keeps its files) and cloud models.
func FormatOf(engine EngineKind) string {
	switch engine.Kind {
	case KindParakeetUnified, KindParakeetEou, KindParakeetTdt, KindNemotron, KindNemotronMultilingual,
		KindCohere, KindCanary, KindSenseVoice, KindParaformer, KindFluidVad, KindFluidDiarization:
		return FormatFluidAudio
	case KindWhisper:
		return FormatWhisperKit
	case KindSherpa, KindSherpaVad, KindSherpaDiarization:
		return FormatSherpaOnnx
	case KindWhisperCpp:
		return FormatWhisperCpp
	case KindParakeetCpp:
		return FormatParakeetCpp
	}
	return ""
}

// File is one file of an artifact.
type File struct {
	Path   string `json:"path"`
	Size   uint64 `json:"size"`
	SHA256 string `json:"sha256"`
}

// Source kinds.
const (
	SourceHuggingFace = "hugging_face"
	SourceArchive     = "archive"
	SourceFile        = "file"
)

// Source is where the files of an artifact come from, pinned to a revision.
type Source struct {
	Kind     string `json:"kind"`
	Repo     string `json:"repo,omitempty"`
	Revision string `json:"revision,omitempty"`
	Folder   string `json:"folder,omitempty"`
	URL      string `json:"url,omitempty"`
}

// FileURL is the URL of file `path` of the artifact, or "" for an archive,
// whose files come from one download.
func (s Source) FileURL(path string) string {
	switch s.Kind {
	case SourceHuggingFace:
		if s.Folder == "" {
			return fmt.Sprintf("https://huggingface.co/%s/resolve/%s/%s", s.Repo, s.Revision, path)
		}
		return fmt.Sprintf("https://huggingface.co/%s/resolve/%s/%s/%s", s.Repo, s.Revision, s.Folder, path)
	case SourceFile:
		return s.URL
	}
	return ""
}

// Artifact is a set of files that never change, stored once under artifacts/<digest>.
type Artifact struct {
	Source Source `json:"source"`
	Digest string `json:"digest"`
	Files  []File `json:"files"`
}

// Size is the size of all files.
func (a Artifact) Size() uint64 {
	var n uint64
	for _, f := range a.Files {
		n += f.Size
	}
	return n
}

// Part is an artifact of a descriptor and its role in the format.
type Part struct {
	Role     string `json:"role"`
	Artifact string `json:"artifact"`
}

// Descriptor is the artifacts of one model in one format.
type Descriptor struct {
	Model    string `json:"model"`
	Format   string `json:"format"`
	Revision uint32 `json:"revision"`
	Parts    []Part `json:"parts"`
}

// Descriptors is the data of descriptors.json.
type Descriptors struct {
	Artifacts   map[string]Artifact `json:"artifacts"`
	Descriptors []Descriptor        `json:"descriptors"`
}

// Find is the descriptor of `model` in `format`.
func (d *Descriptors) Find(model, format string) (Descriptor, bool) {
	for _, x := range d.Descriptors {
		if x.Model == model && x.Format == format {
			return x, true
		}
	}
	return Descriptor{}, false
}

// RoleArtifact is an artifact with its role.
type RoleArtifact struct {
	Role     string
	Artifact Artifact
}

// Parts lists the artifacts of a descriptor with their roles.
func (d *Descriptors) Parts(desc Descriptor) ([]RoleArtifact, error) {
	out := make([]RoleArtifact, 0, len(desc.Parts))
	for _, p := range desc.Parts {
		a, ok := d.Artifacts[p.Artifact]
		if !ok {
			return nil, fmt.Errorf("no artifact %s", p.Artifact)
		}
		out = append(out, RoleArtifact{p.Role, a})
	}
	return out, nil
}

// OfFormats keeps the descriptors of `formats` and the artifacts they use.
func (d *Descriptors) OfFormats(formats ...string) *Descriptors {
	out := &Descriptors{Artifacts: map[string]Artifact{}}
	for _, x := range d.Descriptors {
		for _, f := range formats {
			if x.Format == f {
				out.Descriptors = append(out.Descriptors, x)
				for _, p := range x.Parts {
					out.Artifacts[p.Artifact] = d.Artifacts[p.Artifact]
				}
			}
		}
	}
	return out
}

//go:embed data/descriptors.json
var descriptorsJSON []byte

var (
	descriptorsOnce sync.Once
	descriptors     *Descriptors
)

// AllDescriptors is the built-in data.
func AllDescriptors() *Descriptors {
	descriptorsOnce.Do(func() {
		var d Descriptors
		if err := json.Unmarshal(descriptorsJSON, &d); err != nil {
			panic("descriptors.json is not valid: " + err.Error())
		}
		descriptors = &d
	})
	return descriptors
}

// ParseDescriptors reads descriptors JSON (SAYSO_STORE_TEST_DESCRIPTORS).
func ParseDescriptors(b []byte) (*Descriptors, error) {
	var d Descriptors
	if err := json.Unmarshal(b, &d); err != nil {
		return nil, err
	}
	return &d, nil
}

// Render writes the data in the form of scripts/model-descriptors.py: one
// line for each file. The embedded file is in this form.
func Render(d *Descriptors) string {
	s := func(v string) string { b, _ := json.Marshal(v); return string(b) }
	names := make([]string, 0, len(d.Artifacts))
	for n := range d.Artifacts {
		names = append(names, n)
	}
	sort.Strings(names)
	var out strings.Builder
	out.WriteString("{\n  \"artifacts\": {\n")
	for i, name := range names {
		a := d.Artifacts[name]
		var source string
		switch a.Source.Kind {
		case SourceHuggingFace:
			folder := ""
			if a.Source.Folder != "" {
				folder = ", \"folder\": " + s(a.Source.Folder)
			}
			source = fmt.Sprintf("{\"kind\": \"hugging_face\", \"repo\": %s, \"revision\": %s%s}", s(a.Source.Repo), s(a.Source.Revision), folder)
		case SourceArchive:
			source = fmt.Sprintf("{\"kind\": \"archive\", \"url\": %s}", s(a.Source.URL))
		default:
			source = fmt.Sprintf("{\"kind\": \"file\", \"url\": %s}", s(a.Source.URL))
		}
		fmt.Fprintf(&out, "    %s: {\n      \"source\": %s,\n      \"digest\": %s,\n      \"files\": [\n", s(name), source, s(a.Digest))
		for j, f := range a.Files {
			comma := ""
			if j+1 < len(a.Files) {
				comma = ","
			}
			fmt.Fprintf(&out, "        {\"path\": %s, \"size\": %d, \"sha256\": %s}%s\n", s(f.Path), f.Size, s(f.SHA256), comma)
		}
		if i+1 < len(names) {
			out.WriteString("      ]\n    },\n")
		} else {
			out.WriteString("      ]\n    }\n")
		}
	}
	out.WriteString("  },\n  \"descriptors\": [\n")
	for i, x := range d.Descriptors {
		parts := make([]string, len(x.Parts))
		for j, p := range x.Parts {
			parts[j] = fmt.Sprintf("{\"role\": %s, \"artifact\": %s}", s(p.Role), s(p.Artifact))
		}
		comma := ""
		if i+1 < len(d.Descriptors) {
			comma = ","
		}
		fmt.Fprintf(&out, "    {\"model\": %s, \"format\": %s, \"revision\": %d, \"parts\": [%s]}%s\n", s(x.Model), s(x.Format), x.Revision, strings.Join(parts, ", "), comma)
	}
	out.WriteString("  ]\n}\n")
	return out.String()
}

// CheckPath checks a plain path relative to an artifact: parts joined by /,
// no empty part, no . or .., no \, :, or control character.
func CheckPath(path string) error {
	plain := path != ""
	if plain {
		for _, part := range strings.Split(path, "/") {
			if part == "" || part == "." || part == ".." {
				plain = false
				break
			}
		}
	}
	if plain {
		for _, c := range path {
			if c == '\\' || c == ':' || c < 0x20 || (c >= 0x7f && c <= 0x9f) {
				plain = false
				break
			}
		}
	}
	if !plain {
		return fmt.Errorf("the path %q is not a plain relative path in / form", path)
	}
	return nil
}

// Digest is the SHA-256 (lowercase hex) of the canonical file list: entries
// sorted by path bytes, each "path NUL size NUL sha256 LF".
func Digest(files []File) (string, error) {
	sorted := append([]File(nil), files...)
	sort.Slice(sorted, func(i, j int) bool { return sorted[i].Path < sorted[j].Path })
	seen := map[string]bool{}
	h := sha256.New()
	for _, f := range sorted {
		if err := CheckPath(f.Path); err != nil {
			return "", err
		}
		// Two paths that differ in letter case only are one file on macOS and Windows.
		low := strings.ToLower(f.Path)
		if seen[low] {
			return "", fmt.Errorf("the path %q is in the list twice (letter case ignored)", f.Path)
		}
		seen[low] = true
		if !isHex64(f.SHA256) {
			return "", fmt.Errorf("the hash of %q is not 64 lowercase hex digits", f.Path)
		}
		h.Write([]byte(f.Path))
		h.Write([]byte{0})
		h.Write([]byte(strconv.FormatUint(f.Size, 10)))
		h.Write([]byte{0})
		h.Write([]byte(f.SHA256))
		h.Write([]byte{'\n'})
	}
	return hex.EncodeToString(h.Sum(nil)), nil
}

func isHex64(s string) bool {
	if len(s) != 64 {
		return false
	}
	for i := 0; i < len(s); i++ {
		c := s[i]
		if !(c >= '0' && c <= '9' || c >= 'a' && c <= 'f') {
			return false
		}
	}
	return true
}

// DescriptorFor is the built-in descriptor of a catalog model, if its files
// are in the store.
func DescriptorFor(m ModelInfo) (Descriptor, bool) {
	format := FormatOf(m.Engine)
	if format == "" {
		return Descriptor{}, false
	}
	return AllDescriptors().Find(string(m.ID), format)
}
