package localengine

import (
	"encoding/json"
	"errors"
	"fmt"
	"strings"
)

// recipe is how the engine loads the files of an archive.
type recipe int

const (
	// recipeParakeetTdt is a NeMo Parakeet TDT transducer.
	recipeParakeetTdt recipe = iota
	// recipeParakeetTdtCtc is NeMo Parakeet TDT-CTC.
	recipeParakeetTdtCtc
	// recipeParakeetGguf is a Parakeet TDT GGUF on parakeet.cpp (parakeet_cpp kind).
	recipeParakeetGguf
	// recipeWhisperCpp is a Whisper GGML file on whisper.cpp (whisper_cpp kind).
	recipeWhisperCpp
	// recipeZipformerStreaming is a streaming zipformer transducer. Live preview only.
	recipeZipformerStreaming
	recipeWhisper
	recipeSenseVoice
	recipeMoonshine
	recipeSileroVad
	recipeTenVad
	recipeDiarization
)

// task is what a model does with audio.
type task int

const (
	taskSpeech task = iota
	taskVad
	taskDiarization
)

func (t task) name() string {
	switch t {
	case taskVad:
		return "VAD"
	case taskDiarization:
		return "diarization"
	}
	return "speech"
}

func parseRecipe(name string) (recipe, error) {
	switch name {
	case "parakeet_tdt":
		return recipeParakeetTdt, nil
	case "parakeet_tdt_ctc":
		return recipeParakeetTdtCtc, nil
	case "zipformer_streaming":
		return recipeZipformerStreaming, nil
	case "whisper":
		return recipeWhisper, nil
	case "sense_voice":
		return recipeSenseVoice, nil
	case "moonshine":
		return recipeMoonshine, nil
	}
	return 0, fmt.Errorf("unknown sherpa recipe %s", name)
}

func (r recipe) task() task {
	switch r {
	case recipeSileroVad, recipeTenVad:
		return taskVad
	case recipeDiarization:
		return taskDiarization
	}
	return taskSpeech
}

// finalPass is true when the main archive can do the final pass.
func (r recipe) finalPass() bool {
	return r.task() == taskSpeech && r != recipeZipformerStreaming
}

// maxChunkSeconds is the longest piece of audio that one decode gets; 0 is
// no limit. Whisper reads 30 second windows. Moonshine (sherpa-onnx 1.13.8)
// returns no text for 10 seconds or more. whisper.cpp cuts long audio itself.
func (r recipe) maxChunkSeconds() int {
	switch r {
	case recipeWhisper:
		return 28
	case recipeSenseVoice:
		return 25
	case recipeMoonshine:
		return 7
	case recipeParakeetTdt, recipeParakeetTdtCtc, recipeParakeetGguf:
		return 60
	}
	return 0
}

// singleFile is true when the main archive is one downloaded file.
func (r recipe) singleFile() bool {
	return r == recipeParakeetGguf || r == recipeWhisperCpp
}

// sourceKind is where the main archive of a model comes from.
type sourceKind int

const (
	// sourceSherpaRelease is a .tar.bz2 in the sherpa-onnx release.
	sourceSherpaRelease sourceKind = iota
	// sourceHuggingFace is file in the Hugging Face repo at the commit archive.
	sourceHuggingFace
	// sourceSherpaFile is file in the sherpa-onnx asr-models release (a VAD model).
	sourceSherpaFile
	// sourceDiarization is a segmentation archive and an embedding file.
	sourceDiarization
)

type source struct {
	kind sourceKind
	repo string
	// file is the Hugging Face file, the VAD file, or the embedding file.
	file string
}

// modelSpec is one model as the engine sees it.
type modelSpec struct {
	recipe recipe
	// archive is the folder of the main archive: the release archive name
	// without .tar.bz2, or the commit of a Hugging Face file.
	archive        string
	source         source
	previewArchive string
	// redecodePreview: the final pass also gives the live preview.
	redecodePreview bool
}

