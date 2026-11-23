// Command capi is Sayso's mobile typing core as a C library: built with
// -buildmode=c-shared for Android and -buildmode=c-archive for iOS. The ABI
// is sayso_core.h; API.md describes it. No Go pointer crosses it: engines
// are integer handles, and text goes into buffers the caller owns.
package main

/*
#cgo CFLAGS: -I${SRCDIR}
#define SAYSO_CORE_NO_PROTOTYPES
#include "sayso_core.h"
*/
import "C"

import (
	"errors"
	"os"
	"runtime"
	"runtime/debug"
	"sync"
	"unsafe"

	"github.com/watzon/sayso/mobile/core/decoder"
	"github.com/watzon/sayso/mobile/core/engine"
	"github.com/watzon/sayso/mobile/core/lexicon"
	"github.com/watzon/sayso/mobile/core/touch"
)

// Version names the core and the model format it reads.
const Version = "sayso-core 0.1.0 (sslm v1)"

// memoryLimit is the Go heap's soft limit. The model is mapped, not on the
// heap; iOS keyboard extensions may be killed near 48 MB in total.
const memoryLimit = 24 << 20

var versionC = C.CString(Version) // lives as long as the process

func init() {
	runtime.GOMAXPROCS(2)
	if os.Getenv("GOMEMLIMIT") == "" {
		debug.SetMemoryLimit(memoryLimit)
	}
}

func main() {}

// A session is one engine and the model it maps. Every call holds mu for
// reading while it uses them; destroy takes it for writing, so it waits for
// calls in flight before it unmaps the model, and calls that come after see
// closed. The engine serializes the calls themselves.
type session struct {
	e *engine.Engine
	m *lexicon.Model

	mu     sync.RWMutex
	closed bool

	errMu   sync.Mutex
	lastErr string
}

var (
	sessionsMu sync.RWMutex
	sessions           = map[uintptr]*session{}
	nextID     uintptr = 1
	createMu   sync.Mutex
	createErr  string
)

func lookup(h C.sayso_engine) *session {
	sessionsMu.RLock()
	s := sessions[uintptr(h)]
	sessionsMu.RUnlock()
	return s
}

// acquire returns the live session for h, held until release, or nil when
// h is unknown or destroyed.
func acquire(h C.sayso_engine) *session {
	s := lookup(h)
	if s == nil {
		return nil
	}
	s.mu.RLock()
	if s.closed {
		s.mu.RUnlock()
		return nil
	}
	return s
}

func (s *session) release() { s.mu.RUnlock() }

func (s *session) fail(code C.int32_t, err error) C.int32_t {
	s.errMu.Lock()
	s.lastErr = err.Error()
	s.errMu.Unlock()
	return code
}

func goString(p *C.char, n C.int32_t) (string, bool) {
	if n < 0 || (p == nil && n > 0) || n > 1<<20 {
		return "", false
	}
	if n == 0 {
		return "", true
	}
	return C.GoStringN(p, C.int(n)), true
}

// writeText copies s into a caller buffer of cap bytes, NUL-terminated
// when there is room, and returns the bytes written (s may be cut short).
func writeText(buf *C.char, cap C.int32_t, s string) int {
	if buf == nil || cap <= 0 {
		return 0
	}
	dst := unsafe.Slice((*byte)(unsafe.Pointer(buf)), int(cap))
	n := copy(dst, s)
	if n < len(dst) {
		dst[n] = 0
	}
	return n
}

//export sayso_version
func sayso_version() *C.char { return versionC }

//export sayso_last_error
func sayso_last_error(h C.sayso_engine, buf *C.char, cap C.int32_t) C.int32_t {
	var msg string
	if h == 0 {
		createMu.Lock()
		msg = createErr
		createMu.Unlock()
	} else if s := lookup(h); s != nil {
		s.errMu.Lock()
		msg = s.lastErr
		s.errMu.Unlock()
	}
	writeText(buf, cap, msg)
	return C.int32_t(len(msg))
}

func failCreate(code C.int32_t, err error) C.int32_t {
	createMu.Lock()
	createErr = err.Error()
	createMu.Unlock()
	return code
}

