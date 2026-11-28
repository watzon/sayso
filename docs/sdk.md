# The Sayso SDKs

Status: the design of 2026-10-29 ([ADR 0005](adr/0005-apps-use-sayso-through-the-sdk.md)), with the SDKs in Go, TypeScript, and Rust over the wire protocols since 2026-11-27 ([ADR 0007](adr/0007-sdks-in-go-typescript-rust-over-the-wire.md)). Sayso itself is a Go app (`cmd/sayso`). The history of the Rust version of this plan (its order of work, its reviews, and the write inventory of the Rust engine) is in the history of this file.

This file says how a separate app uses the speech features of Sayso. The app uses one SDK. When Sayso runs, the SDK uses Sayso: its engine, its loaded models, its microphone, and its scheduler. When Sayso does not run, the SDK starts its own engine. The app calls the same API in both cases.

## Goal

- An app built on an SDK works alone, and it uses a running Sayso when it can. A change of mode is one option. Both modes use the same messages and the same rules for the end of each piece of work.
- Each model artifact is on disk once for each user. The exception is a Flatpak: it has its own store, so an app in local mode and a Sayso on the other side of the sandbox use two stores.
- While Sayso runs, each model is in memory once, with four exceptions: an app in Local mode, an app that the user refused, local work that started before Sayso and still runs, and a sandboxed app without the permission for the socket.
- One microphone capture serves dictation and the app while Sayso runs. Dictation stays first.
- The SDKs capture the microphone and the system audio in both modes where the system allows it ("System audio").
- Sayso installs nothing, lists nothing, and starts nothing. There is no Marketplace and no registry.

## Terms

`CONTEXT.md` has the terms: **Connected app**, **Approval**, **SDK**, **Sayso mode**, **Local mode**, **Mode change**, **Audio helper**, **Model store**, **Artifact**, **Job**, **Live session**, and **Capture source**.

## Decisions

1. **There are no extensions.** Sayso does not install, list, start, or update other apps (ADR 0005).
2. **Any app can connect when the user approves it.** The first connection of an app shows a question in Sayso, and Sayso remembers the answer by the identity of the executable ("Approval").
3. **Three SDKs with one API and three modes.** Sayso mode, Local mode, and Auto. The modes use the messages of the Host API and the same end rules: each job ends with one `job_done` or `job_failed`, each live session with one end, and each request with one reply.
4. **The SDKs talk over the wire, and they are MIT.** Each SDK speaks the Host API to Sayso, the engine protocol to an engine, and the cleanup protocol to the cleanup runtime. No SDK links or imports code of the app (ADR 0007).
5. **Auto follows Sayso.** It uses Sayso when Sayso runs, and Local mode when it does not. It changes mode only when the app has no active work.
6. **Features of the user stay in Sayso.** `enhance`, `subscribe` (`dictation_done`), and the download question exist only in Sayso mode. In Local mode they get `unsupported`, and `download_model` downloads without a question.
7. **The app ships its own engine.** It packs the engine release file of its systems, or installs `@justsayso/engine`. The SDK never starts the engine inside an installed Sayso, because a Sayso update would then change the engine of the app.
8. **Auto does not start Sayso.** Sayso mode with `start` starts Sayso when the app asks for it.

## The SDKs

| SDK | Where | Package | What runs in the process of the app |
|---|---|---|---|
| Go | `sdk/go` | module `github.com/watzon/sayso/sdk/go`, `CGO_ENABLED=0` | Everything of Local mode: jobs and live sessions (`speech`), decoding and DSP (`audio`), capture (`capture/mic`, `capture/system`, `permission`), the cleanup client (`cleanup`), `deepgram`, and `embedded` |
| TypeScript | `sdk/typescript` | `@justsayso/sdk` (ESM, types, no runtime dependency) | Jobs and live sessions in TypeScript. Capture and the FLAC, MP3, and AAC decoders come from `sayso-audio`, a program ("Audio helper") |
| Rust | `sdk/rust` | crate `sayso-sdk`, features `local`, `capture`, `cleanup`, `deepgram` | Everything of Local mode, with `cpal` and the Core Audio tap for capture |

All three have the same version as Sayso (`internal/update/version.go`), and `scripts/sdk-boundary` checks it. Each speaks:

- the Host API to Sayso: [sdk/protocol/host-api.md](../sdk/protocol/host-api.md), with the paths of the socket and the pipe in [sdk/protocol/paths.md](../sdk/protocol/paths.md);
- the engine protocol to an engine: [engine-protocol.md](engine-protocol.md);
- the cleanup protocol v1 to the cleanup runtime: `sdk/protocol/testdata/cleanup` and `sdk/go/cleanup`;
- the audio helper protocol to `sayso-audio` (TypeScript only): [sdk/protocol/audio-helper.md](../sdk/protocol/audio-helper.md).

