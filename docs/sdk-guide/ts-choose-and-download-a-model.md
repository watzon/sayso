# How to choose a model and download it in TypeScript

Use `speech.listModels` to see which models this computer can run. Use `speech.downloadModel` to put one on disk. Then tell the SDK which model to use. The rules are the same as in [How to choose a model and download it](choose-and-download-a-model.md). This page shows the TypeScript code.

You need a connected `Speech`. In Sayso mode, `listModels` needs no grant, and `downloadModel` needs the capability `models.download`.

## List the models

`speech.listModels` takes an optional role: `final` (the final pass on a recording), `streaming` (text while the audio comes in), `vad` (voice detection), or `diarization` (who speaks when). A model can have more than one role.

<!-- check: verbatim packages/sdk/examples/choose-and-download-a-model.ts -->
```ts
export async function showModels(speech: Speech): Promise<void> {
	for (const model of await speech.listModels("final")) {
		const chosen = model.choice?.join(", ") ?? "";
		console.log(`${model.id} (${model.name}): ${model.state}, ${Math.round(model.size_bytes / 1_000_000)} MB, chosen for: ${chosen}`);
	}
}
```

Each `ModelInfo` has these fields. The names are in `snake_case`, as in the Host API.

- `id` and `name`. The id is stable and goes into requests.
- `roles`, `languages` (empty when the model has no list), `timestamps` (`word`, `segment`, or `none`), and `size_bytes`.
- `state`: `not_downloaded`, `downloading`, `downloaded`, `optimizing`, `ready`, or `failed`. A model is on disk when its state is `downloaded`, `optimizing`, or `ready`.
- `choice`: the roles for which the SDK uses this model when a request names none. The field is missing when there is no role.

## Download a model

Call `speech.downloadModel` with the id and a reason. In local mode, the download starts at once, and the promise resolves when the model is on disk. If the model is on disk already, it resolves at once.

In Sayso mode, Sayso asks the user first and shows your reason. Write one sentence that the user understands. If the user says no, the promise rejects with the error code `declined`. In local mode there is no question, so ask your own user before you call it.

The events `model_state` and `download_progress` arrive while the download runs. This function shows the progress and removes its listener at the end:

<!-- check: verbatim packages/sdk/examples/choose-and-download-a-model.ts -->
```ts
export async function download(speech: Speech, model: string): Promise<void> {
	const stop = speech.on((event) => {
		if (event.kind === "message" && event.message.type === "download_progress") {
			console.error(`${event.message.model}: ${Math.round(event.message.fraction * 100)} %`);
		}
	});
	try {
		await speech.downloadModel(model, "To transcribe your recordings on this computer");
	} finally {
		stop();
	}
}
```

The events of a download are `model_state { model, state: "downloading" }`, then `download_progress { model, fraction, bytes_done, bytes_total }`, then `model_state { model, state: "downloaded" }`. If the download fails, the last events are `model_state { state: "failed", message, reason }`, and the promise rejects with a `SaysoError` that has the same `reason` and no `code`.

An unknown id rejects with `unknown_model`.

## Choose a model for requests

A request chooses its models in this order:

1. The field of the request, for example `model` in `transcribeFile` or `startSession`.
2. In Sayso mode, the choice of the user in Sayso. Your app cannot change it, and the option `models` does not count.
3. In local mode, the option `models`. Then the first suitable model on disk.

The option `models` sets the choice of the app for local mode. Its names are in `camelCase`: `finalModel`, `previewModel`, `vadModel`, `diarizationModel`, and `language`.

<!-- check: verbatim packages/sdk/examples/choose-and-download-a-model.ts -->
```ts
export function optionsWithModels(engine: Engine): SpeechOptions {
	return {
		mode: "local",
		app: { name: "Notetaker" },
		engine,
		models: { finalModel: "whisper-tiny", vadModel: "basic", language: "en" },
	};
}
```

For `vadModel`, `basic` means Basic VAD. If the voice detection model that you chose is not on disk, local mode falls back to Basic VAD. For `finalModel`, the request rejects with `model_not_downloaded` instead.

A request that names a model checks it before the work starts. The codes are `unknown_model`, `wrong_role`, `model_not_downloaded`, `unsupported`, and `model_retired`. See the [table of codes](choose-and-download-a-model.md#choose-a-model-for-requests) and [How to handle errors and ends](ts-handle-errors-and-ends.md).
