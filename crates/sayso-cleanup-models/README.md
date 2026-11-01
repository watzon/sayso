# sayso-cleanup-models

What [Sayso](https://github.com/watzon/sayso) knows about cleanup models: small local models that clean up text with fixed settings and read no prompt. They are S1-mini by Superwhisper and Cleanup 1 by Spokenly.

- The catalog: each model with its pinned file, size, and hash.
- `render`: the exact input of each model.
- `plan_parts`, `join_answers`: how long text splits and joins.
- `check`: the checks of an answer. A failed check means that the app keeps the text that went in.
- `restore_spelling`, `dictation_language`, `detect_language`.
- `protocol`: the messages of the cleanup runtime.

This crate does no I/O. `sayso-cleanup-client` runs the models.

S1-mini must always show as "S1-mini" by "Superwhisper". That is a term of its license. Keep `LICENSE` and `NOTICE` of each model beside its file.

## License

MIT. See `LICENSE`.