`sdk/protocol/testdata` is the wire corpus of every message, and each SDK and the host test against it.

### The boundary

- Everything under `sdk/` is MIT. The app and the programs that an app runs (the engines, `sayso-cleanup`) are GPL-3.0-only.
- `sdk/go` never imports a package of the app. The app imports `sdk/go`: the host gives live sessions the system audio of `sdk/go/capture/system`, and contract tests in `internal/cleanupd` and `internal/enhance` check that the app and `sdk/go/cleanup` agree.
- All Rust code is in `sdk/rust`, as one crate, with no native Node addon anywhere in the repository.
- An MIT npm package names a GPL package (`packaging/npm`) only as an optional peer.
- Code that came from the app is listed in `sdk/PROVENANCE.md`, with the check that its only author is Chris.

`go run ./scripts/sdk-boundary` checks the rules, and `.github/workflows/sdk-boundary.yml` runs it on each pull request.

### API

The shape is the same in each SDK. In Go:

```go
s, err := sayso.Connect(ctx, sayso.Options{
	Mode:         sayso.Auto,
	App:          "Notetaker",
	Capabilities: []sayso.Capability{sayso.CapTranscribeFile},
	Engine:       sayso.FromRelease(filepath.Join(resources, "engine")), // Local mode
})
s.Mode()                                  // sayso.ModeSayso or sayso.ModeLocal, now
job, err := s.TranscribeFile(ctx, path, sayso.FileOptions{})
sess, err := s.StartSession(ctx, sayso.SessionOptions{Sources: []sayso.Source{"mic"}, RecordTo: folder})
for e := range s.Events() { /* messages of the Host API, and ModeChanged */ }
text, err := s.StopSession(ctx, sess.ID)
```

TypeScript has `Speech.connect`, a promise for each request, and an async iterator of events. Rust has `Speech::connect`, blocking calls, and a channel of events. The READMEs of each SDK have the full API.

- Sayso mode fails when Sayso does not run (`host_not_running`) or the user refused the app (`not_approved`).
- Local mode never talks to Sayso.
- Auto tries Sayso, and uses Local mode when Sayso does not run or refused the app.
- `features()` says what the mode that the app has now can do. `mode_changed {mode, features}` says when that changes. An empty feature list from the host means a Sayso older than 0.17.3: the SDK then sends each request and reads the reply.
- The ids of jobs and sessions come from the SDK, in both modes, and never come back after a mode change.
- A live session ends with the reply of `stop_session`, or else with `session_ended`. Only the first counts.
- Sayso mode does not change mode. After the end of a connection, work ends with `connection_lost` and later requests fail. TypeScript and Rust send `disconnected` and have `reconnect()`; a Go app calls `Connect` again.
- Three error codes come only from the SDK: `host_not_running`, `connection_lost`, and `host_closing`.

### What differs between modes

The messages and the end rules are the same. These things differ, and the tests cover each one:

- **Downloads.** In Sayso mode the user can refuse a download (`declined`). In Local mode a download starts at once.
- **Models.** In Sayso mode a request without a model gets the choice of the user. In Local mode it gets the choice of the app, else the first suitable model on disk of the engine catalog.
- **Queues.** In Sayso mode the limits (one live session, one job at a time) are for all apps together. In Local mode they are for this app only.
- **Priority.** Sayso mode puts dictation first, then live sessions, then jobs. Local mode has no dictation, and still puts live sessions before jobs.
- **Features.** `enhance`, `subscribe`, and the download question are in Sayso mode only. The features of the engine can differ too, because Sayso and the app can ship different engine versions.

### Mode changes in Auto

- **Active work.** A request that has no reply yet, a job that waits or runs, a live session (also while it stops), a download, and an approval question are active work.
- **One lock.** The SDK starts a request and changes mode under one lock. A request that starts during a change waits for the change, and a change waits until no work is active.
- **To Sayso.** While in Local mode, the SDK watches for the socket of the host. When it appears, the SDK finishes `hello` (the approval, the protocol version, and the features), and only then, with no active work, changes mode. It sends `mode_changed`, and then stops its engine, so the model is in memory once. When `hello` fails, it stays in Local mode and tries again after 30 s or when the socket changes.
- **To Local.** On `host_closing`, the work of the host ends, and the SDK changes to Local mode and sends `mode_changed`.
- **A lost connection.** When the connection ends without `host_closing`, each request without a reply gets `connection_lost`, each job gets `job_failed {code: "connection_lost"}`, and each live session ends with the same code. The SDK never sends a request again by itself, because it cannot know if the host did it.
- **One end for each piece of work.** The first end of a piece of work removes it from the table of open work, and only that end goes to the app.
- **Generations.** Each connection has a number, and the SDK drops events of an older connection.
- A change never moves work. A long job in Local mode ends in Local mode, and the next job goes to Sayso.
- When `hello` waits for a person, the host sends `approval_waiting` at once. Without that event or a reply in 10 s, the SDK counts Sayso as not running.
- A watcher looks for the socket every 200 ms. A socket has an identity on Unix (device, inode, and change time). A named pipe on Windows has none, so only the 30 s help there after a failed `hello`.

