# Extensions

Status: proposed on 2026-10-23. Not built. Chris confirmed the decisions in "Decisions" on 2026-10-23, and the answers to the open questions the same day. No question is open. The ADRs are in [adr/](adr/).

This file says how other apps use the speech features of Sayso: models, transcription, voice activity detection, audio capture, and the user's AI providers. Sayso stays a dictation app. Meeting notes, media transcription, and the ideas of other developers become extensions: separate apps that Sayso installs, lists, and starts.

## Goal

- Chris can build a meeting notetaker and a media transcriber without adding their UI or their code to Sayso.
- Other developers can build extensions, and users can install the reviewed ones from inside Sayso.
- A developer who does not want Sayso can build their own app on the engine alone.
- Dictation stays as fast as it is today while an extension runs.

"Bloat" here means two things: more UI in Sayso, and a larger Sayso binary. The design adds one button in the sidebar footer, the Marketplace modal, one popover menu, and the speech features that extensions need. Everything else is in the extensions.

## Terms

These go into `CONTEXT.md` with the code.

**Extension**
A separate app that uses the speech features of Sayso through the Host API. Sayso installs it, lists it, and starts it. It has its own process, windows, and data.
_Avoid_: plug-in, add-on, module.

**Host**
Sayso, when it serves extensions.

**Host API**
The protocol between the host and an extension, over a local socket. It is a superset of the engine protocol.

**Engine protocol**
The NDJSON protocol of the engine (`docs/engine-protocol.md`). Sayso uses it, and an app that runs the engine alone uses it.

**Capability**
A permission that an extension declares in its manifest and the user grants at install, for example `audio.mic`.

**Registry**
The GitHub repository with the signed index of reviewed extensions. It is the data. The Marketplace shows it.
_Avoid_: store.

**Marketplace**
The modal in Sayso where the user finds, installs, opens, and manages extensions. It opens from the sidebar footer of the Hub.
_Avoid_: store, extension manager, Extensions page.

**Listing**
The entry of one extension in the registry: its versions, and for each platform, the file, size, and hash.

**Platform**
One system and processor that an extension can support: `macos-arm64`, `windows-x64`, `linux-x64`, or `linux-arm64`. An extension supports one platform or more, not necessarily all of them.

**Sideloaded extension**
An extension that the user installed from a folder in developer mode. Nobody reviewed it.

**VAD model**
A local model that finds the parts of audio that contain speech. VAD means voice activity detection.

**Segment**
A part of a transcript with a start time and an end time, and a speaker when the audio was diarized.

**Job**
A transcription of a file that the host runs in the background for an extension.

**Live session**
Audio from a capture source that the host transcribes for an extension while it arrives.

**Capture source**
Where the audio of a live session comes from: the microphone, the system audio, or the extension itself.

## Decisions

Chris confirmed these on 2026-10-23.

1. **Sayso is the host, and extensions are separate processes.** No code from an extension runs in the Sayso process. Sayso owns the engine, the audio capture, the permissions of the system, the models, the API keys, the dictionary, and the styles. An extension owns its UI and its data. See [ADR 0001](adr/0001-extensions-are-separate-processes.md).
2. **The engine does not become a shared service.** Each engine has one owner. Sayso starts its own engine. An app that uses the engine alone starts its own engine. Sayso serves extensions through the host and not through its engine.
3. **Extensions make their own windows.** An extension is an app. It can have a Dock icon, its own windows, and its own menus, and it can call any API of the system. Sayso does not draw UI for extensions and does not support it either way.
4. **The SDK crates, `sayso-ui`, and the protocol specs are MIT. The app stays GPL-3.0-only.** Extensions can then use the same theme and components as Sayso. See [ADR 0002](adr/0002-mit-sdk-gpl-app.md) and "SDK and licensing".
5. **The registry is a GitHub repository with a signed JSON index.** Each extension lives in the public repository of its author. People review each pull request to the registry. The registry CI checks the files but does not build them. See [ADR 0003](adr/0003-registry-with-human-review.md).
6. **Signing is required on macOS only.** A macOS build must be signed with a Developer ID and notarized to be listed. Windows and Linux builds need no signature, because Sayso itself is not signed on Windows today.
7. **The repository of a listed extension must be public.**
8. **An extension chooses its platforms.** Its manifest names each platform that it supports, and its listing has one file for each of those platforms. The Marketplace shows only extensions that support the platform of the user.
9. **An app outside the registry has two paths.** It can run the engine alone and do what it wants, or the user can sideload it in developer mode. Notarization is not necessary for either path.
10. **VAD is a model role**, like the final and preview roles in [model-roles.md](model-roles.md). The user chooses the VAD model. An extension can ask for a specific VAD model.
11. **Dictation does not use VAD.** VAD is for jobs and live sessions. Cutting silence from a dictation may come later as a separate change.
12. **Extensions can list models and choose a model** for each job and live session: a final model, a streaming model, a VAD model, or a diarization model. A download always needs the consent of the user.
13. **Windows moves to `sayso-engine`, and `native/portable` is deleted.** Then there are two engines, not three. See "Windows engine" and [ADR 0004](adr/0004-one-engine-for-windows-and-linux.md).
14. **Diarization is a model role, and it runs offline in the first version:** on a file, and on the recording of a live session after it stops. Live speaker labels during a session are not in the first version.
15. **Paid extensions are not in the first version.** The registry lists every extension the same way. If an author charges money, that happens outside Sayso.

## Two layers

```
  ┌───────────────────────────── Sayso (host) ──────────────────────────────┐
  │ dictation, capture, permissions, model choice, keys, dictionary, styles │
  │                                                                         │
  │   Host API server ──── scheduler ──── engine client ──── engine         │
  └──────────┬──────────────────────────────────────────────────────────────┘
             │ local socket, NDJSON
     ┌───────┴────────┬──────────────────┐
  Notetaker         Media          other extension

  App without Sayso ──── stdio, NDJSON ──── its own engine
```

| Layer | Who uses it | What it gives |
|---|---|---|
| Engine protocol | Sayso, and apps that run the engine alone | Models, download, load, `transcribe`, streaming, VAD, diarization. No capture, no UI, no keys, no scheduling. |
| Host API | Extensions | All of the engine protocol, plus capture sources, jobs with VAD chunking, model consent, the AI providers and styles of the user, dictation events, and a scheduler that puts dictation first. |

The Host API uses the same message shapes as the engine protocol where the two overlap. `list_models` and `transcribe` are the same messages. So the SDK has one set of types and two transports, and an app can move from the engine alone to an extension with few changes.

The engine is not a shared service, for these reasons:

- A shared engine must capture audio, hold permissions, and serve many clients. That work happens once for each engine (Swift on macOS, `sayso-engine` on Windows and Linux). In the host it happens once, in Rust.
- Only one process can put dictation first. If the engine served clients directly, a client could fill the engine with work while the user dictates.
- An app that runs the engine alone loads its own copy of a model. That costs memory only when the user runs Sayso and that app at the same time. An extension does not have that cost, because it uses the engine of Sayso.

## The engine as a public product

### Release

The Release workflow attaches the engine as separate files for each system: `sayso-engine-<version>-macos-arm64.tar.gz`, `sayso-engine-<version>-linux-x86_64.tar.gz`, `sayso-engine-<version>-linux-arm64.tar.gz`, and `sayso-engine-<version>-windows-x64.zip` (a zip, because Windows opens it without another program). Each file has the engine, the libraries that it needs, `LICENSE`, the third-party licenses, and a short `README.md`, in the folder `sayso-engine-<version>/`. The engine has the version of the Sayso release that it comes with, and a test in `sayso-core` fails when the Swift engine says another version. Its protocol version and its features are in `hello`. On macOS the binary is signed and notarized, as it is in the app bundle today. Sayso still ships its own engine in the bundle and never downloads one.

The engine stays GPL-3.0-only. An app that starts it as a separate program and talks NDJSON to it is not a derived work of the engine. The spec of the protocol is MIT, so anyone can write a client.

An app that runs the engine alone uses its own models directory (`SAYSO_MODELS_DIR`). Two engines that write to one directory at the same time is not supported.

### Changes to the protocol

The protocol becomes a public contract, so it needs these changes before anyone outside the repository depends on it:

1. **Features in `hello`.** The reply gets `features: ["segments", "vad", "concurrent_streams", ...]`. A client checks for a feature before it uses it. New requests add a feature and do not change `v`. `v` changes only when an existing message changes in a way that breaks old clients.
2. **Concurrent streams.** Today a new stream on a model ends the older stream (`Engine.swift`, "One live stream per manager"). A dictation would end the live session of a meeting. Each engine must run at least three streams at the same time, on one model or on more: a dictation, and the microphone and the system audio of a live session.
3. **Segments.** `transcribe` gets `segments: true`. The `final` reply then has `segments: [{start_ms, end_ms, text}]`, with `words` when the model gives word times. The catalog says what each model gives: `timestamps = "none" | "segment" | "word"`.
4. **VAD.** `vad {model, path}` replies `ok {speech: [{start_ms, end_ms}]}`. `vad_start {session, model}` starts VAD on a stream. The client sends audio with `stream_audio`, and the engine sends `speech_start` and `speech_end` events with times.
5. **Diarization.** `diarize {model, path, speakers?}` replies `ok {turns: [{start_ms, end_ms, speaker}]}`. `speakers` is the number of speakers when the client knows it. Without it, the engine finds the number. FluidAudio and sherpa-onnx have diarization.
6. **Model ids are public.** After the first release of the spec, an id does not change. A model that goes away stays in the catalog as `retired`, so a client gets a clear error and not "unknown model".

