# The Sayso SDK for TypeScript

Status: built on the branch `typescript-sdk` (2026-11-10), not released. Chris made decisions 1 to 6 on 2026-11-08. On 2026-11-10 he said to execute the plan, so the build uses decisions 7 to 11 as this file recommends; he can still change them. He answered the open questions the same day ("Answers to the open questions"). "Build status" says what works and what is left. The ADR is [0006](adr/0006-typescript-sdk-wraps-the-rust-sdk.md). The design of the SDK itself is [sdk.md](sdk.md), and this file changes nothing in it.

This file says how an app in TypeScript or JavaScript uses the Sayso SDK. The package `@justsayso/sdk` is a native Node addon around the crate `sayso-sdk`, so a TypeScript app gets the same modes, messages, and end rules as a Rust app, from the same code. The engines ship as their own npm packages.

## Goal

- An app in Node, Electron, Bun, or Deno uses everything that `sayso-sdk` has: `Speech` with `Mode::Sayso`, `Mode::Local`, and `Mode::Auto`, the embedded API, the low-level client, Deepgram, the cleanup models, and the helpers.
- The rules live once, in Rust. The TypeScript layer adds no rule of its own, except what JavaScript needs: promises, iterators, and the lifetime of objects.
- The types of the messages come from the Rust structs, so they cannot drift.
- `npm install @justsayso/sdk @justsayso/engine` is all that an app needs for local mode on the five platforms of the engine.
- An app that uses only Sayso mode installs no engine.

## Decisions

Chris made these on 2026-11-08.

1. **One native addon around `sayso-sdk`.** napi-rs, no second implementation in TypeScript. See ADR 0006.
2. **No WASM.** Not for the engine and not for the SDK (ADR 0006, "Considered options").
3. **The engines are their own npm packages.** One for each platform, GPL-3.0-only, with the files of the engine release file. `@justsayso/sdk` does not depend on them.
4. **The name is `@justsayso/sdk`**, in the npm scope `@justsayso`.
5. **The first version is complete.** It has all of the public API of `sayso-sdk` ("API").
6. **In this repository.** The packages have the version of the crates and the engine.

The build uses these. Each was the recommendation of this plan.

7. **Messages keep their names from the wire.** A message of the protocol is the same object in TypeScript as in JSON: `{ type: "job_done", job, text }`, with `snake_case` fields. The options and the methods of the SDK use `camelCase`, as JavaScript does. So the reference of the Host API ([sdk-guide/reference.md](sdk-guide/reference.md)) is also the reference of the TypeScript messages, and no layer converts names.
8. **The runtimes.** Node 22, 24, and 26; Electron in the main process, in the versions that Electron supports; Bun and Deno. CI tests each one ("Tests").
9. **The platforms.** The five of the engine: macOS arm64 and x64, Linux x64 and arm64 with glibc, Windows x64. No musl, no Windows ARM.
10. **Calls run on threads of the addon.** A call that waits (`connect`, `stopSession`, `downloadModel`, a request to Sayso) runs on a thread of its own, not on the libuv thread pool. `Speech.connect` can wait minutes for the approval question, and four such waits would stop all file I/O of Node, because the pool has four threads.
11. **The local cleanup models are in the first version**, with the runtime `sayso-cleanup` as npm packages too. The runtime has no release file today, so this needs one (step T8).

## Build status

What exists on 2026-11-10, and how it was checked on Chris's Mac (macOS 27, Apple silicon):

