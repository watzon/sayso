# The Sayso SDK

Status: proposed on 2026-10-29. Not approved, not built. It replaces steps 8 to 12 of [extensions.md](extensions.md). Chris decided on 2026-10-29 to drop extensions and to build this instead. The ADR is [0005](adr/0005-apps-use-sayso-through-the-sdk.md). Codex (GPT-6-Astra) reviewed the first version on 2026-10-29. This version includes the corrections for its findings ("Review").

This file says how a separate app uses the speech features of Sayso. The app uses one SDK. When Sayso runs, the SDK uses Sayso: its engine, its loaded models, its microphone, and its scheduler. When Sayso does not run, the SDK starts its own engine. The app calls the same API in both cases.

## Goal

- An app built on the SDK works alone, and it uses a running Sayso when it can. A change of mode is one option. Both modes use the same messages and the same rules for the end of each piece of work.
- Each model artifact is on disk once for each user.
- While Sayso runs, each model is in memory once, with three exceptions: an app in `Mode::Local`, an app that the user refused, and local work that started before Sayso and still runs.
- One microphone capture serves dictation and the app while Sayso runs. Dictation stays first, and it stays as fast as it is today.
- The SDK captures the microphone and the system audio in both modes, so a meeting app can record a call.
- Sayso installs nothing, lists nothing, and starts nothing. There is no Marketplace and no registry.

## What changes

Steps 1 to 7 of `extensions.md` are done, and their work stays: the engine protocol, the VAD and diarization roles, the engine release files, the Host API, and the MIT crates. This plan changes what is around them.

| Part of `extensions.md` | Now |
|---|---|
| Engine protocol, conformance suite, engine release files | Kept. The engines get a new model store ("Model store"). |
| Host API: transport, requests, scheduling, capture, lifecycle, privacy | Kept. A client is a connected app, not an extension. |
| Identity check against the install folder, `state.toml` | Replaced: the user approves an app at its first connection ("Approval"). The signature check of macOS stays. |
| Capabilities | Kept. The user grants them in the approval question, not at install. |
| `sayso-extension` (the client) | Replaced by `sayso-sdk`, which has the client and the local mode. |
| Jobs and live sessions in `sayso-host` (GPL) | Move to an MIT crate, so the local mode runs the same code. |
| System audio in the host only | In the MIT crate `sayso-capture`, for both modes. |
| Manifest, package, install, update, remove, sideload, developer mode | Dropped. |
| Marketplace, sidebar footer button | Dropped. |
| Registry, its signing key, revocation, review | Dropped. |
| The guide and the template for extensions | A guide for the SDK (step 14). |
| Media and Notetaker extensions | Separate apps on the SDK, each with its own design file. |

## Terms

These go into `CONTEXT.md` with the code. They replace **Extension**, **Host** (as "host of extensions"), and the install part of **Capability**.

**Connected app**
A separate app that uses Sayso through the Host API. The user approved it. Sayso does not install it, start it, or update it.
_Avoid_: extension, plug-in, add-on, client (in UI text).

**Approval**
The answer of the user to the first connection of an app: **Allow** with the capabilities that the app asked for, or **Don't allow**. Sayso remembers it.

**SDK**
The MIT crate `sayso-sdk`. An app uses it to transcribe files and live audio, in Sayso mode or in local mode.

**Sayso mode**
The SDK talks to a running Sayso through the Host API. Sayso does the work with its engine.

**Local mode**
The SDK starts its own engine and does the work in the process of the app. It uses the model store.

**Model store**
The folder of one user with the files of local models, which Sayso and each engine in local mode use (`<models folder>/v1`).

**Artifact**
A folder of model files in the store that never changes after it is written: the files of one model in one format, or a file set that more than one model uses, such as a tokenizer. Its name is a hash of its contents.

**Job**, **Live session**, **Capture source**
As in `extensions.md`. A capture source is `mic`, `system`, or `client`.

## Decisions

Proposed. Chris confirms them before step 9.

