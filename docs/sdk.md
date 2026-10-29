# The Sayso SDK

Status: proposed on 2026-10-29. Not approved, not built. It replaces steps 8 to 12 of [extensions.md](extensions.md). Chris decided on 2026-10-29 to drop extensions and to build this instead. The ADR is [0005](adr/0005-apps-use-sayso-through-the-sdk.md).

This file says how a separate app uses the speech features of Sayso. The app uses one SDK. When Sayso runs, the SDK uses Sayso: its engine, its loaded models, its microphone, and its scheduler. When Sayso does not run, the SDK starts its own engine. The app calls the same API in both cases.

## Goal

- An app built on the SDK works alone, and it uses a running Sayso when it can. A change of mode is one option, and the events are the same in both modes.
- Each model is on disk once on a computer, and in memory once while Sayso runs.
- One microphone capture serves dictation and the app while Sayso runs. Dictation stays first, and it stays as fast as it is today.
- The SDK captures the microphone and the system audio in both modes, so a meeting app can record a call.
- Sayso installs nothing, lists nothing, and starts nothing. There is no Marketplace and no registry.

## What changes

Steps 1 to 7 of `extensions.md` are done, and their work stays: the engine protocol, the VAD and diarization roles, the engine release files, the Host API, and the MIT crates. This plan changes what is around them.

| Part of `extensions.md` | Now |
|---|---|
| Engine protocol, conformance suite, engine release files | Kept. The engine protocol gets the rules of a shared model store ("Model store"). |
| Host API: transport, requests, scheduling, capture, lifecycle, privacy | Kept. A client is a connected app, not an extension. |
| Identity check against the install folder, `state.toml` | Replaced: the user approves an app at its first connection ("Approval"). |
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
The SDK starts its own engine and does the work in the process of the app. It uses the shared model store.

**Model store**
The one folder on a computer with the files of local models. Sayso and each engine in local mode use it. It is the models folder of Sayso.

**Job**, **Live session**, **Capture source**
As in `extensions.md`. A capture source is `mic`, `system`, or `client`.

## Decisions

Proposed. Chris confirms them before step 9.

1. **There are no extensions.** Sayso does not install, list, start, or update other apps. A connected app is an ordinary app that the user gets from its author. See [ADR 0005](adr/0005-apps-use-sayso-through-the-sdk.md).
2. **Any app can connect when the user approves it.** The first connection of an app shows a question in Sayso. Sayso remembers the answer by the identity of the executable, as the identity check does today.
3. **One SDK with two modes and the same API.** `Mode::Sayso`, `Mode::Local`, and `Mode::Auto`. The requests, the replies, and the events are the types of `sayso-protocol` in both modes.
4. **`Mode::Auto` follows Sayso.** It uses Sayso when Sayso runs, and local mode when it does not. Between pieces of work, it moves to Sayso when Sayso starts and to local mode when Sayso quits. It never moves work that runs.
5. **Jobs, live sessions, and capture are MIT.** The code of jobs and live sessions moves from `sayso-host` into the MIT crate `sayso-speech`, with Basic VAD, the resampler, and the file decoder. The microphone capture and the new system audio capture go into the MIT crate `sayso-capture`. Chris is the only author of this code (`git log`, checked on 2026-10-29). Check again before each move.
6. **One model store on each computer.** Local mode uses the models folder of Sayso, also when Sayso is not installed. Each engine follows the lock rules of "Model store", so two engines can use the folder at the same time.
7. **Features of the user stay in Sayso.** `enhance` (the providers and styles of the user), `subscribe` (`dictation_done`), and the download question exist only in Sayso mode. In local mode they get `unsupported`, and `download_model` downloads without a question: the app asks its own user.
8. **The engine still has one owner in each process.** Sayso serves apps through its host, not through its engine. An app in local mode starts its own engine. Decision 2 of `extensions.md` stays.
9. **The app ships its own engine.** It packs the engine release file for its systems. The SDK finds it with `sayso_engine_sdk::find_engine`. The SDK does not start the engine inside an installed Sayso, because a Sayso update would then change the engine of the app.
10. **`Mode::Auto` does not start Sayso.** Sayso has hotkeys and a menu bar icon, and the user did not ask for them. `Mode::Sayso { start: true }` starts Sayso (`connect_or_start`) when the app asks for it.

