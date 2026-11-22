package config

import (
	"fmt"
	"strings"

	"github.com/watzon/sayso/internal/models"
	"github.com/watzon/sayso/internal/models/languages"
)

// Speech providers: cloud services that run the final pass
// (crates/sayso-core/src/speech.rs). API keys are in the secret store,
// never in config.toml.

// SpeechProviderKind is the API a speech provider speaks.
type SpeechProviderKind string

const (
	SpeechOpenAI           SpeechProviderKind = "open_ai"
	SpeechGroq             SpeechProviderKind = "groq"
	SpeechMistral          SpeechProviderKind = "mistral"
	SpeechElevenLabs       SpeechProviderKind = "eleven_labs"
	SpeechDeepgram         SpeechProviderKind = "deepgram"
	SpeechAssemblyAI       SpeechProviderKind = "assembly_ai"
	SpeechOpenAICompatible SpeechProviderKind = "open_ai_compatible"
)

// AllSpeechKinds is the order of the add menu.
var AllSpeechKinds = []SpeechProviderKind{SpeechOpenAI, SpeechGroq, SpeechElevenLabs, SpeechDeepgram, SpeechAssemblyAI, SpeechMistral, SpeechOpenAICompatible}

var speechKindNames = []string{"open_ai", "groq", "mistral", "eleven_labs", "deepgram", "assembly_ai", "open_ai_compatible"}

// Label is the shown name; "" for a custom endpoint, whose name the UI translates
// (core.speech.custom_endpoint).
func (k SpeechProviderKind) Label() string {
	return map[SpeechProviderKind]string{
		SpeechOpenAI: "OpenAI", SpeechGroq: "Groq", SpeechMistral: "Mistral", SpeechElevenLabs: "ElevenLabs",
		SpeechDeepgram: "Deepgram", SpeechAssemblyAI: "AssemblyAI",
	}[k]
}

// DefaultBaseURL is the API root, or "" for a custom endpoint.
func (k SpeechProviderKind) DefaultBaseURL() string {
	return map[SpeechProviderKind]string{
		SpeechOpenAI:     "https://api.openai.com/v1",
		SpeechGroq:       "https://api.groq.com/openai/v1",
		SpeechMistral:    "https://api.mistral.ai/v1",
		SpeechElevenLabs: "https://api.elevenlabs.io/v1",
		SpeechDeepgram:   "https://api.deepgram.com/v1",
		SpeechAssemblyAI: "https://sync.assemblyai.com/v1",
	}[k]
}

// KeyURL is where the user creates an API key.
func (k SpeechProviderKind) KeyURL() string {
	return map[SpeechProviderKind]string{
		SpeechOpenAI:     "https://platform.openai.com/api-keys",
		SpeechGroq:       "https://console.groq.com/keys",
		SpeechMistral:    "https://console.mistral.ai/api-keys",
		SpeechElevenLabs: "https://elevenlabs.io/app/settings/api-keys",
		SpeechDeepgram:   "https://console.deepgram.com",
		SpeechAssemblyAI: "https://www.assemblyai.com/dashboard/api-keys",
	}[k]
}

// KnownModel is a model a known provider offers.
type KnownModel struct {
	ID, Name   string
	Vocabulary bool
	Speed      uint8
}

// DescriptionKey is the i18n key of the model's note.
func (m KnownModel) DescriptionKey() string {
	return "core.speech.model." + strings.NewReplacer("-", "_", ".", "_").Replace(m.ID)
}

// KnownModels are the models Sayso offers for the provider.
func (k SpeechProviderKind) KnownModels() []KnownModel {
	switch k {
	case SpeechOpenAI:
		return []KnownModel{{"gpt-4o-transcribe", "GPT-4o Transcribe", true, 4}, {"gpt-4o-mini-transcribe", "GPT-4o mini Transcribe", true, 5}, {"gpt-transcribe", "GPT Transcribe", true, 4}, {"whisper-1", "Whisper", true, 3}}
	case SpeechGroq:
		return []KnownModel{{"whisper-large-v3-turbo", "Whisper large-v3 turbo", true, 5}, {"whisper-large-v3", "Whisper large-v3", true, 5}}
	case SpeechMistral:
		return []KnownModel{{"voxtral-mini-latest", "Voxtral Mini Transcribe", true, 4}}
	case SpeechElevenLabs:
		return []KnownModel{{"scribe_v2", "Scribe v2", true, 3}}
	case SpeechDeepgram:
		return []KnownModel{{"nova-3", "Nova-3", true, 5}, {"nova-2", "Nova-2", false, 5}}
	case SpeechAssemblyAI:
		return []KnownModel{{"universal-3-5-pro", "Universal 3.5 Pro", true, 4}}
	}
	return nil
}

