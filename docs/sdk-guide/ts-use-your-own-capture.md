# How to use your own capture in TypeScript

Use an `AudioSource` when your app captures the audio itself, or in a test. The SDK then takes the audio from your code and not from the microphone or the system audio.

This works in local mode only. In Sayso mode, Sayso captures the audio, and the option `capture` has no effect.

You need to know how to start a live session (see [How to run a live session](ts-run-a-live-session.md)).

## Write a source

A source is an object with an `open` method. A live session calls `open` when it starts with that source. `open` gets an `AudioWriter` and an `OpenInfo`. Give audio to the writer until the session no longer needs it.

The audio is 16 kHz mono samples in a `Float32Array`, with values from -1 to 1. `writer.push(samples, timeMs)` takes them. `timeMs` is the time of the first sample on a clock that the sources of one session share. Leave it out when your source has no such clock. `push` returns `false` when the writer is closed.

`writer.onClose(listener)` calls your function when the session no longer needs the audio. Stop your capture there. `writer.closed` is `true` after that.

<!-- check: verbatim packages/sdk/examples/own-capture.ts -->
```ts
export const microphone: AudioSource = {
	open(writer: AudioWriter) {
		const stop = myCapture.start(
			(samples, timeMs) => {
				writer.push(samples, timeMs);
			},
			(message) => writer.end("failed", message),
		);
		writer.onClose(stop);
	},
};
```

If your capture has `Int16Array` samples, convert them first:

<!-- check: verbatim packages/sdk/examples/own-capture.ts -->
```ts
export function toFloat32(samples: Int16Array): Float32Array {
	return Float32Array.from(samples, (sample) => sample / 32_768);
}
```

## Say what the capture leaves out

For the system audio, `info.appPid` is the process id of your app. The capture can leave out the sound of that process. Return `{ excludes }` from `open`. `startSession` gives these names as `excludes` in its reply.

## Refuse

If the user refused the capture, throw a `SaysoError` with the code `permission` from `open`. If this computer cannot capture the audio, use the code `unsupported`. `startSession` rejects with the same code. Another error fails the start of the session.

<!-- check: verbatim packages/sdk/examples/own-capture.ts -->
```ts
export const systemAudio: AudioSource = {
	async open(writer: AudioWriter, info: OpenInfo) {
		if (!(await permissionIsGranted())) {
			throw new SaysoError({ code: "permission", message: "The user did not allow the capture of the system audio." });
		}
		const stop = myCapture.start(
			(samples, timeMs) => {
				writer.push(samples, timeMs);
			},
			(message) => writer.end("failed", message),
			{ excludePid: info.appPid },
		);
		writer.onClose(stop);
		return { excludes: info.appPid === undefined ? [] : ["app"] };
	},
};
```

## Report a gap or the end

If your source lost audio, for example when the device changed, call `writer.gap(ms)`. The session goes on, and the recording has `ms` of silence there. `gap` returns `false` when the writer is closed.

<!-- check: verbatim packages/sdk/examples/own-capture.ts -->
```ts
export function markGap(writer: AudioWriter, ms: number): void {
	if (!writer.gap(ms)) console.error("The session no longer takes audio.");
}
```

If the capture fails and does not come back, call `writer.end("failed", message)`. If the user took the permission back, call `writer.end("permission")`. The session sends `source_ended` with that reason. The other sources go on.

## Give the sources to the SDK

Pass the sources in the option `capture`. A source that you leave out is the real capture of the SDK.

<!-- check: verbatim packages/sdk/examples/own-capture.ts -->
```ts
export function connectWithCapture(engine: Engine): Promise<Speech> {
	return Speech.connect({
		mode: "local",
		app: { name: "Notetaker" },
		engine,
		capture: { microphone, systemAudio },
	});
}
```
