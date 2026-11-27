# Host API, version 1

The Host API is how an app uses Sayso: it lists and downloads models, transcribes files, runs live sessions, runs the styles of the user, and observes dictations. Sayso is the host. The app is the client.

The examples of every message are in `testdata/host/*.ndjson`. An SDK decodes and encodes each line of that corpus in its tests.

## Transport

- macOS and Linux: a Unix socket. Windows: a named pipe. `paths.md` gives the path for each system.
- Each message is one JSON object on one line of UTF-8, with `\n` at the end.
- Each message has `"v": 1` and a string `type`. A request can have a string `id`.
- The host writes the keys in this order: `type`, the fields of the message, `v`, then `id`. A client must not depend on the order.
- Both sides ignore fields that they do not know.

## Requests, replies, and events

- A request with an `id` gets exactly one reply: `ok` or `error`, with the same `id`.
- An event that a request causes before its reply has the `id` of that request, and comes before the reply. No event with that `id` comes after the reply.
- The events of a job or a live session come after the reply that started it. They have no `id`. Their `job` or `session` field says which work they belong to.
- A request of a type that the host does not know gets `error` with no `code` and the message `unknown request type <type>`.
- A known request with a missing or wrong field gets `error {code: "bad_request"}`.
- A line that is not a JSON object with a string `type` gets `error {code: "bad_request"}` without `id`.

### `error`

| Field | Type | |
|---|---|---|
| `code` | string, optional | one of the error codes below, or a code that this version does not know |
| `message` | string | English, for people |
| `capability` | string, optional | with `code: "capability"`: the capability that is missing |
| `reason` | string, optional | a stable reason of the engine, such as `download_failed` |

## Hello and approval

The first request is `hello`. Every other request before it gets `error {code: "not_approved"}`.

| Request field | Type | |
|---|---|---|
| `sdk` | string, optional | name and version of the SDK, for logs |
| `protocol` | integer | 1 |
| `features` | string list | features of the client; empty in version 1 |
| `app` | `{name}`, optional | the name that the app gives itself |
| `capabilities` | string list, optional | the capabilities that the app asks for |

The reply is `ok` with:

| Field | Type | |
|---|---|---|
| `host_version` | string | the version of Sayso |
| `protocol` | integer | 1 |
| `features` | string list | the optional parts of the Host API that this host serves (see "Host features") |
| `capabilities` | string list | the capabilities that the user granted |
| `denied` | string list | the capabilities that the user refused, and the asked names that this version does not know |

The host knows an app by its executable, not by a token. The first `hello` of an executable, and a `hello` that asks for capabilities that the user has not answered, asks the user. While the question is open, the host sends the event `approval_waiting` with the `id` of the `hello`, and the reply waits for the answer.

- The user refused the app: `error {code: "not_approved"}`, and the host closes the connection.
- At most 4 connections of one app wait, and at most 4 apps wait. One more gets `error {code: "busy"}`.
- When a question goes away without an answer, the app gets `error {code: "busy"}` with no new question for 30 s.

## Capabilities

| Capability | Needed by |
|---|---|
| `models.read` | `list_models` (always granted) |
| `models.download` | `download_model` |
| `transcribe.file` | `transcribe_file` |
| `session.live` | `session_start`, `session_audio` |
| `audio.mic` | `session_start` with the source `mic` |
| `audio.system` | `session_start` with the source `system` |
| `ai.enhance` | `enhance` |
| `dictation.observe` | `subscribe` |

`hello`, `bye`, `cancel_job`, and `session_stop` need no capability. A request without its capability gets `error {code: "capability", capability}`.

## Host features

`hello.features` lists what this host serves. A client checks a feature before it sends a request that needs it, and gives the app an `unsupported` error itself when the feature is missing.

