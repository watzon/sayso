# Engine protocol

This is the specification of the protocol between a speech engine and its client. Sayso uses it to talk to its engines, and an app that runs an engine alone uses it too. Version 1.

This file is under the MIT license (see the end of this file), so anyone can write a client or an engine. The engines of Sayso stay under GPL-3.0-only.

Two engines implement this protocol:

| Engine | Systems | Source |
|---|---|---|
| `SaysoEngine` (Swift) | macOS | `native/macos/SaysoEngine` |
| `sayso-engine` (Rust) | Linux, Windows | `crates/sayso-engine` |

`sayso-fake-engine` (`crates/sayso-engine-client/src/bin/fake_engine.rs`) is a scripted engine for the tests of the client. The notes on each engine are in `crates/sayso-engine-client/NOTES.md` and `crates/sayso-engine/NOTES.md`.

## Transport

- The client starts the engine as a child process. Requests go to the stdin of the engine. Replies and events come from its stdout. Stderr is free text for people, not part of the protocol.
- Each message is one JSON object on one line of UTF-8, with `\n` at the end.
- The engine needs the environment variable `SAYSO_MODELS_DIR`: the folder for its model files. The engine makes the folder when it is not there. Two engines that use the same folder at the same time are not supported.
- The engine exits when stdin closes, and after it replies to `shutdown`.

## Messages

Every message has `"v": 1` and a string `type`. The engine ignores fields that it does not know, and the client does the same.

**Requests** go from the client to the engine. A request that wants a reply has a string `id`. Only `stream_audio` has no `id`.

**Replies** are the messages of type `hello`, `ok`, `final`, and `error`. Each request with an `id` gets exactly one reply, with the same `id`.

**Events** are all other messages from the engine: `model_state`, `download_progress`, `part_state`, `partial`, and `log`. An event that a request caused has the `id` of that request and comes before the reply. No event with that `id` comes after the reply. Events without an `id` can come at any time. A client ignores an event type that it does not know.

The engine runs requests at the same time, so replies can come in any order. Match them by `id`. Two exceptions keep their order:

- `stream_start`, `stream_audio`, and `stream_end` run in the order they arrive.
- `unload` runs before the requests that arrive after it. A `load` of the same model that follows starts a new load.

### Errors

```json
{"v":1,"type":"error","id":"r7","message":"model m1 is not loaded"}
```

- A request that the engine cannot run gets an `error` with its `id` and a `message` in English. Examples: an unknown `type`, a required field that is missing or has the wrong type, a model that is not loaded.
- A line that is not a JSON object with a string `type` is not a request. It gets an `error` without `id`.
- `stream_audio` has no `id`, so its errors have no `id`.
- `error` can also have `code` and `reason` (see "Reason codes").

## Version and features

The first request of a client is `hello`:

```json
{"v":1,"type":"hello","id":"h1"}
{"v":1,"type":"hello","id":"h1","version":"0.11.0","protocol":1,"features":["keep_preview","unload"]}
```

- `version`: the version of the engine. The engines of Sayso have the version of the Sayso release that they come with.
- `protocol`: the version of the protocol, 1. A client checks it before it sends other requests.
- `features`: the optional parts of the protocol that this engine has. A client checks for a feature before it uses it.

The requests of the next section that have no feature are in every engine. A new request or a new optional field adds a feature, and `protocol` stays 1. `protocol` changes only when an existing message changes in a way that breaks old clients.

An engine that sends no `features` is older than this spec. A client can still send an optional request to it and handle the `error` reply.

| Feature | What it adds | Swift | Rust |
|---|---|---|---|
| `unload` | The request `unload`. | yes | yes |
| `keep_preview` | The field `keep_preview` of `unload`. An engine without it unloads the whole model. | no | yes |
| `prepare` | The request `prepare` and the event `part_state`. | yes | no |
| `language_model` | The requests `language_model` and `generate`: the language model of the system. | yes | no |
| `concurrent_streams` | Streams that run at the same time (see "Streams"). | yes | yes |
| `segments` | The field `segments` of `transcribe` (see "Segments"). | yes | yes |