| Part | Where | Check |
|---|---|---|
| The addon | `crates/sayso-node` | `cargo clippy --all-targets -D warnings`; `tests/values.rs` (7 tests: the 2^53 limit, outcomes, events, options). |
| `Speech`, `Engine`, `SaysoError`, sources in JavaScript | `packages/sdk/src` | `tests/local.test.mjs` (12 tests with the fake engine), `tests/lifetime.test.mjs` (4 tests in child processes). The real 0.15.0 engine transcribed `jfk.wav` in 848 ms. |
| `EmbeddedSpeech`, `DeepgramFinal`, a final pass in JavaScript, `Client`, `Deepgram` | `packages/sdk/src` | The tests of Sayso mode, `Client`, `EmbeddedSpeech`, and Deepgram (a fake host and a fake Deepgram in JavaScript). |
| `cleanup`, `RuntimeCleaner` | `packages/sdk/src/cleanup.ts` | `tests/cleanup.test.mjs` (4 tests). With the development runtime and the S1-mini of Sayso, one real cleanup took 1025 ms. |
| The generated types | `packages/sdk/src/protocol` | `cargo test -p sayso-protocol --features ts`, `scripts/check-protocol-ts.sh`, `pnpm --filter @justsayso/sdk test:types`. |
| `@justsayso/engine` and the engine packages | `packages/engine`, `scripts/npm-packages.py` | `packages/engine/test`; the packages of the 0.15.0 release file, packed and installed. |
| The addon packages | `packages/sdk/npm/<platform>` | Packed and installed with npm. |
| Runtimes | | `tests/smoke.mjs` passes in Node 24, Bun 1.3.14, and Deno 2.9.7. Electron 45.2.3 transcribed `jfk.wav` in development and packaged with asar (`Engine.fromPackage()`). |
| The cleanup runtime | `scripts/package-cleanup.sh`, `.ps1`, `packages/cleanup`, `release.yml` | The macOS arm64 release file, packed and installed: `cleanup.runtimeFromPackage()` found it, and S1-mini cleaned a text. The release steps, Linux, Windows, and the Intel build in CI are not run yet. |
| The guide | `docs/sdk-guide/ts-*.md`, `packages/sdk/examples` | `tsc -p examples`; `scripts/check-sdk-guide.py` checks the `ts` blocks against the examples. The tutorial was not run from `npm init`, because the packages are not on npm. |
| The Mach-O check | `scripts/check-macho.py` | `scripts/test_check_macho.py`; the spike file `raw.node` fails, and the same file with SDK 26.5 passes. |
| The start error of the engine | `crates/sayso-engine-sdk`, `crates/sayso-sdk` | The error has the last lines of the stderr of the engine (a test with a fake program that exits with 127). |
| CI | `.github/workflows/npm.yml` | Not run yet. It builds and tests the five addons, runs the runtimes, and publishes with trusted publishing. |

Not done:

- Not run anywhere: Windows, Linux x64, Intel macOS, yarn, Node 22 and 26, and Sayso mode against the real Sayso (the approval question).

### Changes from the plan

- **Sources in JavaScript.** `open(writer, info)` gets the writer, and can return `{ excludes }`. A source refuses with a `SaysoError` with the code `permission` or `unsupported`. `writer.onClose(listener)` says when the session no longer needs the audio.
- **The event loop.** A timer of the TypeScript layer holds the loop while work is active, not `ref` and `unref` of the thread-safe function. The thread-safe functions are weak. A promise of the addon holds the loop until it settles.
- **Values.** Requests, replies, and events cross as JSON text, and each promise gives `{"ok": value}` or `{"err": ErrorReply}`. napi turns a `u64` above 2^32 into a `BigInt`, and a JavaScript number above 2^32 into a float that serde refuses for a `u64`, so the addon does not use the serde conversion of napi.
- **`close()`.** `sayso-sdk` has a new `Speech::close(&self)`, so JavaScript can close a `Speech` while calls still run. After `close()` no events come, also no end of the work that ran, as after a drop in Rust. Calls that wait get `connection_lost`.
- **`Speech.transcribe`.** A helper that starts a job and gives `{ job, text, segments, speakers }` at its end. It adds no rule: it reads the events.
- **The final pass in JavaScript** routes the models of its `models` list to the function, and the others to the engine, as `DeepgramFinal` does for `deepgram-nova-3`.
- **Development builds.** `packages/sdk/scripts/build-addon.mjs` writes `sayso.<platform>.node` next to the package, and the loader takes it before the platform package (or the file in `SAYSO_NODE_ADDON`). The script deletes the old file first: macOS kills a process that loads a signed file that changed in place.
- **The workspace.** `pnpm-workspace.yaml` ignores the platform packages, which are not on npm during development, and links `@justsayso/engine` from the repository.

## What the tests showed

A spike on 2026-11-08 tested the design on Chris's Mac (macOS 27, Apple silicon). The spike is in `/tmp/sayso-node-spike`: napi-rs 3.16, `sayso-sdk` by path, with no change to the crate.