### Conformance tests

A test suite sends the same requests to each engine and checks the replies against the spec. It runs in CI for the engines that CI can build. `sayso-fake-engine` passes it too, so the tests of the client stay honest. A feature is in the spec only when each engine passes its tests, or each engine that does not support it leaves it out of `features`.

## VAD models

VAD is a model role. The host uses it to cut long audio into parts at pauses, so it can:

- transcribe a long file in parts, with progress, and stop at the end of a part when a dictation starts
- cut a live session into utterances for the final pass
- give segments for models that have no timestamps

| VAD model | macOS | Windows and Linux | Download |
|---|---|---|---|
| Basic (energy level, in the host) | yes | yes | none |
| Silero VAD | FluidAudio (Core ML) | sherpa-onnx | about 2 MB |
| TEN VAD | no | sherpa-onnx | about 1 MB |
| FSMN VAD | FluidAudio | no | small |

Basic VAD runs in the host and needs no engine. A new install uses Basic until the user chooses a VAD model, so long files work on day one.

The catalog (step 4) has Silero VAD on each system, id `silero-vad`: 1,063,427 bytes on macOS and 643,854 bytes on Linux and Windows. TEN VAD is not in the catalog. Its license adds conditions to Apache 2.0: no use that competes with Agora, and no use that lets a third party develop or deploy applications. Sayso lets third parties build extensions, so it cannot accept them for its users. `sayso-engine` still runs TEN VAD for a client that sends its `engine` object. FSMN VAD is not in the catalog, because the Swift engine runs Silero only.

Config: `models.vad = "auto"` (the default) or a model id. Automatic means the best VAD model on disk, else Basic. A job or a live session can name a VAD model. If it names none, the host uses the choice of the user.

Dictation does not use VAD (decision 11).

## Diarization models

Diarization is a model role. It runs offline: on the file of a job, and on the recording of a live session after `session_stop`. sherpa-onnx has only offline diarization, so all platforms work the same way.

| Diarization model | macOS | Windows and Linux |
|---|---|---|
| Speaker diarization (FluidAudio) | `FluidInference/speaker-diarization-coreml` | no |
| Speaker diarization (sherpa-onnx) | no | pyannote segmentation 3.0 and one speaker embedding model |

Each entry is one model in the catalog, although it has more than one file. Both have the id `speaker-diarization`. The macOS entry is 21,787,007 bytes. The sherpa-onnx entry uses `3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx` (28,281,164 bytes), in the constant `SPEAKER_EMBEDDING` of `models_onnx.rs`. The rule for the choice: run the candidates of the sherpa-onnx releases on the same short meeting recordings, in English and in one other language, and take the one with the fewest speaker errors at a speed of at least 10 times real time on the CPU.

The benchmark of 2026-10-26 (sherpa-onnx 1.13.8, an Apple M5 Pro, CPU only) used three 300 s parts of AMI meetings in English (4 speakers each) and three 300 s parts of AISHELL-4 in Mandarin (6 speakers each). The score is the diarization error rate (DER, collar 0.25 s, overlap scored), English / Mandarin:

| Embedding model | MB | DER, number of speakers given | DER, best threshold | Speed, 1 thread |
|---|---|---|---|---|
| 3D-Speaker CAM++ zh_en (chosen) | 28.3 | 28.4 / 17.7 | 16.8 / 17.3 (1.0) | 17.5x |
| NeMo TitaNet small | 40.3 | 30.7 / 28.6 | 17.1 / 23.0 (0.9) | 22.6x |
| 3D-Speaker ERes2NetV2 | 71.4 | 22.0 / 21.7 | 15.7 / 23.2 (0.9) | 3.9x, too slow |
| WeSpeaker ResNet34 (English) | 26.5 | 18.8 / 35.0 | 22.8 / 49.4 (0.7) | 7.7x, too slow |
| WeSpeaker ResNet34 (Chinese) | 26.5 | 19.8 / 37.2 | 19.2 / 33.0 (0.7) | 7.9x, too slow |
| 3D-Speaker CAM++ English | 29.6 | 48.9 / 55.7 | 53.0 / 47.1 (1.1) | 17x |

The default threshold of sherpa-onnx, 0.5, is bad for each model: it finds 20 to 30 speakers, with a DER of 32 to 69. So the Rust engine uses 0.9 when the client gives no number of speakers (`CLUSTER_THRESHOLD` in `crates/sayso-engine/src/diarization.rs`). For CAM++ zh_en the DER at 0.9 is 18.9 / 24.0, and at 1.1 it is 32.8 / 22.2, so 0.9 stays away from that edge. The sample is small, and the best thresholds come from the same data, so these numbers are optimistic. An extension that knows the number of speakers should give it.

FluidAudio also has streaming diarization (Sortformer, LS-EEND). It can give live speaker labels on macOS later, as a separate change.

Config: `models.diarization = "auto"` (the default) or a model id. Automatic means the diarization model on disk. There is no built-in fallback. A request with `diarize: true` and no diarization model on disk gets `error {code: "model_not_downloaded"}`, and the extension can send `download_model`.

In a job, the host gives each segment a speaker (`crates/sayso-host/src/jobs/speakers.rs`):

1. **Small speakers are folded.** Without a number of speakers, the diarizers find extra speakers that say almost nothing: with CAM++ zh_en at the threshold 0.9, the benchmark found 7 to 10 speakers in 300 s parts of 4-speaker meetings, and many of them had only 0.4 to 9 s of speech (0.1 to 2.6 %). So, when the request has no `speakers`, a speaker with less than 2 % of all speech and less than 10 s of speech (`SMALL_SPEAKER_SHARE`, `SMALL_SPEAKER_MS`) is folded: each of its turns goes to the speaker that overlaps it most, else to the speaker of the nearest turn. The speaker with the most speech always stays. The 10 s limit keeps a real speaker of a long meeting, who has a small share but more than a few words. With these rules, the speakers of those results go from 7, 10, and 8 to 5, 7, and 5 in English (4 real), and from 12, 8, and 8 to 8, 5, and 5 in Mandarin (6 real). In two Mandarin parts that is one speaker too few: a person who says very little can be folded. When the request has `speakers`, the host keeps the speakers of the engine.
2. **Each segment gets one speaker:** the speaker with the most time in the segment, else the speaker of the nearest turn. The host does not split a segment at a change of speaker, also for models with word times. The segments go to the extension while the job runs, and diarization runs after the last one. Segments of `word` models end at sentence ends and at pauses of 700 ms, where speakers usually change.
3. **The turns of `speakers {job, turns}` follow the segments.** Each run of segments of one speaker is one turn, from the start of its first segment to the end of its last segment. So each segment is in exactly one turn, and the turns do not overlap. The labels are `"1"`, `"2"`, and so on, in the order in which the speakers first speak.

## Models page

The Models page gets two sections under the speech models: **Voice detection** (VAD models) and **Speakers** (diarization models). They have the same rows and the same actions as the other sections. They show only when an extension is installed. A user who only dictates sees the Models page of today.

## Windows engine

Windows moves from `native/portable` to `crates/sayso-engine`, the engine of Linux. After the move, `native/portable` is deleted. The macOS engine is Swift, and Windows and Linux share `sayso-engine`. Every protocol change in this file is then built two times, not three times. sherpa-onnx gives Windows VAD and diarization with the code that Linux uses.

The Windows build of `sayso-engine` has:

- **sherpa-onnx** for Parakeet, the streaming models, SenseVoice, Moonshine, VAD, and diarization, as on Linux. On Windows it comes as DLLs (`sherpa-onnx-c-api.dll`, `onnxruntime.dll`) beside the engine: its Windows libraries use the static C runtime (`/MT`), and whisper.cpp uses the DLL runtime (`/MD`), so one exe cannot link both statically. The engine delay-loads the DLL, so the stdout redirect comes first.
- **whisper.cpp for Whisper**, through `transcribe-rs` with only the feature `whisper-cpp`. Without the feature `onnx`, `transcribe-rs` brings no ONNX Runtime, so the binary has one ONNX Runtime: the one in sherpa-onnx. Whisper on Windows keeps the speed of whisper.cpp, and the Whisper files that users already downloaded stay valid. Linux keeps Whisper on sherpa-onnx, because parakeet.cpp links its own ggml there, and two ggml copies in one binary can clash (`docs/cleanup-models.md`, decision 1).
- **The live preview by decoding again**, moved from `native/portable/src/preview.rs`. Parakeet TDT v3 keeps its multilingual live preview on Windows, and Linux gets it too. Before 0.12, Parakeet TDT v3 had no live preview on Linux.
- **The stdout redirect for Windows** from `native/portable/src/stdio.rs`, so a print in a C++ library cannot corrupt the protocol.

The Windows catalog is the catalog of `models_onnx.rs` without the parakeet.cpp models (Fluid Speech Mini), plus the Whisper entries of `models/portable.rs` with the kind `whisper_cpp`. The Whisper ids stay the same, and `whisper-large-v3` and `whisper-medium` stay on Windows.

### Models that users already downloaded

Parakeet TDT v2 and v3 keep their ids on Windows, but sherpa-onnx reads other files than `transcribe-rs`. At the first start after the update, the engine finds the old files (`encoder-model.int8.onnx`) in the folder of a Parakeet model and reports `model_state failed` with the new reason `format_changed`. Sayso then:

1. Downloads the final model and the preview model of the user again, if one of them has the reason `format_changed`. The overlay and the Models page show the download as they do today. The download removes the old files when it starts.
2. Deletes the folders of the other Parakeet models with old files. They show as not downloaded.

Dictation with a local model does not work until the download ends. The release notes say so. Whisper files stay: the engine moves each GGML file into its new folder at the first start, and nothing downloads again.

### Check first

Do these checks before the other work of step 2, on a Windows machine or a Windows runner of GitHub Actions:

- `sayso-engine` links with sherpa-onnx and whisper.cpp in one MSVC binary. If the two use different C runtimes (`/MT` and `/MD`), use the feature `shared` of sherpa-onnx and ship its DLLs beside the engine. Answer: they do differ (the sherpa-onnx 1.13.8 Windows archives are `/MT`), so the engine uses `shared`. The release build on the i9-13900K links and runs (2026-10-23).
- The size of the engine binary. Today `SaysoEngine.exe` is 25 MB. Answer: the exe and its DLLs are about 26.7 MB together.
- The speed of Parakeet TDT v3 and Whisper on `short.wav` and `long30.wav`, against the table in `native/portable/NOTES.md`, on the same i9-13900K. Parakeet on sherpa-onnx must not be more than 20 % slower. If it is, stop and decide again. Answer: the old and the new engine ran in turns on the same day. Parakeet on sherpa-onnx is faster: `short.wav` 264 to 293 ms against 296 to 315 ms, `long30.wav` 2066 to 2093 ms against 3469 to 4006 ms. The model loads about 0.4 s slower. The old table (1550 to 1600 ms for `long30.wav`) was not reproduced with the same old binary. `crates/sayso-engine/NOTES.md` has the figures.

## Host API

### Transport

- macOS and Linux: a Unix socket, `host.sock`, in a directory that only the user can open (mode `0700`). Its path is in the env `SAYSO_HOST_SOCKET` when Sayso starts an extension, and the SDK knows the default path.
- Windows: a named pipe, `\\.\pipe\sayso-host-<user SID>`, with an ACL for the user only.
- The messages are NDJSON, as in the engine protocol. Every message has `"v":1`.

This socket is not the command socket of Linux (`crates/sayso-platform-linux/src/ipc.rs`). The two protocols have different users and different rules.

### Who is calling

The host knows a client by its executable, not by a token:

- macOS: the audit token of the peer gives its code signature. The host compares the signature with the one it checked at install (team id and bundle id). An extension that the user opens from Finder or Spotlight connects too.
- Linux: `SO_PEERCRED` gives the pid, and `/proc/<pid>/exe` must be in the install directory of an extension.
- Windows: `GetNamedPipeClientProcessId` gives the pid, and its image path must be in the install directory of an extension.

The host refuses a client that is not an installed extension, with the error `not_an_extension`. An app that is not an extension uses the engine alone, or the user sideloads it.

This check works because the identity of the executable survives a restart of Sayso, for example after an update. A token from the last run of Sayso would not survive. Malware that runs as the same user can get around any of these checks. That is outside what the host can protect against (see "Security").

### Requests

Every request with an `id` gets one reply: `ok` or `error`. Events can arrive at any time. An event that a request caused before its reply has the `id` of that request. The events of a job or a live session come after the reply, so they have no `id`: their `job` or `session` field tells which work they belong to. A request that needs a capability that the user did not grant gets `error {code: "capability", capability}`.

| Request | Capability | Fields | Reply and events |
|---|---|---|---|
| `hello` | none | `sdk, protocol, features` | `ok {host_version, protocol, features, extension_id, capabilities}` |
| `list_models` | `models.read` | `role?` (`final`, `streaming`, `vad`, `diarization`) | `ok {models: [{id, name, roles, state, languages, timestamps, size_bytes, choice}]}`. `choice` lists the roles for which the host uses this model when a request names none. |
| `download_model` | `models.download` | `model, reason` | The host asks the user, unless the model is on disk or already downloads. Then `model_state` and `download_progress` events, and `ok`, or `error {code: "declined"}`, or `error {code: "unknown_model"}`, or `error` with the message of the engine when the download fails. |
| `transcribe_file` | `transcribe.file` | `path, model?, vad_model?, language?, diarize?, diarization_model?, speakers?` | `ok {job}` at once. Then `job_progress {job, fraction}` and `segment {job, start_ms, end_ms, text}`. With `diarize`, `speakers {job, turns}` after the last segment. At the end `job_done {job, text}` or `job_failed {job, message, code}`. |
| `cancel_job` | `transcribe.file` | `job` | `ok`, also for a job of this connection that already ended. The job gets `job_failed` at once. A job that this connection did not start gets `error {code: "bad_request"}`. |
| `session_start` | `session.live`, plus `audio.mic` or `audio.system` for those sources | `sources` (`mic`, `system`, `client`), `model?, preview_model?, vad_model?, language?, diarize?, diarization_model?, record_to?` | `ok {session}`, or `error {code: "busy"}` when another live session runs, or `error {code: "too_many_streams"}` when the engine has no free stream. Then `partial {session, source, committed, tentative}` and `segment {session, source, start_ms, end_ms, text}` events. |
| `session_audio` | `session.live` | `session, pcm` (base64, 16 kHz mono i16). Only for the source `client`. | Without `id`: no reply, and an `error` without `id` when the audio is wrong. With `id`: `ok` or `error`. |
| `session_stop` | `session.live` | `session` | Last segments. With `diarize`, `speakers {session, source, turns}` for each source. Then `ok {text}`. |
| `enhance` | `ai.enhance` | `text`, `style?` or `prompt?`, `provider?` | `ok {text, provider, model}`, or `error` with the codes below. |
| `subscribe` | `dictation.observe` | `events: ["dictation_done"]` | `ok {events}`: the events that the connection gets now. Then `dictation_done {text, transcript, app, site, style, time}` after each insert. |
| `bye` | none | | `ok`, then the host closes the socket |

`list_models` lists the local models of this computer: speech, VAD, and diarization models. Cloud models are not in the list, and Basic VAD is not a model. `state` is the state that the engine reports (`not_downloaded`, `downloading`, `downloaded`, `optimizing`, `ready`, `failed`). With `role`, the list has only the models with that role. A role that this version does not know gives an empty list. If the engine does not run, `list_models` and `download_model` get `error {code: "unsupported"}`, because the host cannot know the state of any model. At the start of Sayso, the host waits up to 10 seconds for the engine to answer first.

`transcribe_file` checks its models before the reply: `unknown_model` (not a model of this computer), `wrong_role` (for example a speech model as `vad_model`, or a model that cannot do the final pass), `model_not_downloaded` (also for `diarize: true` without a diarization model on disk), `unsupported` (the engine does not run, it lacks the feature `vad` or `diarization`, or the final model of the user is a cloud model and the request names no model), and `bad_request` (a path that is not absolute, or `speakers: 0`). A missing field takes the choice of the user, as in `choice`. When the choice of the user cannot run, VAD falls back to Basic VAD, and an engine without the feature `segments` gives one segment for each chunk. `diarization_model` without `diarize` asks for diarization. A file that the host cannot read gives `job_failed`, with a message that names the file and the formats.

`choice` is what the host does when a request names no model: the final model of the user (`final`), the model that gives the live preview (`streaming`), the VAD model that is on disk (`vad`, nothing for Basic VAD), and the diarization model that is on disk (`diarization`). A model can have more than one role in `choice`.

`download_model` follows these rules:

- An unknown id (also the id of a cloud model) gets `error {code: "unknown_model"}`.
- A model that is on disk gets `ok` at once, with no question and no event.
- A model that downloads now (the user started it, or another request) is followed. The host starts no second download and asks no question. The first event is the progress that exists now.
- Else the host asks the user and shows the extension, the model, its size, and the `reason`. The `reason` is text from the extension. The host cuts it to 300 characters and shows it as a quote of the extension. The user can say **Download** or **Cancel**, and closing the question counts as **Cancel**.
- An extension has one question at a time. A second `download_model` of the same extension waits for the first question, and then looks at the state again. A second request for the same model usually follows the download that the first one started. After a **Cancel**, a waiting request asks its own question.
- The events have the `id` of the request: `model_state {state: "downloading"}`, then `download_progress` (at most 8 times a second), then `model_state {state: "downloaded"}` and `ok`. When the download fails, the last events are `model_state {state: "failed", message, reason}` and then `error {message, reason}` with the same `reason` (`download_failed`, `incomplete_download`, ...), and no `code`. If the engine stops during the download, the reply is an `error` and the download is lost.
- When the extension disconnects, the host stops the request. A download that the engine already runs goes on.

A request that takes time runs on its own thread, so the connection stays free. While a question is open, `hello` and `list_models` of the same connection still get their replies. The reply of a request comes after the last event with its `id`, and no event with that `id` comes after it.

When a request names no model, the host uses the choice of the user for that role. When it names a model that is not on disk, the reply is `error {code: "model_not_downloaded"}`, and the extension can send `download_model`.

A speaker in `turns` is a label such as `"1"` or `"2"`, unique in one source of one job or live session. Names are the business of the extension.