// SpeechProvider is a configured speech provider.
type SpeechProvider struct {
	// ID is used in model ids ("<id>:<model>"), so it has no colon.
	ID   string
	Name string
	Kind SpeechProviderKind
	// BaseURL is needed for a custom endpoint; for a known provider it
	// replaces the default address.
	BaseURL *string
	// APIKeyAccount is the secret store account; nil for a server without a key.
	APIKeyAccount *string
	// Models are model ids to offer besides the known ones.
	Models []string
}

// BaseURLResolved is the API root without a trailing slash, or "".
func (p SpeechProvider) BaseURLResolved() string {
	u := ""
	if p.BaseURL != nil {
		u = strings.TrimSpace(*p.BaseURL)
	}
	if u == "" {
		u = p.Kind.DefaultBaseURL()
	}
	return strings.TrimRight(u, "/")
}

// IsCloud is true when audio leaves this computer.
func (p SpeechProvider) IsCloud() bool {
	u := p.BaseURLResolved()
	return u == "" || !IsLocalURL(u)
}

// ModelID is the catalog id of one of this provider's models.
func (p SpeechProvider) ModelID(model string) models.ModelID {
	return models.ModelID(p.ID + ":" + model)
}

// Catalog is this provider's models as catalog entries.
func (p SpeechProvider) Catalog() []models.ModelInfo {
	entry := func(id, name, desc string, vocabulary bool, speed uint8) models.ModelInfo {
		return models.ModelInfo{
			ID: p.ModelID(id), Name: name, Vendor: p.Name, DescriptionKey: desc,
			Engine:    models.EngineKind{Kind: models.KindRemote, Provider: p.ID, Model: id},
			Languages: languages.All(), FinalPass: true, Vocabulary: vocabulary, Speed: speed,
			Punctuation: true, MinMacOS: 14,
		}
	}
	known := p.Kind.KnownModels()
	var list []models.ModelInfo
	for _, k := range known {
		list = append(list, entry(k.ID, k.Name, k.DescriptionKey(), k.Vocabulary, k.Speed))
	}
	for _, id := range p.Models {
		id = strings.TrimSpace(id)
		if id == "" {
			continue
		}
		dup := false
		for _, k := range known {
			if k.ID == id {
				dup = true
			}
		}
		for _, m := range list {
			if m.ID == p.ModelID(id) {
				dup = true
			}
		}
		if dup {
			continue
		}
		place := "core.speech.runs_on_own_server"
		if p.IsCloud() {
			place = "core.speech.runs_on_provider"
		}
		// The OpenAI-compatible API takes a prompt, so the words go there.
		vocabulary := p.Kind == SpeechOpenAICompatible || p.Kind == SpeechOpenAI || p.Kind == SpeechGroq
		name := id
		if i := strings.LastIndex(id, "/"); i >= 0 {
			name = id[i+1:]
		}
		list = append(list, entry(id, name, place, vocabulary, 3))
	}
	return list
}

// Speech is the [speech] section.
type Speech struct {
	Providers []SpeechProvider
	// TimeoutMs is the longest a cloud final pass may take, the upload included.
	TimeoutMs uint64
}

func defaultSpeech() Speech { return Speech{Providers: []SpeechProvider{}, TimeoutMs: 30_000} }

// Provider finds a provider by id.
func (s Speech) Provider(id string) (SpeechProvider, bool) {
	for _, p := range s.Providers {
		if p.ID == id {
			return p, true
		}
	}
	return SpeechProvider{}, false
}

// Catalog is the models of every configured provider.
func (s Speech) Catalog() []models.ModelInfo {
	var out []models.ModelInfo
	for _, p := range s.Providers {
		out = append(out, p.Catalog()...)
	}
	return out
}