| Test | Result |
|---|---|
| Node 24 loads the addon, `Mode::Local`, `list_models` | 27 models from the Swift engine of `build/Sayso.app`. |
| A file job from Node, with events through a thread-safe function | `jfk.wav` with `parakeet-unified-en` in 674 ms: `job_progress`, three `segment`, `job_done` with the right text. The JavaScript thread ran its timers during the job. |
| The engine release file as npm packages, from a local registry (Verdaccio), with npm, pnpm, and bun | Each installer took only the arm64 package and skipped the x64 package (`os` and `cpu`). The program kept its mode `rwxr-xr-x` and its Developer ID signature (`codesign --verify --strict` passes). The SDK found the engine through an optional peer dependency, also with the strict layout of pnpm. |
| The same packages in Bun 1.3.14 and Deno 2.9.7 | Both transcribed `jfk.wav` (1313 ms and 733 ms). |
| Electron 45.2.3 (Node 24.22.2), main process, development | Transcribed `jfk.wav`. |
| Electron, packaged with `@electron/packager` and asar | The addon loads from `app.asar.unpacked` by itself. The engine does not: its path pointed into `app.asar`, and the SDK said "there is no engine at …/app.asar/…". With the path changed to `app.asar.unpacked`, the packaged app transcribed `jfk.wav`. |
| The weak link of CoreAudio, set in the `build.rs` of the addon | Without it, CoreAudio is `LC_LOAD_DYLIB`, and the addon would not load on macOS 14.0 and 14.1. With `cargo:rustc-cdylib-link-arg=-Wl,-weak_framework,CoreAudio`, it is `LC_LOAD_WEAK_DYLIB`, and `_AudioHardwareCreateProcessTap` is a weak import. The app needs no `build.rs`. |
| The first start of the signed 0.15.0 engine | 33.8 s for the first job, then 0.75 to 1.3 s. It was the first run of that engine on this Mac. The cause is not known. Step T6 measures it again. |
| Linux arm64 in Docker (`rust:1-bookworm`, glibc 2.36, Node 24.11), with the engine release file `linux-arm64` of 0.15.0 | The addon built (1 min 19 s) and loaded. `listModels` gave 14 models. `downloadModel` downloaded `parakeet-tdt-ctc-110m` (104 MB), and the file job gave the right text with word times in 1339 ms. The first two tries failed with HTTP 504 from GitHub, where the sherpa-onnx models are; a later try passed. |
| Types from `sayso-protocol` with ts-rs 12.0.1 and with schemars 1.2.3 | Both pass `tsc --strict` against 31 values that `encode` wrote. ts-rs needs fewer steps ("Types: the test"). |
| The identity of a script for Sayso | Official Node (`Developer ID Application: Node.js Foundation`, team `HX7739G8FX`) and Bun (team `7FRXF46ZSN`) are signed, so the host sees `signed:HX7739G8FX/node` and `signed:7FRXF46ZSN/bun`, with the interpreter flag. Deno from Homebrew, the Electron of `node_modules`, and an app from `@electron/packager` are ad hoc, so the host sees their path. |

### The load error of macOS 27

The first build did not load: dyld said `mis-aligned LINKEDIT string pool, fileOffset=0x003938E4`. The tests found the cause:

- The offset is `stroff` of `LC_SYMTAB`, which is `indirectsymoff + 4 × nindirectsyms`. The linker of Xcode 27.0 (ld-27031) and 27.2 (ld-27106.1) does not pad it to 8 bytes. So when the count of indirect symbols is odd, the string table starts 4 bytes off.
- The dyld of macOS 27 refuses this only for a binary built with SDK 27 or newer. The same file loads after `vtool -set-build-version macos 11.0 26.5 -replace` and an ad hoc signature. `@swc/core-darwin-arm64` 1.16.22 (SDK 26.5, ld 1267) has the same offset and loads.
- `strip`, `rust-objcopy`, `-ld_classic`, and `-C strip=symbols` do not change the offset. rust-lld does not run on this Mac.
- Whether a build is odd is chance. Of the bisect builds, `ureq` alone was odd, and `ring` and `webpki-roots` alone were even.

CI builds with Xcode 26 on `macos-26`, so the addons of CI load on macOS 27 today. The binaries of Sayso have the same risk: on 2026-11-08 `Sayso`, `SaysoEngine`, and `sayso-engine` had even counts. When CI moves to Xcode 27, every Mach-O can break after any change. Step T2 adds a check to CI for all Mach-O files of a release, not only the addon.

## Packages

| Package | License | What it has |
|---|---|---|
| `@justsayso/sdk` | MIT | The TypeScript layer, the generated types, and the loader of the addon. |
| `@justsayso/sdk-<platform>` | MIT | `sayso.<platform>.node`, the addon of one platform. An optional dependency of `@justsayso/sdk`. |
| `@justsayso/engine` | GPL-3.0-only | `releaseFolder()`, which finds the engine package of this platform. An optional peer dependency of `@justsayso/sdk`. |
| `@justsayso/engine-<platform>` | GPL-3.0-only | The contents of `sayso-engine-<version>-<os>-<arch>`: the program, `LICENSE`, and `licenses/`. The `README.md` of the release file becomes `ENGINE.md`, because the package has its own `README.md`. |
| `@justsayso/cleanup` and `@justsayso/cleanup-<platform>` | GPL-3.0-only | The runtime `sayso-cleanup` (decision 11). On x64, the builds of each CPU level (`sayso-cleanup`, `-avx`, `-baseline`), so `cpu::best_in` can choose. |