### Local mode

- **The engine** comes from the release file that the app ships (`FromRelease`), from a program (`FromProgram`), or in TypeScript from `@justsayso/engine` (`Engine.fromPackage`). In a release folder the SDK picks the build that the processor runs (`-avx` and `-baseline` on x64). It checks `hello`: the protocol version, and the features that a request needs. It passes the model store (`SAYSO_MODEL_STORE`) only to an engine that lists `store_v1`. It starts the engine at the first request that needs it, and again after a crash when no open work uses it.
- **Models** come from the model store. A model stays in memory for 60 s after the last work that used it.
- **The checks and the codes** of a request are the same as in Sayso. The scenario tests run one table of requests in both modes.
- **Capture** runs in the process of the app, or of `sayso-audio` for TypeScript, so the system asks the user for the permission of the app. On macOS the app needs `NSMicrophoneUsageDescription` and, for system audio, `NSAudioCaptureUsageDescription` in its `Info.plist`.
- **Cleanup.** An app can run the local cleanup models itself: the catalog (`s1-mini`, `spokenly-cleanup-1`), the prompt, the plan of parts, the check of the answer, `restore_spelling`, the download with SHA-256 and `ready.json`, and the client of the runtime `sayso-cleanup` (`hello`, `load`, `count`, `complete`, `unload`).
- **Embedded sessions** (`embedded`) run a live session inside the app with an engine layer of the app, such as `CaptureMicrophone` with a program engine, or `DeepgramFinal` for the final role.

## The programs that an app ships

| Program | Release file | npm |
|---|---|---|
| The engine | `sayso-engine-<version>-<macos-arm64\|macos-x86_64\|linux-x86_64\|linux-arm64\|windows-x64>` (`scripts/package-engine.sh`, `.ps1`) | `@justsayso/engine` and `@justsayso/engine-<platform>` |
| The cleanup runtime | `sayso-cleanup-<version>-<macos-arm64\|linux-x86_64\|linux-arm64\|windows-x64>` (`scripts/package-cleanup.sh`, `.ps1`). There is no Intel Mac file, because Sayso has no Intel app to take it from. | `@justsayso/cleanup` and `@justsayso/cleanup-<platform>` |
| The audio helper (TypeScript only) | none | `@justsayso/audio-<platform>`, MIT |

Each release file is a `.tar.gz` (Windows: `.zip`) with a `.sha256`, a README, the GPL `LICENSE`, and the licenses of its libraries. The cleanup file keeps the layout in which the runtime finds its llama.cpp libraries (`internal/llamalib`): on macOS `bin/sayso-cleanup` with `Frameworks/llama/`, elsewhere the libraries beside the runtime. The runtime downloads its libraries only when they are not there, which takes up to 10 minutes, so the release checks each file on a clean runner (`engine-check.yml`, `cleanup-check.yml`) before users get the release. The npm packages under `packaging/npm` are GPL-3.0-only and hold these files (`scripts/npm-packages`), and the TypeScript SDK reads their folders without running code of them. An Electron app unpacks them from the asar file.

## Tests

- **Programs for tests.** `cmd/sayso-testhost` is the Host API of the app on WAV files (a Unix socket, or a named pipe on Windows). `sdk/go/cmd/sayso-fake-engine` and `sayso-fake-cleanup` are fakes of the engine and the cleanup runtime, scripted with `SAYSO_FAKE_SCRIPT`. Each SDK takes them from `SAYSO_TESTHOST`, `SAYSO_FAKE_ENGINE`, and `SAYSO_FAKE_CLEANUP`, else builds them with Go. With `SAYSO_TEST_REQUIRE_FAKES=1`, as in CI, a missing program fails a test and never skips it.
- **Engines.** `sdk/go/conformance` (and the command `sayso-engine-check`) checks an engine against the engine protocol.
- **CI.** `sdk-boundary.yml` (the boundary rules, a module outside the repository that builds against `sdk/go`, and the licenses of the Go dependencies), `sdk-go.yml`, `sdk-typescript.yml`, `sdk-rust.yml` (each SDK on macOS, Linux, and Windows), and `sdk-integration.yml` (the three suites against the same test host and fakes on each system).
- **Not covered by CI.** A real engine and a real cleanup model in an SDK run (by hand: an SDK in Local mode with the release engine and a copy of a model store), the real microphone and system audio (they wait for a person), and a real call for the echo rule.

