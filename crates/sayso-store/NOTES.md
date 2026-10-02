# sayso-store

SQLite history and dictionary, FLAC audio files, retention.

## API

`Store` is `Send + Sync` (one connection behind a mutex). All calls block.

- `Store::open(db_path, audio_dir)`: creates folders, sets WAL, migrates with `PRAGMA user_version`. A newer schema gives `SchemaTooNew`.
- History: `insert_entry` (ignores `id`), `update_entry`, `get`, `list(&HistoryQuery)`, `count`, `delete` (also the audio file), `clear_all` (history and all `.flac` files, keeps the dictionary), `entries_since`, `app_list`.
- `HistoryQuery`: `text` (FTS5 over transcript and final text; words are quoted, last word is a prefix), `enhanced_only`, `has_audio`, `app_bundle_id`, `limit`, `offset`. Newest first.
- Audio: `save_audio(&[f32]) -> file name`, `load_audio(name)`, `audio_path(name)`. 16 kHz mono, 16-bit FLAC (`flacenc`, `claxon`). Fewer than 16 samples are padded with silence.
- Dictionary: `list_words`, `add_word` (case-insensitive dedupe), `remove_word`, `list_replacements`, `add_replacement`, `get_replacement`, `update_replacement`, `remove_replacement`, `record_replacement_uses` (adds `count` to `uses`).
- Retention: `apply_retention(keep_text_days, keep_audio_days, now) -> RetentionReport`. Audio expiry deletes only the file and sets `audio_file = NULL`.

JSON columns: `app`, `replacements`, `enhance`, `insert_result`. `waveform` is a BLOB. Timestamps are Unix milliseconds.

## Tested

From-scratch create and reopen, WAL, newer-schema refusal, `Send + Sync`, entry round trip, update, paging, FTS (including quotes and operators as plain text, updates, deletes), all filters, app list, `entries_since`, delete with audio, `clear_all`, FLAC round trip (tolerance half a 16-bit step), clipping, short audio, path traversal guard, word and replacement CRUD, use counting, retention (audio only, text with audio, both, none, missing file).

## Not tested

Concurrent writers from several threads, very large databases, migrations from a real older version (only version 0 to 1 exists), disk-full errors.