The platforms:

| Platform name | `os`, `cpu` (and `libc`) | Rust target | Engine release file | Built on |
|---|---|---|---|---|
| `darwin-arm64` | `darwin`, `arm64` | `aarch64-apple-darwin` | `macos-arm64` | `macos-26` |
| `darwin-x64` | `darwin`, `x64` | `x86_64-apple-darwin` | `macos-x86_64` | `macos-26`, tested on `macos-15-intel` |
| `linux-x64-gnu` | `linux`, `x64`, `glibc` | `x86_64-unknown-linux-gnu` | `linux-x86_64` | `ubuntu-24.04` |
| `linux-arm64-gnu` | `linux`, `arm64`, `glibc` | `aarch64-unknown-linux-gnu` | `linux-arm64` | `ubuntu-24.04-arm` |
| `win32-x64-msvc` | `win32`, `x64` | `x86_64-pc-windows-msvc` | `windows-x64` | `windows-2025` |

The sizes: an addon is about 4.3 MB on macOS arm64. The engine release files of 0.15.0 are 3.2 MB (macOS arm64), 3.5 MB (macOS x64), 16 MB (Windows), 25 MB (Linux arm64), and 26 MB (Linux x64), compressed. The models are larger, and they go to the model store in every case.

## In the repository

```text
crates/sayso-node/            The addon (Rust, MIT, publish = false). A member of the workspace
                              through crates/*, so clippy and hakari include it.
packages/sdk/                 @justsayso/sdk: src/*.ts, the generated types, examples/, tests/.
packages/sdk/npm/<platform>/  The package.json of each addon package. CI puts the .node file in.
packages/engine/              @justsayso/engine, and npm/<platform>/ for the engine packages.
packages/cleanup/             @justsayso/cleanup, and npm/<platform>/.
scripts/check-macho.py        The checks of "macOS" for each Mach-O file.
scripts/npm-packages.py       Makes the engine and cleanup packages from the release files.
```

The root gets a `package.json` and a `pnpm-workspace.yaml` for `packages/*`. The JavaScript tools are TypeScript, `tsc`, and the test runner of Node (`node:test`), with no bundler.

## The addon

### Threads

- Each call that can wait runs on a thread of the addon and gives a promise (decision 10). napi-rs `AsyncTask` uses the libuv pool, so the addon does not use it for these calls.
- `sessionAudio` does not wait (`Speech::session_audio` only sends), so it is a plain call.
- One thread reads `Speech::events` and gives each event to JavaScript through a thread-safe function.
- A call into JavaScript from a thread of Rust (an audio source or a final pass in JavaScript, "API") waits for the promise of JavaScript. The JavaScript thread never waits for Rust, so this cannot deadlock.

### Events

- The SDK gives each event to every listener. `speech.events()` gives an async iterator that starts at the moment of the call, and `speech.on(type, listener)` calls a function. In Rust, two receivers of one channel take turns, so two readers would each miss events. JavaScript apps expect each listener to get all events.
- Events wait in JavaScript until something reads them, as they wait in the channel in Rust.
- The thread-safe function holds the event loop of Node only while work is active: a request without a reply, a job before its end, or a live session before its end. These are the end rules of `sdk.md`, so the layer knows them. Without work, the process can end. With work, Node does not end before the last event.

### Lifetime

- `speech.close()` ends the work and closes the connection or stops the engine, as the drop of `Speech` does in Rust. `Speech` has `[Symbol.asyncDispose]`, so `await using speech = await Speech.connect(...)` closes it.
- A `Speech` that the garbage collector takes without `close()` closes in the finalizer of the addon. The time of that is not known, so the guide tells apps to call `close()`.
- `close()` ends the thread of the events, so a closed `Speech` keeps no thread.

### Errors

- An `ErrorReply` of Rust becomes a `SaysoError extends Error` with `code`, `message`, `capability`, and `reason`. `code` is a string, the same as on the wire (`host_not_running`, `not_approved`, ...).
- An error of the embedded API (`embedded::Error`) is a `SaysoError` with the code of the engine or the session, or `io`.
- An error of the addon itself (a bad argument) is an `Error` with the `code` `InvalidArg` of napi, not a `SaysoError`. A number above 2^53 is a bad argument too.

### Values

- Ids and sizes are `u64` in Rust. They become `number`. The ids come from counters of the SDK, and a size of a model is far below 2^53. The addon checks each value and fails with a `RangeError` at 2^53, so a value never changes in silence.
- Audio for `sessionAudio` is an `Int16Array` (16 kHz mono), with no copy. The embedded API and the audio sources take a `Float32Array`, as in Rust.
- Paths are strings. The addon gives them to Rust as `PathBuf`.