func readSpeech(v any) (Speech, error) {
	s := defaultSpeech()
	r := newReader(v)
	if arr := r.tables("providers"); arr != nil {
		s.Providers = []SpeechProvider{}
		for _, e := range arr {
			p, err := readSpeechProvider(e)
			if err != nil {
				return defaultSpeech(), err
			}
			s.Providers = append(s.Providers, p)
		}
	} else if _, ok := r.get("providers"); ok && r.err == nil {
		s.Providers = []SpeechProvider{}
	}
	r.u64("timeout_ms", &s.TimeoutMs)
	return s, r.err
}

func readSpeechProvider(v any) (SpeechProvider, error) {
	var p SpeechProvider
	r := newReader(v)
	for _, key := range []string{"id", "name", "kind"} {
		if _, ok := r.get(key); !ok && r.err == nil {
			return p, fmt.Errorf("missing field `%s`", key)
		}
	}
	r.str("id", &p.ID)
	r.str("name", &p.Name)
	r.enum("kind", speechKindNames, func(i int) { p.Kind = SpeechProviderKind(speechKindNames[i]) })
	r.optStr("base_url", &p.BaseURL)
	r.optStr("api_key_account", &p.APIKeyAccount)
	r.strings("models", &p.Models)
	return p, r.err
}

func (s Speech) toTable() *Table {
	t := newTable()
	providers := make([]any, 0, len(s.Providers))
	for _, p := range s.Providers {
		pt := newTable().Set("id", p.ID).Set("name", p.Name).Set("kind", string(p.Kind))
		if p.BaseURL != nil {
			pt.Set("base_url", *p.BaseURL)
		}
		if p.APIKeyAccount != nil {
			pt.Set("api_key_account", *p.APIKeyAccount)
		}
		if len(p.Models) > 0 {
			pt.Set("models", stringsToAny(p.Models))
		}
		providers = append(providers, pt)
	}
	t.Set("providers", providers)
	t.Set("timeout_ms", s.TimeoutMs)
	return t
}

// SpeechError is the error of a speech request.
type SpeechError struct {
	Kind    SpeechErrorKind
	Status  int    // Http
	Message string // Http, Network, InvalidResponse
	Ms      uint64 // Timeout
	Missing MissingSetup
}

// SpeechErrorKind is the kind of a speech error.
type SpeechErrorKind int

const (
	SpeechTimeout SpeechErrorKind = iota
	SpeechNotConfigured
	SpeechHTTP
	SpeechNetwork
	SpeechInvalidResponse
)

// MissingSetup is what a provider needs before use.
type MissingSetup int

const (
	MissingAddress MissingSetup = iota
	MissingAPIKey
)

func (e *SpeechError) Error() string {
	switch e.Kind {
	case SpeechTimeout:
		return fmt.Sprintf("timed out after %d ms", e.Ms)
	case SpeechNotConfigured:
		if e.Missing == MissingAddress {
			return "the provider is not set up: no address is set"
		}
		return "the provider is not set up: no API key is saved"
	case SpeechHTTP:
		return fmt.Sprintf("HTTP %d: %s", e.Status, e.Message)
	case SpeechNetwork:
		return "network error: " + e.Message
	}
	return "the response had no transcript: " + e.Message
}

// UserMessageKey is the i18n key of the sentence for the overlay.
func (e *SpeechError) UserMessageKey() string {
	switch {
	case e.Kind == SpeechTimeout:
		return "core.speech.error.timeout"
	case e.Kind == SpeechNotConfigured && e.Missing == MissingAddress:
		return "core.speech.error.no_address"
	case e.Kind == SpeechNotConfigured:
		return "core.speech.error.no_api_key"
	case e.Kind == SpeechHTTP && (e.Status == 401 || e.Status == 403):
		return "core.speech.error.key_refused"
	case e.Kind == SpeechHTTP && e.Status == 429:
		return "core.speech.error.rate_limit"
	case e.Kind == SpeechHTTP && e.Message == "":
		return "core.speech.error.http"
	case e.Kind == SpeechHTTP:
		return "core.speech.error.http_message"
	case e.Kind == SpeechNetwork:
		return "core.speech.error.network"
	}
	return "core.speech.error.invalid_response"
}