| Feature | Meaning |
|---|---|
| `dictation_done` | `subscribe {events: ["dictation_done"]}` |
| `download_question` | `download_model` asks the user |
| `enhance` | `enhance` (each request can still fail when the user has no AI set up) |
| `silence_limit` | the field `silence_limit_ms` of `session_start` |
| `record_to` | the field `record_to` of `session_start` |
| `mic_capture` | the source `mic` |
| `system_capture` | the source `system` |
| `echo_rule` | with both `mic` and `system`, the host drops microphone segments that repeat the system audio |
| `vad`, `diarization`, `segments`, `detected_language` | the engine of the host has this engine feature |

An empty `features` list comes from a host older than this table. A client then sends each request and handles the `error` reply.

## Requests

| Request | Fields | Reply and events |
|---|---|---|
| `hello` | see above | see above |
| `list_models` | `role?` | `ok {models}` |
| `download_model` | `model`, `reason` | `model_state` and `download_progress` events with the `id`, then `ok`, or `error` |
| `transcribe_file` | `path`, `model?`, `vad_model?`, `language?`, `diarize?`, `diarization_model?`, `speakers?` | `ok {job}`, then the events of the job |
| `cancel_job` | `job` | `ok`; the job ends with `job_failed` |
| `session_start` | `sources`, `model?`, `preview_model?`, `vad_model?`, `language?`, `diarize?`, `diarization_model?`, `record_to?`, `silence_limit_ms?` | `ok {session, excludes}`, then the events of the session |
| `session_audio` | `session`, `pcm` | none without `id`; `ok` with `id`. An error without `id` when the audio is wrong. |
| `session_stop` | `session` | the last `segment` and `speakers` events, then `ok {text}` |
| `enhance` | `text`, `style?` or `prompt?`, `provider?` | `ok {text, provider, model}` |
| `subscribe` | `events` | `ok {events}`: the events that the connection gets now |
| `bye` | | `ok`, then the host closes the connection |

### Models

`list_models` lists the local models of this computer. With `role` (`final`, `streaming`, `vad`, `diarization`), only the models with that role. A role that this version does not know gives an empty list. Each model:

| Field | Type |
|---|---|
| `id`, `name` | string |
| `roles` | role list |
| `state` | `not_downloaded`, `downloading`, `downloaded`, `optimizing`, `ready`, `failed` |
| `languages` | string list |
| `timestamps` | `word`, `segment`, `none` |
| `size_bytes` | integer |
| `choice` | role list, optional: the roles for which the host uses this model when a request names none |

`download_model`: an unknown id gets `unknown_model`. A model on disk gets `ok` at once. A model that downloads now is followed, with no question. Otherwise the host asks the user, with the `reason` of the app (cut to 300 characters). **Cancel** gives `declined`. A failed download ends with `model_state {state: "failed", message, reason}`, then `error {message, reason}` with no `code`. When the engine does not run, `list_models` and `download_model` get `unsupported`.

### Jobs

`transcribe_file` reads WAV, FLAC, MP3, AAC, and M4A at an absolute `path`. It checks its models before the reply: `unknown_model`, `wrong_role`, `model_not_downloaded`, `unsupported`, `bad_request` (a relative path, `speakers: 0`).

After `ok {job}`: `job_progress {job, fraction}`, `segment {job, start_ms, end_ms, text, words?}` for each segment, `speakers {job, turns}` with `diarize`, then exactly one of `job_done {job, text}` or `job_failed {job, message, code?}`. `cancel_job` of a job of this connection gives `ok` and `job_failed` with the message `The job was cancelled.`. A job of another connection gets `bad_request`. One job runs at a time; the others wait with `fraction: 0`.

### Live sessions

One live session runs at a time on the host. A second `session_start` gets `busy`. Sources:

- `mic`: the microphone that the user chose in Sayso.
- `system`: the sound that the computer plays. `excludes` in the reply lists what the capture leaves out (`sayso`, `app`).
- `client`: audio that the app sends with `session_audio`: base64 of 16 kHz mono little-endian 16-bit PCM.

A source that this host cannot capture gets `unsupported`. A capture that the system refuses gets `permission`. When the engine has no free stream, `too_many_streams`.

