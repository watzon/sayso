# @justsayso/sdk

The speech features of [Sayso](https://justsayso.app) for apps in TypeScript and JavaScript. Your app transcribes files and runs live sessions with one API:

- When Sayso runs on the computer, the SDK uses Sayso: its engine, its models, and its microphone. The user approves your app once.
- When Sayso does not run, the SDK starts the engine that your app ships, and does the work in the process of your app.

The package is a native addon around the Rust crate [`sayso-sdk`](https://crates.io/crates/sayso-sdk), so the rules are the same as for a Rust app. It runs in Node 22.12 or newer (Node 24 to run `.ts` files without a build step), in the main process of Electron, in Bun, and in Deno, on macOS (Apple silicon and Intel), Linux (x64 and arm64, glibc), and Windows (x64).

## Install

For local mode, install the engine too:

```sh
npm install @justsayso/sdk @justsayso/engine
```

An app that only uses Sayso mode needs only `@justsayso/sdk`. The SDK is MIT. The engine package is GPL-3.0-only: the SDK starts it as a separate program and never links it.

On Linux the engine needs `libvulkan1`, and the capture needs `libasound2`. The engine needs glibc 2.35 or newer.

## Transcribe a file

```ts
import { resolve } from "node:path";
import { Engine, Speech } from "@justsayso/sdk";

await using speech = await Speech.connect({
	mode: "auto",
	app: { name: "Notetaker" },
	capabilities: ["transcribe.file"],
	engine: Engine.fromPackage(),
});

console.log("mode:", speech.mode());
const transcript = await speech.transcribe({ path: resolve("talk.wav") });
console.log(transcript.text);
for (const segment of transcript.segments) {
	console.log(segment.start_ms, segment.end_ms, segment.text);
}
```

`await using` closes the `Speech` at the end of the block. Node 24 and later run it in JavaScript, and TypeScript compiles it for Node 22. Without it, call `await speech.close()`.

A path in a request must be absolute. The SDK refuses a relative path with the code `bad_request`.

In local mode a model must be on disk. Download one with `speech.downloadModel(id, reason)`, or give the model folders with `Engine.fromPackage().store(folder)` and `.modelsDir(folder)`. `speech.listModels("final")` lists the models and their state.

## Pick the mode

| `mode` | What it does |
|---|---|
| `"auto"` | Uses Sayso when it runs, else the engine of your app. It changes mode only when your app has no active work, and sends the event `mode_changed`. It never starts Sayso. |
| `"local"` | Uses the engine of your app. It never talks to Sayso. |
| `"sayso"` | Uses Sayso, and fails with `host_not_running` when Sayso does not run, or with `not_approved` when the user refused your app. |
| `{ sayso: { start: true } }` | As `"sayso"`, and starts Sayso when it does not run. Do this only when the user asked for it: Sayso has hotkeys and a menu bar icon. |

`speech.features()` says what the current mode can do. For example, `enhance` and `subscribe` work only in Sayso mode.

The first time your app connects to Sayso, Sayso asks the user to approve it, and `Speech.connect` waits for the answer. `capabilities` lists what your app wants, for that question.

## Read the events

Jobs and live sessions report their progress as events. Each listener and each iterator gets every event, from the moment it started:

```ts
// Start to read before the job starts, so no event of the job is lost.
const events = speech.events();
const job = await speech.transcribeFile({ path: resolve("talk.wav"), diarize: true });
for await (const event of events) {
	if (event.kind !== "message") continue;
	const message = event.message;
	if (message.type === "segment" && message.job === job) console.log(message.text);
	if (message.type === "job_done" && message.job === job) break;
	if (message.type === "job_failed" && message.job === job) throw new Error(message.message);
}
```

An iterator gets the events from the moment that `speech.events()` makes it, so make it before the work starts. Or use a function: `const stop = speech.on((event) => { ... })`. Call `stop()` to remove it.

The messages have the names and fields of the Host API, in `snake_case`, as on the wire. The TypeScript types come from the Rust types, so they cannot differ. The events are:

- `{ kind: "message", message }`: `job_progress`, `segment`, `speakers`, `job_done`, `job_failed`, `partial`, `source_gap`, `source_ended`, `part_failed`, `session_ended`, `model_state`, `download_progress`, or `dictation_done`.
- `{ kind: "mode_changed", mode, features }`: the mode changed. No work of your app was active.
- `{ kind: "error", error }`: an error that belongs to no request.
- `{ kind: "disconnected", code }`: Sayso mode only. The connection ended, and requests fail until `speech.reconnect()`.

Each job ends with one `job_done` or `job_failed`, and each live session with the reply of `stopSession` or the event `session_ended`. While work is active, Node does not end the process. Without work, the process can end.

## Run a live session

```ts
const { session } = await speech.startSession({ sources: ["mic"], preview_model: "nemotron-en" });
speech.on((event) => {
	if (event.kind === "message" && event.message.type === "partial") console.log(event.message.tentative);
});
// Later:
const text = await speech.stopSession(session);
```

The sources are `mic`, `system` (the sound of other apps and calls), and `client` (audio from your app, with `speech.sessionAudio(session, samples)`, 16 kHz mono `Int16Array`).

To use your own capture in local mode, give the sources in `capture`:

```ts
const microphone = {
	open(writer) {
		// Give 16 kHz mono Float32Array samples until the session ends.
		const stop = myCapture.start((samples) => writer.push(samples));
		writer.onClose(stop);
	},
};
await Speech.connect({ mode: "local", app: { name: "Notetaker" }, engine: Engine.fromPackage(), capture: { microphone } });
```

To refuse, throw `new SaysoError({ code: "permission", message: "..." })` from `open`.

## Handle errors

Each failed call throws a `SaysoError` with a `code`, the same codes as in Rust and on the wire: `host_not_running`, `not_approved`, `capability`, `declined`, `model_not_downloaded`, `unsupported`, `bad_request`, `connection_lost`, and others. A bad argument (for example a job id that is not an integer) throws an `Error` that is not a `SaysoError`, with the `code` `InvalidArg`.

## macOS

In local mode, macOS asks the user for the microphone and the system audio in the name of the app that is responsible for the process:

- An Electron app needs `NSMicrophoneUsageDescription` and `NSAudioCaptureUsageDescription` in its `Info.plist`, and the entitlement `com.apple.security.device.audio-input` with the hardened runtime.
- A script in a terminal gets the permission of the terminal. `microphonePermission.request()` gives `false` in a process without `NSMicrophoneUsageDescription`.

In Sayso mode, Sayso knows your app by its executable. `node app.js` with the official Node is the signed program `node`, so one approval covers each script that runs with it. Ship an app for other people with its own executable, for example Electron.

## Electron

Use the SDK in the main process. In a packaged app, the engine cannot start from inside `app.asar`, so unpack it, and unpack the addon:

```json
{ "asarUnpack": ["**/node_modules/@justsayso/sdk-*/**", "**/node_modules/@justsayso/engine-*/**"] }
```

`Engine.fromPackage()` finds the engine in `app.asar.unpacked`. A signing tool such as `@electron/osx-sign` signs the engine again with the identity of your app. That is correct.

## More of the API

- `EmbeddedSpeech`: a session in the process of your app with a layer around the engine, for example `new DeepgramFinal(key)` or your own final pass in JavaScript (`finalPass: { models, transcribe }`). It never talks to Sayso.
- `Client`: the plain client of the Host API, with `request(kind, fields)` and the events as they are.
- `Deepgram`: a small client of the HTTP API of Deepgram.
- `cleanup` and `RuntimeCleaner`: the local cleanup models (S1-mini by Superwhisper, Cleanup 1 by Spokenly), their download, and the runtime `sayso-cleanup` from `@justsayso/cleanup`.
- `findEngine`, `modelStore`, `microphonePermission`, `devices`, `launchers`, `startSayso`, and `protocolVersion`.

The design is in [docs/sdk-typescript.md](https://github.com/watzon/sayso/blob/main/docs/sdk-typescript.md). The guide of the Rust SDK, [docs/sdk-guide](https://github.com/watzon/sayso/tree/main/docs/sdk-guide), explains the modes, the models, and the Host API in more detail; its reference is also the reference of the messages here.
