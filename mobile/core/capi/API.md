# Sayso typing core: C API

The typing core is one Go code base for Android and iOS, exposed through a
flat C ABI: [`sayso_core.h`](sayso_core.h). Android loads it as
`libsayso_core.so` (`-buildmode=c-shared`) through JNI; iOS links
`libsayso_core.a` (`-buildmode=c-archive`) into the keyboard extension.

```sh
cd mobile
./scripts/build-core-android.sh   # build/jniLibs/{arm64-v8a,x86_64}/libsayso_core.so + build/include/sayso_core.h
./scripts/check-ios-core.sh       # build/ios/libsayso_core.a, reports its size
./scripts/test-capi.sh            # host archive + C smoke test + on_touch timing through cgo + destroy race
```

## Rules

- **Handles.** An engine is an opaque `sayso_engine` (`uintptr_t`); 0 is never
  live. A destroyed or unknown handle gives `SAYSO_ERR_HANDLE`.
- **Threads.** Any thread may call any function. Calls on one engine are
  serialized inside the core. `sayso_engine_destroy` may race other calls on
  the same engine: it waits for calls in flight, and later calls get
  `SAYSO_ERR_HANDLE`. The core starts no threads of its own beyond
  the Go runtime's (`GOMAXPROCS=2`, a 24 MiB soft heap limit unless the
  `GOMEMLIMIT` environment variable is set).
- **Memory.** No Go pointer crosses the ABI. Strings in are UTF-8 with a byte
  length (no NUL needed). Text out goes into buffers the caller owns; when
  there is room, a NUL follows it. Action text buffers must hold
  `SAYSO_TEXT_CAP` (1024) bytes, or the call fails with `SAYSO_ERR_ARG` before
  changing anything.
- **Errors.** Functions return `SAYSO_OK` or a negative `SAYSO_ERR_*`.
  `sayso_last_error(e, …)` gives the message (`e = 0` for a failed create).
- **The model** is mapped read-only and shared by the OS page cache; it does
  not count against the Go heap. The `.sslm` format is in
  [`../lexicon/FORMAT.md`](../lexicon/FORMAT.md).

## Lifecycle

| Function | Use |
|---|---|
| `sayso_version()` | static version string |
| `sayso_engine_create(model, len, journal, len, &e)` | open a model file; `journal` may be empty (nothing kept) |
| `sayso_engine_create_fd(fd, offset, length, journal, len, &e)` | open a model inside another file: an APK asset via `AssetFileDescriptor` (store `.sslm` uncompressed: `androidResources { noCompress += "sslm" }`). The fd may be closed afterwards |
| `sayso_engine_destroy(e)` | close the journal and unmap the model |
| `sayso_set_journal_path(e, path, len)` | switch the learner's journal (for example per user profile) |
| `sayso_last_error(e, buf, cap)` | the last error message |

## Keyboard and field

| Function | Use |
|---|---|
| `sayso_set_layout(e, keys, n)` | the keys: code (code point or `SAYSO_KEY_*`) and rectangle. Call on every layout or size change. Units are yours (px, points) but must match taps |
| `sayso_set_editor_flags(e, flags)` | per field: `PRIVACY` (incognito, passwords: learn and write nothing), `AUTOCAP`, `NO_AUTOCORRECT`, `NO_SUGGEST` |
| `sayso_set_context(e, text, len)` | the text before the cursor when the cursor moved on its own (field focus, a tap in the text, an external edit). Passing the text the engine already has is a no-op, so calling it from every selection update is safe |
| `sayso_should_capitalize(e)` | 1 when the next letter should be a capital (drive the shift key from it) |
| `sayso_set_preferences(e, autocorrect, predictions)` | the user's settings: `SAYSO_AUTOCORRECT_OFF` (suggest only; replacements still expand), `NORMAL` (default), or `STRICT` (each correction must win by 2 more nats); `predictions` 0 turns off next-word suggestions before the first tap |
| `sayso_current_word(e, buf, cap)` | the word being typed, as typed (for example to show what an autocorrection replaced); returns its length in bytes |

## Typing

| Function | Use |
|---|---|
| `sayso_on_touch(e, x, y, time_ms, shifted, &r)` | a tap. With hit-target expansion it picks the key. A letter (`SAYSO_TOUCH_LETTER`) has already joined the word: insert `r.code`. Anything else is only resolved: pass characters to `sayso_type_char` and the delete key to `sayso_backspace`. `time_ms` is reserved. No heap allocation |
| `sayso_type_char(e, cp, &a, text, cap)` | a character that was not a tapped letter: space and marks end the word and may autocorrect it; an apostrophe joins the word; a hardware-key letter joins it too |
| `sayso_backspace(e, &a, text, cap)` | the delete key. Right after an autocorrection (or replacement) it undoes it: `REPLACE` with the typed word, which then commits as typed. Otherwise `NONE`: delete one character yourself |
| `sayso_revert_last_autocorrect(e, &a, text, cap)` | undo the last autocorrection explicitly (for example from the suggestion strip); `NONE` if there is nothing to undo |