Events: `partial {session, source, committed, tentative}`, `segment {session, source, start_ms, end_ms, text}`, `source_gap {session, source, ms}`, `source_ended {session, source, reason}` (`permission` or `failed`), `part_failed {session, source, start_ms, end_ms, message, code?}`, `silence_limit {session, silence_ms}` (once, with `silence_limit_ms`), `speakers {session, source, turns}` at the stop with `diarize`, and `session_ended {session, text, code?}` when the session ends without `session_stop` (the user stopped it, or the grant went away).

`record_to` is an absolute folder. The host writes `mic.wav`, `system.wav`, and `client.wav` there.

With `mic` and `system` together, the host drops a microphone segment that a system segment covers in time and in at least 60 % of its words.

### Enhance

`enhance` runs a style of the user, or a free `prompt`, on `text`, with an AI provider of the user. Errors: `bad_request` (no text, both `style` and `prompt`, an empty prompt, no such style or provider, a style with no AI), `unavailable` (AI is off or not set up, the provider failed), `unsupported` (a prompt for a cleanup model), `timeout`, `refused`, or no `code` (a reply that Sayso cannot use).

### Subscribe

`subscribe` replaces the events of the connection. Names that this version does not know are left out of the reply. An empty list stops the events. After each dictation: `dictation_done {text, transcript, app, site, style, time}`, where `app`, `site`, and `style` can be `null`, and `time` is RFC 3339 in UTC. No event comes for a dictation with incognito on.

## Events

| Event | Fields |
|---|---|
| `model_state` | `model`, `state`, `load_ms?`, `preview?`, `message?`, `reason?` |
| `download_progress` | `model`, `fraction`, `bytes_done`, `bytes_total` |
| `job_progress` | `job`, `fraction` |
| `segment` | `job?`, `session?`, `source?`, `start_ms`, `end_ms`, `text`, `words?` |
| `speakers` | `job?`, `session?`, `source?`, `turns: [{start_ms, end_ms, speaker}]` |
| `job_done` | `job`, `text` |
| `job_failed` | `job`, `message`, `code?` |
| `partial` | `session`, `source?`, `committed`, `tentative` |
| `source_gap` | `session`, `source`, `ms` |
| `source_ended` | `session`, `source`, `reason` |
| `part_failed` | `session`, `source`, `start_ms`, `end_ms`, `message`, `code?` |
| `session_ended` | `session`, `text`, `code?` |
| `silence_limit` | `session`, `silence_ms` |
| `dictation_done` | `text`, `transcript`, `app?`, `site?`, `style?`, `time` |
| `approval_waiting` | |
| `host_closing` | |

`speaker` in a turn is a string label, unique in one source of one job or session. A client also reads a number there. `fraction` is a float.

A client ignores an event type that it does not know.

## Error codes

| Code | Meaning |
|---|---|
| `not_approved` | the user refused the app, or did not approve it |
| `capability` | the user did not grant the capability in `capability` |
| `busy` | the host cannot take this now (question queue full, a live session runs) |
| `declined` | the user said no to a download |
| `model_not_downloaded` | the model is not on disk |
| `wrong_role` | the model cannot do this |
| `unsupported` | this host cannot do it |
| `bad_request` | a field is missing or wrong |
| `too_many_streams` | the engine has no free stream |
| `unknown_model` | no model has this id |
| `model_retired` | the model was taken out of the catalog |
| `unavailable`, `refused`, `timeout` | an AI provider failed |
| `permission` | the system refused a capture permission |
| `host_not_running` | (SDK) no host answers |
| `connection_lost` | (SDK) the connection ended without `host_closing` |
| `host_closing` | (SDK) the host said `host_closing` |

A code that this version does not know is kept as it is.

## The end of a connection

- `bye`: the host replies `ok` and closes.
- When Sayso quits, the host stops the live session, ends each job with `job_failed`, sends `host_closing`, and closes. An SDK fails each open request with `host_closing`.
- When the connection ends without `host_closing`, an SDK fails each open request with `connection_lost`, and gives each open job and session one end.

## Numbers

Job and session ids are integers below 2^53. `fraction` is a float in `[0, 1]`. Times are milliseconds.