The Rust engine replies `ok` to `prepare` and does nothing, so it does not list `prepare`.

## Requests

`engine` is the engine object of a model: which family of models runs it, with the settings of that family (see "Models"). `model` is the id of a model.

| Request | Fields | Events, then the reply |
|---|---|---|
| `hello` | | `hello {version, protocol, features}` |
| `list_models` | `models: [{id, engine}]` | one `model_state` for each model, then `ok` |
| `download` | `model, engine, size_bytes?` | `model_state downloading`, `download_progress`, `model_state downloaded`, then `ok` |
| `delete` | `model, engine?` | `model_state not_downloaded`, then `ok` |
| `load` | `model, engine` | `model_state optimizing`, `model_state ready {load_ms}`, then `ok` |
| `unload` | `model, keep_preview?` | `model_state downloaded` when the model was in memory, then `ok` |
| `prepare` | `model, preview, vocabulary` | `part_state` for each part, then `ok` |
| `transcribe` | `model, path, language, vocabulary, segments?` | `final {text, elapsed_ms, segments?}` |
| `stream_start` | `session, model, language?` | `ok`, then `partial` events |
| `stream_audio` | `session, pcm` (no `id`) | `partial` events, no reply |
| `stream_end` | `session` | the last `partial` when the text changed, then `ok {session, text}` |
| `language_model` | | `ok {available, model?, message?, reason?}` |
| `generate` | `instructions, prompt, timeout_ms, temperature?` | `ok {text, model}` |
| `shutdown` | | `ok`, then the engine exits |

### `list_models`

The engine checks the disk for each model and sends its state. It uses no network. After a `list_models`, the engine knows the `engine` object of each model, so a later `delete` can leave out `engine`. A `delete` without `engine` of a model that the engine does not know can get an `error`.

### `download`

`size_bytes` is the size of the model files from the catalog. An engine whose download library reports only a fraction uses it for `bytes_done`. `download_progress {fraction, bytes_done, bytes_total}` comes at most 8 times a second. A model that is already on disk gets `model_state downloaded` and `ok` at once. When the download fails, the engine sends `model_state failed` and then `error`.

### `load` and `unload`

`load` reads a model into memory. `model_state ready` has `load_ms`, the time of the load in milliseconds. `load` of a model that is not downloaded gets an `error`. When the load fails, the engine sends `model_state failed` and then `error`.

`unload` takes a model out of memory and keeps its files. It ends the streams of the model. A stream or a final pass that runs can keep its part of the model until it ends. `unload` of a model that is not in memory gets only `ok`. A model with a download, a load, or a delete in progress is busy, and `unload` gets an `error`.

`keep_preview: true` is for a model that does the final pass and the live preview as two parts. The engine then drops only the final pass and sends `model_state downloaded` with `preview: true`, also in later `list_models` replies. In that state `stream_start` works, `transcribe` gets an `error`, and `load` loads only the final pass. A model in one part leaves whole.

The engines never unload a model by themselves. The client decides when a model leaves memory.

### `prepare`

`prepare` loads the optional parts of a loaded model before the first request needs them. Send it after `load`, and again when the live preview setting or the dictionary words change.

- `preview: true` asks for the live preview part. `vocabulary` asks for the dictionary part for exactly these words.
- `part_state {model, part, state}`: `part` is `preview` or `dictionary`. `state` is `preparing`, `ready`, or `failed`.
- `ok` means that the preparation ended, also when a part failed. A failed part does not change the state of the model, and a later request can load it again.
- `prepare` of a model that is not loaded gets an `error`.

### `transcribe`

The final pass on a whole recording. `path` is a WAV file: 16 kHz, mono, 16-bit PCM. The file belongs to the client, and the engine does not delete it. `language` is a code such as `en`, or `auto`. `vocabulary` is a list of words that the model should prefer. A model that cannot use them ignores them. `final` has the `text` and `elapsed_ms`, the time of the pass in milliseconds.

