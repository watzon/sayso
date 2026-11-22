package localengine

import (
	_ "embed"
	"encoding/json"
	"runtime"
)

// The built-in catalogs (docs/engine-protocol.md, "Models"): the output of
// sayso_core::engine_catalog::json(Engine::Rust(System::Linux | Windows)),
// so a client gets the same models as from the Rust engine. They are
// generated, not written by hand; regenerate them when
// crates/sayso-core/src/models_onnx.rs changes.

//go:embed catalog/catalog_linux.json
var catalogLinux []byte

//go:embed catalog/catalog_windows.json
var catalogWindows []byte

// catalogEntry is one model of the catalog, with the fields the engine reads.
// The reply of list_models sends the raw catalog.
type catalogEntry struct {
	ID              string          `json:"id"`
	Engine          json.RawMessage `json:"engine"`
	Languages       []string        `json:"languages"`
	DetectsLanguage bool            `json:"detects_language"`
}

type retiredEntry struct {
	ID      string `json:"id"`
	Message string `json:"message"`
}

type catalog struct {
	raw     []byte
	models  []catalogEntry
	retired []retiredEntry
}

// catalogFor is the catalog of the engine on goos.
func catalogFor(goos string) (*catalog, error) {
	data := catalogLinux
	if goos == "windows" {
		data = catalogWindows
	}
	var parsed struct {
		Models  []catalogEntry `json:"models"`
		Retired []retiredEntry `json:"retired"`
	}
	if err := json.Unmarshal(data, &parsed); err != nil {
		return nil, err
	}
	return &catalog{raw: data, models: parsed.Models, retired: parsed.Retired}, nil
}

func currentCatalog() (*catalog, error) { return catalogFor(runtime.GOOS) }

func (c *catalog) model(id string) *catalogEntry {
	for i := range c.models {
		if c.models[i].ID == id {
			return &c.models[i]
		}
	}
	return nil
}

func (c *catalog) retiredModel(id string) *retiredEntry {
	for i := range c.retired {
		if c.retired[i].ID == id {
			return &c.retired[i]
		}
	}
	return nil
}

// reply is the catalog as fields of the ok reply of list_models.
func (c *catalog) reply() fields {
	var parsed struct {
		Models  json.RawMessage `json:"models"`
		Retired json.RawMessage `json:"retired"`
	}
	_ = json.Unmarshal(c.raw, &parsed)
	return fields{"models": parsed.Models, "retired": parsed.Retired}
}