## Approval

### Identity

The host knows an app by its executable, as in "Who is calling" of `extensions.md`. Only the rule changes: the host compares the identity with the records of approved apps, not with the install folders.

| System | Identity |
|---|---|
| macOS, signed with a team id (Developer ID or Apple Development) | The team id and the signing identifier, from the audit token of the peer |
| macOS, ad hoc or not signed | The path of the executable. The question says that the app is not signed. |
| Linux | `/proc/<pid>/exe`. In a Flatpak, the app id from `/proc/<pid>/root/.flatpak-info`. |
| Windows | The image path from `GetNamedPipeClientProcessId` |

A path identity breaks when the app moves. Then Sayso asks again. A signed macOS app keeps its identity through updates, so Sayso does not ask again. An ad hoc build of a developer gets a new signature with each build, which is why it uses the path.

### The question

`hello` gets two new fields: `app {name}` and `capabilities: [...]`, the capabilities that the app asks for. When the identity has no record, the host asks the user, and holds the reply of `hello` until the answer:

- It shows the name from the system (the bundle name on macOS, else the file name), the identity (team and identifier, or the path), and the name that the app gave, as a quote.
- It lists each capability in plain words ("Record from your microphone", "Record the sound of other apps and calls", "See each dictation after it is inserted").
- **Allow** grants all listed capabilities. **Don't allow** refuses. Closing the question counts as **Don't allow**.
- The host remembers both answers in `<data dir>/apps.toml`. After **Don't allow**, `hello` gets `error {code: "not_approved"}` at once, with no question, until the user removes the record in Settings.
- One question for each identity at a time. A second connection of the same app waits for the answer.
- An approved app that asks for a new capability gets a question for the new capabilities only. Until the answer, the app has its old grants.
- `models.read` needs no grant.

The system permissions stay separate. Sayso asks the system for the system audio permission the first time a live session in Sayso mode asks for the source `system`.

`apps.toml` replaces `extensions/state.toml`:

```toml
[[apps]]
name = "Notetaker"            # The name that Sayso shows.
team_id = "ABCDE12345"        # macOS, signed apps.
identifier = "app.justsayso.notetaker"
# path = "/opt/notetaker/bin/notetaker"   # Other identities.
allowed = true
granted = ["session.live", "audio.mic", "audio.system", "transcribe.file"]
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
| `sayso-capture` | New. The microphone (cpal) and the system audio, as 16 kHz mono frames. |
| `sayso-speech` | New. Jobs and live sessions over an engine: VAD chunking, the final pass of each part, diarization with the speaker rules, the echo rule, Basic VAD, the resampler, and the file decoder (`symphonia`). It has no I/O of its own: the engine, the capture, and a gate come in through traits. |
| `sayso-sdk` | New. Replaces `sayso-extension`. `Speech` with the three modes. |

The GPL crate `sayso-host` depends on `sayso-speech` and `sayso-capture`, and gives them the engine client of Sayso and `DictationGate`. Local mode gives them `sayso-engine-sdk` and a gate that never waits. So the two modes run the same code for jobs and live sessions.

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
- `speech.supports(Feature::Enhance)` tells the app what the mode it has now can do.

### Mode changes in `Mode::Auto`

- The SDK watches the socket of the host while it is in local mode.
- When the host appears and the SDK has no job, no live session, and no download, it connects and sends `mode_changed {mode: "sayso"}`. Then it stops its engine, so the model is in memory once.
- When the host sends `host_closing`, its work ends as it does today (`job_failed`, the end of the live session). The SDK sends `mode_changed {mode: "local"}`. The app can start the work again, and it runs in local mode.
- A move never happens while work runs. If Sayso starts during a long job in local mode, the job ends in local mode, and the next job goes to Sayso.

### Local mode

- The engine: from the release file that the app ships (decision 9). The SDK checks `hello`: the protocol version, and the features that the request needs.
- Models: the model store, with the lock rules. `download_model` downloads at once.
- Capture: `sayso-capture` in the process of the app. On macOS the app needs its own microphone and system audio permissions, and the usage text in its `Info.plist` (`NSMicrophoneUsageDescription`, `NSAudioCaptureUsageDescription`).
- Scheduling: the limits of the host (one live session, one job at a time) apply in the SDK too. There is no dictation in local mode, so the gate never waits.
- When Sayso dictates at the same time, the two engines share the hardware, and dictation can be slower. `Mode::Auto` keeps this time short, because it moves to Sayso at the next piece of work.

## Model store

The store is the models folder of Sayso (`Paths::models_dir`): `~/Library/Application Support/Sayso/models` on macOS, `%LOCALAPPDATA%\Sayso\models` on Windows, and `$XDG_DATA_HOME/sayso/models` on Linux. In the Flatpak of Sayso it is `~/.var/app/dev.sayso.Sayso/data/sayso/models`. The SDK has the same rules, because an MIT crate cannot use `sayso_core::paths`. Local mode uses the store also when Sayso is not installed, so a later Sayso finds the models.

Today two engines must not use one models folder at the same time. The new engine feature `shared_store` adds these rules. Both engines and the conformance suite need them.

- **Locks.** Each model has a lock file, `<store>/.locks/<model id>.lock`. A download, a delete, and a change of format take an exclusive lock. A load takes a shared lock and keeps it while the model is loaded. The locks are `flock` on macOS and Linux and `LockFileEx` on Windows, so a process that stops lets them go.
- **Download.** An engine that cannot get the exclusive lock because another engine downloads the model reports `downloading` and waits for the lock. Then it reads the state again. So one download runs, and both clients see it.
- **Complete files only.** A download writes into a temporary folder in the store and renames it into place. A stopped download leaves no model that looks complete. Check what each engine does today.
- **Delete.** A delete of a model that another engine has loaded gets `error {code: "in_use"}`. On the Models page, Sayso says "Another app is using this model." and does not delete it.
- **Format.** Two engines of different versions can need different files for one model id (`format_changed` today). An engine never replaces the files of a format that another engine has loaded. The exact rule is open (see "Open questions").
- **Older Sayso versions** do not know the locks. A Sayso before this release can delete a model that an app in local mode has loaded. The app then gets an engine error, and no file is lost. The SDK uses the store only with an engine that lists `shared_store`.

## System audio

`sayso-capture` captures the system audio on each system. The host uses it for the source `system` in Sayso mode, and the SDK uses it in local mode.

- **macOS:** a Core Audio process tap of all processes except its own, through an aggregate device. It needs macOS 14.2 or later and the system audio permission. Sayso runs on 14.0, so on 14.0 and 14.1 the source `system` gets `unsupported`.
- **Windows:** WASAPI loopback of the default output. It follows a change of the default output.
- **Linux:** the monitor of the default sink in PipeWire, or through the PulseAudio interface of PipeWire. The Flatpak of Sayso needs access to the audio server. Check its permissions in the build.

Each source of a live session is a separate stream, as in "Capture" of `extensions.md`. The echo rule (drop a microphone segment that a system segment covers in time and in words) is in `sayso-speech`, so both modes have it. Set its limit with test recordings, and write it in this file.

The microphone part of `sayso-capture` comes from `sayso-platform-common/src/audio.rs` and the shared capture of `sayso-app/src/mic.rs`. The shared capture (one stream for dictation and a live session) stays in Sayso. `sayso-capture` gives the device and the frames.

## Security

- **Sayso does not say that an app is safe.** No person reviews connected apps. The approval question says what the app asks for and what the system knows about it.
- **An approved app is a native program with the rights of the user.** Approval controls only what Sayso gives it.
- **A path identity trusts the file at that path.** A program that replaces the file gets the grants. Malware that runs as the user can do that and more, which is outside what Sayso can protect against, as in `extensions.md`.
- **An app cannot borrow the permissions of Sayso in local mode.** It captures in its own process, so the system asks the user for the app.
- **In Sayso mode, Sayso records for the app.** The menu bar shows it, and **Stop** ends it.

## What goes away

- The `extensions` code paths of the host and the app: `state.toml`, the install folder check, `not_an_extension`, `extensions::any_installed`.
- The sections of `extensions.md` on the manifest, the package, install, update, remove, start, the Marketplace, the registry, the guide for extensions, and "Names".
- Outside the repository (Chris decides; nothing is changed by this plan): the private repositories `watzon/sayso-extensions` and `watzon/sayso-extension-template`, and the secret `SAYSO_REGISTRY_SIGNING_KEY` with its 1Password copy. `watzon/sayso-media` and `watzon/sayso-notetaker` stay as the repositories of the two apps.

## Order of work

Each step ends with a check that proves it. The numbers go on from `extensions.md`.

8. **Decisions.** This file and ADR 0005. A review by Codex (GPT-6-Astra). Check: Chris approves the decisions.
9. **Approval.** Paper designs first. The identity rules, `apps.toml`, `hello` with `app` and `capabilities`, the question, `not_approved`, Settings › Connected apps, the recording state of the menu bar, and the removal of the extension code. Check: the example app connects from any folder, Sayso asks once, the app works after **Allow**, gets `not_approved` after **Don't allow**, and stops at the next request when the user takes a grant back. On macOS and Linux here, on Windows through a pull request.
10. **MIT core.** `sayso-speech` and `sayso-capture` with the code from `sayso-host`, `sayso-core`, `sayso-platform-common`, and `sayso-app`. `sayso-host` uses them. Check: the tests of `sayso-host` pass with no change to what they expect, `cargo tree` of the MIT crates has no GPL crate, and the dictation benchmark (`docs/benchmark.md`) does not get slower.
11. **`sayso-sdk`.** The three modes, local mode, and the mode changes. Check: one test program gets the same events in both modes for a file job and a live session with the source `client`, with the fake engine and with the real engine. In `Mode::Auto` it moves to Sayso when Sayso starts and to local mode when Sayso quits, and never during work.
12. **Model store.** The feature `shared_store` in both engines and in the conformance suite, and the store in the SDK. Check: two engines on one store make one download for two requests of the same model, a delete of a loaded model gets `in_use`, and a download that is stopped leaves no model that looks complete. On each system.
13. **System audio.** `sayso-capture` on each system, the source `system` in the host and in local mode, and the echo rule. Check: a call without headphones gives one copy of each sentence, and the speakers of the system audio apart, in both modes, on each system.
14. **Guide.** A tutorial that builds a small app on the SDK, a how-to for each task, and the reference of the Host API. Publish the crates. Check: someone other than Chris builds an app from the guide alone.

The Notetaker and the media app come after step 13 and step 11. Each gets its own design file and repository.

## Open questions

For Chris:

1. **Names.** "Connected app" in the UI, and the crates `sayso-sdk`, `sayso-speech`, and `sayso-capture`.
2. **Are the two apps Sayso products?** For example "Sayso Notes" on justsayso.app with the release pipeline of Sayso, or brands of their own. This plan does not depend on the answer.

For the review:

3. **Model formats in a shared store.** When a newer engine needs new files for a model id that an older engine has loaded, where do the new files go? One option: a folder for each format (`<id>/<format>/`), and the old folder goes away when no engine has it loaded.
4. **Files outside the store.** WhisperKit and FluidAudio keep caches outside the models folder (for example the compiled Core ML models, and `~/Library/Application Support/FluidAudio`). Two engines may write them at the same time.
5. **A Sayso in a Flatpak and an app outside it.** The app must find the socket in `$XDG_RUNTIME_DIR/app/dev.sayso.Sayso/`, and the host must read the identity of a process in another namespace.