func register(m *lexicon.Model, journal string, out *C.sayso_engine) C.int32_t {
	e, err := engine.New(m, journal)
	if err != nil {
		m.Close()
		return failCreate(C.SAYSO_ERR_IO, err)
	}
	sessionsMu.Lock()
	id := nextID
	nextID++
	sessions[id] = &session{e: e, m: m}
	sessionsMu.Unlock()
	*out = C.sayso_engine(id)
	return C.SAYSO_OK
}

func modelErr(err error) C.int32_t {
	if errors.Is(err, lexicon.ErrFormat) {
		return failCreate(C.SAYSO_ERR_MODEL, err)
	}
	return failCreate(C.SAYSO_ERR_IO, err)
}

//export sayso_engine_create
func sayso_engine_create(modelPath *C.char, modelLen C.int32_t, journal *C.char, journalLen C.int32_t, out *C.sayso_engine) C.int32_t {
	path, ok1 := goString(modelPath, modelLen)
	jp, ok2 := goString(journal, journalLen)
	if !ok1 || !ok2 || out == nil || path == "" {
		return failCreate(C.SAYSO_ERR_ARG, errors.New("bad argument"))
	}
	m, err := lexicon.Open(path)
	if err != nil {
		return modelErr(err)
	}
	return register(m, jp, out)
}

//export sayso_engine_create_fd
func sayso_engine_create_fd(fd C.int32_t, offset, length C.int64_t, journal *C.char, journalLen C.int32_t, out *C.sayso_engine) C.int32_t {
	jp, ok := goString(journal, journalLen)
	if !ok || out == nil || fd < 0 || offset < 0 || length <= 0 {
		return failCreate(C.SAYSO_ERR_ARG, errors.New("bad argument"))
	}
	m, err := lexicon.OpenFD(int(fd), int64(offset), int64(length))
	if err != nil {
		return modelErr(err)
	}
	return register(m, jp, out)
}

//export sayso_engine_destroy
func sayso_engine_destroy(h C.sayso_engine) {
	sessionsMu.Lock()
	s := sessions[uintptr(h)]
	delete(sessions, uintptr(h))
	sessionsMu.Unlock()
	if s == nil {
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if !s.closed {
		s.closed = true
		s.e.Close()
		s.m.Close()
	}
}

//export sayso_set_layout
func sayso_set_layout(h C.sayso_engine, keys *C.sayso_key, count C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	if keys == nil || count <= 0 || count > touch.MaxKeys {
		return s.fail(C.SAYSO_ERR_ARG, errors.New("bad key count"))
	}
	ks := make([]touch.Key, count)
	for i, k := range unsafe.Slice(keys, int(count)) {
		ks[i] = touch.Key{Code: rune(k.code), X: float32(k.x), Y: float32(k.y), W: float32(k.w), H: float32(k.h)}
	}
	if err := s.e.SetLayout(ks); err != nil {
		return s.fail(C.SAYSO_ERR_ARG, err)
	}
	return C.SAYSO_OK
}

//export sayso_set_editor_flags
func sayso_set_editor_flags(h C.sayso_engine, flags C.uint32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	s.e.SetFlags(uint32(flags))
	return C.SAYSO_OK
}

//export sayso_set_journal_path
func sayso_set_journal_path(h C.sayso_engine, path *C.char, n C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	p, ok := goString(path, n)
	if !ok {
		return s.fail(C.SAYSO_ERR_ARG, errors.New("bad path"))
	}
	if err := s.e.SetJournal(p); err != nil {
		return s.fail(C.SAYSO_ERR_IO, err)
	}
	return C.SAYSO_OK
}

//export sayso_set_preferences
func sayso_set_preferences(h C.sayso_engine, autocorrect, predictions C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	if autocorrect < C.SAYSO_AUTOCORRECT_OFF || autocorrect > C.SAYSO_AUTOCORRECT_STRICT {
		return s.fail(C.SAYSO_ERR_ARG, errors.New("bad autocorrect level"))
	}
	s.e.SetPreferences(engine.AutocorrectLevel(autocorrect), predictions != 0)
	return C.SAYSO_OK
}

//export sayso_set_context
func sayso_set_context(h C.sayso_engine, text *C.char, n C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	t, ok := goString(text, n)
	if !ok {
		return s.fail(C.SAYSO_ERR_ARG, errors.New("bad text"))
	}
	s.e.SetContext(t)
	return C.SAYSO_OK
}

//export sayso_should_capitalize
func sayso_should_capitalize(h C.sayso_engine) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	if s.e.ShouldCapitalize() {
		return 1
	}
	return 0
}