## API

A sketch. The names change in the build, but each part of `sayso-sdk` has a place.

```ts
import { Speech, Engine, SaysoError } from "@justsayso/sdk";

await using speech = await Speech.connect({
	mode: "auto", // "local" | "auto" | { sayso: { start: boolean } }
	app: { name: "Notetaker" },
	capabilities: ["session.live", "audio.mic", "audio.system", "transcribe.file"],
	engine: Engine.fromPackage(), // or Engine.fromRelease(folder), Engine.fromProgram(path); .store(path), .modelsDir(path)
	models: { finalModel: "parakeet-unified-en", language: "en" },
	// socket, timing, capture: optional, as in Options of Rust
});

speech.mode(); // "sayso" | "local"
speech.features(); // Features, with camelCase fields
const job = await speech.transcribeFile({ path: "/tmp/talk.wav", diarize: true });
for await (const event of speech.events()) {
	if (event.kind === "message" && event.message.type === "job_done" && event.message.job === job) {
		console.log(event.message.text);
		break;
	}
}
const { session, excludes } = await speech.startSession({ sources: ["mic"], record_to: folder });
const text = await speech.stopSession(session);
```

The requests are the message types of the protocol (decision 7), so `transcribeFile` takes a `TranscribeFile` and `startSession` a `SessionStart`, with the same fields as in Rust.

What the package has, by the module of `sayso-sdk`:

| `sayso-sdk` | TypeScript |
|---|---|
| `Speech`, `Options`, `Mode`, `ActiveMode`, `Engine`, `ModelChoices`, `Timing`, `Features`, `Event` | `Speech` with every method of Rust (`connect`, `mode`, `features`, `events`, `listModels`, `downloadModel`, `transcribeFile`, `cancelJob`, `startSession`, `sessionAudio`, `stopSession`, `enhance`, `subscribe`, `reconnect`), and `close`. `Event` is `{ kind: "message", message } \| { kind: "mode_changed", mode, features } \| { kind: "error", error } \| { kind: "disconnected", code }`. |
| `CaptureSources`, `capture::{Microphone, SystemAudio, AudioFeed, ...}` | `capture: { microphone?, systemAudio? }` in the options. A source in JavaScript is an object with `open()`, which gives a writer with `push(samples: Float32Array, captureTimeMs?)`, `gap(ms)`, and `end(reason)`. The addon gives it to `sayso-speech` as a `Microphone` or a `SystemAudio`. |
| `embedded` | `EmbeddedSpeech` with `connect`, `mode`, `models`, `download`, `delete`, `load`, `unload`, `events`, `startSession`, `sessionAudio`, `stopSession`, and `cancelSession`. `EngineSource::Program`, a JavaScript microphone, `CaptureMicrophone` with `setDevice`, `setLevel`, `openEarly`, and `closeEarly`, and `devices()`. |
| `connect_with` and its layer | `finalPass` in the options of `EmbeddedSpeech`: `new DeepgramFinal(key)`, or a function `(samples: Float32Array, model, language) => Promise<string>`. The addon puts a `SpeechEngine` around the engine that calls it for the final pass. `EngineSource::Custom` with an engine in JavaScript is left out ("Answers to the open questions", 2). |
| `deepgram` | `Deepgram` with `checkKey` and `transcribe`, `DeepgramFinal`, and the constants `MODEL`, `CODE_CONNECTION`, and `CODE_KEY`. |
| `client` | `Client` with `connect({ path, app, start, waitMs, startSayso })`, which covers `connect`, `connect_or_start`, `connect_to`, and `connect_as` of Rust, and `hello`, `events`, `on`, `request(kind, fields)`, `send`, `reconnect`, and `close`. Also `startSayso()`, `launchers()`, `appId()`, `sdkName()`, and `startWaitMs()`. |
| `cleanup` | `cleanup.catalog()`, `cleanup.find(id)`, `cleanup.download(modelsDir, model, { endpoint, signal, onProgress })` (an `AbortSignal` for the `cancel` flag of Rust), `cleanup.isReady`, `cleanup.modelPath`, and `RuntimeCleaner` with the runtime of `@justsayso/cleanup` (decision 11). |
| `find_engine`, `model_store`, `permission` | `findEngine(folder)`, `modelStore()`, and `microphonePermission` with `status()`, `request()` (a promise), and `settingsUrl()`. |
| `protocol` | The generated types ("Types"), and `protocolVersion`. |

