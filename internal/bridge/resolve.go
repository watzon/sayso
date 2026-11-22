package bridge

import (
	"fmt"
	"path/filepath"
	"time"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/models"
)

// The models of a job or a live session, checked with the choices of the
// user and the catalog (jobs/resolve.rs and live/resolve.rs).

// Refusal is a request that the host does not run.
type Refusal struct {
	Code    ErrorCode
	Message string
}

func (r *Refusal) Error() string { return r.Message }

func refuse(code ErrorCode, format string, args ...any) *Refusal {
	return &Refusal{Code: code, Message: fmt.Sprintf(format, args...)}
}

// UserChoices are the models of the user.
type UserChoices struct {
	FinalModel  models.ModelID
	Language    string
	Vad         models.RoleChoice
	Diarization models.RoleChoice
	Preview     models.RoleChoice
}

// EngineView is what resolve needs of the engine.
type EngineView struct {
	OnDisk   func(models.ModelID) bool
	Features []string
}

func (e EngineView) has(feature string) bool { return hasFeature(e.Features, feature) }

func choicesOf(cfg *config.Config) UserChoices {
	return UserChoices{FinalModel: cfg.Dictation.Model, Language: cfg.Dictation.Language, Vad: cfg.Models.Vad, Diarization: cfg.Models.Diarization, Preview: cfg.Dictation.PreviewModel}
}

func viewOf(eng Engine) EngineView {
	return EngineView{OnDisk: func(id models.ModelID) bool { return eng.Status(string(id)).IsOnDisk() }, Features: eng.Features()}
}

// ResolveJob checks transcribe_file.
func ResolveJob(req *TranscribeFile, user UserChoices, eng EngineView) (JobSpec, *Refusal) {
	if !filepath.IsAbs(req.Path) {
		return JobSpec{}, refuse(CodeBadRequest, "path must be absolute: %s", req.Path)
	}
	if req.Speakers != nil && *req.Speakers == 0 {
		return JobSpec{}, refuse(CodeBadRequest, "speakers must be 1 or more")
	}
	model, r := finalModel(req.Model, user, eng)
	if r != nil {
		return JobSpec{}, r
	}
	ts := TimestampsNone
	if eng.has("segments") {
		ts = TimestampLevel(model.Engine.Timestamps())
	}
	vad, r := resolveVad(req.VadModel, user, eng)
	if r != nil {
		return JobSpec{}, r
	}
	dia, r := resolveDiarization(req.Diarize, req.DiarizationModel, user, eng)
	if r != nil {
		return JobSpec{}, r
	}
	lang := user.Language
	if req.Language != nil {
		lang = *req.Language
	}
	return JobSpec{Path: req.Path, Model: string(model.ID), Timestamps: ts, Vad: vad, Diarization: string(dia), Language: lang, Speakers: req.Speakers}, nil
}

func finalModel(asked *string, user UserChoices, eng EngineView) (models.ModelInfo, *Refusal) {
	var model models.ModelInfo
	if asked != nil {
		m, r := checked(models.ModelID(*asked), models.TaskSpeech, "model", eng)
		if r != nil {
			return m, r
		}
		model = m
	} else {
		id := user.FinalModel
		info, ok := models.FindLocal(id)
		if !ok || info.Task() != models.TaskSpeech {
			return info, refuse(CodeUnsupported, "the final model of Sayso, %s, is not a model of this computer; name a local model in model", id)
		}
		if !eng.OnDisk(id) {
			return info, refuse(CodeModelNotDownloaded, "the final model of Sayso, %s, is not downloaded", id)
		}
		model = info
	}
	if !model.FinalPass {
		return model, refuse(CodeWrongRole, "%s cannot do the final pass", model.ID)
	}
	return model, nil
}

func resolveVad(asked *string, user UserChoices, eng EngineView) (VadRun, *Refusal) {
	if asked != nil {
		if *asked == models.BasicVAD {
			return VadRun{Basic: true}, nil
		}
		info, r := checked(models.ModelID(*asked), models.TaskVad, "vad_model", eng)
		if r != nil {
			return VadRun{}, r
		}
		if r := needFeature(eng, "vad", info.ID); r != nil {
			return VadRun{}, r
		}
		return VadRun{Model: string(info.ID)}, nil
	}
	v := models.VadFor(nil, user.Vad, eng.OnDisk)
	if !v.Basic && v.Model != "" && eng.has("vad") {
		return VadRun{Model: string(v.Model)}, nil
	}
	return VadRun{Basic: true}, nil
}

