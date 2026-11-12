# How to handle errors and the end of each piece of work in TypeScript

Every piece of work in the SDK ends in a way that you can count on. This page lists the ends, shows how to read a `SaysoError`, and says what keeps Node running. The error codes and their meaning are in [How to handle errors and the end of each piece of work](handle-errors-and-ends.md#read-the-error-codes). The codes are the same strings in TypeScript.

## Know the ends

| Work | How it ends |
|---|---|
| A request (`listModels`, `downloadModel`, `transcribeFile`, and the others) | One promise. It resolves, or it rejects with one `SaysoError`. |
| A job | One `job_done` or one `job_failed` event. No event of the job comes after it. `speech.transcribe` turns the two into a resolved or a rejected promise. |
| A live session | The promise of `stopSession`, or else one `session_ended` event. The first of the two counts. |
| A download | The promise of `downloadModel`, after the last `model_state` event. |
| `sessionAudio` | It returns at once. An error of the audio itself arrives as an event with `kind: "error"`. |

These rules are the same in Sayso mode and in local mode. They also hold when the mode changes or the connection breaks. The SDK never sends a request again by itself.

## Read a SaysoError

Each failed call rejects with a `SaysoError`. It has these fields:

- `code`: a string such as `not_approved`, or `undefined`. Act on `code`.
- `message`: English text for people. Show it to the user only as detail.
- `capability`: the missing capability, with the code `capability`.
- `reason`: a code of the engine, for example `download_failed`.

An error without a `code` has a `message` only. It comes from a failed download or from a file that the SDK could not read.

`host_not_running`, `connection_lost`, and `host_closing` come only from the SDK. Sayso never sends them.

<!-- check: verbatim packages/sdk/examples/handle-errors-and-ends.ts -->
```ts
export function explain(error: unknown): string {
	if (!(error instanceof SaysoError)) return error instanceof Error ? error.message : String(error);
	switch (error.code) {
		case "not_approved":
			return "The user did not allow this app in Sayso. Tell them where to change it: Settings > Connected apps.";
		case "capability":
			return `The user did not grant ${error.capability ?? "a capability"}.`;
		case "busy":
			return "Sayso is busy. Try again later.";
		case "host_not_running":
			return "Sayso does not run.";
		default:
			return error.message;
	}
}
```

A bad argument, for example an unknown `mode`, rejects with an `Error` that is not a `SaysoError`. It has no `code`. Fix the code of your app.

## Stop a session

Call `stopSession` once. A second call for the same session rejects with `bad_request`, and so does a call for a session that ended. If a `session_ended` event came first, the session is over, and the event has its `text`.

<!-- check: verbatim packages/sdk/examples/handle-errors-and-ends.ts -->
```ts
export async function stopAndGetText(speech: Speech, session: number): Promise<string> {
	try {
		return await speech.stopSession(session);
	} catch (error) {
		if (error instanceof SaysoError && (error.code === "connection_lost" || error.code === "host_closing")) {
			console.error(`The session ended with the connection: ${error.message}`);
		}
		throw error;
	}
}
```

## Read errors that belong to no request

An event with `kind: "error"` carries an error that no request caused. For example, the audio of `sessionAudio` was not valid. Log it, and show it to the user if the session cannot go on.

<!-- check: verbatim packages/sdk/examples/handle-errors-and-ends.ts -->
```ts
export function logErrorsOfNoRequest(speech: Speech): () => void {
	return speech.on((event) => {
		if (event.kind === "error") console.error(`${event.error.code ?? "error"}: ${event.error.message}`);
	});
}
```

## Close the Speech

Call `speech.close()` when you are done. It ends the work, closes the connection to Sayso or stops the engine of the app, and ends the iterators of `speech.events()` after the events that wait. A second call gives the same promise. After `close()` no event comes, also not the end of work that ran. A call that waits rejects with `connection_lost`.

Stop your live sessions first, so you get their text.

`Speech` has `[Symbol.asyncDispose]`, so `await using` closes it at the end of the block:

<!-- check: verbatim packages/sdk/examples/handle-errors-and-ends.ts -->
```ts
export async function withSpeech(options: SpeechOptions, work: (speech: Speech) => Promise<void>): Promise<void> {
	await using speech = await Speech.connect(options);
	await work(speech);
}
```

Node 24 and later run `await using`. In an older Node, compile the file with TypeScript, or use `try` and `finally`:

<!-- check: verbatim packages/sdk/examples/handle-errors-and-ends.ts -->
```ts
export async function withSpeechAndFinally(options: SpeechOptions, work: (speech: Speech) => Promise<void>): Promise<void> {
	const speech = await Speech.connect(options);
	try {
		await work(speech);
	} finally {
		await speech.close();
	}
}
```

A `Speech` that nothing closes closes when the garbage collector takes it. The time of that is not known, so call `close()` yourself. Call it on an `EmbeddedSpeech` and on a `Client` too.

## Know what keeps Node running

Node ends when it has nothing left to do. The SDK holds the event loop while work is active:

- a request that has no reply yet,
- a job before its end,
- a live session before its end.

A `Speech` with no work does not hold the loop, and neither does a listener of `speech.on` or an iterator of `speech.events()`. So a program that only waits for events ends at once. To keep it alive, give it work of your own, for example a server or a timer. `close()` releases everything that the SDK holds.

A program with a job ends after the last event of the job. That is the end rule of the job.
