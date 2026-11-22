package providers

import (
	"encoding/binary"
	"encoding/json"
	"fmt"
	"sort"
	"strings"
	"sync/atomic"
	"unicode/utf8"

	"github.com/watzon/sayso/internal/config"
)

// ---- multipart ----

var boundaryCounter atomic.Uint64

// multipart is a small multipart/form-data body builder.
type multipart struct {
	boundary string
	body     []byte
}

func newMultipart() *multipart {
	n := boundaryCounter.Add(1) - 1
	return &multipart{boundary: fmt.Sprintf("----SaysoFormBoundary7d3f9a1c5e%08x", n)}
}

func (m *multipart) contentType() string { return "multipart/form-data; boundary=" + m.boundary }

func (m *multipart) text(name, value string) {
	m.begin(name, "", "")
	m.body = append(m.body, value...)
	m.body = append(m.body, "\r\n"...)
}

func (m *multipart) part(name, contentType string, data []byte) {
	m.begin(name, "", contentType)
	m.body = append(m.body, data...)
	m.body = append(m.body, "\r\n"...)
}

func (m *multipart) file(name, filename, contentType string, data []byte) {
	m.begin(name, filename, contentType)
	m.body = append(m.body, data...)
	m.body = append(m.body, "\r\n"...)
}

func (m *multipart) finish() []byte {
	return append(m.body, "--"+m.boundary+"--\r\n"...)
}

func (m *multipart) begin(name, filename, contentType string) {
	head := "--" + m.boundary + "\r\nContent-Disposition: form-data; name=\"" + quoted(name) + "\""
	if filename != "" {
		head += "; filename=\"" + quoted(filename) + "\""
	}
	head += "\r\n"
	if contentType != "" {
		head += "Content-Type: " + contentType + "\r\n"
	}
	head += "\r\n"
	m.body = append(m.body, head...)
}

func quoted(v string) string {
	v = strings.NewReplacer("\r", "", "\n", "").Replace(v)
	return strings.ReplaceAll(v, "\"", "%22")
}

// ---- OpenAI-compatible: OpenAI, Groq, Mistral, custom ----

const (
	promptLimit      = 800
	contextBiasLimit = 100
	gptTranscribe    = "gpt-transcribe"
)

func bearer(t *target) []header {
	if t.key == "" {
		return nil
	}
	return []header{{"Authorization", "Bearer " + t.key}}
}

func firstString(v any, ptrs ...string) string {
	for _, p := range ptrs {
		if s, ok := str(v, p); ok {
			return s
		}
	}
	return ""
}

func openAIError(v any) string { return firstString(v, "/error/message", "/message", "/detail", "/error") }

func openAITranscribe(t *target, r Request, d *deadline) (string, error) {
	form := openAIForm(t.kind, r)
	ct := form.contentType()
	rep, err := t.post(t.url("/audio/transcriptions"), bearer(t), ct, form.finish(), d)
	if err != nil {
		return "", err
	}
	v, err := rep.intoJSON(openAIError)
	if err != nil {
		return "", err
	}
	return textAt(v, "/text")
}

func openAIForm(kind config.SpeechProviderKind, r Request) *multipart {
	form := newMultipart()
	form.file("file", "audio.wav", "audio/wav", r.WAV)
	form.text("model", r.Model)
	form.text("response_format", "json")
	gpt := kind == config.SpeechOpenAI && r.Model == gptTranscribe
	if code := hintLanguage(r); code != "" {
		if gpt {
			form.text("languages[]", code)
		} else {
			form.text("language", code)
		}
	}
	words := hintWords(r)
	if len(words) == 0 {
		return form
	}
	switch {
	case gpt:
		var lines []string
		for _, w := range words {
			w = strings.TrimSpace(strings.Map(func(c rune) rune {
				if c == '<' || c == '>' || c == '\r' || c == '\n' {
					return -1
				}
				return c
			}, w))
			if w != "" {
				lines = append(lines, w)
			}
		}
		if len(lines) > 0 {
			form.text("keywords", strings.Join(lines, "\n"))
		}
	case kind == config.SpeechMistral:
		for i, w := range words {
			if i == contextBiasLimit {
				break
			}
			form.text("context_bias", w)
		}
	default:
		if p := truncatePrompt(words, promptLimit); p != "" {
			form.text("prompt", p)
		}
	}
	return form
}