`Engine.fromPackage()` finds the engine with `releaseFolder()` of `@justsayso/engine`. In a packaged Electron app it changes `app.asar` in the path to `app.asar.unpacked`, because a program inside an asar file cannot start (see "What the tests showed"). Without `@justsayso/engine`, it throws a `SaysoError` with the code `unsupported` and a message that says to install the package. The rest is as in Rust: with no `engine` option, `"local"` and `"auto"` fail at `connect` with `bad_request`.

## Types

The types of the messages come from `sayso-protocol`, with ts-rs, behind a feature `ts` of the crate. So a Rust app that uses `sayso-protocol` does not build ts-rs.

- `cargo test -p sayso-protocol --features ts` writes the files, and `scripts/gen-protocol-ts.py` writes the index files. They are in the repository (`packages/sdk/src/protocol/`). `scripts/check-protocol-ts.sh` writes them again and fails when they differ. It runs in the manual Linux test job of `ci.yml`, and `npm.yml` checks the types against the values of Rust (`test:types`).
- A field with `skip_serializing_if = "Option::is_none"` is optional (`field?: T`), not `T | null`. This needs `optional_fields` on each type with such a field. Every `Option` field of the crate has `skip_serializing_if`, so the rule is true for all of them.
- A field with `#[serde(default)]` is required in the output of ts-rs. Of the requests, only `Hello.features` has one, and the TypeScript layer never sends `Hello` (the addon does), so no request type needs a change.
- The name types of `names.rs` (`Capability`, `ErrorCode`, and the others) become a union of the known names and `(string & {})`. A newer Sayso can send a name that this version does not know, and the Rust types accept it too. ts-rs takes only a plain literal in `ts(type = ...)`, so the `names!` macro implements `ts_rs::TS` itself and builds the union from `ALL`. It cannot drift from the Rust names, and `names::ts_tests` checks the seven types.
- `u64` becomes `number` ("Values"). ts-rs gives `bigint` by default, which is wrong for `JSON.parse`, so `.cargo/config.toml` sets `TS_RS_LARGE_INT = "number"`.
- The types of the engine protocol (`engine.rs`) have the same names as some types of the Host API, so they go to `engine/` and the index exports them as `engine`.
- The envelope of a line (`v`, `id`) and the list of the bodies of `Reply::Ok` stay hand-written in the TypeScript package, because `Message<T>` is not `Serialize`.
- A test serializes real values in Rust (each `Event`, each request with its defaults) and checks them with `tsc` against the generated types. Cases that must fail fail: a missing field, an unknown `type`, `model: null`, and `5n` for a `u64`.

### Types: the test

A test on 2026-11-08 compared ts-rs 12.0.1 with schemars 1.2.3 and json-schema-to-typescript 16.0.0, on a copy of `sayso-protocol`. The work is in `/tmp/protocol-ts`.

| | ts-rs | schemars and json2ts |
|---|---|---|
| Lines added to the crate | 68 derives (`event.rs` 7, `request.rs` 12, `reply.rs` 10, `types.rs` 5, `engine.rs` 34), 2 `ts(optional)`, 1 in the `names!` macro | 68 derives, 6 renames of engine types, a `JsonSchema` impl in `names!` |
| Other steps | `TS_RS_LARGE_INT` and `TS_RS_EXPORT_DIR` in `.cargo/config.toml` | A generator of about 70 lines, a rewrite of tagged newtype variants to `allOf` (else json2ts loses their fields), and a step that removes `null` from each `Option` |
| `Option` fields | `field?: T` with `optional_fields` | `field?: T` after the `null` step |
| `#[serde(default)]` fields | Required | Optional in the schema for input, required in the schema for output |
| `u64` | `number` with `TS_RS_LARGE_INT` | `number` |
| The check against 31 values of `encode` | Passes | Passes |

The choice is ts-rs, because it is one `cargo test`, with no JSON Schema step and no rewrites that depend on the internals of json2ts. schemars would be the choice for checks at run time with ajv, which the SDK does not need: the messages come from Rust, which already checks them.

## macOS

- **The weak link of CoreAudio** is in the `build.rs` of `sayso-node`, so apps need nothing. `scripts/check-macho.py` fails when an addon links CoreAudio with `LC_LOAD_DYLIB`.
- **The string table.** `scripts/check-macho.py` fails for a Mach-O file with SDK 27 or newer and `stroff % 8 != 0`. CI runs it for the addons, the engines, and the app.
- **Permissions in local mode.** The system asks for the process that is responsible for the app. For an Electron app, that is the app, which needs `NSMicrophoneUsageDescription` and `NSAudioCaptureUsageDescription` in its `Info.plist`, and the entitlement `com.apple.security.device.audio-input` with the hardened runtime. For a script in a terminal, it is the terminal app. `microphonePermission.request()` gives false in a process without `NSMicrophoneUsageDescription` (`sayso_capture::permission`), so a script cannot ask before the capture. The guide says this.
- **Signing.** npm keeps the signature of the engine. A tool that signs an Electron app (`@electron/osx-sign`) signs each program in the bundle again with the identity of the app, the engine too. That is correct: the engine records nothing, and it runs as a child of the app. The guide shows the `asarUnpack` rule for the addon and the engine.