`record_to` is an absolute path of a directory. The host makes it when it does not exist, and writes one 16 kHz mono WAV for each source in it (`mic.wav`, `system.wav`, `client.wav`), so the extension can keep the audio. A file with that name is replaced. Without `record_to`, the host keeps the audio in a temporary file for diarization and deletes it after `session_stop`.

`enhance` runs a style of the user, or a free prompt, on `text` with an AI provider of the user. It uses the code of the style step of a dictation (`crates/sayso-app/src/style_step.rs`), with the same base prompt, dictionary words, and time limits. The host never writes the text to the log.

- `style` is the id of a style, or its name (case does not matter). Without `style` and `prompt`, the host uses the active style of the user.
- `prompt` is an instruction that the host uses as a standalone style: the cleanup rules of the base prompt do not apply.
- `provider` is the id of a provider, or its name. Without it, the host uses the provider of the style, else the default provider of the user. A named provider does not get the model of the style of another provider.
- `bad_request`: `text` is missing or empty, the request has both `style` and `prompt`, the prompt is empty, the user has no such style or provider, or the style uses no AI (Raw).
- `unavailable`: AI is off, no provider is set up for the style, the provider has no model chosen, the provider cannot run the style, the provider is not set up, it cannot be reached, it replied with an HTTP error, its program failed, or a cleanup model does not support the language of the text.
- `unsupported`: a `prompt` for a cleanup model, which cannot follow a prompt.
- `timeout`: the provider gave no reply in its time limit.
- `refused`: the provider refused the text.
- An `error` without `code`: the provider gave a reply that Sayso cannot use.

The message names the provider and the kind of failure, never the text or the body of a reply. The request runs on its own thread, so the connection stays free while the provider works.

`subscribe` replaces the list of events of the connection. A name that this version does not know is not an error: the reply leaves it out, so the extension can see what it gets. An empty list stops the events. The host checks the capability of each event when it sends it, so a grant that the user takes back stops the next event, and the subscription counts again when the user grants it again. `dictation_done` comes after the text is in the app, once for each dictation:

- `text` is the inserted text, `transcript` the text of the final pass before the style.
- `app` is the name of the app, `site` the host of the web page when Sayso knows it, and `style` the id of the style. Each can be `null`.
- `time` is the time of the insert, in RFC 3339 with seconds, in UTC.

`transcribe_file` reads WAV, FLAC, MP3, AAC, and M4A (with `symphonia`). For other formats (Opus in WebM from a video site, for example) the extension converts the file first, or sends the audio as a live session with the source `client`.

### Scheduling

The host puts the work of the engine in this order:

1. Dictation.
2. Live sessions.
3. Jobs.

Limits:

- One live session at a time, for all extensions together. A second `session_start` gets `busy`.
- One job at a time. Other jobs wait in a queue, first in, first out, and send `job_progress` with `fraction: 0` until they start.

A live session waits only for dictation. A job waits for dictation, and also while the final pass of a part of a live session or its diarization is in the engine (`DictationGate::live_part`). A job does not stop a live session, and the preview stream and the VAD of a live session never wait.

A job runs one VAD part at a time. When a dictation starts, the job finishes the current part, waits, and goes on after the insert. So a job slows dictation by at most one part. The host keeps parts under about 30 s for this reason.

How a job runs (`crates/sayso-host/src/jobs/`):

- **Chunks.** VAD gives the speech parts. Neighbouring parts join into chunks of at most 30 s (10 s for a model with the timestamp level `none`, which gets one segment for each chunk), when the pause between them is at most 3 s. Each chunk gets 200 ms of audio before and after its speech. A VAD model gets the file in windows of 10 minutes.
- **Dictation and live sessions first.** `AppModel::dispatch` tells the host (`DictationGate`) when the dictation state machine enters `Recording` or `Processing`, and when it leaves both. Every way out of a dictation (insert, failed insert, no speech, cancel, failed capture, a recording that is too short) goes through `dispatch`, and a dictation that has not ended after 20 minutes no longer holds jobs. A job waits before each call to the engine (a load, a VAD window, a chunk, diarization) while a dictation runs, and while a live session has a part or its diarization in the engine. Diarization runs once on the whole file, so on a long file it is the one call that can make a dictation wait for more than one part.
- **Progress.** `job_progress` gives the audio time done over the length of the file. With diarization, the transcription is 90 % and diarization the last 10 %. A job that waits in the queue sends `fraction: 0` when it enters the queue and each time a job ahead of it starts.
- **Cancel.** A cancelled job gets `job_failed` with the message "The job was cancelled." at once, and no event after it. A waiting job leaves the queue. A running job stops its work after its current call to the engine. When an extension closes its connection, the host cancels its jobs.
- **Memory.** The host keeps the audio of a job as 16-bit samples: 2 hours of audio take 230 MB. A chunk becomes floats only when it goes to the engine. Diarization needs the whole file as floats, and the engine client writes it as a WAV file, so diarization of 2 hours needs about 1 GB more for a short time.

A model that an extension uses counts in the memory policy (`crates/sayso-app/src/memory.rs`) while its job or live session runs. The host unloads it after the same idle time as other models. A job holds its models (`ModelUse`) before it loads them, and the app does not unload a held model, or a model that a job let go less than `dictation.keep_model_minutes` ago.

### Capture

The host captures the audio and gives it to the dictation and to live sessions. One microphone stream can feed a dictation and a live session at the same time. The live session uses the microphone that the user chose in Settings.

Each source of a live session is its own stream in the engine, and every `partial` and `segment` names its source. Diarization runs on each source alone. In a call, the microphone is usually one person and the system audio is everyone else, so the extension can name the speakers of the system audio and call the microphone "You".

The microphone is one capture in Sayso (`crates/sayso-app/src/mic.rs`). It starts when the first user (a dictation or a live session) needs it, and stops when the last one leaves. Each user gets every frame from the moment it joined. Dictation does not get slower:

- A dictation gets its frames on the thread of the capture, as before. When a live session already captures, a dictation gets frames at once, without the time to open the device.
- A live session gets its frames through a queue of its own. The capture never waits for that queue, so a slow live session does not delay or drop a frame of a dictation.
- When the user chooses another microphone in Settings while a live session runs, the capture stays on the old microphone until the next user joins (the next dictation, or the next live session). Then the capture moves to the new microphone for all its users: the new microphone starts first, and the old one stops after that. The live session loses only the frames between the two, and never gets the frames of both.

How a live session runs (`crates/sayso-host/src/live/`):

- **One pipeline for each source.** The audio goes to the recording, to a preview stream of the engine when the session has a streaming model (`partial` events), and to the VAD: a VAD session of the engine, or Basic VAD in the host. Each speech that ends is a part, with 200 ms of audio before and after it. The final model transcribes the parts one at a time, and each segment of a part becomes a `segment` event with times from the start of the session.
- **Dictation first.** The final pass of a part and the diarization wait while a dictation runs. The preview stream and the VAD do not wait, so the session hears all the audio.
- **Models.** A model that the request does not name is the choice of the user, with the checks and error codes of jobs (`unknown_model`, `wrong_role`, `model_not_downloaded`, `unsupported`). The preview model is `preview_model`, else the live preview model of the user. A preview needs the engine feature `concurrent_streams`: without it, a named `preview_model` gets `unsupported`, and a session without one gets no preview, so that a session cannot end the preview of a dictation. The session holds its models (`ModelUse`) from its start to its end, so the memory policy of the app does not unload them.
- **Engine session ids.** A dictation numbers its engine sessions from 1. The host numbers the preview streams and VAD sessions of live sessions from 2^40, so the two never meet. A source uses at most two streams of the engine (a preview stream and a VAD session). When the engine has no free stream, `session_start` gets `error {code: "too_many_streams"}`, and the host closes the streams that it opened.
- **End of the VAD.** At the stop, the host closes a VAD session of the engine with `stream_end` and waits for its reply (`SttBackend::end_vad`). The engine sends the last `speech_end` before that reply, and the bus of the host passes it on before `end_vad` returns, so the host does not wait for a time of silence. An engine client that cannot wait for the reply keeps the default `end_vad`; then the host ends the open speech at the end of the audio.
- **Stop.** `session_stop` ends the audio, closes the streams, waits for the part that the engine transcribes now, transcribes the last part, and runs diarization on the recording of each source. The `segment` and `speakers` events of a session have no `id`; they come before the `ok {text}` reply. `text` is the text of all segments in the order of time. When the extension closes its connection, or Sayso quits, the host stops its session without diarization, and does not transcribe the parts that wait. When Sayso quits, this happens before `host_closing`.
- **Sources.** This version captures `mic` and `client`. `system` gets `error {code: "unsupported"}`.

There is no echo cancellation in the first version. When the user has no headphones, the microphone hears the call too. The host then drops a microphone segment when a system segment overlaps it in time and has most of the same words. Set the limit for "most" with test recordings at the build, and write it in this file.

System audio is new work for each system:

- macOS: a Core Audio process tap. It needs macOS 14.2 or later and a system audio permission for Sayso.
- Windows: WASAPI loopback.
- Linux: the monitor source of the default output in PipeWire.

Sayso asks for the system audio permission the first time a live session asks for the source `system`, and not at onboarding.

### Lifecycle

