# Host API reference

This page describes the Host API: the messages between a connected app and Sayso, and the way the SDK shows them. The names come from the crates `sayso-protocol` and `sayso-sdk` at version 0.13.0. When this page and the code differ, the code is right.

An app that uses `sayso-sdk` does not write these messages. It calls methods of `Speech` and reads `Event` values. [The SDK on top of the Host API](#the-sdk-on-top-of-the-host-api) maps one to the other.

- [Transport](#transport)
- [Requests](#requests)
- [Replies](#replies)
- [Events](#events)
- [Data types](#data-types)
- [Error codes](#error-codes)
- [Capabilities](#capabilities)
- [Scheduling and limits](#scheduling-and-limits)
- [What differs between modes](#what-differs-between-modes)
- [The SDK on top of the Host API](#the-sdk-on-top-of-the-host-api)

## Transport

The app connects to a local socket. Sayso listens on it. The SDK finds the socket with `sayso_protocol::socket::host_socket_path`, in this order:

1. The path in the environment variable `SAYSO_HOST_SOCKET`, when it is set and not empty.
2. Linux: `$XDG_RUNTIME_DIR/sayso/host.sock`. In a Flatpak: `$XDG_RUNTIME_DIR/app/<FLATPAK_ID>/host.sock`. Without `XDG_RUNTIME_DIR`: `/tmp/sayso-<uid>/host.sock`.
3. macOS: `sayso/host.sock` in the temporary folder of the user (`confstr(_CS_DARWIN_USER_TEMP_DIR)`). Without it: `/tmp/sayso-<uid>/host.sock`.
4. Other Unix systems: `/tmp/sayso-<uid>/host.sock`.
5. Windows: the named pipe `\\.\pipe\sayso-host-<user SID>`.

On macOS and Linux the folder of the socket has mode `0700`. Use `Options::socket` to set another path.

Each message is one JSON object on one line of UTF-8, ended by `\n`. Every message has these fields:

| Field | Type | Meaning |
|---|---|---|
| `v` | integer | The protocol version, `1`. A reader does not check it. |
| `type` | string | The name of the message. |
| `id` | string, optional | The id of a request. A reply and the events that the request caused have the same `id`. |

Rules for readers:

- A reader ignores a field that it does not know.
- A client ignores a message type that it does not know. A host replies to an unknown request type with `error` (no `code`) and the `id` of the request.
- A line that is not a JSON object with a string `type` gets `error {code: "bad_request"}` without `id`.
- A known request with a missing field or a field of the wrong type gets `error {code: "bad_request"}` with its `id`.
- A reader keeps a capability or an error code that it does not know.

Rules for order:

- A request with an `id` gets exactly one reply: `ok` or `error`.
- An event that a request caused has its `id` and comes before the reply. No event with that `id` comes after the reply.
- The events of a job or a live session come after the reply that started it. They have no `id`. Their `job` or `session` field says which work they belong to.
- A job ends with exactly one `job_done` or `job_failed`, and no event of the job comes after it.
- Sayso sends `host_closing` last, then closes the connection.

Ids of jobs and sessions are integers. In Sayso, the host chooses them. The SDK gives your app its own ids and maps them (see [Differences](#what-differs-between-modes)).

Audio in messages is `pcm`: base64 of 16 kHz, mono, little-endian 16-bit samples (`sayso_protocol::pcm`).

## Requests

The first request is `hello`. The host knows an app by its executable. The first `hello` of an identity can show a question to the user, and the host holds the reply until the user answers.

| Request | Capability | Reply |
|---|---|---|
| [`hello`](#hello) | none | `ok` with `host_version`, `protocol`, `features`, `capabilities`, `denied` |
| [`list_models`](#list_models) | `models.read` | `ok {models}` |
| [`download_model`](#download_model) | `models.download` | `ok` after `model_state` and `download_progress` events |
| [`transcribe_file`](#transcribe_file) | `transcribe.file` | `ok {job}` |
| [`cancel_job`](#cancel_job) | none | `ok` |
| [`session_start`](#session_start) | `session.live`, and `audio.mic` or `audio.system` for those sources | `ok {session, excludes}` |
| [`session_audio`](#session_audio) | `session.live` | none |
| [`session_stop`](#session_stop) | none | `ok {text}` |
| [`enhance`](#enhance) | `ai.enhance` | `ok {text, provider, model}` |
| [`subscribe`](#subscribe) | `dictation.observe` | `ok {events}` |
| [`bye`](#bye) | none | `ok`, then the host closes the connection |

A request that needs a capability that the user did not grant gets `error {code: "capability", capability}`. The host reads the grants again at each request, so a grant that the user takes back stops the next request.

### `hello`

| Field | Type | Meaning |
|---|---|---|
| `sdk` | string, optional | The name and version of the client, for example `sayso-sdk 0.13.0`. |
| `protocol` | integer | The protocol version the client speaks. `1`. |
| `features` | array of strings | The optional parts of the protocol that the client knows. Default: empty. |
| `app` | `{name}`, optional | What the app says about itself. The host does not check it. It shows the name as a quote of the app, cuts it to 40 characters, and removes control characters. |
| `capabilities` | array of [capabilities](#capabilities) | What the app asks for. The host asks the user about the ones that have no answer yet. |

Errors: `not_approved` (the user refused the app, or the host cannot trust the identity of the program; the host closes the connection), `busy` (too many apps or connections wait for a question), `bad_request` (an earlier `hello` of this connection still waits for the user).

### `list_models`

| Field | Type | Meaning |
|---|---|---|
| `role` | [role](#roles), optional | Return only models with this role. A role that the host does not know returns an empty list. |

The list has the local models of this computer. It has no cloud models, and Basic VAD is not in it. Error: `unsupported` when the engine does not run.

### `download_model`

| Field | Type | Meaning |
|---|---|---|
| `model` | string | The id of a model. |
| `reason` | string | Why the app needs the model. The host shows it to the user in the question and cuts it to 300 characters. |

In Sayso mode, the host asks the user first, unless the model is on disk or already downloads. A model on disk gets `ok` at once with no event. The events `model_state` (`downloading`), `download_progress`, and `model_state` (`downloaded`) come before `ok` and have the `id` of the request. At most 8 progress events come each second.

Errors: `unknown_model`, `declined` (the user said no), `unsupported` (the engine does not run). A failed download sends `model_state {state: "failed", message, reason}` and then `error {message, reason}` without `code`.

### `transcribe_file`

| Field | Type | Meaning |
|---|---|---|
| `path` | string | An absolute path. Required. |
| `model` | string, optional | A speech model with the role `final`. |
| `vad_model` | string, optional | A voice detection model, or `basic`. |
| `language` | string, optional | A code such as `en`. |
| `diarize` | boolean, optional | `true` turns speaker labels on. `false` turns them off. |
| `diarization_model` | string, optional | A diarization model. Without `diarize`, it turns diarization on. |
| `speakers` | integer, optional | The number of speakers, when the app knows it. It must be 1 or more. |

A field that is missing takes the choice of the user in Sayso mode, and the choice of the app (`Options::models`) in local mode. The reply `ok {job}` comes at once. The file formats are WAV, FLAC, MP3, AAC, and M4A.

The request checks its models before the reply. Errors: `unknown_model`, `wrong_role`, `model_not_downloaded` (also for `diarize` without a diarization model on disk), `unsupported` (the engine does not run or lacks the feature `vad` or `diarization`), and `bad_request` (a path that is not absolute, or `speakers: 0`). A file that cannot be read ends the job with `job_failed`, with a message that names the file and the formats.

### `cancel_job`

| Field | Type | Meaning |
|---|---|---|
| `job` | integer | The id of a job of this connection. |

The reply is `ok`, also for a job of this connection that ended already. The job gets `job_failed` with the message `The job was cancelled.` and no `code`. A job that this connection did not start gets `bad_request`.

### `session_start`

| Field | Type | Meaning |
|---|---|---|
| `sources` | array of [sources](#sources) | Required. `mic`, `system`, `client`. |
| `model` | string, optional | The final model. |
| `preview_model` | string, optional | The model for `partial` events. It needs the engine feature `concurrent_streams`. |
| `vad_model` | string, optional | A voice detection model, or `basic`. |
| `language` | string, optional | A code such as `en`. |
| `diarize` | boolean, optional | Run diarization on each source when the session stops. |
| `diarization_model` | string, optional | A diarization model. |
| `record_to` | string, optional | An absolute path of a folder. The host makes it and writes one 16 kHz mono WAV for each source: `mic.wav`, `system.wav`, `client.wav`. |

The reply is `ok {session, excludes}`. `excludes` lists what the capture of the source `system` leaves out: `sayso`, `app`, or both. It is empty without that source and on Windows and Linux.

Errors: `busy` (another live session runs), `too_many_streams`, `permission` (the system refused a capture permission), `unsupported` (for `system` on macOS 14.0 and 14.1), and the model errors of `transcribe_file`.

### `session_audio`

| Field | Type | Meaning |
|---|---|---|
| `session` | integer | The id of a session. |
| `pcm` | string | Base64 of 16 kHz mono 16-bit little-endian samples. |

For the source `client` only. Without an `id`, there is no reply, and an error comes as `error` without `id`. With an `id`, the reply is `ok` or `error`.

### `session_stop`

| Field | Type | Meaning |
|---|---|---|
| `session` | integer | The id of a session of this connection. |

The host sends the last `segment` events and the `speakers` events, then `ok {text}`. `text` is the text of all segments in the order of time. If the user already stopped the session in Sayso, there is no reply, and `session_ended` is the end.

### `enhance`

Sayso mode only. Runs a style of the user, or a free prompt, on text with an AI provider of the user.

| Field | Type | Meaning |
|---|---|---|
| `text` | string | The text. Required, not empty. |
| `style` | string, optional | The id or name of a style. Without `style` and `prompt`, the active style. |
| `prompt` | string, optional | An instruction that runs as a style of its own. Not with `style`. |
| `provider` | string, optional | The id or name of a provider. |

Errors: `bad_request`, `unavailable`, `unsupported`, `timeout`, `refused`. An output that Sayso cannot use gives `error` without `code`.

### `subscribe`

Sayso mode only.

| Field | Type | Meaning |
|---|---|---|
| `events` | array of strings | Event types, for example `dictation_done`. |

The list replaces the earlier list of the connection. The reply `ok {events}` lists the events that the connection gets now. A name that the host does not know is not in it, and that is not an error. An empty list stops the events.

### `bye`

The host replies `ok`, then closes the connection.

## Replies

A reply is `ok` or `error`.

### `ok`

`ok` has the fields of its request:

| To | Fields |
|---|---|
| `hello` | `host_version` (string), `protocol` (integer), `features` (array of strings), `capabilities` (granted), `denied` (asked for and not granted, or not known to this version of Sayso) |
| `list_models` | `models`: array of [`ModelInfo`](#modelinfo) |
| `transcribe_file` | `job` (integer) |
| `session_start` | `session` (integer), `excludes` (array of strings) |
| `session_stop` | `text` (string) |
| `enhance` | `text`, `provider`, `model` (strings) |
| `subscribe` | `events` (array of strings) |
| `download_model`, `cancel_job`, `bye` | none |

### `error`

| Field | Type | Meaning |
|---|---|---|
| `code` | string, optional | A [code](#error-codes) that an app can act on. |
| `message` | string | English text for people. |
| `capability` | string, optional | With the code `capability`: the capability that the user did not grant. |
| `reason` | string, optional | A reason code of the engine protocol, for example `download_failed`. |

## Events

| Event | Fields | Sent when |
|---|---|---|
| `model_state` | `model`, `state`, `load_ms?`, `preview?`, `message?`, `reason?` | The state of a model changed during a request. `load_ms` is set with `ready`. `message` and `reason` are set with `failed`. |
| `download_progress` | `model`, `fraction`, `bytes_done`, `bytes_total` | A download runs. |
| `job_progress` | `job`, `fraction` | A job advances. `fraction` goes from 0 to 1. A job that waits in the queue sends 0. |
| `segment` | `job?`, `session?`, `source?`, `start_ms`, `end_ms`, `text`, `words?` | A part of a transcript. |
| `speakers` | `job?`, `session?`, `source?`, `turns` | The speakers of a job, or of one source of a session. |
| `job_done` | `job`, `text` | A job ended. |
| `job_failed` | `job`, `message`, `code?` | A job failed or was cancelled. |
| `partial` | `session`, `source?`, `committed`, `tentative` | The text of one source so far. |
| `source_gap` | `session`, `source`, `ms` | A source lost audio. |
| `source_ended` | `session`, `source`, `reason` | One source ended and the others go on. |
| `session_ended` | `session`, `text`, `code?` | A session ended without `session_stop`. |
| `dictation_done` | `text`, `transcript`, `app?`, `site?`, `style?`, `time` | A dictation was inserted. Needs `subscribe`. |
| `approval_waiting` | none | `hello` waits for the answer of a person. `Speech` handles this event, so the app sees it only with `sayso_sdk::client`. |
| `host_closing` | none | Sayso quits or restarts. |

Details:

- `segment` has `job` for a job, and `session` with `source` for a live session. Its times are in milliseconds from the start of the file or the session. `words` is set when the model has the timestamp level `word`.
- `speakers` comes after the last `segment` of a job. For a live session, one comes for each source, after the last `segment` and before the reply to `session_stop`.
- `partial`: `committed` does not change any more. `tentative` can change. `PartialEvent::text()` gives both as one text, with one space where the text needs one: not before punctuation that closes, and not in a script without spaces.
- `source_gap`: the recording and the times of the source have `ms` of silence there, at most 60 seconds. It comes at a change of the device, and after the computer sleeps.
- `source_ended`: the last `segment` of the source comes before it. `reason` is `permission` or `failed`.
- `session_ended`: the user stopped the session in Sayso, or the SDK ended it because the connection ended. In that case `code` is set (`connection_lost` or `host_closing`). The last `segment` and `speakers` events come before it. There is no reply to `session_stop` after it.
- `dictation_done`: `text` is the inserted text, `transcript` is the text of the speech model before replacements and the style, `app` is the name of the app, `site` the host of the web page, and `style` the id of the style. `time` is in RFC 3339 and UTC. No event comes while incognito is on.
- `host_closing`: the host ends each job with `job_failed` and stops each live session before it. The SDK does not give this event to the app.

## Data types

### `Segment`

| Field | Type | Meaning |
|---|---|---|
| `start_ms`, `end_ms` | integer | Times in milliseconds. |
| `text` | string | The text. |
| `words` | array of `Word`, optional | Each word has `start_ms`, `end_ms`, and `text`. |

### `Turn`

| Field | Type | Meaning |
|---|---|---|
| `start_ms`, `end_ms` | integer | Times in milliseconds. |
| `speaker` | string | A label such as `"1"`. It is unique in one source of one job or session. |

### `ModelInfo`

| Field | Type | Meaning |
|---|---|---|
| `id` | string | The stable id. |
| `name` | string | The name for people. |
| `roles` | array of [roles](#roles) | What the model can do. |
| `state` | [state](#model-states) | The state on this computer. |
| `languages` | array of strings | Language codes. Empty when the model has no list. |
| `timestamps` | `word`, `segment`, or `none` | How much timing the model gives. |
| `size_bytes` | integer | The size of the download. |
| `choice` | array of roles | The roles for which the host or the SDK uses this model when a request names none. Absent when empty. |

### Sources

`mic`, `system`, `client`. `client` is audio that the app sends with `session_audio`.

### Roles

`final` (the final pass on a recording), `streaming` (text while audio comes in), `vad` (voice detection), `diarization` (who speaks when).

### Model states

`not_downloaded`, `downloading`, `downloaded`, `optimizing` (a load runs), `ready` (in memory), `failed`.

## Error codes

| Code | Sent by | Meaning |
|---|---|---|
| `not_approved` | Sayso | The user did not approve the app, or the host cannot trust its identity. The host closes the connection. |
| `capability` | Sayso | The user did not grant the capability in `capability`. |
| `busy` | Sayso, local mode | Another live session runs, or too many apps or connections wait for an approval question. |
| `declined` | Sayso | The user said no to a download. |
| `model_not_downloaded` | both | The model is not on disk. |
| `wrong_role` | both | The model cannot do the role of the request. |
| `unsupported` | both | The engine, the mode, or the system cannot do it. |
| `bad_request` | both | A field is missing or wrong, or the id names no work of the app. |
| `too_many_streams` | both | The engine cannot open one more stream. |
| `unknown_model` | both | The id is not in the catalog. |
| `model_retired` | both | The model left the catalog. |
| `unavailable`, `refused`, `timeout` | Sayso | The AI provider of `enhance` is not available, refused the text, or gave no reply in time. |
| `permission` | both | The system refused a capture permission. |
| `host_not_running` | SDK only | Sayso does not run. |
| `connection_lost` | SDK only | The connection to Sayso ended without `host_closing`. A request may have run or not. |
| `host_closing` | SDK only | Sayso quit and did not end this work first. |

A code that this version does not know reads as `ErrorCode::Other(String)`.

## Capabilities

| Capability | Allows |
|---|---|
| `models.read` | List models. Always granted. |
| `models.download` | Ask the user to download a model. |
| `transcribe.file` | Jobs. |
| `session.live` | Live sessions with audio from the app. |
| `audio.mic` | Live sessions from the microphone. |
| `audio.system` | Live sessions from the system audio. |
| `ai.enhance` | The AI providers and styles of the user. |
| `dictation.observe` | `dictation_done` events. |

## Scheduling and limits

- The engine does work in this order: dictation, live sessions, jobs.
- One live session runs at a time. A second `session_start` gets `busy`.
- One job runs at a time. Other jobs wait in a queue, first in, first out, and send `job_progress` with `fraction: 0`.
- A job waits while a dictation runs or while a part of a live session is in the engine. A job does not stop a live session.
- In Sayso mode, these limits are for all apps together. In local mode, they are for your app.
- Approval questions: one on the screen at a time. At most 4 connections of one identity and 4 identities wait. More get `busy`. After the last waiting connection of an identity closes, that identity gets `busy` for 30 seconds.
- The echo rule: in a session with `mic` and `system`, a microphone segment is dropped when a system segment covers at least half of its time and at least 60 % of its words. A segment of 3 words or fewer must also be at least half of the system words of its time, so a short real reply stays. A microphone segment can wait for the system source, up to 60 seconds.
- A gap of more than 10 seconds in a source gives `source_gap`. A capture that does not come back for 60 seconds ends with `source_ended {reason: "failed"}`.

## What differs between modes

The messages and the end rules are the same in both modes. These things differ:

| Part | Sayso mode | Local mode |
|---|---|---|
| `hello`, `denied`, approval | The host asks the user. | None. The SDK does not talk to Sayso. |
| `download_model` | The user is asked, and can say no (`declined`). | The download starts at once. `model_state` and `download_progress` come only for downloads that your app started. |
| Models for a request that names none | The choice of the user. | `Options::models`, then the first suitable model on disk. |
| `enhance`, `subscribe`, `dictation_done` | Work if granted. | `unsupported`. |
| Queues and limits | For all apps together. | For your app. |
| Priority | Dictation, live sessions, jobs. | Live sessions, jobs. |
| `list_models` and `choice` | The state and choice of Sayso. | The state of the engine of the app, and the choice of the app. |
| Features | `Features::host`, from `hello`. | `Features::engine`, from the engine of the app. |
| `excludes` on macOS | `["sayso", "app"]` | `["app"]` |
| `partial` and `segment` text | From the engine of Sayso. | From the engine of the app. The same model can give a different text when the engine versions differ. |
| Error messages | Written by Sayso. | Written by the SDK. The errors of the shared code are true in both modes, and do not name Sayso. |

Only the SDK gives these:

- `Event::ModeChanged { mode, features }`, when `Mode::Auto` changes mode, or `reconnect` connects.
- `Event::Disconnected { code }`, when a `Mode::Sayso` connection ends.
- `Event::Error(ErrorReply)`, for an `error` that belongs to no request.
- The error codes `host_not_running`, `connection_lost`, and `host_closing`.
- The ids of jobs and sessions. They count up from 1 in the SDK, in both modes, and no id comes back after a change of mode.
- The `code` of `session_ended`.

## The SDK on top of the Host API

### `Speech`

| Method | Request | Returns |
|---|---|---|
| `Speech::connect(Options)` | `hello` (Sayso mode) | `Result<Speech, Error>` |
| `mode()` | | `ActiveMode::Sayso` or `ActiveMode::Local` |
| `features()` | | `Features` |
| `events()` | | A `Receiver<Event>`. All clones take from one channel. |
| `list_models(Option<Role>)` | `list_models` | `Vec<ModelInfo>` |
| `download_model(&str, &str)` | `download_model` | `()` |
| `transcribe_file(TranscribeFile)` | `transcribe_file` | The job id (`u64`) |
| `cancel_job(u64)` | `cancel_job` | `()` |
| `start_session(SessionStart)` | `session_start` | `SessionReply {session, excludes}` |
| `session_audio(u64, &[i16])` | `session_audio` | `()` |
| `stop_session(u64)` | `session_stop` | The text (`String`) |
| `enhance(Enhance)` | `enhance` | `EnhanceReply` |
| `subscribe(&[&str])` | `subscribe` | The event names (`Vec<String>`) |
| `reconnect()` | `hello` | `()`. For `Mode::Sayso` only. `connection_lost` when the new connection ended at once. |

The calls block. `&Speech` can be shared between threads. Dropping the `Speech` ends its work, closes the connection or stops the engine.

### `Options`

| Item | Meaning |
|---|---|
| `Options::new(mode, app)` | `mode` is `Mode::Sayso { start }`, `Mode::Local`, or `Mode::Auto`. `app` is `App::new("Name")`. |
| `.capabilities([..])` | The capabilities that the app asks for. |
| `.engine(Engine)` | The engine of the app. Needed for `Local` and `Auto`. |
| `.models(ModelChoices)` | The choices of the app for local mode: `final_model`, `preview_model`, `vad_model`, `diarization_model`, `language`. |
| `.socket(path)` | Another socket for Sayso. |
| `.timing(Timing)` | `watch` (200 ms), `retry` (30 s), `start_wait` (10 s), `hello_limit` (10 s), `keep_model` (60 s). |
| `.capture(CaptureSources)` | Another microphone and system audio, instead of the real capture. |

`Mode::Sayso { start: true }` starts Sayso when it does not run and waits up to `Timing::start_wait`. `Mode::Auto` never starts Sayso.

### `Engine`

`Engine::from_release(folder)`, `Engine::from_program(path)`, `.store(path)`, and `.models_dir(path)`. See [How to ship the engine in your app](ship-the-engine.md).

### `Features`

| Field | Meaning |
|---|---|
| `enhance`, `subscribe`, `download_question` | True in Sayso mode only. |
| `system_audio` | The source `system` can start. In Sayso mode, the user granted `audio.system`. In local mode, this system supports it and did not refuse the app. |
| `capabilities`, `denied` | What the user granted and refused (Sayso mode). In local mode, `capabilities` lists all, and `denied` is empty. |
| `host` | The optional parts of the Host API (`hello.features`), Sayso mode. Empty today. |
| `engine` | The features of the engine, in local mode after the engine started. |

### `Event`

| Variant | Meaning |
|---|---|
| `Event::Message(protocol::Event)` | An event of [Events](#events), with the ids of the SDK. |
| `Event::ModeChanged { mode, features }` | The mode changed. |
| `Event::Error(ErrorReply)` | An error that belongs to no request. |
| `Event::Disconnected { code }` | A `Mode::Sayso` connection ended. |

### The plain client

`sayso_sdk::client::Client` is a client of the Host API without local mode. It gives the messages as they are: `Client::connect`, `connect_or_start`, `request`, `send`, `events`, and `reconnect`. Most apps use `Speech`.
