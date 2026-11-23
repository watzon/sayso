package main

/*
#define SAYSO_CORE_NO_PROTOTYPES
#include "sayso_core.h"
*/
import "C"

import (
	"context"
	"errors"
	"path/filepath"
	"strconv"
	"strings"
	"time"
	"unsafe"

	"github.com/watzon/sayso/mobile/core/cloud"
	"github.com/watzon/sayso/mobile/core/engine"
	"github.com/watzon/sayso/mobile/core/voice"
)

// The voice functions: post-processing, styles, cloud calls and history.
// Cloud calls hold the session only to read what they need, never during
// the network call, so the keyboard's calls and destroy do not wait on it.

// voicePath is the voice store's file next to the typing journal, or ""
// (memory only) when the engine keeps no journal.
func voicePath(journal string) string {
	if journal == "" {
		return ""
	}
	return filepath.Join(filepath.Dir(journal), "voice.journal")
}

// maxPromptWords bounds the dictionary words sent as a transcription hint.
const maxPromptWords = 50

// sizedText is the size rule of the list functions for one text: it
// returns the bytes text needs and writes it (with a NUL when there is
// room) only when it fits.
func sizedText(buf *C.char, cap C.int32_t, text string) C.int32_t {
	if buf != nil && len(text) <= int(cap) {
		writeText(buf, cap, text)
	}
	return C.int32_t(len(text))
}

func listArgs(s *session, buf *C.char, cap C.int32_t) C.int32_t {
	if cap < 0 || (buf == nil && cap > 0) {
		return s.fail(C.SAYSO_ERR_ARG, errors.New("bad buffer"))
	}
	return C.SAYSO_OK
}

//export sayso_post_process_transcript
func sayso_post_process_transcript(h C.sayso_engine, raw *C.char, rawLen C.int32_t, before *C.char, beforeLen C.int32_t,
	after *C.char, afterLen C.int32_t, out *C.char, cap C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	r, ok1 := goString(raw, rawLen)
	b, ok2 := goString(before, beforeLen)
	a, ok3 := goString(after, afterLen)
	if !ok1 || !ok2 || !ok3 {
		return s.fail(C.SAYSO_ERR_ARG, errors.New("bad text"))
	}
	if rc := listArgs(s, out, cap); rc != C.SAYSO_OK {
		return rc
	}
	return sizedText(out, cap, voice.PostProcess(r, b, a, s.e.VoiceDictionary()))
}

//export sayso_list_styles
func sayso_list_styles(h C.sayso_engine, buf *C.char, cap C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	if rc := listArgs(s, buf, cap); rc != C.SAYSO_OK {
		return rc
	}
	var fields []string
	for _, st := range s.voice().Styles() {
		b := "0"
		if st.BuiltIn {
			b = "1"
		}
		fields = append(fields, st.ID, st.Name, st.Prompt, b)
	}
	return writeList(buf, cap, fields)
}

//export sayso_put_style
func sayso_put_style(h C.sayso_engine, id *C.char, idLen C.int32_t, name *C.char, nameLen C.int32_t, prompt *C.char, promptLen C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	i, ok1 := goString(id, idLen)
	n, ok2 := goString(name, nameLen)
	p, ok3 := goString(prompt, promptLen)
	if !ok1 || !ok2 || !ok3 {
		return s.fail(C.SAYSO_ERR_ARG, errors.New("bad style"))
	}
	if err := s.voice().PutStyle(voice.Style{ID: i, Name: n, Prompt: p}); err != nil {
		return s.fail(C.SAYSO_ERR_ARG, err)
	}
	return C.SAYSO_OK
}

//export sayso_remove_style
func sayso_remove_style(h C.sayso_engine, id *C.char, idLen C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	i, ok := goString(id, idLen)
	if !ok {
		return s.fail(C.SAYSO_ERR_ARG, errors.New("bad style id"))
	}
	if err := s.voice().RemoveStyle(i); err != nil {
		return s.fail(C.SAYSO_ERR_ARG, err)
	}
	return C.SAYSO_OK
}

// cloudConfig reads a sayso_cloud and the time the call may take.
func cloudConfig(c *C.sayso_cloud) (cloud.Config, time.Duration, bool) {
	if c == nil {
		return cloud.Config{}, 0, false
	}
	key, ok1 := goString(c.api_key, c.api_key_len)
	base, ok2 := goString(c.base_url, c.base_url_len)
	model, ok3 := goString(c.model, c.model_len)
	timeout := time.Duration(c.timeout_ms) * time.Millisecond
	if c.timeout_ms <= 0 {
		timeout = C.SAYSO_DEFAULT_TIMEOUT_MS * time.Millisecond
	}
	return cloud.Config{Provider: int(c.provider), APIKey: key, BaseURL: base, Model: model}, min(timeout, 120*time.Second), ok1 && ok2 && ok3
}

// voiceOut checks the buffers of a cloud call.
func voiceOut(out *C.char, cap C.int32_t, outLen *C.int32_t) bool {
	return out != nil && cap >= C.SAYSO_VOICE_TEXT_CAP && outLen != nil
}

// writeVoiceText writes text, cut on a rune boundary to fit.
func writeVoiceText(out *C.char, cap C.int32_t, outLen *C.int32_t, text string) {
	if len(text) >= int(cap) {
		text = cutRunes(text, int(cap)-1)
	}
	*outLen = C.int32_t(writeText(out, cap, text))
}

func cutRunes(s string, n int) string {
	for n > 0 && n < len(s) && s[n]&0xC0 == 0x80 {
		n--
	}
	return s[:n]
}