1. **There are no extensions.** Sayso does not install, list, start, or update other apps. A connected app is an ordinary app that the user gets from its author. See [ADR 0005](adr/0005-apps-use-sayso-through-the-sdk.md).
2. **Any app can connect when the user approves it.** The first connection of an app shows a question in Sayso. Sayso remembers the answer by the identity of the executable. The macOS signature check of today stays.
3. **One SDK with two modes and one API.** `Mode::Sayso`, `Mode::Local`, and `Mode::Auto`. Both modes use the message types of `sayso-protocol` and the same end rules: each job ends with one `job_done` or `job_failed`, each live session with one end, and each request with one reply. The order and the contents of events can differ between modes ("What differs between modes").
4. **`Mode::Auto` follows Sayso.** It uses Sayso when Sayso runs, and local mode when it does not. It changes mode only when the app has no active work, and the change and the next request cannot overlap.
5. **Jobs, live sessions, and capture are MIT.** The code of jobs and live sessions moves from `sayso-host` into the MIT crate `sayso-speech`, with Basic VAD, the resampler, and the file decoder. The microphone capture and the new system audio capture go into the MIT crate `sayso-capture`. Chris is the only author of this code (`git log`, checked on 2026-10-29). Check again before each move.
6. **A new model store with immutable artifacts.** Sayso and every engine in local mode use one store for each user. Artifacts never change. A model id points to its artifacts through a manifest. The store is a new folder (`v1`), so the engines of older versions never change it.
7. **Features of the user stay in Sayso.** `enhance` (the providers and styles of the user), `subscribe` (`dictation_done`), and the download question exist only in Sayso mode. In local mode they get `unsupported`, and `download_model` downloads without a question: the app asks its own user.
8. **The engine still has one owner in each process.** Sayso serves apps through its host, not through its engine. An app in local mode starts its own engine. Decision 2 of `extensions.md` stays.
9. **The app ships its own engine.** It packs the engine release file for its systems. The SDK finds it with `sayso_engine_sdk::find_engine`. The SDK does not start the engine inside an installed Sayso, because a Sayso update would then change the engine of the app.
10. **`Mode::Auto` does not start Sayso.** Sayso has hotkeys and a menu bar icon, and the user did not ask for them. `Mode::Sayso { start: true }` starts Sayso (`connect_or_start`) when the app asks for it.

## Approval

### Identity

The host knows an app by its executable, as in "Who is calling" of `extensions.md` (`crates/sayso-host/src/identity.rs`). Only the rule changes: the host compares the identity with the records of approved apps, not with the install folders.

| System | Identity |
|---|---|
| macOS, signed with a team id (Developer ID or Apple Development) | The team id and the signing identifier. The host checks them as today (`identity::signed_by`): the requirement `anchor apple generic`, the team id, and the identifier, against the audit token of the peer. |
| macOS, with a signature that is not valid | None. The host refuses the connection with `not_approved` and asks no question. It does not treat the program as a program without a signature. |
| macOS, ad hoc or not signed | The path of the executable. The question says that the app is not signed. |
| Linux | `/proc/<pid>/exe`. When the file was deleted or replaced after the start (the link ends in ` (deleted)`), the host refuses the connection. |
| Windows | The image path of `QueryFullProcessImageNameW` for the pid of `GetNamedPipeClientProcessId`, made canonical with `GetFinalPathNameByHandleW`. Paths are compared without case. |

Notes:

- A path identity breaks when the app moves. Then Sayso asks again. A signed macOS app keeps its identity through updates.
- An ad hoc build of a developer gets a new signature with each build, which is why it uses the path.
- **Interpreters.** A script that runs in Python, Node, or another interpreter has the path of the interpreter. An approval of it approves every script of that interpreter. The question says this when the executable is a known interpreter. An app that needs its own identity ships an executable.
- **Flatpak.** A Sayso in a Flatpak and an app outside it, or the reverse, is not supported until step 9a proves it (see "Flatpak"). The host never trusts a Flatpak app id that the client sends.

### The question

`hello` gets two new fields: `app {name}` and `capabilities: [...]`, the capabilities that the app asks for. When the identity has no record, the host asks the user, and holds the reply of `hello` until the answer.

What the question shows:

- The name from the system (the bundle name on macOS, else the file name), and the identity (team and identifier, or the path).
- The name that the app gave, marked as "The app calls itself …". The host removes control characters and characters that change the direction of text, and cuts the name to 40 characters.
- Each capability in plain words ("Record from your microphone", "Record the sound of other apps and calls", "See each dictation after it is inserted").

The answers:

- **Allow** grants all listed capabilities. **Don't allow** refuses. Closing the question counts as **Don't allow**.
- The host remembers both answers in `<data dir>/apps.toml`. After **Don't allow**, `hello` gets `error {code: "not_approved"}` at once, with no question, until the user changes the answer in Settings.
- An approved app that asks for a new capability gets a question for the new capabilities only. Until the answer, the app has its old grants. A capability that the user refused is in `denied`, and the host does not ask for it again. The `hello` reply lists the granted and the denied capabilities.
- `models.read` needs no grant.

Limits, so an app cannot fill the screen with questions:

- One approval question is on the screen at a time, for all apps.
- At most four connections wait for an approval question. A fifth gets `error {code: "busy"}` at once.
- A connection that closes while it waits takes back its question.
- Questions of the same identity count as one.

The system permissions stay separate. Sayso asks the system for the system audio permission the first time a live session in Sayso mode asks for the source `system`.

`apps.toml` replaces `extensions/state.toml`:

```toml
[[apps]]
name = "Notetaker"            # The name from the system, which Sayso shows.
team_id = "ABCDE12345"        # macOS, signed apps.
identifier = "app.justsayso.notetaker"
# path = "/opt/notetaker/bin/notetaker"   # Other identities.
allowed = true
granted = ["session.live", "audio.mic", "audio.system", "transcribe.file"]
denied = ["dictation.observe"]
first_seen = 2026-10-29T10:00:00Z
```

The host reads the file again when it changes, as today. A grant that the user takes back stops the next request.

### In Sayso

Each item needs a design in Paper before code.

- **The approval question**, a small window like the download question (`download_ask.rs`).
- **Settings › Connected apps**: each app with its identity, its grants as switches, and **Remove**. An app that the user refused is in the list too, so the user can change the answer.
- **The recording state** of the menu bar icon (the tray icon on Windows and Linux) while a live session captures the microphone or the system audio. The popover names the app and has **Stop**. This was in step 8 of `extensions.md`.
- **The Models page** shows the **Voice detection** and **Speakers** sections when at least one app is approved. Today the rule is "when `<data dir>/extensions/` has a folder".
- The quit question and the update wait exist. They name the app by its record in `apps.toml`.

## The SDK

### Crates

All MIT, with `publish = true`.

| Crate | What it has |
|---|---|
| `sayso-protocol` | Kept. The messages of the engine protocol and the Host API. |
| `sayso-engine-sdk` | Kept. Start an engine and talk to it over stdio. |
| `sayso-capture` | New. The microphone (cpal) and the system audio, as 16 kHz mono frames with capture times. |
| `sayso-speech` | New. Jobs and live sessions over an engine (see below). |
| `sayso-sdk` | New. Replaces `sayso-extension`. `Speech` with the three modes. |

`sayso-speech` has the code of `crates/sayso-host/src/jobs/` and `src/live/`: VAD chunking, the final pass of each part, diarization with the speaker rules, the echo rule, Basic VAD, the resampler, and the file decoder (`symphonia`). It reads the files of jobs and writes recordings, so it does file I/O. It owns its types, and it gets the rest through traits:

| Trait or type | What it does | Sayso (`sayso-host`) | Local mode (`sayso-sdk`) |
|---|---|---|---|
| Model, event, and error types | The types of jobs and live sessions, MIT | Converts from `sayso-core` types | Uses them |
| `SpeechEngine` | `transcribe`, streams, VAD, diarization | The engine client of Sayso (`SttBackend`) | `sayso-engine-sdk` |
| `ModelResolver` | The model of each role when a request names none, and the checks (`unknown_model`, `wrong_role`, ...) | The choices of the user and the catalog of Sayso | The options of the app and the engine catalog |
| `ModelRetention` | Hold and release a model while work runs | `ModelUse` and the memory policy | Load and unload in the SDK |
| `Gate` | Wait while dictation runs | `DictationGate` | Never waits |
| `EventSink` | Where events go | The connection of the app | The event channel of the SDK |
| `Recordings` | Where `record_to` and temporary recordings go | The folders of today | The same rules |
| Cancellation | Stop a job or a session between engine calls | As today | As today |

Live sessions already have `LiveEngine` and capture traits (`live/engine.rs`), but they use GPL types. Jobs call `SttBackend` directly (`jobs/runner.rs`). Step 10 changes both to the traits above. The settings of the user and the policy of the catalog stay in the adapters, not in `sayso-speech`.

`sayso-extension` 0.13.0 is on crates.io. Its last version says in its README that it moved to `sayso-sdk`.

### API

A sketch. The names change in the build.

