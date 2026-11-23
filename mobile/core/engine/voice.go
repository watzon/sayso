package engine

import (
	"github.com/watzon/sayso/mobile/core/lexicon"
	"github.com/watzon/sayso/mobile/core/textrules"
	"github.com/watzon/sayso/mobile/core/voice"
)

// VoiceDictionary is what post-processing a transcript reads from the
// keyboard: the words and replacements the user added, the model's
// spellings, and whether the field capitalizes sentence starts. It is a
// copy; Spelling reads only the read-only model, so it may be used without
// the engine's lock.
func (e *Engine) VoiceDictionary() voice.Dictionary {
	e.mu.Lock()
	defer e.mu.Unlock()
	d := voice.Dictionary{Words: e.learn.AddedWords(), Autocap: e.flags&FlagAutocap != 0}
	for i, r := range e.learn.Replacements() {
		d.Replacements = append(d.Replacements, textrules.Rule{ID: int64(i), From: r.From, To: r.To, CaseSensitive: r.CaseSensitive})
	}
	m := e.model
	d.Spelling = func(word string) (string, bool) { return modelSpelling(m, word) }
	return d
}

// modelSpelling is the most probable spelling of word in m, ignoring case.
func modelSpelling(m *lexicon.Model, word string) (string, bool) {
	id, ok := m.Find([]byte(word), true)
	if !ok {
		return "", false
	}
	return string(m.Word(id)), true
}