## Approval

### Identity

The host knows an app by its executable, (`internal/bridge/identity.go`, `identity_darwin.go`, `identity_linux.go`). Only the rule changes: the host compares the identity with the records of approved apps, not with the install folders.

| System | Identity |
|---|---|
| macOS, signed with a team id (Developer ID or Apple Development) | The team id and the signing identifier. The host checks them as today (`internal/bridge/identity_darwin.go`): the requirement `anchor apple generic`, the team id, and the identifier, against the audit token of the peer. |
| macOS, with a signature that is not valid | None. The host refuses the connection with `not_approved` and asks no question. It does not treat the program as a program without a signature. |
| macOS, ad hoc or not signed | The path of the executable. The question says that the app is not signed. |
| Linux | `/proc/<pid>/exe`. When the file was deleted or replaced after the start (the link ends in ` (deleted)`), the host refuses the connection. |
| Linux, an app in a Flatpak sandbox | The app id, from `/.flatpak-info` of the process and the checks of "Flatpak". The key in `apps.toml` is `flatpak = "<id>"`, so it cannot collide with a path. The question says that the app is not signed. |
| Windows | The image path of `QueryFullProcessImageNameW` for the pid of `GetNamedPipeClientProcessId`, made canonical with `GetFinalPathNameByHandleW`. Paths are compared without case. |

Notes:

- A path identity breaks when the app moves. Then Sayso asks again. A signed macOS app keeps its identity through updates.
- An ad hoc build of a developer gets a new signature with each build, which is why it uses the path.
- **Interpreters.** A script that runs in Python, Node, or another interpreter has the path of the interpreter. An approval of it approves every script of that interpreter. The question says this when the executable is a known interpreter. An app that needs its own identity ships an executable.
- **Flatpak.** The host never trusts a Flatpak app id that the client sends. It reads the id from the sandbox of the process (see "Flatpak"). A Flatpak identity stays the same through updates of the app.

### The question

`hello` gets two new fields: `app {name}` and `capabilities: [...]`, the capabilities that the app asks for. When the identity has no record, the host asks the user, and holds the reply of `hello` until the answer.

What the question shows:

- The name from the system (the bundle name on macOS, else the file name), and the identity (team and identifier, or the path).
- The name that the app gave, marked as "The app calls itself …". The host removes control characters and characters that change the direction of text or that show as nothing, and cuts the name to 40 characters. A path shows these characters as `�`, and the identity keeps the real path.
- Each capability in plain words ("Record from your microphone", "Record the sound of other apps and calls", "See each dictation after it is inserted").

The answers:

- **Allow** grants all listed capabilities. **Don't allow** refuses. Closing the question counts as **Don't allow**.
- The host remembers both answers in `<data dir>/apps.toml`. After **Don't allow**, `hello` gets `error {code: "not_approved"}` at once, with no question, until the user changes the answer in Settings.
- An approved app that asks for a new capability gets a question for the new capabilities only. Until the answer, the app has its old grants. A capability that the user refused is in `denied`, and the host does not ask for it again. The `hello` reply lists the granted and the denied capabilities.
- `models.read` needs no grant.

Limits, so an app cannot fill the screen with questions:

- One approval question is on the screen at a time, for all apps.
- Questions of the same identity count as one. At most four connections of one identity wait for it. A fifth gets `error {code: "busy"}` at once.
- At most four identities wait for an approval question, which includes the one on the screen. A fifth gets `error {code: "busy"}` at once. So one app with many connections does not block other apps.
- A connection that closes while it waits takes back its question. When the last one closes, the window closes, and the identity gets `busy` with no question for 30 seconds. So an app that connects and closes in a loop does not take the focus each time.
- **Allow** is inactive for 500 ms after the window shows, because the window takes the focus. A click that was meant for another window does not answer yes. The download question has the same delay. Nothing moves when the delay ends, so reduced motion changes nothing.

The system permissions stay separate. Sayso asks the system for the system audio permission the first time a live session in Sayso mode asks for the source `system`.

`apps.toml` replaces `extensions/state.toml`:

```toml
[[apps]]
name = "Notetaker"            # The name from the system, which Sayso shows.
team_id = "ABCDE12345"        # macOS, signed apps.
identifier = "app.justsayso.notetaker"
signer = "Example LLC"        # The organization on the certificate, which Sayso shows.
# path = "/opt/notetaker/bin/notetaker"   # Other identities.
allowed = true
granted = ["session.live", "audio.mic", "audio.system", "transcribe.file"]
denied = ["dictation.observe"]
first_seen = 2026-10-29T10:00:00Z
```

