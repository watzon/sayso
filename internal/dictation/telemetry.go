package dictation

import (
	"slices"

	"github.com/watzon/sayso/internal/config"
	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/store"
	"github.com/watzon/sayso/internal/telemetry"
)

// Telemetry hears each inserted dictation (internal/telemetry).
type Telemetry interface {
	DictationCompleted(telemetry.Dictation)
}

// countDictation tells telemetry that a dictation was inserted. An incognito
// dictation is not counted.
func (c *Controller) countDictation(id SessionID) {
	if c.d.Telemetry == nil {
		return
	}
	s := c.session(id)
	if s.incognito || c.d.Incognito() {
		return
	}
	cfg := c.config()
	model := s.model
	if model == "" {
		model = cfg.Dictation.Model
	}
	c.d.Telemetry.DictationCompleted(TelemetryDictation(cfg, model, s.enhance))
}

// TelemetryDictation is what dictation.completed tells about a dictation:
// the model as a public name only, and the kind of style. A provider's own
// id or name, a custom endpoint's model, and a model the user typed in stay
// out.
func TelemetryDictation(cfg *config.Config, model models.ModelID, e *store.EnhanceOutcome) telemetry.Dictation {
	d := telemetry.Dictation{ModelID: "other", ModelKind: telemetry.ModelLocal, StyleKind: telemetry.StyleRaw}
	if providerID, name, ok := SplitModelID(model); ok {
		p, found := cfg.Speech.Provider(providerID)
		if !found {
			d.ModelKind = telemetry.ModelCloud
		} else {
			d.ModelID = string(p.Kind)
			if slices.ContainsFunc(p.Kind.KnownModels(), func(k config.KnownModel) bool { return k.ID == name }) {
				d.ModelID += ":" + name
			}
			if p.IsCloud() {
				d.ModelKind = telemetry.ModelCloud
			}
		}
	} else if _, ok := models.FindLocal(model); ok {
		d.ModelID = string(model)
	}
	if e != nil && e.Result == store.EnhanceApplied {
		d.StyleKind = telemetry.StyleLocal
		if p, ok := cfg.AI.Provider(e.ProviderID); ok && p.IsCloud() {
			d.StyleKind = telemetry.StyleCloud
		}
	}
	return d
}
