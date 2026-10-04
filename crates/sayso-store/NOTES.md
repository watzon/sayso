# sayso-store

SQLite history and dictionary, FLAC audio files, retention.

## API

`Store` is `Send + Sync` (one connection behind a mutex). All calls block.

- `Store::open(db_path, audio_dir)`: creates folders, sets WAL, migrates with `PRAGMA user_version`. A newer schema gives `SchemaTooNew`.
- History: `insert_entry` (ignores `id`), `update_entry`, `get`, `list(&HistoryQuery)`, `count`, `delete` (also the audio file), `clear_all` (history and all `.flac` files, keeps the dictionary, then `VACUUM` so the deleted text leaves the file), `clear_audio` (all `.flac` files, the entries stay), `storage_use` (entry count, database size, audio file count and size), `entries_since`, `app_list`.
- `HistoryQuery`: `text` (FTS5 over transcript and final text; words are quoted, last word is a prefix), `enhanced_only`, `has_audio`, `app_bundle_id`, `limit`, `offset`. Newest first.
- Audio: `save_audio(&[f32]) -> file name`, `load_audio(name)`, `audio_path(name)`. 16 kHz mono, 16-bit FLAC (`flacenc`, `claxon`). Fewer than 16 samples are padded with silence.
- Dictionary: `list_words`, `add_word` (case-insensitive dedupe), `remove_word`, `list_replacements`, `add_replacement`, `get_replacement`, `update_replacement`, `remove_replacement`, `record_replacement_uses` (adds `count` to `uses`).
- Pindrop import: `pindrop::found(dir)`, `pindrop::read(dir) -> PindropData` (reads a copy of the SwiftData store, so Pindrop's files do not change), `Store::import_pindrop(&data) -> ImportReport`. A second import adds nothing: `history.import_id` holds `pindrop:<record id>` and is unique. Only plain dictations come over, without audio.
- Retention: `apply_retention(keep_text_days, keep_audio_days, now) -> RetentionReport`. Audio expiry deletes only the file and sets `audio_file = NULL`.

JSON columns: `app`, `replacements`, `enhance`, `insert_result`. `waveform` is a BLOB. Timestamps are Unix milliseconds.

## Tested

From-scratch create and reopen, WAL, newer-schema refusal, `Send + Sync`, entry round trip, update, paging, FTS (including quotes and operators as plain text, updates, deletes), all filters, app list, `entries_since`, delete with audio, `clear_all` (also that the database shrinks), `clear_audio`, `storage_use`, FLAC round trip (tolerance half a 16-bit step), clipping, short audio, path traversal guard, word and replacement CRUD, use counting, retention (audio only, text with audio, both, none, missing file), the migration from version 1 to 2 with data, the Pindrop import (a fixture store, an old store without the newer columns, a second import, unchanged source files).

## Not tested

Concurrent writers from several threads, very large databases, a Pindrop store older than schema 1.0.14 from a real install (only a fixture), disk-full errors.
