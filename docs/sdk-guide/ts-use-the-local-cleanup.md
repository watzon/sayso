# How to clean up text with a local cleanup model in TypeScript

Use a cleanup model to fix a transcript: capital letters, the final period, filler words, and lists. A cleanup model runs on the computer of the user, so the text does not leave it. It reads no prompt. It uses fixed settings (tone, layout, and format).

The SDK has two cleanup models: S1-mini by Superwhisper (English) and Cleanup 1 by Spokenly (seven languages). A separate program, the cleanup runtime `sayso-cleanup`, runs them. The package `@justsayso/cleanup` has the runtime. The terms Cleanup model and Cleanup runtime are in [CONTEXT.md](../../CONTEXT.md).

You need the package `@justsayso/cleanup`: `npm install @justsayso/cleanup`. It has the runtime `sayso-cleanup` for your platform. The runtime is GPL-3.0-only. The SDK starts it as a separate program.

```sh
npm install @justsayso/sdk @justsayso/cleanup
```

## List the models

`cleanup.catalog()` lists the models that the SDK can download and run. Each `CleanupModel` has `id`, `name`, `publisher`, `sizeBytes`, `languages`, `tones`, `layouts`, `formats`, and `context`.

Show the name and the publisher of a model together. The license of S1-mini asks for this: show it as "S1-mini" by "Superwhisper", exactly. `name` and `publisher` have those words.

<!-- check: verbatim packages/sdk/examples/use-the-local-cleanup.ts -->
```ts
export function label(model: CleanupModel): string {
	return `${model.name} by ${model.publisher}`;
}
```

<!-- check: verbatim packages/sdk/examples/use-the-local-cleanup.ts -->
```ts
export function showCatalog(): void {
	for (const model of cleanup.catalog()) {
		console.log(`${label(model)}: ${Math.round(model.sizeBytes / 1_000_000)} MB, languages: ${model.languages.join(", ")}`);
	}
}
```

## Download a model

The SDK does not ask your user. Ask first, and then call `cleanup.download`. It downloads the model file and its license files to `<modelsDir>/cleanup/<id>`, and resolves with the path of the model file. Keep the license files with the model. `modelsDir` can be any folder of your app.

Pass an `AbortSignal` to let the user cancel. A canceled download rejects with the code `declined`. The partial file stays, and the next download goes on from it. `onProgress` gets `bytesDone` and `bytesTotal`. While a download runs, Node does not end the process.

<!-- check: verbatim packages/sdk/examples/use-the-local-cleanup.ts -->
```ts
export async function download(modelsDir: string, id: string, signal: AbortSignal): Promise<string> {
	const ready = cleanup.modelPath(modelsDir, id);
	if (ready) return ready;
	try {
		return await cleanup.download(modelsDir, id, {
			signal,
			onProgress: ({ bytesDone, bytesTotal }) => console.error(`${Math.round((bytesDone / bytesTotal) * 100)} %`),
		});
	} catch (error) {
		if (error instanceof SaysoError && error.code === "declined") console.error("The download stopped. A later call goes on from the partial file.");
		throw error;
	}
}
```

`cleanup.isReady(modelsDir, id)` says if the model is complete on disk. `cleanup.modelPath(modelsDir, id)` gives the model file, or `null`. `cleanup.delete(modelsDir, id)` removes the files. A model that is not on disk gives no error.

## Clean a text

`RuntimeCleaner` runs a model in the cleanup runtime. Give it the runtime and the `modelsDir` of the download. `cleanup.runtimeFromPackage()` gives the runtime of `@justsayso/cleanup` for this platform. In a packaged Electron app it points into `app.asar.unpacked`, so unpack the package (see [How to ship in Electron](ts-ship-in-electron.md#unpack-the-addon-and-the-engine)).

The runtime starts at the first cleanup. It stops after a time without use, or when you call `stop()`. `RuntimeCleaner` has `[Symbol.dispose]`, so `using` stops it at the end of the block.

The request has `model`, `text`, and optional `settings`, `app`, `site`, and `timeoutMs`:

- `settings` has `tone` (`very-casual`, `casual`, or `formal`), `layout` (`paragraphs` or `list`), and `format` (`plain` or `email`). A missing value is `formal`, `paragraphs`, or `plain`. A model uses only the settings that it has: `tones`, `layouts`, and `formats` of the catalog say which.
- `app` and `site` are for a model with `context: true`.
- `timeoutMs` is one deadline for the whole text.

A failure keeps nothing. Use the text that went in, as this function does:

<!-- check: verbatim packages/sdk/examples/use-the-local-cleanup.ts -->
```ts
export async function cleanText(modelsDir: string, text: string): Promise<string> {
	using cleaner = new RuntimeCleaner({ runtime: cleanup.runtimeFromPackage(), modelsDir });
	try {
		const result = await cleaner.clean({ model: "s1-mini", text, settings: { layout: "paragraphs", format: "plain" } });
		return result.text;
	} catch (error) {
		console.error(`The cleanup failed: ${error instanceof Error ? error.message : error}`);
		return text;
	}
}
```

S1-mini knows English only. Check `model.languages` before you call a model with another language.

## Load the model early

`cleaner.preload(id)` loads the model while the microphone records. It is a hint, and it gives no error.

<!-- check: verbatim packages/sdk/examples/use-the-local-cleanup.ts -->
```ts
export function loadWhileRecording(cleaner: RuntimeCleaner): void {
	cleaner.preload("s1-mini");
}
```

`cleaner.isRunning()` says if the runtime runs. `cleaner.stop()` stops it and frees the memory of the model. The next cleanup starts it again.
