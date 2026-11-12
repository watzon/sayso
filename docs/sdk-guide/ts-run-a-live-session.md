# How to run a live session in TypeScript

Use a live session to transcribe audio while it comes in. The audio can come from the microphone, from the sound of other apps, or from your own code. The sources, the capabilities, and the limits are the same as in [How to run a live session](run-a-live-session.md). This page shows the TypeScript code.

You need a connected `Speech` and a speech model on disk. In Sayso mode, your app needs the capability `session.live`, and also `audio.mic` for the microphone and `audio.system` for the system audio. See [How to ask for capabilities](ask-for-capabilities.md). On macOS in local mode, your app also needs the usage texts in its `Info.plist` (see [How to ship in Electron](ts-ship-in-electron.md#add-the-usage-texts-to-infoplist)).

## Check the microphone permission

On macOS, a refused microphone gives silence and no error. Check the permission before you start the session:

<!-- check: verbatim packages/sdk/examples/run-a-live-session.ts -->
```ts
export async function microphoneIsAllowed(): Promise<boolean> {
	if (microphonePermission.status() === "granted") return true;
	return microphonePermission.request();
}
```

`microphonePermission.status()` returns `granted`, `not_determined`, or `denied`. It does not ask the user. `microphonePermission.request()` shows the question of the system when the system has not asked yet. In a process that has no `NSMicrophoneUsageDescription`, `request()` shows nothing and resolves with `false`. A script in a terminal is such a process. The permission of the terminal app counts for the script.

Windows and Linux tie no permission to the app. A refused microphone there is an error of the capture.

## Start a session from the microphone and the system audio

`speech.startSession` resolves with the id of the session and `excludes`. `excludes` lists what the capture of the system audio leaves out: `sayso`, `app`, or both. When it is empty, the capture records the mix of the system, and the sound of your own app can be in the text. It is empty on Windows and Linux.

<!-- check: verbatim packages/sdk/examples/run-a-live-session.ts -->
```ts
export async function startCall(speech: Speech, folder: string): Promise<SessionReply> {
	const started = await speech.startSession({ sources: ["mic", "system"], record_to: folder });
	if (started.excludes.length === 0) {
		console.error("The capture records the mix of the system, with the sound of this app.");
	}
	return started;
}
```

On macOS, a first `startSession` with the source `system` can wait up to 120 seconds for the answer of the user to a question of the system. The other calls of `Speech` go on while it waits. If the system refuses a permission, `startSession` rejects with the error code `permission`.

## Read the events and stop the session

A session ends in one of two ways. Your app calls `speech.stopSession`, which resolves with the full text after the last events. Or the user stops the session in Sayso, or the connection ends, and you get one `session_ended` event. Only the first of the two counts. Handle both.

`follow` prints the events of one session. It returns a function that stops the listener, and a promise for the text of `session_ended`.

<!-- check: verbatim packages/sdk/examples/run-a-live-session.ts -->
```ts
function follow(speech: Speech, session: number): { stop(): void; ended: Promise<string> } {
	let onEnded: (text: string) => void = () => {};
	const ended = new Promise<string>((resolve) => {
		onEnded = resolve;
	});
	const stop = speech.on((event) => {
		if (event.kind !== "message") return;
		const message = event.message;
		switch (message.type) {
			case "partial":
				if (message.session === session) console.log(`${message.source}: ${message.committed} ${message.tentative}`);
				break;
			case "segment":
				if (message.session === session) console.log(`${message.source}: ${message.text}`);
				break;
			case "source_gap":
				if (message.session === session) console.error(`${message.source} lost ${message.ms} ms of audio`);
				break;
			case "source_ended":
				if (message.session === session) console.error(`${message.source} ended: ${message.reason}`);
				break;
			case "session_ended":
				if (message.session === session) onEnded(message.text);
				break;
		}
	});
	return { stop, ended };
}
```

`recordFor` starts a session, follows it, and stops it after `ms` milliseconds. If `session_ended` comes first, it returns the text of that event.

<!-- check: verbatim packages/sdk/examples/run-a-live-session.ts -->
```ts
export async function recordFor(speech: Speech, folder: string, ms: number): Promise<string> {
	const { session } = await startCall(speech, folder);
	const { stop, ended } = follow(speech, session);
	try {
		const text = await Promise.race([sleep(ms).then(() => undefined), ended]);
		return text ?? (await speech.stopSession(session));
	} finally {
		stop();
	}
}
```

The events of a session are:

- `partial { session, source, committed, tentative }`. The text of one source so far. `committed` does not change any more. `tentative` can still change. A model without the role `streaming` sends no `partial`. To get partials, name a streaming model in `preview_model` of the request, or in `previewModel` of the option `models`.
- `segment { session, source, start_ms, end_ms, text, words? }`. A part of the text that will not change. Times are from the start of the session.
- `source_gap { session, source, ms }`. A source lost audio, for example when the output device changed. The session goes on.
- `source_ended { session, source, reason }`. One source ended and the others go on. `reason` is `permission` or `failed`.
- `part_failed { session, source, start_ms, end_ms, message, code? }`. The final pass of one part failed, for example because of a wrong key. The session has no segment for that audio, and it goes on.
- `session_ended { session, text, code? }`. The session ended without a `stopSession` of your app.

See the [reference](reference.md#events) for every field.

## Send your own audio

For the source `client`, send 16 kHz mono samples of type `Int16Array` with `speech.sessionAudio`. Chunks of 1600 samples (100 ms) are a good size. `sessionAudio` does not wait for a reply. An error of the audio itself arrives as an event with `kind: "error"`.

Send some silence at the end, so the voice detection can end the last sentence.

<!-- check: verbatim packages/sdk/examples/run-a-live-session.ts -->
```ts
export async function sendAudio(speech: Speech, samples: Int16Array): Promise<string> {
	const { session } = await speech.startSession({ sources: ["client"] });
	for (let at = 0; at < samples.length; at += 1_600) {
		speech.sessionAudio(session, samples.subarray(at, at + 1_600));
	}
	speech.sessionAudio(session, new Int16Array(16_000));
	return speech.stopSession(session);
}
```

`EmbeddedSpeech.sessionAudio` takes a `Float32Array` with values from -1 to 1. `Speech.sessionAudio` takes an `Int16Array`.

## Keep the audio

Set `record_to` in the request to a folder. The SDK writes one 16 kHz mono WAV file for each source there: `mic.wav`, `system.wav`, and `client.wav`. An existing file with that name is replaced.

## Know the limits

- One live session runs at a time. A second `startSession` rejects with `busy`.
- While a session is active, Node does not end the process. See [How to handle errors and ends](ts-handle-errors-and-ends.md#know-what-keeps-node-running).
- The other limits are the same as in the [Rust page](run-a-live-session.md#know-the-limits): the echo rule for `mic` with `system`, and the free stream of the engine.
- To capture the audio with your own code, see [How to use your own capture](ts-use-your-own-capture.md).
