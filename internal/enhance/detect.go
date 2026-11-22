package enhance

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"slices"
	"strings"
	"time"
)

// Detection for onboarding and Settings › Providers (sayso-enhance
// detect.rs): a local Ollama, the models of an OpenAI-compatible server,
// and the test call of a provider.

// OllamaURL is where Ollama listens by default.
const OllamaURL = "http://localhost:11434"

// DetectOllama asks a local Ollama for its models, sorted. ok is false when
// Ollama does not answer within timeout.
func DetectOllama(timeout time.Duration) (names []string, ok bool) {
	return DetectOllamaAt(OllamaURL, timeout)
}

// DetectOllamaAt is DetectOllama for a server at baseURL.
func DetectOllamaAt(baseURL string, timeout time.Duration) ([]string, bool) {
	body, status, err := get(strings.TrimRight(baseURL, "/")+"/api/tags", "", timeout)
	if err != nil || status >= 400 {
		return nil, false
	}
	var tags struct {
		Models []struct {
			Name string `json:"name"`
		} `json:"models"`
	}
	if json.Unmarshal(body, &tags) != nil {
		return nil, false
	}
	names := []string{}
	for _, m := range tags.Models {
		if m.Name != "" {
			names = append(names, m.Name)
		}
	}
	slices.Sort(names)
	return names, true
}

// ListModels lists the models of an OpenAI-compatible server: GET
// {baseURL}/models. It works for OpenAI, OpenRouter, Ollama (/v1), LM Studio
// and llama.cpp. The list is sorted by id, without repeats.
func ListModels(baseURL, apiKey string, timeout time.Duration) ([]ModelChoice, error) {
	body, status, err := get(strings.TrimRight(baseURL, "/")+"/models", apiKey, timeout)
	if err != nil {
		return nil, mapTransport(err, uint64(timeout.Milliseconds()))
	}
	if status >= 400 {
		return nil, &Error{Kind: ErrHTTP, Status: status, Message: firstRunes(string(body), 500)}
	}
	return ParseModelList(body)
}

// ParseModelList reads a {"data": [{"id": ..., "name": ...}]} body. The name
// is the entry's "name" when it has one, else its id. Ollama with no models
// sends "data": null, an empty list.
func ParseModelList(body []byte) ([]ModelChoice, error) {
	var v struct {
		Data *json.RawMessage `json:"data"`
	}
	if err := json.Unmarshal(body, &v); err != nil {
		return nil, invalidOutput("model list is not JSON: " + err.Error())
	}
	if v.Data == nil {
		if strings.Contains(string(body), `"data"`) {
			return []ModelChoice{}, nil
		}
		return nil, invalidOutput(`model list has no "data" array`)
	}
	var entries []struct {
		ID   string `json:"id"`
		Name string `json:"name"`
	}
	if string(*v.Data) == "null" {
		return []ModelChoice{}, nil
	}
	if err := json.Unmarshal(*v.Data, &entries); err != nil {
		return nil, invalidOutput(`model list has no "data" array`)
	}
	out := []ModelChoice{}
	for _, e := range entries {
		if e.ID == "" {
			continue
		}
		name := e.Name
		if name == "" {
			name = e.ID
		}
		out = append(out, ModelChoice{ID: e.ID, Name: name})
	}
	slices.SortStableFunc(out, func(a, b ModelChoice) int { return strings.Compare(a.ID, b.ID) })
	return slices.CompactFunc(out, func(a, b ModelChoice) bool { return a.ID == b.ID }), nil
}

// TestProvider makes one tiny call for the "Test connection" button and
// returns how long it took. It makes a real request, so tests give it a fake.
func TestProvider(e Enhancer) (uint64, error) {
	zero := float32(0)
	start := time.Now()
	r, err := e.Enhance(Request{
		SystemPrompt: `Repeat the transcript exactly as the "text" field.`,
		Transcript:   "Testing one two three.",
		Temperature:  &zero,
		Timeout:      20 * time.Second,
	})
	if err != nil {
		return 0, err
	}
	if r.ElapsedMs > 0 {
		return r.ElapsedMs, nil
	}
	return uint64(time.Since(start).Milliseconds()), nil
}

func get(url, apiKey string, timeout time.Duration) ([]byte, int, error) {
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return nil, 0, fmt.Errorf("bad URL: %w", err)
	}
	if apiKey != "" {
		req.Header.Set("Authorization", "Bearer "+apiKey)
	}
	resp, err := httpClientFor(url).Do(req)
	if err != nil {
		return nil, 0, err
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(io.LimitReader(resp.Body, 8<<20))
	return body, resp.StatusCode, err
}
