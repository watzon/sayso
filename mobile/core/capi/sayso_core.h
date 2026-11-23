/*
 * sayso_core.h: the C ABI of Sayso's mobile typing core.
 *
 * One engine per keyboard session. Every function is safe to call from any
 * thread; calls on one engine are serialized. Strings are UTF-8 with an
 * explicit byte length (no NUL needed). Output text goes into buffers the
 * caller owns; no pointer into the core's memory is ever returned, except
 * sayso_version's static string. See API.md.
 */
#ifndef SAYSO_CORE_H
#define SAYSO_CORE_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Results. Functions return SAYSO_OK (0) or a negative error. */
#define SAYSO_OK 0
#define SAYSO_ERR_ARG (-1)    /* a bad argument; nothing was changed */
#define SAYSO_ERR_IO (-2)     /* a file could not be read or written */
#define SAYSO_ERR_MODEL (-3)  /* the model file is not a valid .sslm */
#define SAYSO_ERR_HANDLE (-4) /* the engine handle is not live */

/* Action text buffers must hold at least this many bytes. */
#define SAYSO_TEXT_CAP 1024

/* An opaque engine handle; 0 is never a live engine. */
typedef uintptr_t sayso_engine;

/* Editor flags (sayso_set_editor_flags). */
#define SAYSO_EDITOR_PRIVACY 1u        /* learn and write nothing */
#define SAYSO_EDITOR_AUTOCAP 2u        /* capitalize sentence starts */
#define SAYSO_EDITOR_NO_AUTOCORRECT 4u /* never replace what is typed */
#define SAYSO_EDITOR_NO_SUGGEST 8u     /* no suggestions (passwords) */

/* Autocorrect levels (sayso_set_preferences). */
#define SAYSO_AUTOCORRECT_OFF 0    /* suggest only; replacements still expand */
#define SAYSO_AUTOCORRECT_NORMAL 1 /* the default */
#define SAYSO_AUTOCORRECT_STRICT 2 /* correct only clear winners */

/* Parts of the personal lexicon (sayso_list_words). */
#define SAYSO_WORDS_ADDED 1u   /* added with sayso_add_word */
#define SAYSO_WORDS_LEARNED 2u /* learned by keeping a word the model lacks */

/* Function key codes in sayso_key.code; letters, space and marks use their
 * Unicode code point. */
#define SAYSO_KEY_SHIFT (-1)
#define SAYSO_KEY_BACKSPACE (-2)
#define SAYSO_KEY_ENTER (-3)
#define SAYSO_KEY_SYMBOLS (-4)
#define SAYSO_KEY_OTHER (-5)

/* One key: what it types and its rectangle (top-left, size), in any unit
 * shared by all keys and taps (Android px, iOS points). */
typedef struct {
    int32_t code;
    float x, y, w, h;
} sayso_key;

/* sayso_touch_result.flags */
#define SAYSO_TOUCH_BORROWED 1u /* hit-target expansion moved the tap off the key under it */
#define SAYSO_TOUCH_CORE 2u     /* the tap was in its key's core */
#define SAYSO_TOUCH_LETTER 4u   /* a letter: the core took it; insert code */

typedef struct {
    int32_t code;      /* the character typed (letters in the right case) or a SAYSO_KEY_* code */
    int32_t key_index; /* index of the key in the layout */
    uint32_t flags;
} sayso_touch_result;

/* sayso_action.kind */
#define SAYSO_ACTION_NONE 0    /* do the usual thing: insert the character / delete one character */
#define SAYSO_ACTION_REPLACE 1 /* delete delete_chars (or delete_utf16) before the cursor, then insert the text */
/* sayso_action.flags */
#define SAYSO_ACTION_CORRECTED 1u /* the action autocorrected a word (backspace undoes it) */

typedef struct {
    int32_t kind;
    int32_t delete_chars; /* code points */
    int32_t delete_utf16; /* the same span in UTF-16 units */
    int32_t text_len;     /* bytes of text written to the caller's buffer */
    uint32_t flags;
} sayso_action;

/* sayso_suggestion.flags */
#define SAYSO_SUG_LITERAL 1u     /* what was typed */
#define SAYSO_SUG_AUTOCORRECT 2u /* committing now writes this */
#define SAYSO_SUG_COMPLETION 4u  /* a longer word the taps start */
#define SAYSO_SUG_PREDICTION 8u  /* a next word */
#define SAYSO_SUG_PERSONAL 16u   /* from the personal lexicon */

typedef struct {
    int32_t text_offset; /* into the caller's text buffer */
    int32_t text_len;
    float score; /* lower is better, in nats */
    uint32_t flags;
} sayso_suggestion;

typedef struct {
    int32_t code;
    float expansion; /* how far the key reaches into its neighbours for the next tap, layout units */
    float dx, dy;    /* learned target offset, layout units */
    int32_t samples; /* confirmed taps that taught the key */
} sayso_key_diag;

typedef struct {
    int32_t text_offset, text_len;
    float total, spatial, lm; /* scores in nats */
    int32_t edits;
    int32_t kind; /* 0 word, 1 completion, 2 split */
} sayso_cand_diag;

