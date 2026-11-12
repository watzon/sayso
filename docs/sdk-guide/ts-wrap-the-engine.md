# How to put your own final pass around the engine in TypeScript

Use `EmbeddedSpeech` when your app keeps its own models, never talks to Sayso, and sends the final pass of some models to another service. `Speech` is the API for most apps. `EmbeddedSpeech` is the TypeScript form of `sayso_sdk::embedded` in the Rust crate (the design is in [docs/sdk.md](../sdk.md)).

You need the engine of your app (see [How to ship in Electron](ts-ship-in-electron.md) for the package) and, for Deepgram, a Deepgram key from your user.

## Connect

`EmbeddedSpeech.connect` gets ready. It starts no engine and loads no model. It takes:

- `app`: the name of your app.
- `engine`: an `Engine` with the folder for your models, for example `Engine.fromPackage().modelsDir(folder)`. `modelsDir` is required here. You can also give `{ program, modelsDir, store? }`: the engine program, the folder for your models, and the model store (the default is `modelStore()`).
- `tempDir`: a folder for the temporary recordings of live sessions. The SDK makes it.
- `microphone`: an `AudioSource` that replaces the microphone of the SDK (see [How to use your own capture](ts-use-your-own-capture.md)). Leave it out to use the microphone of the SDK.
- `finalPass`: a `DeepgramFinal` or a `FinalPassFunction`.

`Engine.fromPackage()` finds the engine of the package `@justsayso/engine`. In a packaged Electron app it points into `app.asar.unpacked`.

## Use Deepgram for the final pass

`new DeepgramFinal(key)` sends the final pass of the model `Deepgram.MODEL` (`deepgram-nova-3`) to Deepgram. The engine keeps all other models.

<!-- check: verbatim packages/sdk/examples/embedded-final-pass.ts -->
```ts
export function connectWithDeepgram(dataFolder: string, deepgramKey: string): Promise<EmbeddedSpeech> {
	return EmbeddedSpeech.connect({
		app: { name: "Notetaker" },
		engine: Engine.fromPackage().modelsDir(join(dataFolder, "models")),
		tempDir: join(dataFolder, "tmp"),
		finalPass: new DeepgramFinal(deepgramKey),
	});
}
```

Check the key of the user before you use it. A key that Deepgram refuses rejects with the code `Deepgram.CODE_KEY`. A Deepgram that cannot be reached rejects with `Deepgram.CODE_CONNECTION`.

<!-- check: verbatim packages/sdk/examples/embedded-final-pass.ts -->
```ts
export async function keyIsValid(key: string): Promise<boolean> {
	try {
		await new Deepgram(key).checkKey();
		return true;
	} catch (error) {
		if (error instanceof SaysoError && error.code === Deepgram.CODE_KEY) return false;
		throw error;
	}
}
```

The SDK keeps the key in memory and writes it in no log.

## Write the final pass in JavaScript

A `FinalPassFunction` has a list of model ids and a `transcribe` function. The SDK calls `transcribe` for each part of a session whose final model is in `models`. It gives `transcribe` the audio of the part as 16 kHz mono samples from -1 to 1. The function resolves with the text. Any other value is an error. The SDK sends the other models to the engine.

<!-- check: verbatim packages/sdk/examples/embedded-final-pass.ts -->
```ts
export const finalPass: FinalPassFunction = {
	models: ["my-service"],
	async transcribe(samples, model, language) {
		return sendToMyService(samples, model, language);
	},
};
```

Pass it as `finalPass`:

<!-- check: verbatim packages/sdk/examples/embedded-final-pass.ts -->
```ts
export function connectWithFunction(dataFolder: string): Promise<EmbeddedSpeech> {
	return EmbeddedSpeech.connect({
		app: { name: "Notetaker" },
		engine: Engine.fromPackage().modelsDir(join(dataFolder, "models")),
		tempDir: join(dataFolder, "tmp"),
		finalPass,
	});
}
```

## Prepare a model

`speech.models()` lists the models of the engine with their state on disk. It starts the engine. `speech.download` puts a model on disk at once, so ask your own user first. `sizeBytes` helps an engine that knows only a fraction. `speech.load` reads a model into memory and waits while it loads.

<!-- check: verbatim packages/sdk/examples/embedded-final-pass.ts -->
```ts
export async function prepareModel(speech: EmbeddedSpeech, id: string): Promise<void> {
	const model = (await speech.models()).find((found) => found.id === id);
	if (!model) throw new SaysoError({ code: "unknown_model", message: `the engine has no model ${id}` });
	if (model.state === "not_downloaded") {
		await speech.download(id, {
			sizeBytes: model.size_bytes,
			onProgress: (progress) => console.error(`${progress.model}: ${Math.round(progress.fraction * 100)} %`),
		});
	}
	await speech.load(id);
}
```

## Run a session

`speech.startSession` waits while the models of the session load. At most one session runs at a time. The options use `camelCase`: `finalModel`, `previewModel`, `vad`, `language`, and `silenceLimitMs`. `sources` lists the sources, for example `["mic"]`. `vad` is a VAD model id, and the default is `basic`.

The events are objects with a `type`: `partial`, `segment`, and `silence_limit`. `speech.stopSession` ends the audio, waits for the last parts, and resolves with the text. The `segment` events come before it resolves.

<!-- check: verbatim packages/sdk/examples/embedded-final-pass.ts -->
```ts
export async function dictate(speech: EmbeddedSpeech, model: string, ms: number): Promise<string> {
	const session = await speech.startSession({ sources: ["mic"], finalModel: model, language: "en" });
	const stop = speech.on((event) => {
		if (event.type === "partial") console.log(`${event.committed} ${event.tentative}`);
		if (event.type === "segment") console.log(event.text);
	});
	try {
		await new Promise((resolve) => setTimeout(resolve, ms));
		return await speech.stopSession(session);
	} finally {
		stop();
	}
}
```

For the source `client`, send audio with `speech.sessionAudio`. It takes a `Float32Array`.

<!-- check: verbatim packages/sdk/examples/embedded-final-pass.ts -->
```ts
export function sendAudio(speech: EmbeddedSpeech, session: number, samples: Float32Array): void {
	speech.sessionAudio(session, samples);
}
```

`speech.cancelSession()` stops the session and forgets the parts that wait. For the microphone of the SDK, `setDevice`, `setLevel`, `openEarly`, and `closeEarly` choose the device, show the level, and start the capture before the models load.

## Close the session object

Call `speech.close()` when you are done, or use `await using`. It stops the session, stops the engine, and ends the iterators after the events that wait.