//export sayso_current_word
func sayso_current_word(h C.sayso_engine, buf *C.char, cap C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	w := s.e.CurrentWord()
	writeText(buf, cap, w)
	return C.int32_t(len(w))
}

//export sayso_on_touch
func sayso_on_touch(h C.sayso_engine, x, y C.float, timeMS C.int64_t, shifted C.int32_t, out *C.sayso_touch_result) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	fx, fy := float32(x), float32(y)
	if out == nil || fx != fx || fy != fy || fx > 1e9 || fx < -1e9 || fy > 1e9 || fy < -1e9 {
		return C.SAYSO_ERR_ARG // NaN or out of any screen
	}
	r := s.e.OnTouch(fx, fy, shifted != 0)
	fillTouch(out, r)
	return C.SAYSO_OK
}

func fillTouch(out *C.sayso_touch_result, r engine.TouchResult) {
	var f C.uint32_t
	if r.Borrowed {
		f |= C.SAYSO_TOUCH_BORROWED
	}
	if r.Core {
		f |= C.SAYSO_TOUCH_CORE
	}
	if r.Code > 0 && ((r.Code >= 'a' && r.Code <= 'z') || (r.Code >= 'A' && r.Code <= 'Z')) {
		f |= C.SAYSO_TOUCH_LETTER
	}
	out.code = C.int32_t(r.Code)
	out.key_index = C.int32_t(r.Key)
	out.flags = f
}

// writeAction fills out and the caller's text buffer.
func writeAction(out *C.sayso_action, text *C.char, a engine.Action) {
	out.kind = C.int32_t(a.Kind)
	out.delete_chars = C.int32_t(a.DeleteChars)
	out.delete_utf16 = C.int32_t(a.DeleteUTF16)
	out.text_len = C.int32_t(writeText(text, C.SAYSO_TEXT_CAP, a.Text))
	out.flags = 0
	if a.Corrected {
		out.flags |= C.SAYSO_ACTION_CORRECTED
	}
}

func actionArgs(s *session, out *C.sayso_action, text *C.char, cap C.int32_t) C.int32_t {
	if out == nil || text == nil || cap < C.SAYSO_TEXT_CAP {
		return s.fail(C.SAYSO_ERR_ARG, errors.New("action buffers must hold SAYSO_TEXT_CAP bytes"))
	}
	return C.SAYSO_OK
}

//export sayso_type_char
func sayso_type_char(h C.sayso_engine, cp C.int32_t, out *C.sayso_action, text *C.char, cap C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	if rc := actionArgs(s, out, text, cap); rc != C.SAYSO_OK {
		return rc
	}
	if cp <= 0 || cp > 0x10FFFF || (cp >= 0xD800 && cp <= 0xDFFF) {
		return s.fail(C.SAYSO_ERR_ARG, errors.New("bad code point"))
	}
	writeAction(out, text, s.e.TypeChar(rune(cp)))
	return C.SAYSO_OK
}

//export sayso_backspace
func sayso_backspace(h C.sayso_engine, out *C.sayso_action, text *C.char, cap C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	if rc := actionArgs(s, out, text, cap); rc != C.SAYSO_OK {
		return rc
	}
	writeAction(out, text, s.e.Backspace())
	return C.SAYSO_OK
}

//export sayso_revert_last_autocorrect
func sayso_revert_last_autocorrect(h C.sayso_engine, out *C.sayso_action, text *C.char, cap C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	if rc := actionArgs(s, out, text, cap); rc != C.SAYSO_OK {
		return rc
	}
	writeAction(out, text, s.e.RevertLast())
	return C.SAYSO_OK
}

