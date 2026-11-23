package voice

import (
	"errors"
	"strings"
	"unicode/utf8"

	"github.com/watzon/sayso/mobile/core/styles"
)

// EnhanceRequest is the system prompt and user message that ask a model to
// rewrite text in style st. vocabulary is the user's terms; only those the
// text can contain go in.
func EnhanceRequest(st Style, vocabulary []string, text string) (system, user string) {
	return styles.SystemPrompt(st.Prompt, false, styles.DefaultBasePrompt, vocabulary, text), styles.UserMessage(text)
}

// CleanReply is a model's reply as the text to insert: trimmed, without the
// transcript tags a model may echo. An empty reply, or one far longer than
// the text (the model answered instead of editing), is an error.
func CleanReply(text, reply string) (string, error) {
	out := strings.TrimSpace(reply)
	out = strings.TrimSpace(strings.TrimSuffix(strings.TrimPrefix(out, "<transcript>"), "</transcript>"))
	if out == "" {
		return "", errors.New("the reply is empty")
	}
	if utf8.RuneCountInString(out) > 3*utf8.RuneCountInString(text)+200 {
		return "", errors.New("the reply is much longer than the text")
	}
	return out, nil
}