const (
	whisperCppRepo     = "ggerganov/whisper.cpp"
	whisperCppRevision = "main"
)

// format is the format of the files of the model in the model store.
func (s *modelSpec) format() string {
	switch s.recipe {
	case recipeParakeetGguf:
		return formatParakeetCpp
	case recipeWhisperCpp:
		return formatWhisperCpp
	}
	return formatSherpaOnnx
}

// knownEngineKinds are the kinds of EngineKind in sayso-core. A kind that is
// known but does not run here gets its own error.
var knownEngineKinds = map[string]bool{
	"parakeet_unified": true, "parakeet_eou": true, "parakeet_tdt": true, "nemotron": true,
	"nemotron_multilingual": true, "cohere": true, "canary": true, "sense_voice": true,
	"paraformer": true, "fluid_vad": true, "fluid_diarization": true, "whisper": true,
	"apple_speech": true, "remote": true, "whisper_cpp": true, "sherpa": true,
	"parakeet_cpp": true, "sherpa_vad": true, "sherpa_diarization": true,
}

// parseSpec reads the engine object of a request.
func parseSpec(raw json.RawMessage) (*modelSpec, error) {
	var object map[string]json.RawMessage
	if err := json.Unmarshal(raw, &object); err != nil || object == nil {
		return nil, errors.New("invalid engine field: not an object")
	}
	var kind string
	if r, ok := object["kind"]; !ok || json.Unmarshal(r, &kind) != nil {
		return nil, errors.New("invalid engine field: missing field `kind`")
	}
	str := func(key string) (string, error) {
		var s string
		r, ok := object[key]
		if !ok || json.Unmarshal(r, &s) != nil {
			return "", fmt.Errorf("invalid engine field: missing string %s", key)
		}
		return s, nil
	}
	optional := func(key string) (string, error) {
		r, ok := object[key]
		if !ok || string(r) == "null" {
			return "", nil
		}
		var s string
		if json.Unmarshal(r, &s) != nil {
			return "", fmt.Errorf("invalid engine field: %s is not a string", key)
		}
		return s, nil
	}
	var spec *modelSpec
	switch kind {
	case "sherpa_vad":
		return vadSpec(str)
	case "sherpa_diarization":
		return diarizationSpec(str)
	case "sherpa":
		name, err := str("recipe")
		if err != nil {
			return nil, err
		}
		archive, err := str("archive")
		if err != nil {
			return nil, err
		}
		preview, err := optional("preview_archive")
		if err != nil {
			return nil, err
		}
		var redecode bool
		if r, ok := object["redecode_preview"]; ok && json.Unmarshal(r, &redecode) != nil {
			return nil, errors.New("invalid engine field: redecode_preview is not a boolean")
		}
		rec, err := parseRecipe(name)
		if err != nil {
			return nil, err
		}
		if redecode && (preview != "" || !rec.finalPass()) {
			return nil, errors.New("a model with redecode_preview needs a final pass and no preview archive")
		}
		spec = &modelSpec{recipe: rec, archive: archive, source: source{kind: sourceSherpaRelease}, previewArchive: preview, redecodePreview: redecode}
	case "parakeet_cpp":
		repo, err := str("repo")
		if err != nil {
			return nil, err
		}
		revision, err := str("revision")
		if err != nil {
			return nil, err
		}
		file, err := str("file")
		if err != nil {
			return nil, err
		}
		preview, err := optional("preview_archive")
		if err != nil {
			return nil, err
		}
		if err := checkRepo(repo); err != nil {
			return nil, err
		}
		if err := checkArchiveName(file); err != nil {
			return nil, err
		}
		spec = &modelSpec{recipe: recipeParakeetGguf, archive: revision, source: source{kind: sourceHuggingFace, repo: repo, file: file}, previewArchive: preview}
	case "whisper_cpp":
		file, err := str("file")
		if err != nil {
			return nil, err
		}
		if err := checkArchiveName(file); err != nil {
			return nil, err
		}
		spec = &modelSpec{recipe: recipeWhisperCpp, archive: whisperCppRevision, source: source{kind: sourceHuggingFace, repo: whisperCppRepo, file: file}}
	default:
		if knownEngineKinds[kind] {
			return nil, fmt.Errorf("engine kind %s does not run in this engine", kind)
		}
		return nil, fmt.Errorf("invalid engine field: unknown variant `%s`", kind)
	}
	if err := checkArchiveName(spec.archive); err != nil {
		return nil, err
	}
	if spec.previewArchive != "" {
		if err := checkArchiveName(spec.previewArchive); err != nil {
			return nil, err
		}
	}
	return spec, nil
}

