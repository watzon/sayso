# Tutorial: build a speech app in TypeScript

In this tutorial you build `scribe`, a small Node program. It transcribes an audio file. It prints the text and the time of each segment.

The program uses `mode: "auto"`. It uses Sayso when Sayso runs. When Sayso does not run, it uses the engine from the npm package `@justsayso/engine`. The same code works in both cases.

## What you need

- Node 24 or later. Run `node --version` to check. Node 24 runs `.ts` files and the `await using` statement with no build step.
- A computer that the engine supports: macOS (Apple silicon or Intel), Linux (x64 or arm64, glibc 2.35 or newer), or Windows (x64). On Linux, install the libraries `libvulkan1` and `libasound2`.
- No Sayso on the computer. If Sayso runs, the program uses it and the output differs (see step 3). Quit Sayso for this tutorial.
- The packages `@justsayso/sdk` and `@justsayso/engine`. They are not on npm yet, so the install command in step 1 works only after the first release.

## 1. Make the project

Make a folder and a `package.json`:

```sh
mkdir scribe
cd scribe
npm init -y
npm pkg set type=module
```

`type=module` makes Node run `scribe.ts` as an ES module. An ES module can use `await` at the top level.

Install the SDK and the engine:

```sh
npm install @justsayso/sdk @justsayso/engine
```

npm installs the engine package and the addon package for your platform only. Look at the result:

```sh
ls node_modules/@justsayso
```

On a Mac with Apple silicon, you see:

```text
engine
engine-darwin-arm64
sdk
sdk-darwin-arm64
```

On other systems, the names `engine-<platform>` and `sdk-<platform>` end with your platform, for example `linux-x64-gnu`.

## 2. Make a sample file

`scribe` reads an audio file. On a Mac, make one with `say`:

```sh
say -o talk.wav --data-format=LEI16@16000 "Hello from the Sayso SDK. This is a short test of the guide. The quick brown fox jumps over the lazy dog."
```

The command prints nothing. The file `talk.wav` is now in the folder.

To use a file that you have, give its path to `scribe` in the next steps. The engine reads WAV, FLAC, MP3, AAC, and M4A files.

## 3. Connect

Make the file `scribe.ts` with this program:

<!-- check: lines packages/sdk/examples/tutorial.ts -->
```ts
import { Engine, SaysoError, Speech } from "@justsayso/sdk";

const file = process.argv[2];
const model = process.argv[3] ?? "whisper-tiny";
if (!file) {
	console.error("usage: node scribe.ts <audio file> [model id]");
	process.exit(2);
}

try {
	await using speech = await Speech.connect({
		mode: "auto",
		app: { name: "Scribe" },
		capabilities: ["models.download", "transcribe.file"],
		engine: Engine.fromPackage(),
	});
	console.error("mode:", speech.mode());
} catch (error) {
	console.error(error instanceof SaysoError ? `${error.code ?? "error"}: ${error.message}` : error);
	process.exitCode = 1;
}
```

`Speech.connect` starts the SDK in the mode that you ask for. `capabilities` lists what the app uses. In local mode the SDK asks nobody, but the list stays the same when Sayso runs. `Engine.fromPackage()` finds the engine in `@justsayso/engine`. `await using` closes the `Speech` at the end of the `try` block. The `catch` block prints the `code` and the `message` of a `SaysoError`.

Run it:

```sh
node scribe.ts talk.wav
```

You see one line, and the program ends by itself:

```text
mode: local
```

The SDK keeps Node running only while work is active. Nothing is active here.