//export sayso_get_suggestions
func sayso_get_suggestions(h C.sayso_engine, out *C.sayso_suggestion, max C.int32_t, text *C.char, cap C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	if max <= 0 || out == nil || text == nil || cap <= 0 {
		return s.fail(C.SAYSO_ERR_ARG, errors.New("bad buffers"))
	}
	sugs := s.e.Suggestions(int(max))
	dst := unsafe.Slice(out, int(max))
	buf := unsafe.Slice((*byte)(unsafe.Pointer(text)), int(cap))
	off, n := 0, 0
	for _, sg := range sugs {
		if off+len(sg.Text) > len(buf) {
			break
		}
		copy(buf[off:], sg.Text)
		dst[n] = C.sayso_suggestion{text_offset: C.int32_t(off), text_len: C.int32_t(len(sg.Text)), score: C.float(sg.Score), flags: C.uint32_t(sg.Flags)}
		off += len(sg.Text)
		n++
	}
	if off < len(buf) {
		buf[off] = 0
	}
	return C.int32_t(n)
}

//export sayso_pick_suggestion
func sayso_pick_suggestion(h C.sayso_engine, index C.int32_t, out *C.sayso_action, text *C.char, cap C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	if rc := actionArgs(s, out, text, cap); rc != C.SAYSO_OK {
		return rc
	}
	writeAction(out, text, s.e.Pick(int(index)))
	return C.SAYSO_OK
}

func wordCall(h C.sayso_engine, w *C.char, n C.int32_t, fn func(*engine.Engine, string)) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	word, ok := goString(w, n)
	if !ok || word == "" {
		return s.fail(C.SAYSO_ERR_ARG, errors.New("bad word"))
	}
	fn(s.e, word)
	return C.SAYSO_OK
}

//export sayso_add_word
func sayso_add_word(h C.sayso_engine, w *C.char, n C.int32_t) C.int32_t {
	return wordCall(h, w, n, (*engine.Engine).AddWord)
}

//export sayso_remove_word
func sayso_remove_word(h C.sayso_engine, w *C.char, n C.int32_t) C.int32_t {
	return wordCall(h, w, n, (*engine.Engine).RemoveWord)
}

//export sayso_add_replacement
func sayso_add_replacement(h C.sayso_engine, from *C.char, fromLen C.int32_t, to *C.char, toLen C.int32_t, caseSensitive C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	f, ok1 := goString(from, fromLen)
	t, ok2 := goString(to, toLen)
	if !ok1 || !ok2 || f == "" {
		return s.fail(C.SAYSO_ERR_ARG, errors.New("bad replacement"))
	}
	if err := s.e.AddReplacement(f, t, caseSensitive != 0); err != nil {
		return s.fail(C.SAYSO_ERR_ARG, err)
	}
	return C.SAYSO_OK
}

//export sayso_remove_replacement
func sayso_remove_replacement(h C.sayso_engine, from *C.char, n C.int32_t) C.int32_t {
	return wordCall(h, from, n, (*engine.Engine).RemoveReplacement)
}

//export sayso_reset_learned
func sayso_reset_learned(h C.sayso_engine) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	if err := s.e.ResetLearned(); err != nil {
		return s.fail(C.SAYSO_ERR_IO, err)
	}
	return C.SAYSO_OK
}

// writeList writes NUL-terminated fields into a caller buffer, but only
// when all of them fit, and returns the bytes they need.
func writeList(buf *C.char, cap C.int32_t, fields []string) C.int32_t {
	n := 0
	for _, f := range fields {
		n += len(f) + 1
	}
	if buf != nil && n <= int(cap) {
		dst := unsafe.Slice((*byte)(unsafe.Pointer(buf)), n)
		off := 0
		for _, f := range fields {
			off += copy(dst[off:], f)
			dst[off] = 0
			off++
		}
	}
	return C.int32_t(n)
}

//export sayso_list_words
func sayso_list_words(h C.sayso_engine, which C.uint32_t, buf *C.char, cap C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	if cap < 0 || (buf == nil && cap > 0) {
		return s.fail(C.SAYSO_ERR_ARG, errors.New("bad buffer"))
	}
	return writeList(buf, cap, s.e.PersonalWords(which&C.SAYSO_WORDS_ADDED != 0, which&C.SAYSO_WORDS_LEARNED != 0))
}