The host reads the file again when it changes, as today. A grant that the user takes back stops the next request.

### In Sayso

- **The approval question**, a small window like the download question.
- **Settings › Connected apps**: each app with its identity, its grants as switches, and **Remove**. An app that the user refused is in the list too, so the user can change the answer.
- **The recording state** of the menu bar icon (the tray icon on Windows and Linux) while a live session captures the microphone or the system audio. The popover names the app and has **Stop**.
- **The Models page** shows the **Voice detection** and **Speakers** sections when at least one app is approved.
- The quit question and the update wait exist. They name the app by its record in `apps.toml`.

## Model store

### Why a new store

The models folders before the store (0.13 and older) could not be shared:

- One model id did not own its files. In the Swift engine, Parakeet Unified and the CTC helper are separate folders that a delete of Parakeet Unified removes, and the Whisper variants share a tokenizer folder (`native/macos/SaysoEngine/Sources/SaysoEngine/ModelStore.swift`, `delete`). A lock for each model id could not protect a file that two ids use.
- A download changed files in place: the engines removed old files, renamed each archive into the model folder, and then wrote a marker.
- A load can write. FluidAudio deletes the files and downloads them again when a load fails (`ModelHub.loadModels`), unless `offlineMode` is on.
- The engines of older versions delete model folders without a lock. No new rule can make them wait.

### Layout

The store is `v1` in the models folder of Sayso (`internal/paths`): `~/Library/Application Support/Sayso/models/v1` on macOS, `%LOCALAPPDATA%\Sayso\models\v1` on Windows, and `$XDG_DATA_HOME/sayso/models/v1` on Linux (`~/.local/share/sayso/models/v1` when the variable is not set). `XDG_DATA_HOME` counts on each system, not only on Linux, because `internal/paths` reads it first. Each SDK has the same rules (`ModelStore` in Go, `modelStore()` in TypeScript, `model_store()` in Rust), because an MIT SDK cannot use the paths of the app, and `sdk/protocol/testdata/paths.json` holds the cases that the app and each SDK test. Local mode uses the store also when Sayso is not installed, so a later Sayso finds the models. The engines of older versions never open `v1`.

```
v1/
  artifacts/<digest>/                 files that never change
  models/<model id>/<format>.json     the manifest: descriptor revision and artifact digests
  locks/meta                          the lock of the metadata (see "Rules")
  locks/<digest>.use                  shared while loaded, exclusive to delete
  locks/<digest>.get                  exclusive while one engine downloads the artifact
  staging/<digest>.<pid>.<random>/    a download in progress: an `owner` file first, then `work/` and `files/`
  trash/<digest>.<random>/            artifacts that wait to be deleted
  migrating/<digest>/                 a stage of the migration, which the cleanup of an engine does not touch
  migration.json                      the journal of the migration
  cache/<key>/                        caches that the engines control, each with its own lock
```

- **Descriptor.** The catalog of Sayso, which each engine builds in, gets a descriptor for each model in each format: its artifacts, and for each artifact the source (a URL pinned to a revision: a Hugging Face commit, not `main`, or a release file), and each file with its path, size, and SHA-256. A script makes the descriptors: it downloads each source, hashes the files, and writes them into the catalog. A descriptor has a `revision` number that only goes up. The Go code is in `internal/models` (`descriptors.go`), and the Swift engine gets the descriptors of its formats in `Descriptors.generated.swift`.
- **Digest.** The SHA-256 of the canonical list of the files of an artifact: paths relative to the artifact, in `/` form, sorted by bytes, each with its size and hash. Each file gives one entry: the path, a NUL byte, the size in decimal, a NUL byte, the SHA-256 of the file in lowercase hex, and a line feed (`internal/models/descriptors.go`, with a fixed test vector). The source is not part of the digest, so the same bytes from two revisions are one artifact. The engine knows the digest from the descriptor before it downloads anything.
- **Archives.** For a source that is an archive (the Go engine extracts `.tar.bz2` files), the files are the extracted files after the rule of of the engine: one folder that holds everything is removed (`internal/localengine`). An archive with a link, a path with `..`, or an absolute path is refused. Markers and the metadata that a library writes (for example `.cache/huggingface`) are not files of the artifact.
- **Shared files**, such as a Whisper tokenizer or the CTC helper, are artifacts of their own. Two engines that read the same format use the same artifact.
- **Format.** Each engine kind names the format of its files (for example `fluidaudio-coreml-1`, `sherpa-onnx-1`). Manifests of two formats for one model id live side by side, so engines of different versions and different formats never replace each other's files.

### Rules

These rules are in the engine feature `store_v1`, in each engine (`internal/localengine/store.go`, and `ModelStore.swift` of the Swift engine).