func resolveDiarization(diarize *bool, asked *string, user UserChoices, eng EngineView) (models.ModelID, *Refusal) {
	on := asked != nil
	if diarize != nil {
		on = *diarize
	}
	if !on {
		return "", nil
	}
	if asked != nil {
		info, r := checked(models.ModelID(*asked), models.TaskDiarization, "diarization_model", eng)
		if r != nil {
			return "", r
		}
		if r := needFeature(eng, "diarization", info.ID); r != nil {
			return "", r
		}
		return info.ID, nil
	}
	id, ok := models.DiarizationFor(nil, user.Diarization, eng.OnDisk)
	if !ok {
		return "", refuse(CodeModelNotDownloaded, "no diarization model is downloaded; send download_model")
	}
	if r := needFeature(eng, "diarization", id); r != nil {
		return "", r
	}
	return id, nil
}

func checked(id models.ModelID, task models.ModelTask, field string, eng EngineView) (models.ModelInfo, *Refusal) {
	info, ok := models.FindLocal(id)
	if !ok {
		return info, refuse(CodeUnknownModel, "%s: %s is not a model of this computer", field, id)
	}
	if info.Task() != task {
		what := map[models.ModelTask]string{models.TaskSpeech: "a speech model", models.TaskVad: "a VAD model", models.TaskDiarization: "a diarization model"}[task]
		return info, refuse(CodeWrongRole, "%s: %s is not %s", field, id, what)
	}
	if !eng.OnDisk(id) {
		return info, refuse(CodeModelNotDownloaded, "%s: %s is not downloaded; send download_model", field, id)
	}
	return info, nil
}

func needFeature(eng EngineView, feature string, id models.ModelID) *Refusal {
	if eng.has(feature) {
		return nil
	}
	return refuse(CodeUnsupported, "the speech engine of this computer cannot run %s: it has no %s", id, feature)
}

// SessionSpec is a live session that resolve checked.
type SessionSpec struct {
	Sources      []Source
	FinalModel   string
	PreviewModel string
	Vad          VadRun
	Diarization  string
	Language     string
	RecordTo     string
	SilenceLimit time.Duration
}

// Models are the models of the session.
func (s SessionSpec) Models() []string {
	out := []string{s.FinalModel}
	if s.PreviewModel != "" {
		out = append(out, s.PreviewModel)
	}
	if !s.Vad.Basic && s.Vad.Model != "" {
		out = append(out, s.Vad.Model)
	}
	if s.Diarization != "" {
		out = append(out, s.Diarization)
	}
	return out
}

// ResolveSession checks session_start.
func ResolveSession(req *SessionStart, user UserChoices, eng EngineView) (SessionSpec, *Refusal) {
	final, r := finalModel(req.Model, user, eng)
	if r != nil {
		return SessionSpec{}, r
	}
	lang := user.Language
	if req.Language != nil {
		lang = *req.Language
	}
	var preview models.ModelID
	switch {
	case req.PreviewModel != nil:
		info, r := checked(models.ModelID(*req.PreviewModel), models.TaskSpeech, "preview_model", eng)
		if r != nil {
			return SessionSpec{}, r
		}
		if !info.LivePreview {
			return SessionSpec{}, refuse(CodeWrongRole, "preview_model: %s cannot stream", *req.PreviewModel)
		}
		if r := needFeature(eng, "concurrent_streams", info.ID); r != nil {
			return SessionSpec{}, r
		}
		preview = info.ID
	case eng.has("concurrent_streams"):
		if id, ok := models.PreviewFor(final, user.Preview, lang, eng.OnDisk); ok {
			preview = id
		}
	}
	vad, r := resolveVad(req.VadModel, user, eng)
	if r != nil {
		return SessionSpec{}, r
	}
	dia, r := resolveDiarization(req.Diarize, req.DiarizationModel, user, eng)
	if r != nil {
		return SessionSpec{}, r
	}
	spec := SessionSpec{Sources: req.Sources, FinalModel: string(final.ID), PreviewModel: string(preview), Vad: vad, Diarization: string(dia), Language: lang}
	if req.RecordTo != nil {
		spec.RecordTo = *req.RecordTo
	}
	if req.SilenceLimitMs != nil {
		spec.SilenceLimit = time.Duration(*req.SilenceLimitMs) * time.Millisecond
	}
	return spec, nil
}
