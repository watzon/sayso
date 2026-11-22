// Package providers holds the cloud speech-to-text clients for the final
// pass (a port of crates/sayso-transcribe) and the secret store that keeps
// API keys (a port of sayso-enhance/src/secrets.rs). All calls block.
//
// The caller reads the API key from the secret store and passes it in. This
// package never stores, logs, or returns a key, and never logs audio.
package providers

import (
	"errors"
	"log/slog"
	"strings"
	"time"

	"github.com/watzon/sayso/internal/config"
)

// Request is one recording to transcribe.
type Request struct {
	// Model is the provider's model id, for example "gpt-4o-transcribe".
	Model string
	// WAV is a complete 16 kHz mono 16-bit WAV file.
	WAV []byte
	// Language is an ISO 639-1 code. Empty lets the provider detect it.
	Language string
	// Vocabulary holds dictionary words, sent as the provider's hint.
	Vocabulary []string
	// Timeout is a deadline for the whole call, retries included. Zero means 30 s.
	Timeout time.Duration
}

// Transcribe runs the final pass on one recording and returns the trimmed
// transcript. A recording without speech gives "".
//
// The language and words are hints: when the provider answers HTTP 400 or
// 422 to a request with a hint, it goes out once more without the hints.
func Transcribe(provider config.SpeechProvider, apiKey string, request Request) (string, error) {
	t, err := newTarget(provider, apiKey, request.Model)
	if err != nil {
		return "", err
	}
	deadline := newDeadline(request.Timeout)
	hasHints := hintLanguage(request) != "" || len(hintWords(request)) > 0
	text, err := send(t, request, deadline)
	var se *config.SpeechError
	if err != nil && hasHints && errors.As(err, &se) && se.Kind == config.SpeechHTTP && (se.Status == 400 || se.Status == 422) {
		slog.Debug("speech: hints refused, sending again without them", "provider", provider.ID, "status", se.Status)
		retry := request
		retry.Language = ""
		retry.Vocabulary = nil
		text, err = send(t, retry, deadline)
	}
	if err != nil {
		return "", err
	}
	slog.Debug("speech: transcribed", "provider", provider.ID, "model", request.Model, "ms", deadline.elapsedMs())
	return strings.TrimSpace(text), nil
}

func send(t *target, request Request, d *deadline) (string, error) {
	switch t.kind {
	case config.SpeechElevenLabs:
		return elevenLabsTranscribe(t, request, d)
	case config.SpeechDeepgram:
		return deepgramTranscribe(t, request, d)
	case config.SpeechAssemblyAI:
		return assemblyTranscribe(t, request, d)
	}
	return openAITranscribe(t, request, d)
}

// Check proves that the address and the key work with a cheap request.
func Check(provider config.SpeechProvider, apiKey string, timeout time.Duration) error {
	t, err := newTarget(provider, apiKey, defaultModel(provider))
	if err != nil {
		return err
	}
	d := newDeadline(timeout)
	switch provider.Kind {
	case config.SpeechElevenLabs:
		return elevenLabsCheck(t, d)
	case config.SpeechDeepgram:
		return deepgramCheck(t, d)
	case config.SpeechAssemblyAI:
		return assemblyCheck(t, d)
	}
	return openAICheck(t, d)
}

// ListModels returns the model ids the server reports (GET {base}/models),
// sorted. Kinds without such a list return their known ids.
func ListModels(provider config.SpeechProvider, apiKey string, timeout time.Duration) ([]string, error) {
	switch provider.Kind {
	case config.SpeechOpenAI, config.SpeechGroq, config.SpeechMistral, config.SpeechOpenAICompatible:
		t, err := newTarget(provider, apiKey, "")
		if err != nil {
			return nil, err
		}
		return openAIListModels(t, newDeadline(timeout))
	}
	known := provider.Kind.KnownModels()
	ids := make([]string, len(known))
	for i, m := range known {
		ids[i] = m.ID
	}
	return ids, nil
}

func defaultModel(p config.SpeechProvider) string {
	if known := p.Kind.KnownModels(); len(known) > 0 {
		return known[0].ID
	}
	if len(p.Models) > 0 {
		return p.Models[0]
	}
	return ""
}