**Locks.** `locks/meta` is the lock of the metadata. Whoever reads or changes manifests, publishes an artifact, or decides that an artifact is garbage holds it, and holds it only for those short steps, never during a download or a load. There is one order: `get` before `meta`, `meta` before `use`, and `use` locks in the order of the digests. Nobody waits for a lock while it holds a lock that comes later in the order. Lock files are never deleted. `flock` on macOS and Linux. On Windows, `LockFileEx` on byte 0 with a length of 1, on a handle opened with `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`. A process that stops lets its locks go.

**Load.**

1. Take `meta` shared. Read the manifest of the model in the format of the engine.
2. Take a shared `use` lock on each of its artifacts, in the order of the digests.
3. Check that each artifact folder exists. Let `meta` go.
4. Load. When an artifact is missing in step 3, the model is not on disk, and the engine reports `not_downloaded`. A load never writes in the store: a load that fails reports `failed`. FluidAudio loads through its local loaders only (`AsrModels.loadLocal`, `OfflineDiarizerModels.init`), because its `offlineMode` is one flag for the whole process and it also stops each download (finding 3 of "Write inventory").
5. Keep the `use` locks until the engine no longer holds any object that reads the files: after `unload`, and after the end of each running job and stream that uses the model.

**Download.**

1. Take `meta` shared and read the manifest. When it has the revision of the descriptor or a higher one, and each artifact exists, the model is on disk. Let `meta` go.
2. For each artifact of the descriptor that is missing, in the order of the digests: take its `get` lock exclusive (an engine that waits for it reports `downloading`). Look again: another engine may have published it. Make `staging/<digest>.<pid>.<random>/`, write the `owner` file into it first, download into its `files/`, check each file against the descriptor, and keep the `get` lock.
3. Take `meta` exclusive. Rename each staged `files/` to `artifacts/<digest>`. When an artifact of the descriptor is now missing (a cleanup took it, because no manifest named it after step 1), let the locks go and start again at step 1. Else write the manifest to a unique temporary file and rename it into place, unless the manifest that is there has a higher revision. Let `meta` go, then the `get` locks.

An artifact and the manifest that names it appear in one step under `meta`, so cleanup never sees a new artifact without its manifest. An engine with an older descriptor uses a manifest with a higher revision, because one format means that the engine can read it. A manifest with a lower revision is not on disk for the engine, and its next download replaces it.

**Delete a model** (the user, in Sayso or in an app): take `meta` exclusive and delete the manifest. This never fails because of another app. An app that has the model loaded keeps it until it unloads. At its next `list_models`, the model is not on disk.

**Clean up.** At the start of an engine and after each delete:

1. Take `meta` exclusive. Find the artifacts that no manifest names. When a manifest does not parse, it is not known what is garbage, and no artifact goes. Remove the temporary manifest files of a crash, and the folder of a model without manifests.
2. For each, try its `use` lock exclusive without waiting. When the engine gets it, it renames the artifact into `trash/` and lets the lock go. When it does not, another engine has it loaded, and it stays for the next cleanup.
3. Let `meta` go, then delete the folders in `trash/`.

An artifact that a crash left without a manifest is valid. A download that needs it finds it in step 2 and uses it, and cleanup deletes it when no manifest names it.

An engine also runs a cleanup after the last holder of a loaded model lets it go, so the artifacts of a model that another engine deleted go after the unload.

**Staging.** Cleanup also looks at `staging/`. For each folder, it takes the `get` lock of the digest in its name without waiting, keeps it while it deletes the folder, and then lets it go. A folder whose `get` lock another engine holds is a download in progress, and it stays.

### Migration and downgrade

Sayso moves its models from the old folders into `v1` once (`internal/models/migration.go`). Only one Sayso runs for each data folder (an instance lock on each system), and the migration runs before Sayso starts its engine, so no engine of Sayso writes the old folders while it runs. It runs on the thread that starts the engine, because the hash check takes seconds and the tray and the overlay must not wait for it. An engine that the user chose (the setting `engine_path` or `SAYSO_ENGINE_PATH`) can be older than the store, so Sayso does not migrate for it and does not give it the store. An app that points an engine without `store_v1` at the models folder of Sayso is not supported: two such engines must not use one folder.

1. Take `meta` exclusive for the whole migration. Write the plan into `migration.json`: each model, its descriptor, and its old files.
2. For each artifact of a model: make hard links of the old files in `migrating/<digest>/`, then hash the linked files and compare them with the descriptor. A hard link takes no space and leaves the old folder as it was. On a volume without hard links, copy the files instead.
3. When every artifact of a model matches, publish them and the manifest as in "Download", and mark the model as done in `migration.json`. When a file is missing, cannot be read, or does not match, the model is not migrated and is marked as kept, with the reason, and the user downloads it again. Its old files stay until then.
4. When every model is done, mark `migration.json` as removing, then delete the old folders of the models that moved, and mark `migration.json` as finished. The file stays with the kept models, so the next start does not hash them again. When a kept model is in the store, the next start deletes its old folders.