//export sayso_transcribe_cloud
func sayso_transcribe_cloud(h C.sayso_engine, cfg *C.sayso_cloud, pcm *C.int16_t, samples C.int32_t,
	out *C.char, cap C.int32_t, outLen *C.int32_t, errBuf *C.char, errCap C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	c, timeout, ok := cloudConfig(cfg)
	if !ok || samples < 0 || (pcm == nil && samples > 0) || !voiceOut(out, cap, outLen) {
		s.release()
		return s.fail(C.SAYSO_ERR_ARG, errors.New("bad arguments"))
	}
	words := s.e.VoiceDictionary().Words
	s.release()
	audio := unsafe.Slice((*int16)(unsafe.Pointer(pcm)), int(samples))
	*outLen = 0
	if len(words) > maxPromptWords {
		words = words[:maxPromptWords]
	}
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	text, err := cloud.Transcribe(ctx, c, audio, strings.Join(words, ", "))
	if err != nil {
		writeText(errBuf, errCap, err.Error())
		return C.SAYSO_ERR_CLOUD
	}
	writeText(errBuf, errCap, "")
	writeVoiceText(out, cap, outLen, text)
	return C.SAYSO_OK
}

//export sayso_enhance
func sayso_enhance(h C.sayso_engine, cfg *C.sayso_cloud, styleID *C.char, styleLen C.int32_t, text *C.char, textLen C.int32_t,
	out *C.char, cap C.int32_t, outLen *C.int32_t, errBuf *C.char, errCap C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	c, timeout, ok1 := cloudConfig(cfg)
	id, ok2 := goString(styleID, styleLen)
	t, ok3 := goString(text, textLen)
	if !ok1 || !ok2 || !ok3 || !voiceOut(out, cap, outLen) {
		s.release()
		return s.fail(C.SAYSO_ERR_ARG, errors.New("bad arguments"))
	}
	st, known := s.voice().Style(id)
	dict := s.e.VoiceDictionary()
	s.release()

	notEnhanced := func(reason string) C.int32_t {
		writeText(errBuf, errCap, reason)
		writeVoiceText(out, cap, outLen, t)
		return 0
	}
	switch {
	case !known:
		return notEnhanced("unknown style " + strconv.Quote(id))
	case st.Prompt == "" || c.Provider == cloud.StyleOff || strings.TrimSpace(t) == "":
		return notEnhanced("")
	}
	vocabulary := dict.Words
	for _, r := range dict.Replacements {
		vocabulary = append(vocabulary, r.To)
	}
	system, user := voice.EnhanceRequest(st, vocabulary, t)
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	reply, err := cloud.Enhance(ctx, c, system, user)
	if err == nil {
		reply, err = voice.CleanReply(t, reply)
	}
	if err != nil {
		return notEnhanced(err.Error())
	}
	writeText(errBuf, errCap, "")
	writeVoiceText(out, cap, outLen, reply)
	return 1
}

//export sayso_history_add
func sayso_history_add(h C.sayso_engine, timestampMs, durationMs C.int64_t, engineID *C.char, engineLen C.int32_t,
	styleID *C.char, styleLen C.int32_t, raw *C.char, rawLen C.int32_t, final *C.char, finalLen C.int32_t,
	sensitive C.int32_t, outID *C.int64_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	eid, ok1 := goString(engineID, engineLen)
	sid, ok2 := goString(styleID, styleLen)
	r, ok3 := goString(raw, rawLen)
	f, ok4 := goString(final, finalLen)
	if !ok1 || !ok2 || !ok3 || !ok4 || outID == nil {
		return s.fail(C.SAYSO_ERR_ARG, errors.New("bad entry"))
	}
	*outID = 0
	if sensitive != 0 || s.e.Flags()&engine.FlagPrivacy != 0 {
		return C.SAYSO_OK // a private field: nothing is kept
	}
	*outID = C.int64_t(s.voice().AddEntry(voice.Entry{TimestampMs: int64(timestampMs), DurationMs: int64(durationMs),
		EngineID: eid, StyleID: sid, Raw: r, Final: f}))
	return C.SAYSO_OK
}

//export sayso_history_list
func sayso_history_list(h C.sayso_engine, limit C.int32_t, buf *C.char, cap C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	if rc := listArgs(s, buf, cap); rc != C.SAYSO_OK {
		return rc
	}
	var fields []string
	for _, e := range s.voice().Entries(int(limit)) {
		fields = append(fields, strconv.FormatInt(e.ID, 10), strconv.FormatInt(e.TimestampMs, 10), strconv.FormatInt(e.DurationMs, 10),
			e.EngineID, e.StyleID, e.Raw, e.Final)
	}
	return writeList(buf, cap, fields)
}

//export sayso_history_delete
func sayso_history_delete(h C.sayso_engine, id C.int64_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	s.voice().DeleteEntry(int64(id))
	return C.SAYSO_OK
}

//export sayso_history_clear
func sayso_history_clear(h C.sayso_engine) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	s.voice().ClearEntries()
	return C.SAYSO_OK
}

var _ = [1]struct{}{}[int(C.SAYSO_ASR_OFF)^cloud.ASROff|int(C.SAYSO_ASR_OPENAI_WHISPER)^cloud.ASROpenAIWhisper|
	int(C.SAYSO_ASR_GROQ)^cloud.ASRGroq|int(C.SAYSO_ASR_OPENAI_COMPATIBLE)^cloud.ASROpenAICompatible]
var _ = [1]struct{}{}[int(C.SAYSO_STYLE_OFF)^cloud.StyleOff|int(C.SAYSO_STYLE_OPENAI)^cloud.StyleOpenAI|
	int(C.SAYSO_STYLE_OPENAI_COMPATIBLE)^cloud.StyleOpenAICompatible|int(C.SAYSO_STYLE_ANTHROPIC)^cloud.StyleAnthropic]