```rust
let speech = Speech::connect(Options {
    mode: Mode::Auto,
    app: App::new("Notetaker"),
    capabilities: &[Capability::SessionLive, Capability::AudioMic, Capability::AudioSystem],
    engine: Engine::from_release(resources.join("engine")), // used in local mode
})?;

speech.mode();                                  // Mode::Sayso or Mode::Local, now
let job = speech.transcribe_file(path, FileOptions::default().diarize())?;
let session = speech.start_session(SessionOptions::sources([Source::Mic, Source::System]).record_to(folder))?;
for event in speech.events() { /* segment, partial, speakers, job_done, mode_changed, ... */ }
speech.stop_session(session)?;
```

- `Speech::connect` with `Mode::Sayso` fails when Sayso does not run (`host_not_running`) or the user refused the app (`not_approved`).
- `Mode::Local` never talks to Sayso.
- `Mode::Auto` tries Sayso, and uses local mode when Sayso does not run or refused the app. A refused app does not ask again, because the host remembers the answer.
- `speech.features()` tells the app what the mode that it has now can do. `mode_changed {mode, features}` tells the app when that changes.

### What differs between modes

The messages and the end rules are the same. These things differ, and the tests cover each one:

- **Downloads.** In Sayso mode the user can refuse a download (`declined`). In local mode a download starts at once.
- **Models.** In Sayso mode a request without a model gets the choice of the user. In local mode it gets the choice of the app (`Options`), else the first suitable model on disk.
- **Queues.** In Sayso mode the limits (one live session, one job at a time) are for all apps together, and a job can wait for another app. In local mode they are for this app only.
- **Priority.** Sayso mode puts dictation first, then live sessions, then jobs. Local mode has no dictation, and still puts live sessions before jobs.
- **Features.** `enhance`, `subscribe`, and the download question are in Sayso mode only. The features of the engine can differ too, because Sayso and the app can ship different engine versions.

### Mode changes in `Mode::Auto`

- **Active work.** A request that has no reply yet, a job that waits or runs, a live session (also while it stops), a download, and an approval question are active work.
- **One lock.** The SDK starts a request and changes mode under one lock. So a request that starts during a change waits for the change, and a change waits until no work is active.
- **To Sayso.** While in local mode, the SDK watches for the socket of the host. When it appears, the SDK opens a connection and finishes `hello`: the approval, the protocol version, and the features. Only then, and only when no work is active, does it change mode. It sends `mode_changed`, and then stops its engine, so the model is in memory once. When `hello` fails, it stays in local mode and tries again after 30 s or when the socket changes.
- **To local.** On `host_closing`, the work of the host ends as today (`job_failed`, the end of the live session), and the SDK changes to local mode and sends `mode_changed`.
- **A lost connection.** When the connection ends without `host_closing` (Sayso crashed), each request without a reply gets `error {code: "connection_lost"}`, each job gets `job_failed {code: "connection_lost"}`, and each live session ends with the same code. The SDK never sends a request again by itself, because it cannot know if the host did it.
- **Generations.** Each connection has a number. The SDK drops events of an older connection, so they cannot reach new work.
- A change never moves work. When Sayso starts during a long job in local mode, the job ends in local mode, and the next job goes to Sayso.

### Local mode

- The engine: from the release file that the app ships (decision 9). The SDK checks `hello`: the protocol version, and the features that the request needs.
- Models: the model store. `download_model` downloads at once.
- Capture: `sayso-capture` in the process of the app. On macOS the app needs its own microphone and system audio permissions, and the usage text in its `Info.plist` (`NSMicrophoneUsageDescription`, `NSAudioCaptureUsageDescription`).
- When Sayso dictates at the same time, the two engines share the hardware, and dictation can be slower. `Mode::Auto` keeps this time short, because it moves to Sayso at the next piece of work.

## Model store

### Why a new store

The models folders of today cannot be shared:

- One model id does not own its files. In the Swift engine, Parakeet Unified and the CTC helper are separate folders that a delete of Parakeet Unified removes, and the Whisper variants share a tokenizer folder (`native/macos/SaysoEngine/Sources/SaysoEngine/ModelStore.swift`, `delete`). A lock for each model id cannot protect a file that two ids use.
- A download changes files in place. The Rust engine removes old files, then renames each archive into the model folder, then writes a marker (`crates/sayso-engine/src/store.rs`, `prepare_download`). The Swift engine downloads into the folder of the model and then writes a marker.
- A load can write. FluidAudio deletes the files and downloads them again when a load fails (`ModelHub.loadModels`), unless `offlineMode` is on.
- The engines of older versions delete model folders without a lock (`store.rs`, `delete`). No new rule can make them wait.