## Linux

- The newest glibc symbol of the addon of the test is `GLIBC_2.34`. The program of the engine 0.15.0 has the same newest symbol, but its README asks for glibc 2.35. So the addon sets no new floor, and the guide gives the floor of the engine README. The packages set `libc: ["glibc"]`, so an installer on musl skips them.
- At run time the addon needs `libasound.so.2` (cpal), `libgcc_s.so.1`, `libm`, and `libc`. It is 5.4 MB on arm64.
- The engine needs `libvulkan.so.1`, also for a model on the CPU (its README says this). Without it, the SDK says only `the engine of this app did not start: the engine has ended`. Step T6 changes `sayso-sdk` so that this error has the last lines that the engine wrote to stderr (here, `error while loading shared libraries: libvulkan.so.1`). Rust apps get the better message too. The guide lists `libasound2` and `libvulkan1`.
- The sherpa-onnx models download from GitHub releases. The test saw HTTP 504 from GitHub for two models at the same time, so a download error must say which host failed. The message of the engine already names the URL.
- A path in a request must be absolute. The SDK returns `bad_request` with `path must be absolute`, as in Rust. The guide shows `path.resolve` in each example.

## Windows

- Sayso mode uses the named pipe of `sayso-protocol`, from the addon as in Rust.
- The engine package has the contents of the zip file. Nothing here was tested on Windows. Step T2 runs the first tests on `windows-2025`.

## Approval and identity

The host knows a process by its executable (`sdk.md`, "Identity"), so a TypeScript app gets the identity of the program that runs it:

| How the app runs | Identity on macOS | What an approval covers |
|---|---|---|
| A packaged Electron app, signed by its author | The team and the identifier of the app | This app only, also after updates. |
| A packaged Electron app, not signed (ad hoc) | The path of its executable | This app, at this path. |
| `node app.js` with official Node | `signed:HX7739G8FX/node`, the interpreter flag | Every script of every official Node of this user, also after Node updates. |
| `bun app.ts` | `signed:7FRXF46ZSN/bun`, the interpreter flag | The same for Bun. |
| `deno run app.ts` from Homebrew, or Node from a build that is not signed | The path of the program, the interpreter flag | Every script of that program, at that path. |

The question already says when the program is an interpreter (`identity::is_interpreter`). The guide tells authors to ship an app with its own executable (Electron, or a single executable application of Node or Bun) when the app runs for other users. A signed interpreter is wider than a path: one **Allow** covers every Node version. That is not new (`sdk.md` has the rule), but it is the common case for a JavaScript developer. Chris kept this rule ("Answers to the open questions", 1).

## Tests

- **Rust.** `crates/sayso-node` has unit tests for the conversion of values (the 2^53 limit, the errors, the events).
- **JavaScript, with fakes.** `packages/sdk/tests/` runs with `node --test`. Local mode uses `sayso-fake-engine`. Sayso mode uses a real `sayso_host::Host` in a child process with a temporary socket and data folder, as `crates/sayso-host/tests/sdk_modes.rs` does: a small test program, `publish = false`, GPL. The tests cover the end rules in both modes: a file job, a live session with the source `client`, a refused download, a job that waits in the queue, a feature that one mode does not have, a grant that the user takes back, `Mode::Auto` changes in both directions and never during work, a killed host during a job (`connection_lost`, no request twice), and `close()` during work.
- **JavaScript only.** Two listeners get the same events. A process with no work ends. A process with a job ends after its last event. `close()` and `await using` stop the thread of the events. Four calls to `Speech.connect` that wait do not stop `fs.readFile`. A JavaScript audio source and a JavaScript final pass give the same segments as the built-in ones for the same audio. A value at 2^53 fails.
- **The real engine.** On each runner, `jfk.wav` with the smallest model of the engine, through the installed packages.
- **The packages.** CI publishes all packages to Verdaccio and installs them with npm, pnpm, yarn, and bun on each platform. Each installs only its platform, the engine program can run, and on macOS `codesign --verify --strict` passes.
- **The runtimes.** Node 22, 24, and 26, Bun, and Deno run the tests with the real engine on macOS arm64 and Linux x64. Electron runs a packaged app with asar on macOS and Linux (with `xvfb` on Linux) and transcribes `jfk.wav`.
- **The load checks.** `scripts/check-macho.py` on all Mach-O files. On Chris's Mac (macOS 27): the addon from CI loads. A test of the script with the spike file `raw.node` (odd count, SDK 27.2) fails, and the same file with SDK 26.5 passes.