After a crash, the next start reads `migration.json` and does each step again. Each step can run twice with the same result. Models that were done stay done.

When the migration fails before it marks `migration.json` as removing, each old folder is still there, so the engine runs on them without the store for this start. The next start looks at the old folders again, also for a model that was kept, because the user can have downloaded a model in between. After the mark, the engine uses the store, also when the migration fails. A Parakeet Unified folder without its CTC helper is kept, and the user downloads it again. A kept model has no delete button, because it is not on disk for the engine: its old files go when the user downloads it.

After a downgrade, the older Sayso does not see `v1`, and the user downloads the models again. The release notes of the version with `store_v1` say this.

The engine of an older app in Local mode, without `store_v1`, uses its own folder (`SAYSO_MODELS_DIR`) as today. The SDK passes the store only to an engine that lists `store_v1`: it starts the engine without the variable, reads `hello`, and when the feature is there, it starts the engine again with the variable.

Core ML keeps its compiled models by path. So after the migration, the first load of each model on macOS optimizes it again, and takes as long as the first load after a download (9 to 22 s for Parakeet Unified and Nemotron on the Mac of the check).

### Caches outside the store

What is known about the places where the pinned libraries write outside the store:

- WhisperKit: the model and tokenizer folders go into the models folder today (`Engine.swift`). They move into artifacts.
- FluidAudio: Sayso gives the folders of its loaders. The default `~/Library/Application Support/FluidAudio` must not be used.
- A cache that an engine controls goes into `cache/<key>/` with a key that includes the format and the library version, and a lock for its writer.
- Caches that the system controls (the compiled Core ML models of macOS) and models that Apple manages (Apple Speech) are outside the store and outside the promise of one copy.

## System audio

The source `system` of a live session is the sound that the computer plays. The permission belongs to the process that captures: Sayso in Sayso mode, the app (or `sayso-audio`) in Local mode.

| System | Go (`sdk/go/capture/system`, which the host of the app uses too) | Rust (`sdk/rust`) |
|---|---|---|
| macOS 14.2 and later | Not yet: `unsupported`, and the host lists no `system_capture` in `hello`. The Core Audio process tap needs Objective-C blocks, which the Go code does not have. | A Core Audio process tap through a private aggregate device. Needs the system audio permission and `NSAudioCaptureUsageDescription`. On 14.0 and 14.1 it gives `unsupported`. The app links CoreAudio weakly, as the README of the crate says. |
| Windows | WASAPI loopback of the default output device | WASAPI loopback through `cpal` |
| Linux | The monitor of the default sink (`parec @DEFAULT_MONITOR@`), on PulseAudio or `pipewire-pulse` | The monitor of the default sink through the native protocol of PulseAudio |

TypeScript gets the system audio of the Go code through `sayso-audio`.

Rules for every capture:

- **What is left out.** `excludes` in the reply of `session_start` lists what the capture really leaves out: `sayso`, `app`, or both. The macOS tap of the Rust SDK leaves out the app. Windows and Linux leave out nothing (`excludes: []`): endpoint loopback and a monitor source record the mix of the device, and leaving out one stream would need another routing of the sound of the user.
- **Times.** Each frame has the time of one capture clock in the process, taken at the audio callback. The microphone and the system audio of one session use it, so the echo rule can compare them. The first audio of the session is time 0, and a source that starts later gets silence before its first audio.
- **Silence.** When no audio arrives for 200 ms and the stream is fine (Windows while nothing plays), the capture gives silent frames, so the times of the source go on. A hole of more than 10 s, or a jump of the wall clock against the capture clock of more than 2 s (a sleep), gives `source_gap`.
- **Device changes** give `source_gap {session, source, ms}` when the new stream is open, and the capture goes on with the new device.
- **Permissions.** A refused permission gives `error {code: "permission"}` at `session_start`. The system audio opens before the other sources, so a refusal opens no microphone and leaves no recording. A permission that the user takes back during a session ends that source with `source_ended {reason: "permission"}`, and the other sources go on.