- An extension with an open live session or job holds Sayso open. When the user quits Sayso with **Quit** in the popover or in the tray menu, Sayso names the extension and asks first: "Notetaker is recording. Quit anyway?" (a live session) or "Media is transcribing a file. Quit anyway?" (a job), with **Cancel** and **Quit** (`crates/sayso-app/src/quit.rs`). Without such work, Sayso quits at once. `Host::holds` gives the work, live sessions first, and names each extension by the `name` of its record in `state.toml`, else by its id.
- A quit that the system starts (log out, shut down, **Quit** in the Dock on macOS) does not ask, because the system does not wait for an answer. The host still stops the work and sends `host_closing`. A restart to update does not ask: it does not start while an extension works (next item).
- Sayso does not install an update while a live session or a job runs. **Restart to update** then shows "Sayso restarts to update when Notetaker stops." and waits. The host tells the app when work starts or ends (`Host::set_holds_observer`), and the app installs the update and restarts when no extension works and no dictation runs. Automatic updates only download and prepare the update, so this button is the only way that an update installs while Sayso runs.
- When an extension closes its socket or its process ends, the host stops its live sessions, cancels its jobs, and closes its download question (as **Cancel**).
- When Sayso quits or restarts, the host stops the live sessions, ends each job with `job_failed`, closes the open download questions, then sends `host_closing` and closes the socket. The SDK reports that the host is gone and connects again when the host comes back. Jobs and live sessions do not survive a restart. The extension starts them again.
- `Client::connect` never starts Sayso. `Client::connect_or_start` starts Sayso when no host answers on the socket, and waits up to 10 s for the socket. It starts Sayso with `open -g -b dev.sayso.Sayso` on macOS, `Sayso.exe` in `%LOCALAPPDATA%\Programs\Sayso` or `%ProgramFiles%\Sayso` on Windows, and on Linux the folders of the tarball and the packages (`~/.local/lib/sayso`, `/usr/local/lib/sayso`, `/usr/lib/sayso`), then the Flatpak (`flatpak run dev.sayso.Sayso`), then `sayso` on the `PATH`. In the Flatpak of Sayso, only `/app/bin/sayso`. Sayso starts as a process of its own, without the input, the output, and the `SAYSO_HOST_SOCKET` of the extension.
- Sayso does not stop an extension when it quits. The extension is an app, and the user closes it.

### Privacy

- While incognito is on, the host sends no `dictation_done` events. The rule is the rule of history: a dictation makes no event when incognito was on at its start or at its insert (`sayso_core::dictation::incognito_during`). Incognito does not change live sessions or jobs, because the extension started them.
- Jobs and live sessions make no history entries in Sayso. The text belongs to the extension. Only the dictation controller of the app writes history, and the host has no access to the store.
- While a live session captures the microphone or the system audio, the menu bar icon (the tray icon on Windows and Linux) shows that Sayso records. The popover names the extension and has a **Stop** button, which stops the live session.

### Code

Step 6 builds the Host API in these crates:

| Crate | License | What it has |
|---|---|---|
| `sayso-protocol` | MIT | The messages of the engine protocol and the Host API, with serde. The error codes and the capabilities. The PCM helpers (base64 of 16-bit PCM). No I/O. |
| `sayso-extension` | MIT | The client of the Host API. Blocking calls and a channel of events, with no async runtime, like the rest of Sayso. |
| `sayso-engine-sdk` | MIT | Start an engine and talk to it over stdio. |
| `sayso-host` | GPL | The server: the socket, the identity check, `state.toml`, the capabilities, the scheduler, jobs, and live sessions. No GPUI. |

`sayso-host` gets the engine, the capture, and the user interface of Sayso through traits that `sayso-app` gives it (`HostServices`). The microphone of live sessions is `HostServices::microphone`: in `sayso-app` it joins the shared microphone (`SharedMic::join_queued`) on the device of `audio.input_device`. `Host::live_session` and `Host::set_live_observer` tell the app which live session runs, for the menu bar of step 8. For example, `sayso-app` shows the download question (`ask_download`), and `sayso-host` waits for the answer on a thread of its own. The host gives `ask_download` a check (`withdrawn`), and the app closes the window when it is true. `HostServices::styles` and `run_style` give `enhance` the styles of the user and the style step of a dictation. The app tells the host each dictation with `Host::dictation_done`, and asks `Host::holds` before it quits or installs an update. The traits also give the engine (`SttBackend`) and the settings of the user. So the host runs and is tested without a window.

Inside the host, a request that takes time gets a `Responder` (its events with the request `id`, then its one reply, which the type allows only once) on a thread of its own. Work that goes on after the reply keeps a `Notifier` (events without an `id`). The host has one subscription to the events of the engine, and gives a receiver to each piece of work that needs one (`Bus::watch`). Jobs and live sessions use the same parts.

The socket:

- `SAYSO_HOST_SOCKET`, when it is set, is the path. Sayso sets it for the extensions that it starts.
- Else on Linux: `$XDG_RUNTIME_DIR/sayso/host.sock`, or `$XDG_RUNTIME_DIR/app/<app id>/host.sock` in a Flatpak.
- Else on macOS: `sayso/host.sock` in the temporary folder of the user (`confstr(_CS_DARWIN_USER_TEMP_DIR)`). Each process of the user gets the same folder, also when Finder starts it.
- Else: `/tmp/sayso-<uid>/host.sock`.
- Windows: `\\.\pipe\sayso-host-<user SID>`.

The folder of the socket has mode `0700`. The SDK has the same rules, because an MIT crate cannot use the paths code of `sayso-core`.

`state.toml` has one record for each installed extension. Step 8 writes it. Until then, tests and developers write it by hand. `name` is the name that Sayso shows in its questions; without it, Sayso shows the id.

```toml
[[extensions]]
id = "dev.example.notetaker"
name = "Notetaker"
version = "1.2.0"
# The folder that the executable of the extension must be in.
path = "/Users/me/Library/Application Support/sayso/extensions/dev.example.notetaker/1.2.0"
sideloaded = false
granted = ["models.read", "session.live", "audio.mic"]
# macOS, for an extension from the registry: the code signature that install checked.
team_id = "ABCDE12345"
bundle_id = "dev.example.notetaker"
```

The host reads the file again when it changes, so a grant that the user takes back stops the next request. `models.read` is always granted.

Errors of the Host API have these codes, in addition to the codes of the engine protocol: `not_an_extension`, `capability`, `busy`, `declined`, `model_not_downloaded`, `wrong_role` (a model that cannot do the role of the request, for example a speech model as `vad_model`), `unsupported` (the engine of this computer cannot do it), and `bad_request`.

These rules of the wire format are fixed in `sayso-protocol`:

- A job id and a live session id are integers that the host chooses.
- `choice` in `list_models` is a list of roles: the roles for which the host uses the model when a request names none (see "Requests").
- Events that a request caused have its `id` and come before its reply. After the reply, no event has that `id`. Work that goes on after the reply (jobs and live sessions) sends events without an `id`. So the events of a job have no `id`, and `job` tells the job. The `ok {job}` reply comes before the first event of the job.
- The events of a live session (`partial`, `segment`, `speakers`) have no `id`, and `session` tells the session. The `ok {session}` reply comes before the first event of the session, and the `ok {text}` reply of `session_stop` comes after its last event.
- `session_audio` without an `id` gets no `ok`. When it fails, the `error` has no `id`. `sayso-extension` sends it with `Client::send`, and gives an `error` without an `id` as `Event::Error`.
- A job ends with exactly one `job_done` or `job_failed`, and no event of the job comes after it. A cancelled job gets `job_failed` without `code` and with the message "The job was cancelled.". When the host closes, each job gets `job_failed` before `host_closing`.
- A failed download gives `error` without `code`, with `message` and the `reason` of the engine protocol, after `model_state {state: "failed"}`. `declined` and `unknown_model` have a `code`.
- A line that is not a JSON object with a string `type` gets `error` without `id` and with `code: "bad_request"`. A known request with a missing field or a field of the wrong type gets `bad_request` with its `id`. An unknown request type gets `error` with its `id` and no `code`, as in the engine protocol.
- `dictation_done` has no `id`. The `ok {events}` reply of `subscribe` comes before the first event, and lists only the event names that the host knows.
- `enhance` sends no events: one `ok {text, provider, model}` or one `error`. The error codes are `bad_request`, `unavailable`, `unsupported`, `timeout`, and `refused` (see "Requests"); an output that Sayso cannot use gives `error` without `code`.
- A reader keeps a capability or an error code that it does not know, and ignores a message type that it does not know.

Sub-steps, each with its own pull request:

- **6a.** `sayso-protocol`, the transport, the identity check, `state.toml`, `hello`, `bye`, `host_closing`, and the connection of `sayso-extension`.
- **6b.** `list_models` and `download_model` with the question to the user.
- **6c.** Segments, VAD, and diarization in the engine client. Jobs, the queue, and the scheduler that puts dictation first.
- **6d.** Live sessions from the microphone and from the client, and `record_to`.
- **6e.** `enhance`, `subscribe` and `dictation_done`, the privacy rules, the lifecycle rules, `sayso-engine-sdk`, the test extension, and the dictation benchmark.

The recording state of the menu bar icon is in step 8, with the other UI.

## Capabilities

The user grants the capabilities of an extension at install. An update that adds a capability asks again before it runs. The user can take back a capability with **Permissions** on the Installed tab of the Marketplace. Then the host refuses its requests, and the extension must handle that.

