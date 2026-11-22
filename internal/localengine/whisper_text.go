package localengine

import "strings"

// whisperMinSamples: whisper.cpp ignores input shorter than one second, so
// shorter input is padded with silence to 1.25 s.
const whisperMinSamples = SampleRate + SampleRate/4

// whisperIsSilence: Whisper makes up words ("You") for silence. Near digital
// silence, far below the noise floor of a microphone (about 0.003 to 0.01),
// has no speech.
func whisperIsSilence(samples []float32) bool { return rms(samples) < 0.0005 }

// whisperPadded is samples, padded with silence to whisperMinSamples.
func whisperPadded(samples []float32) []float32 {
	input := append([]float32(nil), samples...)
	for len(input) < whisperMinSamples {
		input = append(input, 0)
	}
	return input
}

// whisperLanguage: auto (or nothing) lets Whisper detect the language, as an
// empty result. A region goes away: whisper.cpp takes "en", not "en-US".
func whisperLanguage(language string) string {
	code := strings.TrimSpace(language)
	if i := strings.IndexAny(code, "-_"); i >= 0 {
		code = code[:i]
	}
	code = strings.ToLower(code)
	if code == "auto" {
		return ""
	}
	return code
}

// whisperPrompt: dictionary words go in as the initial prompt, like the
// WhisperKit prompt tokens on macOS. Whisper has no hard boosting.
func whisperPrompt(vocabulary []string) string {
	var words []string
	for _, w := range vocabulary {
		if w = strings.TrimSpace(w); w != "" {
			words = append(words, w)
		}
	}
	return strings.Join(words, ", ")
}

// cleanWhisperText drops the [BLANK_AUDIO]-like tags that whisper.cpp writes
// for silence, keeps other bracketed text, and collapses white space.
func cleanWhisperText(text string) string {
	var out, tag strings.Builder
	depth := 0
	for _, c := range text {
		switch {
		case c == '[':
			depth++
			tag.Reset()
		case c == ']' && depth > 0:
			depth--
			known := true
			for _, t := range tag.String() {
				if !(t >= 'A' && t <= 'Z' || t == '_' || t == ' ') {
					known = false
					break
				}
			}
			if !known {
				out.WriteByte('[')
				out.WriteString(tag.String())
				out.WriteByte(']')
			}
		case depth > 0:
			tag.WriteRune(c)
		default:
			out.WriteRune(c)
		}
	}
	if depth > 0 {
		out.WriteByte('[')
		out.WriteString(tag.String())
	}
	return strings.Join(strings.Fields(out.String()), " ")
}