Not tested by the spike: Sayso mode from Node and the approval question, `Mode::Auto` changes, live sessions and the microphone, the system audio, Linux x64, Windows, and yarn. The spike did not open Sayso mode, because the question of the running Sayso would save an answer for the official Node of this user.

## CI and publication

- A job for each platform builds the addon with `napi build --release --platform`, runs `scripts/check-macho.py` on macOS, and uploads the `.node` file. The two macOS builds run on `macos-26`, which has Xcode 26.
- The release workflow makes the engine and cleanup packages from its release files with `scripts/npm-packages.py`, so the npm files are the release files.
- All packages of a release get the version of the crates. `@justsayso/sdk` names its addon packages and its optional peer `@justsayso/engine` at that exact version.
- Publication uses the trusted publishing of npm (OIDC from GitHub Actions) with provenance, so the repository keeps no npm token. Chris first makes the scope `@justsayso` on npmjs.com and sets up trusted publishing for each package. Check the rules of npm at that time: a package can need a first publication before trusted publishing.

## Docs

- The guide gets TypeScript pages next to the Rust pages: the tutorial, each how-to, and a page for Electron (asar, `Info.plist`, entitlements, signing).
- `scripts/check-sdk-guide.py` also checks each `ts` block against `packages/sdk/examples/`, which `tsc` compiles in CI.
- `CONTEXT.md` gets the terms of this file with the code, as `sdk.md` did.

## Order of work

Each step ends with a check that proves it.

- **T1. Decisions.** This file and ADR 0006. Done on 2026-11-10.
- **T2. Build and load.** `crates/sayso-node` with `Speech.connect` in local mode and `listModels`, the loader of `@justsayso/sdk`, the CI jobs for the five platforms, and `scripts/check-macho.py`. Check: each runner loads the addon that it built. The x64 macOS addon loads on `macos-15-intel`. Chris's Mac loads the arm64 addon from CI. The script fails for `raw.node`.
- **T3. Types.** The generated types and their CI check. Check: the values from Rust pass `tsc`, and a changed field in `sayso-protocol` fails CI until the types are generated again.
- **T4. `Speech`.** All methods, events, the lifetime, the errors, and the threads. Check: the tests of "JavaScript, with fakes" and "JavaScript only" pass on the five platforms.
- **T5. The rest of the API.** `EmbeddedSpeech`, `Client`, Deepgram, the audio sources and the final pass in JavaScript, and the helpers. Check: their tests pass. From TypeScript, the scenarios of the Rust test `embedded_session` give the same events with the same fake engine. `DeepgramFinal` works against a fake Deepgram server, as in the tests of `deepgram.rs`. Together these cover what Consort does with `sayso_sdk::embedded`.
- **T6. The engine packages.** `scripts/npm-packages.py`, `@justsayso/engine`, `Engine.fromPackage`, the asar rule, and the stderr of the engine in the start error of `sayso-sdk` ("Linux"). Check: on Linux without `libvulkan1`, the error names `libvulkan.so.1`. Then: the tests of "The packages", and a packaged Electron app on macOS and Linux. Measure the first start of a new engine on macOS again and find the cause of the 33.8 s.
- **T7. The runtimes.** The CI jobs of "The runtimes". Check: they pass.
- **T8. Cleanup.** A release file for `sayso-cleanup` on each platform, `@justsayso/cleanup`, and the `cleanup` module. Check: a download of S1-mini and one cleanup in local mode, from TypeScript, on each platform.
- **T9. Guide.** The pages of "Docs". Check: someone other than Chris builds an app from the TypeScript guide alone.
- **T10. Publication.** The scope, trusted publishing, and the first release on npm. Check: `npm install @justsayso/sdk @justsayso/engine` in an empty folder on a clean computer of each platform runs the tutorial.

## Answers to the open questions

Chris answered these on 2026-11-10.

1. **Interpreters.** An approval of a signed interpreter covers every version of that program. The host keeps the identity of `node` by its signature, as today. The guide tells authors to ship an app for other users with its own executable.
2. **An engine in JavaScript.** Not needed. The SDK has the final pass in JavaScript, and `EngineSource::Custom` stays out of the TypeScript API.
3. **The cleanup runtime.** The size of the release file is acceptable. It ships as `sayso-cleanup-<version>-<os>-<arch>` and as `@justsayso/cleanup`.