| Capability | What it allows | Why it asks |
|---|---|---|
| `models.read` | List models and their state | No risk. Granted without a question, and shown in the list. |
| `models.download` | Ask the user to download a model | Each download still asks the user. |
| `transcribe.file` | Jobs | The extension decides which files to send. |
| `session.live` | Live sessions with audio from the extension | |
| `audio.mic` | Live sessions from the microphone | Sayso shows that the microphone is on, as during a dictation. |
| `audio.system` | Live sessions from the system audio | It records other apps and calls. |
| `ai.enhance` | Use the AI providers and styles of the user | It can cost the user money and send text to a cloud provider. |
| `dictation.observe` | Get each dictation after it is inserted | It sees everything that the user dictates. |

Later capabilities, not in the first version: `history.read`, and a step in the pipeline before insertion. A step before insertion adds latency to each dictation and can break dictation, so it needs its own design.

## Package and manifest

### Manifest

Every extension has a `sayso-extension.toml` at the top of its package:

```toml
id = "dev.example.notetaker"   # reverse DNS, unique in the registry
name = "Notetaker"
version = "1.2.0"
host_api = 1                   # the lowest Host API version that it needs
description = "Notes from your meetings."
author = "Example"
homepage = "https://example.dev/notetaker"
repository = "https://github.com/example/notetaker"
license = "MIT"
capabilities = ["models.read", "session.live", "audio.mic", "audio.system", "ai.enhance"]

# Models that the extension cannot work without. Sayso offers them at install.
[models]
needs = ["parakeet-unified-en"]

# Entries in the popover of Sayso. A click starts the extension with SAYSO_COMMAND set,
# or sends a `command` event when it runs.
[[commands]]
id = "new-note"
title = "New meeting note"

# The platforms that the extension supports, and the file that Sayso starts on each.
# A platform that is not here is not supported. This extension has no Windows build.
[platforms]
macos-arm64 = "Notetaker.app"
linux-x64 = "notetaker"
linux-arm64 = "notetaker"
```

### Package

| Platform | File | Rule |
|---|---|---|
| `macos-arm64` | `.zip` with one `.app` | Signed with a Developer ID, notarized, and stapled, to be listed |
| `windows-x64` | `.zip` | No signature necessary |
| `linux-x64`, `linux-arm64` | `.tar.gz` | No signature necessary |

The package for a platform has only the files for that platform.

### Install

1. Download the file and check its size and SHA-256 against the listing (or skip this for a sideloaded extension).
2. Unpack it into a staging directory under the data directory.
3. Read the manifest. Its `id` and `version` must match the listing, and `[platforms]` must have the platform of this computer.
4. On macOS, check the code signature and notarization of the `.app` with the Security framework. Sayso downloads with `ureq`, so the file has no quarantine flag, and Gatekeeper does not check it. Sayso must do that check itself.
5. Show the capabilities and the models in `[models] needs`, and ask the user.
6. Move the directory to `<data dir>/extensions/<id>/<version>/`. Keep the version before it, so the user can go back.

The record of installed extensions and their grants is `<data dir>/extensions/state.toml`, not `config.toml`. It is state, not settings.

### Update and remove

- Sayso does not update extensions by itself in the first version. The user clicks **Update**. If the new version asks for more capabilities, Sayso asks before it installs the version.
- If the extension runs, Sayso asks the user to close it before the update.
- **Remove** stops the connection of the extension and deletes `<data dir>/extensions/<id>/`. The data of the extension is in its own folders, and Sayso does not delete it. The confirmation says so.

### Linux packages

A Linux package of an extension must run with only the system libraries that Sayso itself needs: glibc, and the libraries of GPUI (Wayland, X11, xkbcommon, Vulkan).

- **Flatpak:** extensions run in the sandbox of Sayso, with its permissions. Sayso starts them in the sandbox, and the socket is in the sandbox too.
- **NixOS:** a downloaded binary does not run without `nix-ld`. The Marketplace shows that note on NixOS, and the guide says so.
- **The tarball, `.deb`, `.rpm`, and AppImage:** no special rules.

### Start

- macOS: `NSWorkspace.openApplication` with `SAYSO_HOST_SOCKET` in the environment of the configuration. The extension then has its own identity for the permissions of macOS. It cannot use the microphone permission of Sayso. A child process of Sayso would get that permission.
- Windows and Linux: a detached process in its own process group, with the same environment.

## Extensions in Sayso

Sayso gets no new Hub section. The UI is the Marketplace modal, one button that opens it, and one popover menu.

### Sidebar footer

Today the footer of the Hub sidebar (`main_footer` and `rail_footer` in `crates/sayso-app/src/hub/sidebar.rs`) shows the engine status ("Ready", and the final model), the collapse button, the update slip, and the theme switch.

After this change:

- A **Marketplace** button takes the place of the engine status, with the collapse button on its right as before. In the rail it is an icon with the tooltip "Marketplace".
- The engine status leaves the footer. Settings › General already shows it (`crates/sayso-app/src/hub/settings/general.rs`). The footer shows the engine only when it needs the user: a warning row with **Restart** when the engine stopped or failed to start. "Ready" is the normal state and shows nowhere in the footer.
- The update slip and the theme switch stay as they are.

The engine status is a live region today (`LiveStatus`), so a screen reader hears a change. The warning row keeps that.

### Marketplace modal

The modal works like Model details on the Models page: Escape, the close button, and a click on the dimmed page close it. It has two tabs:

- **Browse** shows the listings for the platform of the user, with a search field. A click on a listing shows its details in the modal: the description, the author, the platforms, the capabilities in plain words, the models it needs with their sizes, and a link to its repository. The action is **Install**, or **Open** when it is installed.
- **Installed** shows each installed extension with its icon, name, and version, an **Open** button, and a menu with **Update**, **Permissions**, and **Remove**. In developer mode it also has **Install from folder**. A sideloaded extension has the tag **Not reviewed**.

The modal opens on Installed when an extension is installed, else on Browse. The button in the footer shows a dot when an update is available.

**Install** shows the capabilities in plain words ("Record system audio, including calls") and the models to download, with their sizes, and asks the user before it starts.

### Other places

- **Popover › Extensions**: a submenu with the commands of the installed extensions. It is hidden when no extension is installed.
- **Settings › Advanced › Developer mode** turns on sideloading.

Sayso does not open files for extensions in the first version. An extension is an app, so the system opens its files.

## Registry

### Repository

```
extensions/<id>.toml     one file per extension: listing and versions
index.json               made by CI from extensions/*.toml
index.json.sig           signature of index.json
revoked.toml             versions that Sayso must turn off
schema/                  JSON Schema for the files above
```

A version in `extensions/<id>.toml`:

```toml
[[versions]]
version = "1.2.0"
host_api = 1
capabilities = ["models.read", "session.live", "audio.mic", "audio.system", "ai.enhance"]

[versions.files.macos-arm64]
url = "https://github.com/example/notetaker/releases/download/v1.2.0/Notetaker-macos-arm64.zip"
size = 18342112
sha256 = "..."

[versions.files.linux-x64]
url = "https://github.com/example/notetaker/releases/download/v1.2.0/notetaker-linux-x64.tar.gz"
size = 21004288
sha256 = "..."
```

The keys under `files` are the platforms of the version. They must be the same as the keys of `[platforms]` in the manifest of each package.

### Add or update a listing

1. The author opens a pull request that adds or changes `extensions/<id>.toml`.
2. CI checks the pull request:
   - The file matches the schema.
   - The `repository` of the listing is a public repository (GitHub API, or an anonymous `git ls-remote` for other hosts).
   - Each file downloads, and its size and SHA-256 match.
   - The manifest in each package matches the listing (`id`, `version`, `capabilities`, and the platforms).
   - If the version has a `macos-arm64` file, `codesign --verify --deep --strict` and `stapler validate` pass on its `.app`, on a macOS runner.
   - A comment lists the capabilities that the version adds, so the reviewer sees them.
3. A person reviews the pull request with the checklist in the registry repository.
4. After the merge, CI makes `index.json`, signs it, and pushes both files.

The index has its own signing key, not the key of the updater. A leak of one key then does not give the other. Sayso has both public keys in its binary.

### In Sayso

Sayso gets the index and its signature when the Marketplace opens, and once a day while an extension is installed. If the signature is wrong, Sayso keeps the last good index. When a new version of an installed extension is in the index, the Installed tab shows **Update**.

A listing can stop supporting a platform in a new version. Then a user on that platform keeps the version they have, and the Installed tab says that no update is available for their platform.

A version in `revoked.toml` gets turned off. Sayso refuses its connections, shows **Turned off by the registry** with the reason, and offers an update when there is one.

## SDK and licensing

New crates, MIT-licensed, each with `publish = true`:

| Crate | What it has |
|---|---|
| `sayso-protocol` | The message types of the engine protocol and the Host API, with serde |
| `sayso-extension` | The client of the Host API: connect, `hello`, requests, events, reconnect, start Sayso |
| `sayso-engine-sdk` | Start an engine and talk to it over stdio, for apps without Sayso |
| `sayso-ink` | The ink, palette, and appearance types, moved from `sayso_core::ink` |
| `sayso-ui` | The theme, the paper, the textures, and the components of Sayso. It is an existing crate that changes its license (see below). |

The GPL crates of Sayso (`sayso-engine-client`, the host in `sayso-app`) depend on `sayso-protocol`. A GPL crate can depend on an MIT crate, not the reverse.

Write `sayso-protocol`, `sayso-extension`, and `sayso-engine-sdk` new. Do not move code into them from the GPL crates without a check. Two contributors other than Chris have commits in `sayso-core` and `sayso-i18n` (`git shortlog -sne HEAD -- crates/sayso-core crates/sayso-i18n`). Their code is GPL until they agree to MIT.