**The echo rule** (`sdk/go/speech/live_echo.go`, and the same rule in each SDK and in the host): in a session with `mic` and `system`, a microphone segment is dropped when a system segment covers at least half of its time (with 1 s of slack) and at least 60% of its words. Words are compared in order (longest common subsequence), without case or punctuation, and a word of 4 letters or more can differ by one letter. A segment of 3 words or fewer must also be at least 50% of the system words of its time. A microphone segment waits until the system source has passed its time, at most 60 s. Partials are not filtered. The limit of 60% comes from `echo_pairs.tsv` (in `sdk/go/speech/testdata` and `internal/bridge/testdata`): 1170 synthetic pairs, where 60% drops 98.1% of the echoes and none of the real text. A real call must still confirm it.

## Flatpak

- **The socket.** On Linux an app looks in `$XDG_RUNTIME_DIR/sayso/host.sock`, also in a Flatpak. A sandboxed app sees that folder only with `--filesystem=xdg-run/sayso:ro` in its manifest. An app without the permission finds no socket, and it uses Local mode.
- **The identity of an app in a sandbox** (`internal/bridge/identity_linux.go`) is `flatpak:<app id>`, from `/.flatpak-info` of the process and the checks of the process (no capabilities, `NoNewPrivs`, a seccomp filter, and a deployed app and runtime). The host never trusts an id that the app sends.
- **A Sayso inside a Flatpak** cannot identify an app, because it sees no pids outside its sandbox. The Rust version had a bridge program on the host side for this. It went with the Rust code (`28830e18`), and the Go app has none, so apps reach a Sayso only outside a sandbox.
- **Two stores.** A Flatpak has its own `XDG_DATA_HOME`, so in Local mode a sandboxed app and the Sayso outside have two stores.
- **A sandboxed app cannot start Sayso**: Sayso mode with `start` gives an error, and Auto uses Local mode.

## Security

- **The check of the writer on macOS has a limit.** macOS gives the last owner of a socket, not the writer of each message, and a read of the client also makes it the owner. So an approved app that gives its socket to another process, and reads at the right moment, can have a line of that process accepted. Such an app can also relay the requests itself, so this gives it nothing new. Linux checks each message with `SCM_CREDENTIALS`. On Windows the host knows only the process that connected, with the same result for such an app.
- **A program without approval has limits.** The host and the bridge keep 64 connections at most, close a connection that sends no line in 5 seconds, and read 64 KiB at most before they know the writer. One connection keeps 16 waiting jobs with an open file at most, and gets `busy` after that.
- **Sayso does not say that an app is safe.** No person reviews connected apps. The approval question says what the app asks for and what the system knows about it.
- **An approved app is a native program with the rights of the user.** Approval controls only what Sayso gives it.
- **A path identity trusts the file at that path.** A program that replaces the file gets the grants. Malware that runs as the user can do that and more, which is outside what Sayso can protect against, .
- **The identity is of the process that writes, not of the process that connected.** A program can connect, give the socket to a child, and then start an approved program. So the host identifies the process that wrote the first line, and closes the connection when a later line comes from another process.
  - macOS: the host reads the audit token after each line. The token follows the last writer. It must have the pid and the pid version of the first line, and the version changes with an exec. A line from before an exec gets no identity, because its token no longer names a running program.
  - Linux: each read gives the pid of its writer through `SCM_CREDENTIALS`. `SO_PEERCRED` is not used, because it says only who connected. `/proc/<pid>/exe` must be the same device and inode as the file at its path, so a bind mount in a private mount namespace does not pass. The process must be in the PID namespace of the host, because a process with a capability in its own namespace can write another pid in its credentials. The one exception is a process in a Flatpak sandbox, which must have no capabilities ("Flatpak"). The Go app has no bridge for a Sayso inside a Flatpak ("Flatpak").
  - Windows: there is no exec, and the pipe keeps the pid of the process that connected. Only pid reuse applies: the process that connected ends, a child keeps the pipe, and an approved program gets the same pid before the first line. The code does not check for this.
- **Lines from before an exec stay open on Linux.** A process can write `hello` and requests, and start an approved program before the host reads them. The pid is the same, so the host runs them as the approved program, and a child that keeps the socket reads the replies. Nothing on Linux tells the host that an exec came after the write. On macOS, the token keeps only the last writer, so a line of a child counts as a line of its parent when the parent writes before the host reads it. For this, the parent has to write to the socket that it inherited.
- **Across a sandbox, the host opens only files that the app can open.** It opens the file in the view of the app, not with the rights of Sayso ("Flatpak"), and a Sayso in a sandbox does not serve apps. So an app in a sandbox cannot use Sayso to read a file of the user.
- **The name that an app gives is not checked.** The question shows the name from the system first, and the name of the app only as a quote.
- **An app cannot borrow the permissions of Sayso in local mode.** It captures in its own process, so the system asks the user for the app.
- **In Sayso mode, Sayso records for the app.** The menu bar shows it, and **Stop** ends it.
- **The store is shared by every program of the user.** A program can change files in it. The engines check the hash of each file at download, not at each load.