// archives is every archive to download, main archive first.
func (s *modelSpec) archives() []string {
	out := []string{s.archive}
	if s.previewArchive != "" && s.previewArchive != s.archive {
		out = append(out, s.previewArchive)
	}
	if e := s.embeddingArchive(); e != "" {
		out = append(out, e)
	}
	return out
}

// embeddingArchive is the folder of the speaker embedding model of a
// diarization model, or "".
func (s *modelSpec) embeddingArchive() string {
	if s.source.kind == sourceDiarization {
		return fileStem(s.source.file)
	}
	return ""
}

// hasPreview is true when the model gives a live preview.
func (s *modelSpec) hasPreview() bool {
	return s.redecodePreview || s.streamArchive() != ""
}

// streamArchive is the archive that streams the live preview, or "".
func (s *modelSpec) streamArchive() string {
	if s.recipe == recipeZipformerStreaming {
		return s.archive
	}
	return s.previewArchive
}

func (s *modelSpec) equal(o *modelSpec) bool { return *s == *o }

func fileStem(file string) string { return strings.TrimSuffix(file, ".onnx") }

func vadSpec(str func(string) (string, error)) (*modelSpec, error) {
	name, err := str("recipe")
	if err != nil {
		return nil, err
	}
	var rec recipe
	switch name {
	case "silero":
		rec = recipeSileroVad
	case "ten":
		rec = recipeTenVad
	default:
		return nil, fmt.Errorf("unknown VAD recipe %s", name)
	}
	file, err := str("file")
	if err != nil {
		return nil, err
	}
	if err := checkArchiveName(file); err != nil {
		return nil, err
	}
	return &modelSpec{recipe: rec, archive: fileStem(file), source: source{kind: sourceSherpaFile, file: file}}, nil
}

func diarizationSpec(str func(string) (string, error)) (*modelSpec, error) {
	segmentation, err := str("segmentation")
	if err != nil {
		return nil, err
	}
	embedding, err := str("embedding")
	if err != nil {
		return nil, err
	}
	if err := checkArchiveName(segmentation); err != nil {
		return nil, err
	}
	if err := checkArchiveName(embedding); err != nil {
		return nil, err
	}
	if fileStem(embedding) == segmentation {
		return nil, errors.New("the segmentation and the embedding model need different names")
	}
	return &modelSpec{recipe: recipeDiarization, archive: segmentation, source: source{kind: sourceDiarization, file: embedding}}, nil
}

// checkRepo checks a Hugging Face repo id: owner/name, each one plain path part.
func checkRepo(repo string) error {
	owner, name, ok := strings.Cut(repo, "/")
	if ok && checkArchiveName(owner) == nil && checkArchiveName(name) == nil {
		return nil
	}
	return fmt.Errorf("invalid Hugging Face repository %q", repo)
}

// checkArchiveName checks that a name is one plain path part: it becomes a
// URL and a folder name.
func checkArchiveName(name string) error {
	plain := name != "" && !strings.HasPrefix(name, ".")
	for _, c := range name {
		if !(c < 0x80 && (c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '-' || c == '_' || c == '.')) {
			plain = false
		}
	}
	if !plain {
		return fmt.Errorf("invalid archive name %q", name)
	}
	return nil
}
