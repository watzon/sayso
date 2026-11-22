package localengine

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

// offlineKind is the family of an offline (final pass) model.
type offlineKind int

const (
	offlineTransducer offlineKind = iota
	offlineNemoCtc
	offlineWhisper
	offlineSenseVoice
	offlineMoonshineV2
	offlineMoonshineV1
)

// offlineFiles are the files of an offline model. Only the fields of its kind are set.
type offlineFiles struct {
	kind                           offlineKind
	encoder, decoder, joiner       string
	model                          string
	preprocessor                   string
	uncachedDecoder, cachedDecoder string
	mergedDecoder                  string
	tokens                         string
}

// paths is every file of the model.
func (f *offlineFiles) paths() []string {
	switch f.kind {
	case offlineTransducer:
		return []string{f.encoder, f.decoder, f.joiner, f.tokens}
	case offlineNemoCtc, offlineSenseVoice:
		return []string{f.model, f.tokens}
	case offlineWhisper:
		return []string{f.encoder, f.decoder, f.tokens}
	case offlineMoonshineV2:
		return []string{f.encoder, f.mergedDecoder, f.tokens}
	default:
		return []string{f.preprocessor, f.encoder, f.uncachedDecoder, f.cachedDecoder, f.tokens}
	}
}

// onlineFiles are the files of a streaming transducer (the live preview).
type onlineFiles struct {
	encoder, decoder, joiner, tokens string
}

// diarizationFiles are the files of a diarization model.
type diarizationFiles struct {
	segmentation, embedding string
}

// listing is the files of one folder, not in subfolders.
type listing struct {
	dir   string
	files []string
}

func readListing(dir string) (*listing, error) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, fmt.Errorf("cannot read %s: %w", dir, err)
	}
	l := &listing{dir: dir}
	for _, e := range entries {
		if e.Type().IsRegular() {
			l.files = append(l.files, filepath.Join(dir, e.Name()))
		}
	}
	sort.Strings(l.files)
	return l, nil
}

func lowerName(path string) string { return strings.ToLower(filepath.Base(path)) }

// findModel is the model file (.onnx or .ort) whose name contains part. An
// int8 file wins over a full precision one, then the shorter name.
func (l *listing) findModel(part string) string {
	best := ""
	better := func(a, b string) bool {
		an, bn := lowerName(a), lowerName(b)
		ai, bi := strings.Contains(an, "int8"), strings.Contains(bn, "int8")
		if ai != bi {
			return ai
		}
		return len(an) < len(bn)
	}
	for _, p := range l.files {
		name := lowerName(p)
		if strings.Contains(name, part) && (strings.HasSuffix(name, ".onnx") || strings.HasSuffix(name, ".ort")) {
			if best == "" || better(p, best) {
				best = p
			}
		}
	}
	return best
}

func (l *listing) model(part string) (string, error) {
	if p := l.findModel(part); p != "" {
		return p, nil
	}
	return "", fmt.Errorf("no %s model file in %s", part, l.dir)
}

func (l *listing) tokens() (string, error) {
	for _, p := range l.files {
		if strings.HasSuffix(lowerName(p), "tokens.txt") {
			return p, nil
		}
	}
	return "", fmt.Errorf("no tokens.txt in %s", l.dir)
}

func findOfflineFiles(r recipe, dir string) (*offlineFiles, error) {
	l, err := readListing(dir)
	if err != nil {
		return nil, err
	}
	tokens, err := l.tokens()
	if err != nil {
		return nil, err
	}
	f := &offlineFiles{tokens: tokens}
	var errs []error
	need := func(dst *string, part string) {
		p, err := l.model(part)
		*dst = p
		if err != nil {
			errs = append(errs, err)
		}
	}
	switch r {
	case recipeParakeetTdt, recipeParakeetTdtCtc:
		// Some TDT-CTC archives hold the transducer, others the CTC model.
		enc, dec, join := l.findModel("encoder"), l.findModel("decoder"), l.findModel("joiner")
		if enc != "" && dec != "" && join != "" {
			f.kind, f.encoder, f.decoder, f.joiner = offlineTransducer, enc, dec, join
		} else {
			f.kind = offlineNemoCtc
			need(&f.model, "model")
		}
	case recipeWhisper:
		f.kind = offlineWhisper
		need(&f.encoder, "encoder")
		need(&f.decoder, "decoder")
	case recipeSenseVoice:
		f.kind = offlineSenseVoice
		need(&f.model, "model")
	case recipeMoonshine:
		if merged := l.findModel("merged"); merged != "" {
			f.kind, f.mergedDecoder = offlineMoonshineV2, merged
			need(&f.encoder, "encode")
		} else {
			f.kind = offlineMoonshineV1
			need(&f.preprocessor, "preprocess")
			need(&f.encoder, "encode")
			need(&f.uncachedDecoder, "uncached")
			need(&f.cachedDecoder, "cached_decode")
		}
	case recipeZipformerStreaming:
		return nil, errors.New("a streaming model has no final pass")
	case recipeParakeetGguf:
		return nil, errors.New("parakeet.cpp loads a GGUF model")
	case recipeWhisperCpp:
		return nil, errors.New("whisper.cpp loads a GGML model")
	default:
		return nil, errors.New("a VAD or diarization model has no final pass")
	}
	if len(errs) > 0 {
		return nil, errs[0]
	}
	return f, nil
}

func findOnlineFiles(dir string) (*onlineFiles, error) {
	l, err := readListing(dir)
	if err != nil {
		return nil, err
	}
	var f onlineFiles
	if f.encoder, err = l.model("encoder"); err != nil {
		return nil, err
	}
	if f.decoder, err = l.model("decoder"); err != nil {
		return nil, err
	}
	if f.joiner, err = l.model("joiner"); err != nil {
		return nil, err
	}
	if f.tokens, err = l.tokens(); err != nil {
		return nil, err
	}
	return &f, nil
}

// vadFile is the file of a VAD model: file in dir.
func vadFile(dir, file string) (string, error) {
	return modelFile(filepath.Join(dir, file))
}

// findDiarizationFiles finds the files of a diarization model. The
// segmentation archive holds model.onnx and model.int8.onnx; the full
// precision file is the one that the sherpa-onnx examples use.
func findDiarizationFiles(segmentationDir, embeddingDir, embedding string) (*diarizationFiles, error) {
	l, err := readListing(segmentationDir)
	if err != nil {
		return nil, err
	}
	segmentation := ""
	for _, p := range l.files {
		if lowerName(p) == "model.onnx" {
			segmentation = p
			break
		}
	}
	if segmentation == "" {
		if segmentation, err = l.model("model"); err != nil {
			return nil, err
		}
	}
	var f diarizationFiles
	if f.segmentation, err = modelFile(segmentation); err != nil {
		return nil, err
	}
	if f.embedding, err = modelFile(filepath.Join(embeddingDir, embedding)); err != nil {
		return nil, err
	}
	return &f, nil
}

// modelFile is path, when it is a file with content.
func modelFile(path string) (string, error) {
	info, err := os.Stat(path)
	if err != nil {
		return "", fmt.Errorf("cannot read %s: %w", path, err)
	}
	if info.Size() == 0 {
		return "", fmt.Errorf("%s is empty", path)
	}
	return path, nil
}