`transcribe` with a model that is not loaded gets an `error`. A `transcribe` on a model with open streams runs. It does not end the streams.

### Segments

With the feature `segments`, `transcribe` takes `segments: true`. The `final` reply then also has `segments`: the text cut into parts with times, in order, without overlaps.

```json
{"v":1,"type":"final","id":"t1","text":"Hello there. How are you?","elapsed_ms":212,
 "segments":[{"start_ms":320,"end_ms":1180,"text":"Hello there.",
              "words":[{"start_ms":320,"end_ms":640,"text":"Hello"},{"start_ms":700,"end_ms":1180,"text":"there."}]},
             {"start_ms":1900,"end_ms":2950,"text":"How are you?","words":[...]}]}
```

How much timing a model gives is its timestamp level: `word`, `segment`, or `none`. The model catalog of Sayso says the level of each model (`EngineKind::timestamps` in `crates/sayso-core/src/models.rs`).

- `word`: each segment has `words`, each with `start_ms`, `end_ms`, and `text`. A segment ends after a word that ends a sentence (`.`, `?`, `!`, `。`, `？`, `！`), before a pause of 700 ms or more between two words, and before it would pass 20 s.
- `segment`: the segments of the model, with no `words`.
- `none`: one segment for each piece of audio that the engine transcribes alone, with the text of that piece. An engine cuts long audio into pieces (the Rust engine cuts at about 30 s), so a short recording is one segment from 0 to the end of the audio. Cut the audio first (for example with VAD) to get shorter segments.

Times are in milliseconds from the start of the file. A segment with no text is left out.

### Streams

A stream gives a live preview while the user speaks.

- `stream_start {session, model, language?}` opens the stream. `session` is an integer that the client chooses. The model must be loaded and have a live preview, else the reply is an `error`.
- `stream_audio {session, pcm}` sends audio: base64 of 16 kHz mono little-endian 16-bit PCM. Chunks of 160 ms work well.
- `partial {session, committed, tentative}` gives the text so far. `committed` does not change any more in this stream. `tentative` is the newest text, which can still change. Each `partial` has all the text of the stream, not only the new text.
- `stream_end {session}` closes the stream. The engine decodes the rest of the audio, sends the last `partial` when the text changed, and replies `ok {session, text}`. No `partial` of the session comes after that reply.
- `stream_audio` or `stream_end` for a session that is not open gets an `error`.

With the feature `concurrent_streams`, streams are independent. The engine runs at least 3 streams at the same time, on one model or on different models, and a `stream_start` does not end another stream. When the engine cannot open one more stream, the reply is an `error` with `code: "too_many_streams"`. An engine can load a second copy of a model for a second stream on that model, so each stream can cost memory.

An engine without the feature has one stream for each model. A `stream_start` on a model with an open stream ends the older session without a reply, and its later `stream_audio` gets an `error`.

A stream is only a preview. The client keeps the audio and sends the whole recording to `transcribe` for the final text.

### `language_model` and `generate`

These run the language model of the system (on macOS 26 and later, Apple Intelligence). The system owns the model, so there is no download, load, or unload.

- `language_model` replies `ok` with `available: true` and the name of the model in `model`, or `available: false` with `message` and `reason`.
- `generate {instructions, prompt, timeout_ms, temperature?}` replies `ok {text, model}`. The engine stops the model after `timeout_ms`. An `error` reply has `code`: `unavailable`, `refused`, or `timeout`.

## Events

| Event | Fields |
|---|---|
| `model_state` | `model, state`, and `load_ms` (state `ready`), `preview` (state `downloaded`), `message` and `reason` (state `failed`) |
| `download_progress` | `model, fraction, bytes_done, bytes_total` |
| `part_state` | `model, part, state` |
| `partial` | `session, committed, tentative` |
| `log` | `level, message` |

Model states:

