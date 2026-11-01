# sayso-cleanup-client

Runs [Sayso](https://github.com/watzon/sayso) cleanup models from your own app.

- `runtime`: starts the cleanup runtime `sayso-cleanup` and talks to it over stdio. A request that passes its deadline kills the process.
- `cleaner`: `RuntimeCleaner`, which runs a whole text through a model.
- `download`: a resumable download of the model files, with a hash check.

The runtime links llama.cpp and has the license GPL-3.0-only. It is a separate program that you ship beside your app. This crate never links it.

## License

MIT. See `LICENSE`.