// truncatePrompt joins words with ", " and stops before the first word that
// would push the text over limit characters.
func truncatePrompt(words []string, limit int) string {
	var b strings.Builder
	length := 0
	for _, w := range words {
		added := utf8.RuneCountInString(w)
		if b.Len() > 0 {
			added += 2
		}
		if length+added > limit {
			break
		}
		if b.Len() > 0 {
			b.WriteString(", ")
		}
		b.WriteString(w)
		length += added
	}
	return b.String()
}

func openAIModels(t *target, d *deadline) (*reply, error) {
	rep, err := t.get(t.url("/models"), bearer(t), d)
	if err != nil {
		return nil, err
	}
	if !rep.isSuccess() {
		return nil, rep.errorWith(openAIError)
	}
	return rep, nil
}

func openAICheck(t *target, d *deadline) error {
	_, err := openAIModels(t, d)
	return err
}

func openAIListModels(t *target, d *deadline) ([]string, error) {
	rep, err := openAIModels(t, d)
	if err != nil {
		return nil, err
	}
	v, err := rep.intoJSON(openAIError)
	if err != nil {
		return nil, err
	}
	data, ok := pointer(v, "/data").([]any)
	if !ok {
		return nil, &config.SpeechError{Kind: config.SpeechInvalidResponse, Message: "the body has no `data` list"}
	}
	seen := map[string]bool{}
	ids := []string{}
	for _, m := range data {
		if id, ok := str(m, "/id"); ok && !seen[id] {
			seen[id] = true
			ids = append(ids, id)
		}
	}
	sort.Strings(ids)
	return ids, nil
}

// ---- ElevenLabs ----

const (
	keytermMaxChars = 50
	keytermLimit    = 100
)

func elevenAuth(t *target) []header {
	if t.key == "" {
		return nil
	}
	return []header{{"xi-api-key", t.key}}
}

func elevenError(v any) string {
	switch detail := pointer(v, "/detail").(type) {
	case map[string]any:
		s, _ := detail["message"].(string)
		return s
	case string:
		return detail
	case []any:
		s, _ := str(detail, "/0/msg")
		return s
	}
	return ""
}

func elevenLabsTranscribe(t *target, r Request, d *deadline) (string, error) {
	form := newMultipart()
	form.file("file", "audio.wav", "audio/wav", r.WAV)
	form.text("model_id", r.Model)
	form.text("tag_audio_events", "false")
	form.text("diarize", "false")
	if code := hintLanguage(r); code != "" {
		form.text("language_code", code)
	}
	n := 0
	for _, w := range hintWords(r) {
		if utf8.RuneCountInString(w) >= keytermMaxChars {
			continue
		}
		if n == keytermLimit {
			break
		}
		form.text("keyterms", w)
		n++
	}
	ct := form.contentType()
	rep, err := t.post(t.url("/speech-to-text"), elevenAuth(t), ct, form.finish(), d)
	if err != nil {
		return "", err
	}
	v, err := rep.intoJSON(elevenError)
	if err != nil {
		return "", err
	}
	return textAt(v, "/text")
}

func elevenLabsCheck(t *target, d *deadline) error {
	rep, err := t.get(t.url("/models"), elevenAuth(t), d)
	if err != nil {
		return err
	}
	if !rep.isSuccess() {
		return rep.errorWith(elevenError)
	}
	return nil
}

// ---- Deepgram ----

const keytermBudgetChars = 400

func deepgramAuth(t *target) []header {
	if t.key == "" {
		return nil
	}
	return []header{{"Authorization", "Token " + t.key}}
}

func deepgramError(v any) string { return firstString(v, "/err_msg", "/message") }

func listenURL(t *target, r Request) string {
	u := t.url("/listen") + "?model=" + percentEncode(r.Model) + "&smart_format=true&punctuate=true"
	if code := hintLanguage(r); code != "" {
		u += "&language=" + percentEncode(code)
	} else {
		u += "&detect_language=true"
	}
	if strings.HasPrefix(r.Model, "nova-3") {
		used := 0
		for _, w := range hintWords(r) {
			used += utf8.RuneCountInString(w)
			if used > keytermBudgetChars {
				break
			}
			u += "&keyterm=" + percentEncode(w)
		}
	}
	return u
}