### `sayso-ui` becomes MIT

Extensions should look like Sayso as much as possible, so `sayso-ui` (the theme, the paper, the textures, and the components) becomes MIT and is published. Chris wrote all of its code. The outside commits in the repository changed only `sayso-core` and some translation files (`settings-recorder`, `core`, and `engine` catalogs), not `sayso-ui` and not `sayso_core::ink`. Check again with `git log --format='%an' -- crates/sayso-ui crates/sayso-core/src/ink.rs` before the change.

An MIT crate cannot depend on a GPL crate, so `sayso-ui` must lose its dependencies on `sayso-core` and `sayso-i18n`:

- `sayso_core::ink` (ink, palette, appearance) moves into the new MIT crate `sayso-ink`, which needs only serde. `sayso-core` (for `config.rs`) and `sayso-ui` both depend on it. It does not go into `sayso-ui`, because then `sayso-core` would depend on GPUI.
- The keycap in `components/basics.rs` takes key labels, not `sayso_core::hotkey::Hotkey`. The app makes the labels.
- The components get their strings from a small built-in English catalog in `sayso-ui` with only component strings (`sayso_ui::l10n::TEXTS`, nine keys). An app calls `sayso_ui::l10n::install(translate, locale)` at startup to give it its catalogs and its locale. The components call the hooks each time they draw, so a change of language shows at once. (The first plan said that `sayso-ui` calls `t!` about 50 times. It calls it nine times.)
- The embedded assets stay, except `app/tray.png` and `app/icon-64.png`. The name Sayso and the app icon stay with Watzon Ventures LLC (`docs/plan.md`), so they are not in an MIT crate. `sayso-app` embeds them (`src/app_assets.rs`).
- The fonts (Fraunces, Instrument Sans, IBM Plex Mono) are under the OFL. The crate ships their license files from `crates/sayso-ui/assets/fonts/licenses`.
- The fonts, icons, textures, and sounds moved into `crates/sayso-ui/assets/`, because `cargo package` does not pack files outside the crate directory.
- No file in the repository says where the SVG icons in `crates/sayso-ui/assets/icons` come from. The first commit added them (Chris Watson), and they are drawn in one style (18 by 18, stroke 1.6, round caps). Chris does not remember a source. A comparison with Lucide on 2026-10-26 found no match: the icons use an 18 by 18 grid with simple coordinates, where Lucide, Feather, Tabler, and Heroicons use 24 by 24, and the shapes differ (for example, `settings` is two sliders, not a gear, and `history` is a clock without an arrow). So they were most likely drawn for Sayso, and they ship under the MIT license of the crate.

`gpui-kit` is pinned with `=`, and `sayso-ui` depends on it. An extension must use the same `gpui-kit` and GPUI versions as the `sayso-ui` version it uses. Each `sayso-ui` release says which versions those are.

### Other languages

The protocol is NDJSON, so an extension can be in any language. The first version ships only the Rust SDK. An extension in another language does not get the look of Sayso from `sayso-ui`.

## Security

Know what this design protects against and what it does not:

- **An extension is not in a sandbox.** It is a native program with the rights of the user. It can read files and use the network. Capabilities control only what the host gives it.
- **The review and notarization reduce the risk. They do not remove it.** A reviewer reads the listing and the repository. A reviewer cannot prove that a binary came from that source. Notarization means that Apple scanned the binary and can revoke it.
- **Sayso does not bypass Gatekeeper.** It checks the hash from the signed index and, on macOS, the signature and notarization.
- **An extension cannot borrow the permissions of Sayso.** It starts through LaunchServices with its own identity (see "Start").
- **The registry can turn off a version.** `revoked.toml` turns it off everywhere at the next fetch of the index.
- **A sideloaded extension has none of these checks.** Developer mode says so before the user turns it on.

## Order of work

Each step ends with a check that proves it.

1. **Decisions.** This file and the ADRs. Check: Chris approves them.
2. **Windows on `sayso-engine`.** Do the checks in "Check first", then move Windows to `sayso-engine`, move the preview and stdio code, make the Windows catalog, add the migration of old Parakeet files, change `scripts/bundle-windows.ps1` and the Release workflow, and delete `native/portable`. Check: the Windows build dictates with Parakeet TDT v3 (with its live preview) and Whisper, an install with old Parakeet files downloads them again, and Linux gets the Parakeet TDT v3 preview.
3. **Engine protocol.** Features in `hello`, three concurrent streams, segments, VAD on files and streams, diarization, `format_changed`, public model ids, the spec in `docs/engine-protocol.md` (MIT), and the conformance suite. Check: the Swift engine and `sayso-engine` pass the suite in CI, or leave the feature out of `features`.
4. **VAD and diarization roles in Sayso.** Catalog entries, Basic VAD in the host, `models.vad`, `models.diarization`, the choice of the sherpa-onnx embedding model, and the two new sections of the Models page. Check: with a test extension installed, a user can choose a VAD model and a diarization model and see both used in a job.
5. **Engine release files.** Check: the engine from the release file answers `hello` on a clean machine of each platform.
6. **Host API and SDK.** The socket, the identity check, capabilities, model listing and download consent, jobs with VAD chunking and diarization, live sessions from the microphone and from the client, `record_to`, `enhance`, `dictation_done`, the scheduler and its limits, the privacy rules, and the lifecycle rules. Check: integration tests with a test extension, and the dictation benchmark (`docs/benchmark.md`) shows no slower dictation while a job runs.
7. **`sayso-ui` as MIT.** Move `ink` into `sayso-ink`, remove the dependencies of `sayso-ui` on `sayso-core` and `sayso-i18n`, change the licenses, and publish both crates. Check: `cargo tree -p sayso-ui` has no GPL crate, and an empty GPUI app outside the repository draws a `sayso-ui` component.
8. **Extensions in Sayso.** Manifest, sideload, install, update, remove, start, consent, the Marketplace button in the sidebar footer, the Installed tab of the Marketplace, and the recording state of the menu bar icon. Check: a sideloaded test extension starts, connects, and transcribes a file on each platform.
9. **Media extension.** A first-party extension in its own repository, built only on the published SDK and `sayso-ui`. Check: it has no path dependency on the Sayso repository.
10. **Registry.** The repository, CI, the signing key, the signed index, revocation, and the Browse tab of the Marketplace. Check: the media extension installs from the registry on each platform that it supports, and a revoked version gets turned off.
11. **Guide and template.** A tutorial that builds a small extension, a how-to for each task, and the reference of the Host API, made from the media extension. Check: someone other than Chris builds an extension from the guide alone.
12. **Meetings.** System audio capture on each platform, live sessions with two sources, the echo rule, and the Notetaker extension. Check: a call without headphones gives one copy of each sentence, with the speakers of the system audio apart.

## Progress

