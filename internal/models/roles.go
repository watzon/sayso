package models

import "sort"

// RoleChoice is the choice of a model for one role: "auto" (Sayso chooses), or
// a model id. In config.toml it is a plain string.
type RoleChoice struct {
	// Model is empty for "auto".
	Model ModelID
}

// Auto is the "auto" choice.
var Auto = RoleChoice{}

func (c RoleChoice) IsAuto() bool { return c.Model == "" }

func (c RoleChoice) String() string {
	if c.Model == "" {
		return "auto"
	}
	return string(c.Model)
}

// ParseRoleChoice reads the config value. "auto" is Auto; anything else is a model id.
func ParseRoleChoice(s string) RoleChoice {
	if s == "auto" {
		return Auto
	}
	return RoleChoice{Model: ModelID(s)}
}

func (c RoleChoice) MarshalText() ([]byte, error) { return []byte(c.String()), nil }

func (c *RoleChoice) UnmarshalText(b []byte) error {
	*c = ParseRoleChoice(string(b))
	return nil
}

// PreviewFor gives the model of the live preview. With a chosen model: that
// model, when it streams, is on disk, and understands the language. With
// "auto": the active model when it streams, else a streaming model on disk
// that understands the language, the fallback first.
func PreviewFor(active ModelInfo, choice RoleChoice, languageSetting string, onDisk func(ModelID) bool) (ModelID, bool) {
	language := active.LanguageFor(languageSetting)
	if !choice.IsAuto() {
		m, ok := Find(choice.Model)
		if ok && m.LivePreview && onDisk(m.ID) && m.Understands(language) {
			return m.ID, true
		}
		return "", false
	}
	if active.LivePreview {
		return active.ID, true
	}
	fallback := PreviewFallback()
	var candidates []ModelInfo
	for _, m := range Catalog() {
		if m.LivePreview && onDisk(m.ID) && m.Understands(language) {
			candidates = append(candidates, m)
		}
	}
	sort.SliceStable(candidates, func(i, j int) bool {
		return candidates[i].ID == fallback && candidates[j].ID != fallback
	})
	if len(candidates) == 0 {
		return "", false
	}
	return candidates[0].ID, true
}

// PreviewLanguageMismatch is true when a chosen preview model does not
// understand the dictation language.
func PreviewLanguageMismatch(active ModelInfo, choice RoleChoice, languageSetting string) bool {
	if choice.IsAuto() {
		return false
	}
	m, ok := Find(choice.Model)
	return ok && !m.Understands(active.LanguageFor(languageSetting))
}

// BasicVAD is the energy VAD that needs no model.
const BasicVAD = "basic"

// Vad is the VAD of a job: Basic, or a model.
type Vad struct {
	Basic bool
	Model ModelID
}

func IsVadModel(id ModelID) bool {
	if id == BasicVAD {
		return true
	}
	for _, m := range ModelsFor(TaskVad) {
		if m.ID == id {
			return true
		}
	}
	return false
}

func IsDiarizationModel(id ModelID) bool {
	for _, m := range ModelsFor(TaskDiarization) {
		if m.ID == id {
			return true
		}
	}
	return false
}

func chosenForRole(asked *ModelID, setting RoleChoice, ofRole func(ModelID) bool) *ModelID {
	if asked != nil && ofRole(*asked) {
		return asked
	}
	if !setting.IsAuto() && ofRole(setting.Model) {
		id := setting.Model
		return &id
	}
	return nil
}

// VadFor picks the VAD: the asked model, else the setting, else the first VAD
// model on disk, else Basic. A chosen model that is not on disk gives Basic.
func VadFor(asked *ModelID, setting RoleChoice, onDisk func(ModelID) bool) Vad {
	chosen := chosenForRole(asked, setting, IsVadModel)
	if chosen != nil {
		if *chosen == BasicVAD || !onDisk(*chosen) {
			return Vad{Basic: true}
		}
		return Vad{Model: *chosen}
	}
	for _, m := range ModelsFor(TaskVad) {
		if onDisk(m.ID) {
			return Vad{Model: m.ID}
		}
	}
	return Vad{Basic: true}
}

// DiarizationFor picks the diarization model, or none.
func DiarizationFor(asked *ModelID, setting RoleChoice, onDisk func(ModelID) bool) (ModelID, bool) {
	chosen := chosenForRole(asked, setting, IsDiarizationModel)
	if chosen != nil {
		return *chosen, onDisk(*chosen)
	}
	for _, m := range ModelsFor(TaskDiarization) {
		if onDisk(m.ID) {
			return m.ID, true
		}
	}
	return "", false
}