typedef struct {
    float margin;          /* how far the best correction clears its bar; negative: no autocorrect */
    int32_t would_correct; /* committing now would autocorrect */
    int32_t literal_is_word;
    float literal_total;
    int32_t key_count;  /* keys written (min of layout size and key_cap) */
    int32_t cand_count; /* candidates written */
    sayso_touch_result last_touch;
} sayso_diag;

#ifndef SAYSO_CORE_NO_PROTOTYPES /* the Go side defines the functions itself */

/* The core's version, a static NUL-terminated string. */
const char *sayso_version(void);

/* The last error message of an engine, or of the last failed create when
 * e is 0. Writes up to cap bytes; returns the full length. */
int32_t sayso_last_error(sayso_engine e, char *buf, int32_t cap);

/* Create an engine from a model file. journal_path may be empty: then
 * nothing learned is kept. */
int32_t sayso_engine_create(const char *model_path, int32_t model_path_len,
                            const char *journal_path, int32_t journal_path_len,
                            sayso_engine *out);
/* Create an engine from length bytes at offset of an open file, such as an
 * uncompressed APK asset (AssetFileDescriptor). The fd may be closed after. */
int32_t sayso_engine_create_fd(int32_t fd, int64_t offset, int64_t length,
                               const char *journal_path, int32_t journal_path_len,
                               sayso_engine *out);
void sayso_engine_destroy(sayso_engine e);

int32_t sayso_set_layout(sayso_engine e, const sayso_key *keys, int32_t count);
int32_t sayso_set_editor_flags(sayso_engine e, uint32_t flags);
int32_t sayso_set_journal_path(sayso_engine e, const char *path, int32_t len);
/* The user's settings: a SAYSO_AUTOCORRECT_* level, and whether next words
 * are predicted before any tap (0 or 1). Defaults: NORMAL, 1. */
int32_t sayso_set_preferences(sayso_engine e, int32_t autocorrect, int32_t predictions);

/* The text before the cursor, when the cursor moved somewhere the engine
 * did not put it. Repeating the text the engine has is a no-op. */
int32_t sayso_set_context(sayso_engine e, const char *text, int32_t len);
/* 1 when the next letter should be a capital, else 0 (or an error). */
int32_t sayso_should_capitalize(sayso_engine e);
/* The word being typed, as typed. Writes up to cap bytes; returns its full
 * length in bytes. */
int32_t sayso_current_word(sayso_engine e, char *buf, int32_t cap);

/* A tap. Letters join the word and come back in out->code with
 * SAYSO_TOUCH_LETTER: insert them. Anything else is only resolved: pass
 * characters to sayso_type_char and backspace to sayso_backspace.
 * time_ms is reserved. No allocation. */
int32_t sayso_on_touch(sayso_engine e, float x, float y, int64_t time_ms, int32_t shifted,
                       sayso_touch_result *out);
/* A character that was not a tapped letter: space and marks end the word
 * (and may autocorrect it), an apostrophe joins it. text must hold
 * SAYSO_TEXT_CAP bytes. */
int32_t sayso_type_char(sayso_engine e, int32_t codepoint, sayso_action *out, char *text, int32_t cap);
/* The delete key. Right after an autocorrection it undoes it. */
int32_t sayso_backspace(sayso_engine e, sayso_action *out, char *text, int32_t cap);
int32_t sayso_revert_last_autocorrect(sayso_engine e, sayso_action *out, char *text, int32_t cap);

/* Up to max suggestions into out, their text into text; returns the count. */
int32_t sayso_get_suggestions(sayso_engine e, sayso_suggestion *out, int32_t max, char *text, int32_t cap);
/* Write suggestion index of the last sayso_get_suggestions, then a space. */
int32_t sayso_pick_suggestion(sayso_engine e, int32_t index, sayso_action *out, char *text, int32_t cap);

int32_t sayso_add_word(sayso_engine e, const char *word, int32_t len);
int32_t sayso_remove_word(sayso_engine e, const char *word, int32_t len);
int32_t sayso_add_replacement(sayso_engine e, const char *from, int32_t from_len,
                              const char *to, int32_t to_len, int32_t case_sensitive);
int32_t sayso_remove_replacement(sayso_engine e, const char *from, int32_t from_len);
int32_t sayso_reset_learned(sayso_engine e);
/* The personal lexicon parts in which (SAYSO_WORDS_*): each word followed by
 * a NUL, added words first, each part sorted. Returns the bytes the list
 * needs; writes it only when that fits in cap (buf may be NULL with cap 0). */
int32_t sayso_list_words(sayso_engine e, uint32_t which, char *buf, int32_t cap);
/* The text replacements, sorted: for each, from NUL to NUL then '1' (case
 * sensitive) or '0' and NUL. Same size rule as sayso_list_words. */
int32_t sayso_list_replacements(sayso_engine e, char *buf, int32_t cap);

/* For the developer overlay. keys/cands/text may be NULL with 0 caps. */
int32_t sayso_get_diagnostics(sayso_engine e, sayso_diag *out,
                              sayso_key_diag *keys, int32_t key_cap,
                              sayso_cand_diag *cands, int32_t cand_cap,
                              char *text, int32_t text_cap);

#endif /* SAYSO_CORE_NO_PROTOTYPES */

#ifdef __cplusplus
}
#endif

#endif /* SAYSO_CORE_H */