| Step | State | Notes |
|---|---|---|
| 1. Decisions | Done (2026-10-23) | This file and ADRs 0001 to 0004. |
| 2. Windows on `sayso-engine` | Done, packaged app check open | Done: the Windows catalog (`models_onnx.rs`, `System`), `whisper_cpp` and `redecode` in `sayso-engine`, the Windows stdout redirect, the shared sherpa-onnx DLLs, the legacy files (`format_changed` and Whisper adoption), the app side of `format_changed`, the scripts, the installer, CI, and the deletion of `native/portable`. Tested on macOS and in Linux Docker, with real Parakeet TDT v3 on Linux. On the Windows reference machine (2026-10-23): the release build, the size and speed checks of "Check first", Parakeet TDT v3 and Whisper through the engine protocol, the preview, and the legacy files with a copy of real 0.11 files. Open: a dictation in the packaged Windows app, and an update from 0.11 through the installer. `crates/sayso-engine/NOTES.md` lists what is not tested. |
| 3. Engine protocol | Done | Done (3a): the spec in `docs/engine-protocol.md` (MIT), `features` in `hello` (Swift, Rust, and the fake engine), and the conformance suite `sayso-engine-conformance` (MIT) with its core, model, and ready levels. The Swift engine passes all three levels with Parakeet Unified, the Rust engine passes the core and model levels in `cargo test`, and the fake engine passes the core and ready levels. Done (3b): concurrent streams (at least 3, `too_many_streams`; a pool of streaming managers in the Swift engine) and segments (`transcribe` with `segments: true`, the timestamp level in the catalog, `EngineKind::timestamps`). Both engines pass the new ready checks: Swift with Parakeet Unified and Nemotron, Rust with Parakeet TDT v3 on Windows. Done (3c): VAD (`vad`, `vad_start`, `speech_start`, `speech_end`) and diarization (`diarize`). Rust: Silero and TEN VAD and sherpa-onnx diarization (pyannote segmentation 3.0 and a speaker embedding model); passes on Linux Docker and on Windows. Swift: FluidAudio Silero VAD and the FluidAudio offline diarizer. Each engine applies the VAD rules itself, because neither library cuts speech at 30 s. Done (3d): public model ids. Each engine has a built-in catalog from `sayso-core` (`engine_catalog`): the Rust engine reads it at start, and the Swift engine gets `Catalog.generated.swift`, which a test keeps up to date. `list_models` without `models` gives the catalog, requests can leave out `engine`, and an id that leaves the catalog stays as `retired`. |
| 4. VAD and diarization roles | Done, full check after step 8 | Done: `ModelTask` (speech, VAD, diarization, from the engine kind) and the kinds `SherpaVad`, `SherpaDiarization`, `FluidVad`, and `FluidDiarization`. The catalog entries `silero-vad` and `speaker-diarization` on each system (TEN VAD is out, see "VAD models"), with the roles `vad` and `diarization` in the engine catalogs. `models::catalog()` and `find` have speech models only, so the final model, the preview model, the popover, onboarding, and `memory::wanted` cannot get one (tests in `models.rs`, `config.rs`, `memory.rs`, and `model_step.rs`). `local_models()` and `find_local` have all of them, for the engine client. `[models] vad` and `diarization` in `config.toml` ("auto" by default, a model of another role warns and counts as "auto"). Basic VAD (`basic_vad.rs`): an energy detector over the noise floor with the rules of the engines, offline and streaming. `vad_for` and `diarization_for` choose the model of a job. The Models page has **Voice detection** (Automatic, Basic, Silero VAD) and **Speakers** (Automatic, Speaker Diarization) with Download, Use, In use, and Model details, when `<data dir>/extensions/` has a folder (`extensions::any_installed`). The benchmark chose the sherpa-onnx embedding model and the clustering threshold ("Diarization models"). Open: "used in a job" needs the Host API (step 6) and extensions (step 8). |
| 5. Engine release files | Done | `scripts/package-engine.sh` and `scripts/package-engine.ps1` pack the engine of each bundle into `sayso-engine-<version>-<os>-<arch>` with a README (`packaging/engine/README.md.in`), its licenses, and a checksum. The Release workflow attaches them. `engine-check.yml` unpacks each file on a clean runner and runs the core level of the conformance suite: it passed on `macos-arm64`, `linux-x86_64`, `linux-arm64`, and `windows-x64` (#41). The Swift engine reports the Sayso version. The Windows and Linux builds now ship the licenses of sherpa-onnx and ONNX Runtime. |
| 6. Host API and SDK | Done, two parts in later steps | Done (6a): `sayso-protocol` (MIT): the messages of the Host API with serde, the error codes, the capabilities, the PCM helpers, and the socket path rules. `sayso-host`: the socket (a Unix socket in a folder with mode `0700`; on Windows a named pipe with an ACL for the user), the identity check (`SO_PEERCRED` on Linux, `LOCAL_PEERTOKEN` and the code signature on macOS, `GetNamedPipeClientProcessId` on Windows), `state.toml`, `hello`, `bye`, the capability check, and `host_closing`. The other requests get `unsupported`. `sayso-extension` (MIT): connect, `hello`, requests, events, `HostGone`, and `reconnect`. Sayso starts the host after its services and closes it when it quits. The integration tests pass on macOS and in Linux Docker. On Windows the code compiles, and only the Windows job of CI runs the tests. The check of a Developer ID signature is tested only with a program that does not have one. Done (6b): `list_models` and `download_model`. A request that takes time runs on a worker thread with a `Responder`, and the host has one subscription to the events of the engine (`Bus`). `ask_download` of `HostServices` shows the question as a small window of `sayso-app` (`download_ask.rs`), and `--ask-download` shows it with sample text. `SttBackend::start_state` tells whether the engine has answered, so the host does not give the empty statuses of an engine that starts. Tested with a fake engine and a fake user in `crates/sayso-host/tests/models.rs`, and by hand with the real app, the fake engine, and a small client. Done (6c): segments, VAD, and diarization in the engine client (`transcribe_segments`, `detect_speech`, `start_vad`, `diarize`, and `features` of `SttBackend`). Jobs in `crates/sayso-host/src/jobs/`: the file with `symphonia` (WAV, FLAC, MP3, AAC, M4A; about 1 MB of binary), the plan of chunks, the speakers with the folding rule, the model checks, the queue, the `DictationGate`, and `ModelUse`. The resampler moved from `sayso-platform-common` to `sayso-core`. `transcribe_file` and `cancel_job` over the socket. Sayso holds jobs while a dictation runs and keeps the models of jobs in memory. Tested with a fake engine in unit tests and in `crates/sayso-host/tests/jobs.rs`, and with the real Swift engine in the ignored test `jobs_real_engine.rs`. Done (6d): live sessions in `crates/sayso-host/src/live/`: `session_start`, `session_audio`, and `session_stop` with the sources `mic` and `client`, `record_to`, the preview stream, the VAD (a VAD session of the engine or Basic VAD), the final pass of each part with segments, and diarization with the speaker rules of jobs. The model checks of jobs, `ModelUse`, and `DictationGate` serve live sessions too. One microphone capture in `sayso-app` (`mic.rs`) feeds dictation and a live session. The engine client waits for the reply of `stream_end` of a VAD session (`end_vad`), and keeps the `code` of an engine error (`too_many_streams`). `sayso-extension` has `Client::send` and `Event::Error`. A closed connection, and Sayso when it quits, stop the session. Tested with a fake engine in unit tests and in `crates/sayso-host/tests/live.rs`, with the real Swift engine in the ignored test `live_real_engine.rs`, and by hand: the real app with temporary folders, the real engine, a small client, and `SAYSO_FAKE_MIC` for a session with the source `mic`. Not tested: a real microphone, a change of the device while a session runs, and a dictation during a live session in the real app. Done (6e): `enhance` with the style step of a dictation (`style_step.rs` in `sayso-app`, shared with dictation and the **Enhance** of history) and the error codes of "Requests". `subscribe` and `dictation_done`, with the incognito rule of history (`incognito_during`) and the grant read for each event. Live sessions before jobs (`DictationGate::live_part`). The quit question (`quit.rs`, on the shared question window `question.rs`), the wait of **Restart to update** (`Host::holds`, `Host::set_holds_observer`), `name` in `state.toml`, and the download question that closes when its extension disconnects. `Client::connect_or_start` in `sayso-extension`. The test extension `sayso-ext` (`crates/sayso-extension/examples/sayso-ext.rs`): a command line program that prints each message as a JSON line. It is an example of the MIT SDK, so it shows how an extension uses the SDK and needs no crate of its own. Tested with fake services in `crates/sayso-host/tests/` (`enhance.rs`, `observe.rs`, `jobs.rs`, `live.rs`, `models.rs`, `host.rs`), with `sayso-ext` against a host in `test_extension.rs`, in unit tests of the app (`host.rs`), and by hand with the real app (temporary folders). The dictation benchmark is in `docs/benchmark.md`, "Dictation while an extension works". Not tested: `connect_or_start` with an installed Sayso on Windows and Linux (the rules have unit tests), the quit question and the wait of an update on Windows and Linux, and `enhance` with each kind of provider (the style step is the code of dictation). `sayso-engine-sdk` (MIT) starts an engine from the release file (`find_engine`), has one blocking method for each request of the engine protocol, events on a channel, and timeouts. `sayso-protocol` has the engine messages (`sayso_protocol::engine`). Its tests run a real engine: the Swift engine on macOS and `sayso-engine` on Linux and Windows. Open: the source `system` (step 12) and the recording state in the menu bar (step 8). |
| 7. `sayso-ui` as MIT | Done, not published | `sayso-ink` is new (MIT, serde only). `sayso_core::ink` re-exports it, and `NamedInk::label` is now `sayso_core::ink::NamedInkLabel`. `sayso-ui` depends only on `sayso-ink` and gpui-kit (no GPL crate in `cargo tree`). Keycaps take labels (`sayso-app` builds them in `widgets::hotkey_keycaps`). Texts and the locale come from hooks (`sayso_ui::l10n`). The tray and window icons moved to `sayso-app`. The fonts, icons, textures, and sounds moved to `crates/sayso-ui/assets`. `sayso-ink` is `license = "MIT"` and `sayso-ui` is `license = "MIT AND OFL-1.1"` (the fonts are OFL), each with a `LICENSE` file and `publish = true`. `cargo package -p sayso-ink -p sayso-ui` passes with `workspace-hack` removed. An empty GPUI app outside the repository draws a `sayso-ui` button. Open: the first `cargo hakari publish`, which waits for the SDK crates of step 6. |
| 8 to 12 | Not started | |

The media extension and the Notetaker extension are products. Each gets its own design file before its step: what it does, its screens, and its data. This file covers only what they need from Sayso.

## Names

These names are proposals. Change them before step 9 if you want others:

| Thing | Name |
|---|---|
| Registry repository | `watzon/sayso-extensions` |
| Template repository | `watzon/sayso-extension-template` |
| Media extension | `watzon/sayso-media`, id `app.justsayso.media` |
| Meeting extension | `watzon/sayso-notetaker`, id `app.justsayso.notetaker` |

The private key of the registry is a secret of the GitHub Actions of the registry repository. Its public key is in Sayso.

## Answered questions

Chris answered these on 2026-10-23.

- **Windows code signing:** not required. Only macOS builds need a signature (decision 6).
- **Source code:** the repository of a listed extension must be public (decision 7).
- **VAD and diarization on Windows:** Windows moves to `sayso-engine` and gets both from sherpa-onnx (decision 13). This replaces the first answer, Silero through `ort` in `native/portable`, because that answer left diarization open and kept three engines.
- **Engine status:** it leaves the sidebar footer. Settings › General already shows it.
- **Paid extensions:** not in the first version (decision 15).
- **VAD in dictation:** not now (decision 11).
- **The look of extensions:** `sayso-ui` becomes MIT (decision 4).