### Layout

The store is `v1` in the models folder of Sayso (`Paths::models_dir`): `~/Library/Application Support/Sayso/models/v1` on macOS, `%LOCALAPPDATA%\Sayso\models\v1` on Windows, and `$XDG_DATA_HOME/sayso/models/v1` on Linux. The SDK has the same rules, because an MIT crate cannot use `sayso_core::paths`. Local mode uses the store also when Sayso is not installed, so a later Sayso finds the models. The engines of older versions never open `v1`.

```
v1/
  artifacts/<digest>/      files that never change, and a `complete` file last
  models/<model id>/<format>.json   the manifest: format, source revision, artifact digests
  locks/<digest>.use       shared while loaded, exclusive to delete
  locks/<digest>.get       exclusive while one engine downloads the artifact
  locks/store              exclusive for a migration
  staging/<random>/        downloads in progress
  cache/<key>/             caches that the engines control, each with its own lock
```

- **Artifact.** The digest is the SHA-256 of the list of its files with their hashes, and the source revision. Shared files, such as a Whisper tokenizer or the CTC helper, are artifacts of their own. Two engines that read the same format use the same artifact.
- **Format.** Each engine kind names the format of its files (for example `fluidaudio-coreml-1`, `sherpa-onnx-1`). Manifests of two formats for one model id live side by side, so engines of different versions and different formats never replace each other's files.

### Rules

These rules are in the engine feature `store_v1`, in both engines and in the conformance suite.

- **Load.** Read the manifest. Take a shared lock on the `use` file of each artifact, in the order of the digests. Check that each artifact has its `complete` file. Then load. A load never writes in the store: a load that fails reports `failed`, and FluidAudio runs with `offlineMode`. The engine keeps the shared locks until it no longer holds any object that reads the files: after `unload`, and after the end of each running job and stream that uses the model.
- **Download.** First check, with no lock, whether the manifest exists and each artifact is complete. If so, the model is on disk. Else, for each artifact that is missing: take the exclusive `get` lock (an engine that waits for it reports `downloading`), check again, download into `staging/`, check each file against its hash, and rename the folder to `artifacts/<digest>`. Then write the manifest to a temporary file and rename it into place. A loaded model never blocks a download, because a download does not use the `use` lock.
- **Delete a model** (the user, in Sayso or in an app): rename its manifest away and delete it. This never fails because of another app. An app that has the model loaded keeps it until it unloads. At its next `list_models`, the model is not on disk.
- **Clean up.** An artifact that no manifest names is garbage. An engine tries the exclusive `use` lock without waiting. When it gets it, the engine deletes the artifact. When it does not, another engine has it loaded, and the engine tries again at its next start and after its next delete.
- **Lock files** are never deleted. `flock` on macOS and Linux. On Windows, `LockFileEx` on byte 0 with a length of 1, on a handle opened with `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`. A process that stops lets its locks go.
- **A stopped download** leaves only a folder in `staging/`. An engine deletes folders in `staging/` that are older than one day and that no `get` lock holds.

### Migration and downgrade

- At its first start, a Sayso with `store_v1` takes the `store` lock and moves the models of the old folder into `v1`: it hashes the files and renames them into artifacts, so there is no copy and no new download. A file that does not match its expected hash stays in the old folder, and the model shows as not downloaded.
- After a downgrade, the older Sayso does not see `v1`, and the user downloads the models again. The release notes of the version with `store_v1` say this.
- The engine of an older app in local mode, without `store_v1`, uses its own folder (`SAYSO_MODELS_DIR`) as today. The SDK passes the store only to an engine that lists `store_v1`.

### Caches outside the store

Step 12 starts with a list of every place where each pinned library writes, for each engine. What is known:

- WhisperKit: the model and tokenizer folders go into the models folder today (`Engine.swift`). They move into artifacts.
- FluidAudio: Sayso gives the folders of its loaders. The default `~/Library/Application Support/FluidAudio` must not be used.
- A cache that an engine controls goes into `cache/<key>/` with a key that includes the format and the library version, and a lock for its writer.
- Caches that the system controls (the compiled Core ML models of macOS) and models that Apple manages (Apple Speech) are outside the store and outside the promise of one copy.