| `state` | Meaning |
|---|---|
| `not_downloaded` | The files of the model are not complete on disk. |
| `downloading` | A download runs. |
| `downloaded` | The files are on disk. The model is not in memory. |
| `optimizing` | A load runs. |
| `ready` | The model is in memory. |
| `failed` | A download or a load failed, or the files cannot be used. `message` says why in English, and `reason` can give a code. |

## Reason codes

`reason` is an optional, stable `snake_case` code in `model_state failed`, in the `language_model` reply when `available` is `false`, and in the `error` replies of `generate`. `message` stays the English detail. A client shows its own text for a code that it knows, and the `message` for a code that it does not know.

| `reason` | Sent in | Meaning |
|---|---|---|
| `incomplete_download` | `model_state failed` | A download ended, but the model files are not complete. |
| `download_failed` | `model_state failed` | A download stopped with an error (network, disk, HTTP status). |
| `load_failed` | `model_state failed` | The model is on disk, but the load failed. |
| `no_gpu` | `model_state failed` | The model needs a GPU that this computer does not have. |
| `format_changed` | `model_state failed` | The files on disk are in an old format. Download the model again. |
| `device_not_eligible` | `language_model`, `generate` | This computer does not support the language model of the system. |
| `intelligence_off` | `language_model`, `generate` | The language model is off in the settings of the system. |
| `model_not_ready` | `language_model`, `generate` | The system still downloads the language model. |
| `needs_macos_26` | `language_model`, `generate` | The language model needs macOS 26 or later. |
| `unavailable` | `language_model`, `generate` | The language model is not available, with no better reason. |
| `timeout` | `generate` | The model gave no reply in time. |
| `refused` | `generate` | The model refused the text. |
| `failed` | `generate` | The model failed in another way. |

## Models

A model has an id, such as `parakeet-tdt-v3`, and an `engine` object. The `engine` object has a `kind` and the settings of that kind. For example, the Swift engine runs `{"kind":"parakeet_tdt","version":"v3"}`, and the Rust engine runs `{"kind":"sherpa","recipe":"parakeet_tdt", ...}`. The two engines know different kinds, and an engine replies `error` to a kind that it does not know.

Today the client sends the `engine` object of each model, from the model catalog of Sayso (`crates/sayso-core/src/models.rs` for macOS, `crates/sayso-core/src/models_onnx.rs` for Linux and Windows). The catalog is not part of this spec yet.

## Conformance tests

`sayso-engine-conformance` (`crates/sayso-engine-conformance`, MIT) checks an engine against this spec. It starts the engine, sends requests, and checks each line that comes back. It has three levels:

| Level | Needs | Checks |
|---|---|---|
| core | nothing | `hello` and `features`, `"v":1` and `type` on every line, one reply for each request, the errors of this spec, unknown stream sessions, `unload` and `language_model` when listed, `shutdown`, and the exit when stdin closes |
| model | a model id and its `engine` object | `list_models`, `delete`, and the errors for a model that is not downloaded or not loaded |
| ready | a models folder with the model on disk, and a WAV file with speech | `list_models`, `load`, `transcribe`, a stream (with `--stream`), and `unload` |

```text
cargo run -p sayso-engine-conformance -- <engine> \
    [--model <id> --engine-json <json>] \
    [--models-dir <dir> --audio <wav> [--stream]]
```

The suite does not download or delete in the ready level, but the engine can write to the models folder (for example, a cache).

Where it runs:

- `cargo test -p sayso-engine` runs the core and model levels against the Rust engine, on Linux and Windows.
- `cargo test -p sayso-engine-client` runs the core and ready levels against `sayso-fake-engine`. The fake engine keeps no download or load state, so it does not run the model level.
- The macOS job of CI builds the Swift engine and runs the core and model levels against it.

A feature is in this spec only when each engine passes its tests, or each engine that does not pass leaves the feature out of `features`.

## Next

These parts come in step 3 of `docs/extensions.md`, each with a feature: VAD, diarization, and public model ids.

## License

Copyright (c) 2026 Chris Watson

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
