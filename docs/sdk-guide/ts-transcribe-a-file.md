# How to transcribe a file in TypeScript

Use a job to transcribe an audio file. A job runs in the background and sends its result as events. The rules are the same as in [How to transcribe a file](transcribe-a-file.md). This page shows the TypeScript code.

You need a connected `Speech` (see the [tutorial](ts-tutorial.md)) and a speech model on disk (see [How to choose a model and download it](ts-choose-and-download-a-model.md)). In Sayso mode, your app also needs the capability `transcribe.file`.

## Transcribe the file

1. Make an absolute path. `resolve` from `node:path` does this. A relative path gets the error code `bad_request`.
2. Call `speech.transcribe`. It resolves when the job ends.

<!-- check: verbatim packages/sdk/examples/transcribe-a-file.ts -->
```ts
export function transcribeFile(speech: Speech, file: string): Promise<Transcript> {
	return speech.transcribe({ path: resolve(file), language: "en" });
}
```

The result is a `Transcript`:

- `job` is the id of the job.
- `text` is the full transcript.
- `segments` has the `segment` events of the job, in order.
- `speakers` is the `speakers` event of the job. It is there only when you asked for speakers.

Each segment has `start_ms` and `end_ms` in milliseconds from the start of the file. The names are in `snake_case` because a segment is the message of the Host API (see the [reference](reference.md#events)). When the model has the timestamp level `word`, a segment also has `words`, with a time for each word:

<!-- check: verbatim packages/sdk/examples/transcribe-a-file.ts -->
```ts
export function showWords(transcript: Transcript): void {
	for (const segment of transcript.segments) {
		for (const word of segment.words ?? []) console.log(word.start_ms, word.end_ms, word.text);
	}
}
```

If the job fails, `speech.transcribe` rejects with a `SaysoError`. Its `message` says why. Its `code` is set only when the job failed with a code.

## Read the events yourself

Use `speech.transcribeFile` when you want to show progress or keep the events. It resolves with the id of the job at once. The job ends with exactly one `job_done` or one `job_failed`. No event of the job comes after that.

Start the iterator before you send the request. The iterator then has every event of the job.

<!-- check: verbatim packages/sdk/examples/transcribe-a-file.ts -->
```ts
export async function transcribeWithProgress(speech: Speech, file: string): Promise<string> {
	const events = speech.events();
	const job = await speech.transcribeFile({ path: resolve(file) });
	for await (const event of events) {
		if (event.kind !== "message") continue;
		const message = event.message;
		if (message.type === "job_progress" && message.job === job) console.error(`${Math.round(message.fraction * 100)} %`);
		if (message.type === "job_done" && message.job === job) return message.text;
		if (message.type === "job_failed" && message.job === job) {
			throw new SaysoError({ code: message.code, message: message.message });
		}
	}
	throw new SaysoError({ code: "connection_lost", message: "the Speech closed before the end of the job" });
}
```

`job_progress { job, fraction }` goes from 0 to 1. A job that waits behind another job sends `fraction: 0` until it starts. With speakers, transcription is the first 90 % and diarization the last 10 %.

Each iterator of `speech.events()` and each listener of `speech.on` gets every event, from the moment it starts. You can read the events in more than one place.

## Change how the file is transcribed

All fields of the request except `path` are optional. The names are in `snake_case`, as in the Host API.

- `model`: the id of a speech model. Without it, the SDK takes the model that you chose in the option `models`. In Sayso mode, the host takes the choice of the user instead.
- `language`: a code such as `en`. The default is `auto`.
- `vad_model`: a voice detection model, or `basic` for Basic VAD. See the [reference](reference.md#transcribe_file).
- `diarize`, `diarization_model`, and `speakers`: see [How to identify speakers](identify-speakers.md). The turns are in `transcript.speakers?.turns`.

## Cancel a job

Call `speech.cancelJob` with the id of the job. The job ends with `job_failed`, no `code`, and the message `The job was cancelled.`

<!-- check: verbatim packages/sdk/examples/transcribe-a-file.ts -->
```ts
export function cancel(speech: Speech, job: number): Promise<void> {
	return speech.cancelJob(job);
}
```

A cancel for a job that ended already is not an error. A job id that the app never got rejects with `bad_request`.

To cancel a job that you started with `speech.transcribe`, pass an `AbortSignal`. When the signal aborts, `transcribe` cancels the job and rejects with a `SaysoError`. The message is `the transcription was cancelled`.

<!-- check: verbatim packages/sdk/examples/transcribe-a-file.ts -->
```ts
export function transcribeWithTimeout(speech: Speech, file: string, ms: number): Promise<Transcript> {
	return speech.transcribe({ path: resolve(file) }, { signal: AbortSignal.timeout(ms) });
}
```

## Know the limits

- The SDK reads WAV, FLAC, MP3, AAC, and M4A files. For another format, convert the file first, or send the audio as a live session with the source `client` (see [How to run a live session](ts-run-a-live-session.md#send-your-own-audio)). A file that the SDK cannot read ends the job with `job_failed`. The message names the formats.
- One job runs at a time. Other jobs wait in a queue, first in, first out. In Sayso mode the queue is shared by all apps. In local mode it is only yours.
- While a job runs, Node does not end the process. See [How to handle errors and ends](ts-handle-errors-and-ends.md#know-what-keeps-node-running).