An action is `NONE` (do the usual thing: insert the character, delete one
character) or `REPLACE`: delete `delete_chars` code points (`delete_utf16`
UTF-16 units, for Android's `deleteSurroundingText`) before the cursor, then
insert the `text_len` bytes of text. `SAYSO_ACTION_CORRECTED` marks an
autocorrection.

## Suggestions

| Function | Use |
|---|---|
| `sayso_get_suggestions(e, out, max, text, cap)` | up to `max` suggestions, best first, text packed into `text`. Before any tap: next-word predictions. While typing: the first is what committing writes (flag `AUTOCORRECT`, else `LITERAL`), then candidates and completions |
| `sayso_pick_suggestion(e, i, &a, text, cap)` | write suggestion `i` of the last `get_suggestions`, then a space. A later mark (`.`, `,`) swaps places with that space |

## Learning

| Function | Use |
|---|---|
| `sayso_add_word` / `sayso_remove_word` | the personal lexicon (seed it from Sayso's dictionary) |
| `sayso_add_replacement(e, from, …, to, …, case_sensitive)` / `sayso_remove_replacement` | text replacements from Sayso's dictionary: typing `from` as a word writes `to` (`\n` is a line break). `SAYSO_ERR_ARG`, changing nothing, when `from` is empty or over 128 bytes or `to` is over 512 |
| `sayso_reset_learned(e)` | forget everything learned, personal words and replacements included, and empty the journal |
| `sayso_list_words(e, which, buf, cap)` | the personal lexicon for a dictionary screen: `SAYSO_WORDS_ADDED`, `SAYSO_WORDS_LEARNED`, or both; each word ends with a NUL, added words first. Returns the bytes needed and writes only when they fit (call with `NULL, 0` to size the buffer) |
| `sayso_list_replacements(e, buf, cap)` | the replacements, sorted: `from`, `to`, then `1` (case sensitive) or `0`, each ending with a NUL. Same size rule |

Privacy mode writes nothing, dictionary edits included. An app that edits the
dictionary outside a text field (a settings screen) clears
`SAYSO_EDITOR_PRIVACY` for the edit if the last field set it.

What the learner does on its own (never in privacy mode):

- Backspace right after an autocorrection undoes it; **two undos** of the same
  correction stop it for good.
- **Two manual fixes** (backspacing into a committed word and retyping it, or
  picking a different suggestion) teach the typo → word pair.
- A word the model lacks joins the personal lexicon after it is kept twice.
- The user's bigrams fade with a 30-day half-life.
- Where taps land on each key is learned from confirmed words only (the next
  word started without undoing the last), per key, row, and keyboard; the
  offset moves at most 0.05 key pitch per word and never past 0.3.
- Everything goes to an append-only journal (one text record per line;
  compacted past 512 KB). Privacy mode writes nothing.

## Diagnostics

`sayso_get_diagnostics(e, &d, keys, key_cap, cands, cand_cap, text, text_cap)`
fills the developer overlay: per key, its expansion into its neighbours for
the next tap (layout units), its learned `(dx, dy)` and sample count; the top
candidates with total, spatial, and language-model scores and edits; the
autocorrect margin and whether a commit would correct; the last tap's
resolution.

## Functions

`sayso_version`, `sayso_last_error`, `sayso_engine_create`,
`sayso_engine_create_fd`, `sayso_engine_destroy`, `sayso_set_layout`,
`sayso_set_editor_flags`, `sayso_set_journal_path`, `sayso_set_preferences`,
`sayso_set_context`, `sayso_should_capitalize`, `sayso_current_word`,
`sayso_on_touch`, `sayso_type_char`,
`sayso_backspace`, `sayso_revert_last_autocorrect`, `sayso_get_suggestions`,
`sayso_pick_suggestion`, `sayso_add_word`, `sayso_remove_word`,
`sayso_add_replacement`, `sayso_remove_replacement`, `sayso_reset_learned`,
`sayso_list_words`, `sayso_list_replacements`, `sayso_get_diagnostics`.

## Sketch: an Android IME

The keyboard's real binding is `NativeTypingEngine` and `sayso_jni.c` under
`mobile/android/app/src/`; in outline:

```kotlin
// onCreate: map the bundled model, keep the journal in app storage.
val afd = assets.openFd("en-US.sslm")
engine = Core.createFd(afd.parcelFileDescriptor.fd, afd.startOffset, afd.length, filesDir.resolve("typing.journal").path)
// onStartInput: flags from EditorInfo; context from the field.
Core.setEditorFlags(engine, flagsFor(editorInfo))
Core.setContext(engine, ic.getTextBeforeCursor(256, 0))
// A tap on the keyboard view:
val r = Core.onTouch(engine, x, y, now, shifted)
when {
    r.isLetter -> ic.commitText(String(Character.toChars(r.code)), 1)
    r.code == KEY_BACKSPACE -> apply(Core.backspace(engine))   // NONE: sendDownUpKeyEvents(KEYCODE_DEL)
    r.code > 0 -> apply(Core.typeChar(engine, r.code))         // NONE: commitText(the char)
}
// apply(REPLACE): ic.deleteSurroundingText(a.deleteUtf16, 0); ic.commitText(a.text, 1)
```
