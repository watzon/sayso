package localengine

import "strings"

// isAutoLanguage reports whether the request lets the model find the
// language: auto or an empty language.
func isAutoLanguage(requested string) bool {
	code := strings.TrimSpace(requested)
	return code == "" || strings.EqualFold(code, "auto")
}

// normalizeLanguage is a language code as the catalog has it, from the form
// that a library gives. SenseVoice gives <|en|>, Whisper gives en. Empty for
// an empty value.
func normalizeLanguage(raw string) string {
	code := strings.TrimSpace(raw)
	for strings.HasPrefix(code, "<|") {
		code = code[2:]
	}
	for strings.HasSuffix(code, "|>") {
		code = code[:len(code)-2]
	}
	return strings.ToLower(strings.TrimSpace(code))
}

// languageTally counts the languages of the pieces of one recording, by the
// audio of each piece.
type languageTally struct {
	codes   []string
	samples []int
}

// add counts a piece. A piece with no text or no language does not count.
func (t *languageTally) add(samples int, text, language string) {
	if language == "" || strings.TrimSpace(text) == "" {
		return
	}
	for i, code := range t.codes {
		if code == language {
			t.samples[i] += samples
			return
		}
	}
	t.codes = append(t.codes, language)
	t.samples = append(t.samples, samples)
}

// mostAudio is the language of the most audio. With equal audio, the language
// that came first.
func (t *languageTally) mostAudio() string {
	best := -1
	for i := range t.codes {
		if best < 0 || t.samples[i] > t.samples[best] {
			best = i
		}
	}
	if best < 0 {
		return ""
	}
	return t.codes[best]
}

// segment is one update from a streaming model.
type segment struct {
	// text is the text of the current segment.
	text string
	// endpoint is true when the segment ended. The next update starts a new one.
	endpoint bool
}

// partials keeps the text of a live preview: committed words and the
// tentative tail. At an endpoint the segment is done and moves to committed.
type partials struct {
	committed, tentative string
	sentCommitted        string
	sentTentative        string
}

func (p *partials) update(s segment) {
	text := strings.TrimSpace(s.text)
	if s.endpoint {
		if text != "" {
			if p.committed != "" {
				p.committed += " "
			}
			p.committed += text
		}
		p.tentative = ""
	} else {
		p.tentative = text
	}
}

// changed returns the committed and tentative text when it differs from the
// last call that returned something.
func (p *partials) changed() (committed, tentative string, ok bool) {
	if p.sentCommitted == p.committed && p.sentTentative == p.tentative {
		return "", "", false
	}
	p.sentCommitted, p.sentTentative = p.committed, p.tentative
	return p.committed, p.tentative, true
}

// text is all text, committed and tentative.
func (p *partials) text() string {
	switch {
	case p.tentative == "":
		return p.committed
	case p.committed == "":
		return p.tentative
	default:
		return p.committed + " " + p.tentative
	}
}
