import { bridge, native, type BridgeFn } from "./native.js";

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
 * An audio source of the app, instead of the capture of the SDK: a test, or an app with
 * its own capture. A live session calls `open` when it starts; give the audio to the
 * writer until `writer.closed`, or until the listener of `writer.onClose` runs.
 *
 * To refuse, throw a `SaysoError` with the code `permission` (the user refused) or
 * `unsupported` (this computer cannot capture it). Another error fails the start of the session.
 */
export interface AudioSource {
	open(writer: AudioWriter, info: OpenInfo): void | Opened | Promise<void | Opened>;
}

/** Why a source ended before the session. */
export type WriterEndReason = "permission" | "failed";

/** Takes the audio of one opened source: 16 kHz mono samples from -1 to 1. */
export class AudioWriter {
	readonly #id: number;
	#closed = false;
	#listeners: Array<() => void> = [];

	/** @internal */
	constructor(id: number) {
		this.#id = id;
	}

	/** True when the session no longer takes audio from this writer. */
	get closed(): boolean {
		return this.#closed;
	}

	/**
	 * Give samples. `timeMs` is the time of the first sample on a clock that the sources of
	 * one session share; leave it out when the source has no such clock. False: the writer is closed.
	 */
	push(samples: Float32Array, timeMs?: number): boolean {
		if (this.#closed) return false;
		const ok = native().writerPush(this.#id, samples, timeMs ?? null);
		if (!ok) this.#close();
		return ok;
	}

	/** The source lost this much audio, for example at a change of the device. False: the writer is closed. */
	gap(ms: number): boolean {
		if (this.#closed) return false;
		const ok = native().writerGap(this.#id, ms);
		if (!ok) this.#close();
		return ok;
	}

	/** The source ended before the session. No more audio comes. */
	end(reason: WriterEndReason = "failed", message?: string): void {
		if (this.#closed) return;
		native().writerEnd(this.#id, reason, message ?? null);
		this.#close();
	}

	/** Call `listener` when the session no longer needs the audio. Stop the capture then. */
	onClose(listener: () => void): void {
		if (this.#closed) {
			queueMicrotask(listener);
		} else {
			this.#listeners.push(listener);
		}
	}

	/** @internal */
	_closed(): void {
		this.#close();
	}

	#close(): void {
		if (this.#closed) return;
		this.#closed = true;
		writers.delete(this.#id);
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

const writers = new Map<number, AudioWriter>();

/** The bridge of a source: Rust calls `open` and `close`. */
export function sourceBridge(source: AudioSource): BridgeFn {
	return bridge({
		open: async (json) => {
			const { writer: id, appPid } = json as { writer: number; appPid?: number | null };
			const writer = new AudioWriter(id);
			writers.set(id, writer);
			const info: OpenInfo = {};
			if (typeof appPid === "number") info.appPid = appPid;
			try {
				const opened = await source.open(writer, info);
				return { excludes: opened?.excludes ?? [] };
			} catch (e) {
				writers.delete(id);
				throw e;
			}
		},
		close: (json) => {
			const { writer } = json as { writer: number };
			writers.get(writer)?._closed();
		},
	});
}