## System audio

`sayso-capture` captures the system audio on each system. The host uses it for the source `system` in Sayso mode, and the SDK uses it in local mode. The permission belongs to the process that captures: Sayso in Sayso mode, the app in local mode.

| System | How | Limits |
|---|---|---|
| macOS 14.2 and later | A Core Audio process tap through an aggregate device | Needs the system audio permission and `NSAudioCaptureUsageDescription`. On 14.0 and 14.1 the source `system` gets `unsupported`. |
| Windows | WASAPI loopback of the default output device | Sound that goes to another device is not captured. Follows a change of the default device, with a gap. |
| Linux | The monitor of the default sink in PipeWire, or through the PulseAudio interface of PipeWire | The Flatpak of Sayso needs access to the audio server. |

Rules for both modes:

- **What is left out.** The capture leaves out the sounds of Sayso and of the app when the system allows it. The macOS tap can leave out a list of processes, so it leaves out both. On Windows and Linux, the step checks what the system allows. The `ok` of `session_start` lists what the capture leaves out (`excludes`), so the app knows.
- **Times.** Each frame has the time of the capture clock. The microphone and the system audio of one session use the same clock, so the echo rule can compare them.
- **Device changes.** A change of the device gives an event `source_gap {source, ms}`.
- **Permissions.** A refused permission gives `error {code: "permission"}` at `session_start`. A permission that the user takes back during a session ends that source with `source_ended {source, reason: "permission"}`. The other sources go on.

Each source of a live session is a separate stream, as in "Capture" of `extensions.md`. The echo rule (drop a microphone segment that a system segment covers in time and in words) is in `sayso-speech`, so both modes have it. Set its limit with test recordings, and write it in this file.

The microphone part of `sayso-capture` comes from `sayso-platform-common/src/audio.rs` and the shared capture of `sayso-app/src/mic.rs`. The shared capture (one stream for dictation and a live session) stays in Sayso. `sayso-capture` gives the device and the frames.

## Flatpak

The Linux Flatpak of Sayso has its own folders, and these parts do not work across the sandbox today:

- **The socket.** `sayso_protocol::socket` uses the `FLATPAK_ID` of the caller (`crates/sayso-protocol/src/socket.rs`). An app outside the Flatpak does not look in `$XDG_RUNTIME_DIR/app/dev.sayso.Sayso/`.
- **The identity.** The host reads `/proc/<pid>/exe` of the peer. A process in another PID namespace can be missing from `/proc` of the sandbox.
- **The store.** The Flatpak has its own `XDG_DATA_HOME` (`~/.var/app/dev.sayso.Sayso/data`), so its store is not the store of an app outside it.

Step 9a tests each direction in real sandboxes before the rest of step 9. When the host cannot know the identity of an app across the sandbox, that combination is not supported, and the SDK of the app uses local mode. The host never trusts an app id that the client sends.

## Security

- **Sayso does not say that an app is safe.** No person reviews connected apps. The approval question says what the app asks for and what the system knows about it.
- **An approved app is a native program with the rights of the user.** Approval controls only what Sayso gives it.
- **A path identity trusts the file at that path.** A program that replaces the file gets the grants. Malware that runs as the user can do that and more, which is outside what Sayso can protect against, as in `extensions.md`.
- **The name that an app gives is not checked.** The question shows the name from the system first, and the name of the app only as a quote.
- **An app cannot borrow the permissions of Sayso in local mode.** It captures in its own process, so the system asks the user for the app.
- **In Sayso mode, Sayso records for the app.** The menu bar shows it, and **Stop** ends it.
- **The store is shared by every program of the user.** A program can change files in it. The engines check the hash of each file at download, not at each load.

## What goes away

- The `extensions` code paths of the host and the app: `state.toml`, the install folder check, `not_an_extension`, `extensions::any_installed`.
- The sections of `extensions.md` on the manifest, the package, install, update, remove, start, the Marketplace, the registry, the guide for extensions, and "Names".
- Outside the repository (Chris decides; nothing is changed by this plan): the private repositories `watzon/sayso-extensions` and `watzon/sayso-extension-template`, and the secret `SAYSO_REGISTRY_SIGNING_KEY` with its 1Password copy. `watzon/sayso-media` and `watzon/sayso-notetaker` stay as the repositories of the two apps.

## Order of work