If Sayso runs on your computer, you see `mode: sayso` instead. Sayso also shows a question about the program `node`. Sayso knows a script by the program that runs it, so the answer covers every script that you run with this Node. See [How to ship in Electron](ts-ship-in-electron.md#know-the-identity-of-your-app).

If the package `@justsayso/engine` is not installed, `Engine.fromPackage()` throws a `SaysoError` with the code `unsupported`. The program prints that code and a message that tells you to install the package.

## 4. Download a model

The engine has no model in it. You ask it to download one. `whisper-tiny` is small and fast, so it suits a first run.

Add these lines below the line `console.error("mode:", speech.mode());`:

<!-- check: lines packages/sdk/examples/tutorial.ts -->
```ts
	speech.on((event) => {
		if (event.kind !== "message") return;
		const message = event.message;
		if (message.type === "download_progress") console.error(`${message.model}: ${Math.round(message.fraction * 100)} %`);
	});

	await speech.downloadModel(model, "To transcribe your files");
```

`speech.on` calls your function for each event. An event with `kind: "message"` carries a message of the Host API, with the names and fields of the [reference](reference.md#events). In local mode, `downloadModel` starts at once and resolves when the model is on disk. The second argument is the reason that Sayso shows to its user in Sayso mode. Write it so a person understands it.

The model goes to the model store of your user. Sayso uses the same store, so a model is on disk once. To keep this tutorial away from your real store, set `XDG_DATA_HOME` to a new folder first. This works on every system:

```sh
export XDG_DATA_HOME="$PWD/data"
```

Run the program again:

```sh
node scribe.ts talk.wav
```

You see `mode: local`, then progress lines such as `whisper-tiny: 12 %`. The number and the count of the lines differ on each run. The program ends when the download ends.

Run it a second time. The model is on disk now, so the program is fast and prints no progress.

If the engine does not list `whisper-tiny`, the program prints `unknown_model` and a message. Run the program with another id as the second argument. [How to choose a model and download it](ts-choose-and-download-a-model.md) shows how to list the ids.

## 5. Transcribe the file

A job runs in the background. `speech.transcribe` starts a job and resolves when the job ends. It gives the text, the segments, and the speakers.

Add this line to the top of the file:

<!-- check: lines packages/sdk/examples/tutorial.ts -->
```ts
import { resolve } from "node:path";
```

Add this line to the function that you passed to `speech.on`, below the line with `download_progress`:

<!-- check: lines packages/sdk/examples/tutorial.ts -->
```ts
		if (message.type === "job_progress") console.error(`${Math.round(message.fraction * 100)} %`);
```

Add these lines to the end of the `try` block:

<!-- check: lines packages/sdk/examples/tutorial.ts -->
```ts
	const transcript = await speech.transcribe({ path: resolve(file), model });
	console.log(transcript.text);
```

A path in a request must be absolute. `resolve` makes one from the path that you gave. A relative path gets the error code `bad_request`.

Run it:

```sh
node scribe.ts talk.wav
```

You see the progress of the job on the error output, then the text on the standard output:

```text
mode: local
0 %
100 %
Hello from the SASOSDK, this is a short test of the guide, the quick brown fox jumps over the lazy dog.
```

The text is not perfect. `whisper-tiny` is the smallest model. A larger model gives a better text.

If the file is not audio, the job fails. `transcribe` rejects with a `SaysoError`, and the program prints `error:` and the message of the failure. Try it:

```sh
echo hi > bad.wav
node scribe.ts bad.wav
```

## 6. Print the segments

A segment is a part of the text with its time. `transcript.segments` has one entry for each `segment` event of the job.

Add these lines below `console.log(transcript.text);`:

<!-- check: lines packages/sdk/examples/tutorial.ts -->
```ts
	for (const segment of transcript.segments) {
		console.log(`${segment.start_ms} ms to ${segment.end_ms} ms: ${segment.text}`);
	}
```

Run it again. After the full text, you see one line for each segment. A line has the start and the end of the segment in milliseconds from the start of the file, then the text of the segment.

The fields are `start_ms` and `end_ms`, in `snake_case`, as in the Host API. Messages keep the names of the wire. The options and the methods of the SDK use `camelCase`.

## 7. The finished program

This is the complete `scribe.ts`. Compare it with yours. The file is also in the repository as `packages/sdk/examples/tutorial.ts`. A script checks that this page and the file match, line for line.

<!-- check: file packages/sdk/examples/tutorial.ts -->
```ts
import { resolve } from "node:path";
import { Engine, SaysoError, Speech } from "@justsayso/sdk";

const file = process.argv[2];
const model = process.argv[3] ?? "whisper-tiny";
if (!file) {
	console.error("usage: node scribe.ts <audio file> [model id]");
	process.exit(2);
}

try {
	await using speech = await Speech.connect({
		mode: "auto",
		app: { name: "Scribe" },
		capabilities: ["models.download", "transcribe.file"],
		engine: Engine.fromPackage(),
	});
	console.error("mode:", speech.mode());

	speech.on((event) => {
		if (event.kind !== "message") return;
		const message = event.message;
		if (message.type === "download_progress") console.error(`${message.model}: ${Math.round(message.fraction * 100)} %`);
		if (message.type === "job_progress") console.error(`${Math.round(message.fraction * 100)} %`);
	});

	await speech.downloadModel(model, "To transcribe your files");

	const transcript = await speech.transcribe({ path: resolve(file), model });
	console.log(transcript.text);
	for (const segment of transcript.segments) {
		console.log(`${segment.start_ms} ms to ${segment.end_ms} ms: ${segment.text}`);
	}
} catch (error) {
	console.error(error instanceof SaysoError ? `${error.code ?? "error"}: ${error.message}` : error);
	process.exitCode = 1;
}
```

## What you learned

- `Speech.connect` with `mode: "auto"` starts the SDK. `Engine.fromPackage()` points to the engine of the package `@justsayso/engine`.
- `downloadModel` puts a model in the model store.
- `speech.on` and `speech.events()` give the events of the SDK.
- `speech.transcribe` runs a job and resolves with the text and the segments.
- `await using` closes the `Speech`.

## Next

- To run a live session from the microphone, see [How to run a live session](ts-run-a-live-session.md).
- To handle each error and each end of work, see [How to handle errors and ends](ts-handle-errors-and-ends.md).
- To put the SDK in an Electron app, see [How to ship in Electron](ts-ship-in-electron.md).
- For every message and field, see the [reference](reference.md).