//export sayso_list_replacements
func sayso_list_replacements(h C.sayso_engine, buf *C.char, cap C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	if cap < 0 || (buf == nil && cap > 0) {
		return s.fail(C.SAYSO_ERR_ARG, errors.New("bad buffer"))
	}
	var fields []string
	for _, r := range s.e.Replacements() {
		cs := "0"
		if r.CaseSensitive {
			cs = "1"
		}
		fields = append(fields, r.From, r.To, cs)
	}
	return writeList(buf, cap, fields)
}

//export sayso_get_diagnostics
func sayso_get_diagnostics(h C.sayso_engine, out *C.sayso_diag, keys *C.sayso_key_diag, keyCap C.int32_t,
	cands *C.sayso_cand_diag, candCap C.int32_t, text *C.char, textCap C.int32_t) C.int32_t {
	s := acquire(h)
	if s == nil {
		return C.SAYSO_ERR_HANDLE
	}
	defer s.release()
	if out == nil || keyCap < 0 || candCap < 0 || textCap < 0 {
		return s.fail(C.SAYSO_ERR_ARG, errors.New("bad buffers"))
	}
	d := s.e.Diagnostics(int(candCap))
	out.margin = C.float(d.Margin)
	out.would_correct = boolC(d.Correct)
	out.literal_is_word = boolC(d.LiteralIsWord)
	out.literal_total = C.float(d.LiteralTotal)
	fillTouch(&out.last_touch, d.LastTouch)
	nk := 0
	if keys != nil {
		kd := unsafe.Slice(keys, int(keyCap))
		for i, k := range d.Keys {
			if i == len(kd) {
				break
			}
			kd[i] = C.sayso_key_diag{code: C.int32_t(k.Code), expansion: C.float(k.Expansion), dx: C.float(k.DX), dy: C.float(k.DY), samples: C.int32_t(k.Samples)}
			nk++
		}
	}
	nc := 0
	if cands != nil && text != nil && textCap > 0 {
		cd := unsafe.Slice(cands, int(candCap))
		buf := unsafe.Slice((*byte)(unsafe.Pointer(text)), int(textCap))
		off := 0
		for i, c := range d.Cands {
			if i == len(cd) || off+len(c.Text) > len(buf) {
				break
			}
			copy(buf[off:], c.Text)
			cd[i] = C.sayso_cand_diag{text_offset: C.int32_t(off), text_len: C.int32_t(len(c.Text)), total: C.float(c.Total),
				spatial: C.float(c.Spatial), lm: C.float(c.LM), edits: C.int32_t(c.Edits), kind: C.int32_t(c.Kind)}
			off += len(c.Text)
			nc++
		}
	}
	out.key_count = C.int32_t(nk)
	out.cand_count = C.int32_t(nc)
	return C.SAYSO_OK
}

func boolC(b bool) C.int32_t {
	if b {
		return 1
	}
	return 0
}

// The flag values must agree with the engine and decoder.
var _ = [1]struct{}{}[uint32(C.SAYSO_EDITOR_PRIVACY)^engine.FlagPrivacy|uint32(C.SAYSO_EDITOR_AUTOCAP)^engine.FlagAutocap|
	uint32(C.SAYSO_EDITOR_NO_AUTOCORRECT)^engine.FlagNoAutocorrect|uint32(C.SAYSO_EDITOR_NO_SUGGEST)^engine.FlagNoSuggest]
var _ = [1]struct{}{}[int32(C.SAYSO_AUTOCORRECT_OFF)^int32(engine.AutocorrectOff)|int32(C.SAYSO_AUTOCORRECT_NORMAL)^int32(engine.AutocorrectNormal)|
	int32(C.SAYSO_AUTOCORRECT_STRICT)^int32(engine.AutocorrectStrict)]
var _ = [1]struct{}{}[uint32(C.SAYSO_SUG_LITERAL)^uint32(decoder.SugLiteral)|uint32(C.SAYSO_SUG_AUTOCORRECT)^uint32(decoder.SugAutocorrect)|
	uint32(C.SAYSO_SUG_COMPLETION)^uint32(decoder.SugCompletion)|uint32(C.SAYSO_SUG_PREDICTION)^uint32(decoder.SugPrediction)|
	uint32(C.SAYSO_SUG_PERSONAL)^uint32(decoder.SugPersonal)]
