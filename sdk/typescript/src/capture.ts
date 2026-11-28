import { SaysoError } from "./errors.js";
import { findAudioHelper, helperError, helperPcm, requireAudioHelper, spawnHelper, type HelperMessage } from "./audio/helper.js";
import { i16ToFloats } from "./audio/wav.js";

/** What the session tells a source when it opens it. */
export interface OpenInfo {
	/** The system audio: the process of the app that asked, for the capture to leave out. */
	appPid?: number;
}

/** What a source can say when it opened. */
export interface Opened {
	/** What the capture leaves out, for the reply of `startSession` (`excludes`). */
	excludes?: string[];
}

/**
 * An audio source of the app, instead of the capture of the SDK: a test, or an app with its own capture. A live
 * session calls `open` when it starts; give the audio to the writer until `writer.closed`, or until the listener of
 * `writer.onClose` runs.
 *
 * To refuse, throw a `SaysoError` with the code `permission` (the user refused) or `unsupported` (this computer
 * cannot capture it). Another error fails the start of the session.
 */
export interface AudioSource {
	open(writer: AudioWriter, info: OpenInfo): void | Opened | Promise<void | Opened>;
}

/** Why a source ended before the session. */
export type WriterEndReason = "permission" | "failed";

/** What a session gives to the writer of a source. */
export interface WriterSink {
	audio(samples: Float32Array, timeMs: number | undefined): void;
	gap(ms: number): void;
	ended(reason: WriterEndReason, message: string | undefined): void;
}

/** Takes the audio of one opened source: 16 kHz mono samples from -1 to 1. */
export class AudioWriter {
	readonly #sink: WriterSink;
	#closed = false;
	#listeners: Array<() => void> = [];

	/** @internal */
	constructor(sink: WriterSink) {
		this.#sink = sink;
	}

	/** True when the session no longer takes audio from this writer. */
	get closed(): boolean {
		return this.#closed;
	}

	/**
	 * Give samples. `timeMs` is the time of the first sample on a clock that the sources of one session share; leave it
	 * out when the source has no such clock. False: the writer is closed.
	 */
	push(samples: Float32Array, timeMs?: number): boolean {
		if (this.#closed) return false;
		this.#sink.audio(samples, timeMs);
		return true;
	}

	/** The source lost this much audio, for example at a change of the device. False: the writer is closed. */
	gap(ms: number): boolean {
		if (this.#closed) return false;
		this.#sink.gap(ms);
		return true;
	}

	/** The source ended before the session. No more audio comes. */
	end(reason: WriterEndReason = "failed", message?: string): void {
		if (this.#closed) return;
		this.#sink.ended(reason, message);
		this._close();
	}

	/** Call `listener` when the session no longer needs the audio. Stop the capture then. */
	onClose(listener: () => void): void {
		if (this.#closed) queueMicrotask(listener);
		else this.#listeners.push(listener);
	}

	/** @internal */
	_close(): void {
		if (this.#closed) return;
		this.#closed = true;
		const listeners = this.#listeners;
		this.#listeners = [];
		for (const listener of listeners) {
			try {
				listener();
			} catch (e) {
				queueMicrotask(() => {
					throw e;
				});
			}
		}
	}
}

/** What the helper captures. */
export interface HelperSourceOptions {
	/** An input device id from `devices()`. Default: the default device. */
	device?: string;
	/** The helper program. Default `findAudioHelper()`. */
	helper?: string;
	/** More environment for the helper (for example `SAYSO_AUDIO_FAKE_WAV` in a test). */
	env?: Record<string, string>;
}

/** An `AudioSource` that captures with the audio helper `sayso-audio`: `mic` or the sound that the computer plays (`system`). */
export function helperSource(kind: "mic" | "system", options: HelperSourceOptions = {}): AudioSource {
	return {
		open(writer, info) {
			const program = options.helper ?? requireAudioHelper(`capturing the ${kind}`);
			const args = ["capture", "--source", kind];
			if (options.device) args.push("--device", options.device);
			if (kind === "system" && info.appPid !== undefined) args.push("--exclude-pid", String(info.appPid));
			return new Promise<Opened>((resolve, reject) => {
				let started = false;
				const handle = (message: HelperMessage): void => {
					switch (message.type) {
						case "started":
							started = true;
							resolve({ excludes: Array.isArray(message.excludes) ? message.excludes.map(String) : [] });
							break;
						case "audio":
							writer.push(i16ToFloats(helperPcm(message)), typeof message.t_ms === "number" ? message.t_ms : undefined);
							break;
						case "gap":
							if (typeof message.ms === "number") writer.gap(message.ms);
							break;
						case "ended":
							writer.end(message.reason === "permission" ? "permission" : "failed", typeof message.message === "string" ? message.message : undefined);
							break;
						case "error":
							if (!started) reject(helperError(message));
							else writer.end("failed", typeof message.message === "string" ? message.message : undefined);
							break;
					}
				};
				const process = spawnHelper(program, args, handle, options.env);
				writer.onClose(() => process.stop());
				void process.exited.then((code) => {
					if (!started) reject(SaysoError.of("unavailable", `the audio helper exited with ${code} before it started: ${process.stderr().join(" | ")}`));
					else writer.end("failed", `the audio helper exited with ${code}`);
				});
			});
		},
	};
}

export { findAudioHelper };