Each step ends with a check that proves it. The numbers go on from `extensions.md`.

8. **Decisions.** This file and ADR 0005, with the review. Check: Chris approves the decisions.
9. **Approval.** Paper designs first.
   - **9a. Flatpak.** Test the socket, the identity, and the store in both directions with real sandboxes. Check: each combination works, or this file says that it is not supported and why.
   - **9b.** The identity rules, `apps.toml`, `hello` with `app` and `capabilities`, the question and its limits, `not_approved`, Settings › Connected apps, the recording state of the menu bar, and the removal of the extension code. Check: the example app connects from any folder, Sayso asks once, the app works after **Allow**, gets `not_approved` after **Don't allow**, and stops at the next request when the user takes a grant back. A program with a broken signature gets no question. Five connections at once get one question and one `busy`. On macOS and Linux here, on Windows through a pull request.
10. **MIT core.** `sayso-speech` and `sayso-capture` with the code from `sayso-host`, `sayso-core`, `sayso-platform-common`, and `sayso-app`, and the traits of "Crates". `sayso-host` uses them. Check: the tests of `sayso-host` pass with no change to what they expect, `cargo tree` of the MIT crates has no GPL crate, and the dictation benchmark (`docs/benchmark.md`) does not get slower.
11. **Model store.** First the list of every place where each library writes. Then `store_v1` in both engines and in the conformance suite, and the migration in Sayso. Check, on each system:
    - Two engines on one store make one download for two requests of the same model.
    - A model that one engine has loaded stays usable when another engine deletes it, and its artifacts go away after the unload.
    - A model that shares an artifact with another model keeps it when the other model is deleted.
    - An engine killed during a download, during a load, and during a migration leaves no artifact that looks complete, and the next start cleans up.
    - The engine of the last release and a new engine on the same computer do not change each other's files.
    - A load that fails does not write in the store.
12. **`sayso-sdk`.** The three modes, local mode, and the mode changes. Check: one test program gets the same message types and end rules in both modes, with the fake engine and with the real engine, for a file job, a live session with the source `client`, a refused download, a job that waits in the queue, a feature that one mode does not have, and a grant that the user takes back. In `Mode::Auto` it moves to Sayso when Sayso starts and to local mode when Sayso quits, and never during work. Kill Sayso during a job: the job ends with `connection_lost`, and no request runs twice.
13. **System audio.** `sayso-capture` on each system, the source `system` in the host and in local mode, and the echo rule. Check, in both modes and on each system: a refused permission, a permission taken back during a session, and a change of the output device give the events of "System audio". Then a call without headphones gives one copy of each sentence, and the speakers of the system audio apart.
14. **Guide.** A tutorial that builds a small app on the SDK, a how-to for each task, and the reference of the Host API. Publish the crates. Check: someone other than Chris builds an app from the guide alone.

The Notetaker and the media app come after steps 13 and 12. Each gets its own design file and repository.

## Review

Codex (GPT-6-Astra) reviewed the first version on 2026-10-29. Its findings, and what changed:

| Finding | Change |
|---|---|
| Locks for each model id cannot protect files that two models share | Immutable artifacts with their own locks ("Model store") |
| A download replaces files in place, and a FluidAudio load can delete and download again | Downloads go through `staging/` and a rename, and a load never writes (`offlineMode`) |
| An older engine deletes without a lock, so "no file is lost" was false | A new store folder `v1` that older engines never open, with a migration and a downgrade rule |
| The approval must keep the signature check, limit the questions, and not trust the name of the app | "Identity" and "The question" |
| A mode change can race with a request, and a crash was not covered | One lock for requests and changes, `connection_lost`, connection numbers |
| The two modes cannot give the same events | Same messages and end rules; "What differs between modes" |
| `sayso-speech` needs its own types and more traits, and it does file I/O | The table in "Crates" |
| System audio needs rules for permissions, devices, and what it leaves out | "System audio" |
| Flatpak does not work across the sandbox today | "Flatpak" and step 9a |
| The store must be ready before the SDK uses it | Step 11 is the store, step 12 the SDK |

## Open questions

For Chris:

1. **Names.** "Connected app" in the UI, and the crates `sayso-sdk`, `sayso-speech`, and `sayso-capture`.
2. **Are the two apps Sayso products?** For example "Sayso Notes" on justsayso.app with the release pipeline of Sayso, or brands of their own. This plan does not depend on the answer.