func deepgramTranscribe(t *target, r Request, d *deadline) (string, error) {
	rep, err := t.post(listenURL(t, r), deepgramAuth(t), "audio/wav", r.WAV, d)
	if err != nil {
		return "", err
	}
	v, err := rep.intoJSON(deepgramError)
	if err != nil {
		return "", err
	}
	return textAt(v, "/results/channels/0/alternatives/0/transcript")
}

func deepgramCheck(t *target, d *deadline) error {
	rep, err := t.get(t.url("/projects"), deepgramAuth(t), d)
	if err != nil {
		return err
	}
	if !rep.isSuccess() {
		return rep.errorWith(deepgramError)
	}
	return nil
}

// percentEncode keeps letters, digits, '-', '.', '_', and '~'.
func percentEncode(v string) string {
	var b strings.Builder
	for i := 0; i < len(v); i++ {
		c := v[i]
		if c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '-' || c == '.' || c == '_' || c == '~' {
			b.WriteByte(c)
		} else {
			fmt.Fprintf(&b, "%%%02X", c)
		}
	}
	return b.String()
}

// ---- AssemblyAI ----

func assemblyHeaders(t *target) []header {
	h := []header{{"X-AAI-Model", t.model}}
	if t.key != "" {
		h = append(h, header{"Authorization", t.key})
	}
	return h
}

func assemblyError(v any) string { return firstString(v, "/error", "/detail", "/error_code") }

// assemblyConfig is the JSON of the config part, with the hints that are set.
func assemblyConfig(r Request) []byte {
	cfg := map[string]any{}
	if code := hintLanguage(r); code != "" {
		cfg["language_codes"] = []string{code}
	}
	words := hintWords(r)
	if len(words) > keytermLimit {
		words = words[:keytermLimit]
	}
	if len(words) > 0 {
		cfg["keyterms_prompt"] = words
	}
	b, _ := json.Marshal(cfg) // keys sorted, as serde_json's map
	return b
}

func assemblySend(t *target, r Request, d *deadline) (*reply, error) {
	form := newMultipart()
	form.part("config", "application/json", assemblyConfig(r))
	form.file("audio", "audio.wav", "audio/wav", r.WAV)
	ct := form.contentType()
	return t.post(t.url("/transcribe"), assemblyHeaders(t), ct, form.finish(), d)
}

func assemblyTranscribe(t *target, r Request, d *deadline) (string, error) {
	rep, err := assemblySend(t, r, d)
	if err != nil {
		return "", err
	}
	v, err := rep.intoJSON(assemblyError)
	if err != nil {
		return "", err
	}
	return textAt(v, "/text")
}

// assemblyCheck sends half a second of silence: the key works when the answer
// is 2xx or when the service says the audio is bad.
func assemblyCheck(t *target, d *deadline) error {
	rep, err := assemblySend(t, Request{Model: t.model, WAV: SilentWAV(500)}, d)
	if err != nil {
		return err
	}
	if rep.isSuccess() {
		return nil
	}
	if rep.status == 400 || rep.status == 422 {
		if v, ok := rep.json(); ok {
			if code, _ := str(v, "/error_code"); code == "audio_too_short" || code == "bad_audio" {
				return nil
			}
		}
	}
	return rep.errorWith(assemblyError)
}

// SilentWAV is a WAV file of silence: 16 kHz, mono, 16-bit.
func SilentWAV(millis uint32) []byte {
	const rate = 16000
	dataLen := uint32(rate / 1000 * millis * 2)
	wav := make([]byte, 44+dataLen)
	copy(wav, "RIFF")
	binary.LittleEndian.PutUint32(wav[4:], 36+dataLen)
	copy(wav[8:], "WAVEfmt ")
	binary.LittleEndian.PutUint32(wav[16:], 16)
	binary.LittleEndian.PutUint16(wav[20:], 1)
	binary.LittleEndian.PutUint16(wav[22:], 1)
	binary.LittleEndian.PutUint32(wav[24:], rate)
	binary.LittleEndian.PutUint32(wav[28:], rate*2)
	binary.LittleEndian.PutUint16(wav[32:], 2)
	binary.LittleEndian.PutUint16(wav[34:], 16)
	copy(wav[36:], "data")
	binary.LittleEndian.PutUint32(wav[40:], dataLen)
	return wav
}
