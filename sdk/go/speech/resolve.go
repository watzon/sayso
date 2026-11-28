package speech

import (
	"fmt"
	"path/filepath"
	"slices"
	"time"

	"github.com/watzon/sayso/sdk/go/protocol"
)

// SessionSpec is a live session with its models chosen.
type SessionSpec struct {
	Sources      []Source
	FinalModel   string
	PreviewModel string
	Vad          VadRun
	Diarization  string
	Language     string
	// RecordTo is a folder for the recordings, <source>.wav, or "" for
	// temporary recordings.
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

// Refusal is a request that a job or a session does not run.
type Refusal struct {
	Code    ErrorCode
	Message string
}

func (r *Refusal) Error() string { return r.Message }

func refuse(code ErrorCode, format string, args ...any) *Refusal {
	return &Refusal{Code: code, Message: fmt.Sprintf(format, args...)}
}

// Catalog is what resolve knows of the engine: its models with their state,
// and the features of its hello.
type Catalog struct {
	Models   []protocol.ModelInfo
	Features []string
	// Language is the language of a request that names none: "auto" when
	// empty.
	Language string
}

func (c Catalog) has(feature string) bool { return hasFeature(c.Features, feature) }

func (c Catalog) find(id string) (protocol.ModelInfo, bool) {
	for _, m := range c.Models {
		if m.ID == id {
			return m, true
		}
	}
	return protocol.ModelInfo{}, false
}

func onDisk(m protocol.ModelInfo) bool {
	switch m.State {
	case protocol.StateDownloaded, protocol.StateReady, protocol.StateOptimizing:
		return true
	}
	return false
}

// first is the first model of the catalog with the role that is on disk.
func (c Catalog) first(role protocol.Role) (protocol.ModelInfo, bool) {
	for _, m := range c.Models {
		if slices.Contains(m.Roles, role) && onDisk(m) {
			return m, true
		}
	}
	return protocol.ModelInfo{}, false
}

func (c Catalog) checked(id string, role protocol.Role, field string) (protocol.ModelInfo, *Refusal) {
	m, ok := c.find(id)
	if !ok {
		return m, refuse(CodeUnknownModel, "%s: the engine has no model %s", field, id)
	}
	if !slices.Contains(m.Roles, role) {
		return m, refuse(CodeWrongRole, "%s: %s cannot do %s", field, id, role)
	}
	if !onDisk(m) {
		return m, refuse(CodeModelNotDownloaded, "%s: %s is not downloaded; download it first", field, id)
	}
	return m, nil
}

func (c Catalog) needFeature(feature, id string) *Refusal {
	if c.has(feature) {
		return nil
	}
	return refuse(CodeUnsupported, "the speech engine cannot run %s: it has no %s", id, feature)
}

func (c Catalog) language(asked *string) string {
	switch {
	case asked != nil:
		return *asked
	case c.Language != "":
		return c.Language
	}
	return "auto"
}

func (c Catalog) final(asked *string) (protocol.ModelInfo, *Refusal) {
	if asked != nil {
		return c.checked(*asked, protocol.RoleFinal, "model")
	}
	m, ok := c.first(protocol.RoleFinal)
	if !ok {
		return m, refuse(CodeModelNotDownloaded, "no final model is downloaded; download one first")
	}
	return m, nil
}

// BasicVAD is the name of the energy VAD in vad_model.
const BasicVAD = "basic"

func (c Catalog) vad(asked *string) (VadRun, *Refusal) {
	if asked != nil {
		if *asked == BasicVAD {
			return VadRun{Basic: true}, nil
		}
		m, r := c.checked(*asked, protocol.RoleVad, "vad_model")
		if r != nil {
			return VadRun{}, r
		}
		if r := c.needFeature("vad", m.ID); r != nil {
			return VadRun{}, r
		}
		return VadRun{Model: m.ID}, nil
	}
	if m, ok := c.first(protocol.RoleVad); ok && c.has("vad") {
		return VadRun{Model: m.ID}, nil
	}
	return VadRun{Basic: true}, nil
}

func (c Catalog) diarization(diarize *bool, asked *string) (string, *Refusal) {
	on := asked != nil
	if diarize != nil {
		on = *diarize
	}
	if !on {
		return "", nil
	}
	var m protocol.ModelInfo
	if asked != nil {
		var r *Refusal
		if m, r = c.checked(*asked, protocol.RoleDiarization, "diarization_model"); r != nil {
			return "", r
		}
	} else {
		var ok bool
		if m, ok = c.first(protocol.RoleDiarization); !ok {
			return "", refuse(CodeModelNotDownloaded, "no diarization model is downloaded; download one first")
		}
	}
	if r := c.needFeature("diarization", m.ID); r != nil {
		return "", r
	}
	return m.ID, nil
}

// ResolveJob checks transcribe_file and picks its models.
func ResolveJob(req *protocol.TranscribeFile, c Catalog) (JobSpec, *Refusal) {
	if !filepath.IsAbs(req.Path) {
		return JobSpec{}, refuse(CodeBadRequest, "path must be absolute: %s", req.Path)
	}
	if req.Speakers != nil && *req.Speakers == 0 {
		return JobSpec{}, refuse(CodeBadRequest, "speakers must be 1 or more")
	}
	model, r := c.final(req.Model)
	if r != nil {
		return JobSpec{}, r
	}
	ts := TimestampsNone
	if c.has("segments") && model.Timestamps != "" {
		ts = model.Timestamps
	}
	vad, r := c.vad(req.VadModel)
	if r != nil {
		return JobSpec{}, r
	}
	dia, r := c.diarization(req.Diarize, req.DiarizationModel)
	if r != nil {
		return JobSpec{}, r
	}
	return JobSpec{Path: req.Path, Model: model.ID, Timestamps: ts, Vad: vad, Diarization: dia, Language: c.language(req.Language), Speakers: req.Speakers}, nil
}

// ResolveSession checks session_start and picks its models. The preview is
// the asked model, else the final model when it can stream.
func ResolveSession(req *protocol.SessionStart, c Catalog) (SessionSpec, *Refusal) {
	final, r := c.final(req.Model)
	if r != nil {
		return SessionSpec{}, r
	}
	var preview string
	switch {
	case req.PreviewModel != nil:
		m, r := c.checked(*req.PreviewModel, protocol.RoleStreaming, "preview_model")
		if r != nil {
			return SessionSpec{}, r
		}
		if r := c.needFeature("concurrent_streams", m.ID); r != nil {
			return SessionSpec{}, r
		}
		preview = m.ID
	case c.has("concurrent_streams") && slices.Contains(final.Roles, protocol.RoleStreaming):
		preview = final.ID
	}
	vad, r := c.vad(req.VadModel)
	if r != nil {
		return SessionSpec{}, r
	}
	dia, r := c.diarization(req.Diarize, req.DiarizationModel)
	if r != nil {
		return SessionSpec{}, r
	}
	spec := SessionSpec{Sources: req.Sources, FinalModel: final.ID, PreviewModel: preview, Vad: vad, Diarization: dia, Language: c.language(req.Language)}
	if req.RecordTo != nil {
		spec.RecordTo = *req.RecordTo
	}
	if req.SilenceLimitMs != nil {
		spec.SilenceLimit = time.Duration(*req.SilenceLimitMs) * time.Millisecond
	}
	return spec, nil
}
