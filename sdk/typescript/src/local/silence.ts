/**
 * The silence limit of a live session (`silence_limit_ms`, a port of sayso-speech/src/live/silence.rs). Each source says how
 * long nobody has spoken in it. When every source has been silent for the limit, the watch runs its action one time. The
 * clock is the audio that a source heard, so a source that gets no audio does not count as silent.
 */
export class SilenceWatch {
	readonly #limitMs: number;
	readonly #action: () => boolean;
	#silentMs: number[] | undefined;

	/** `action` runs when the limit passes. It gives false for "not now, ask again at the next audio". */
	constructor(limitMs: number, sources: number, action: () => boolean) {
		this.#limitMs = limitMs;
		this.#action = action;
		this.#silentMs = new Array<number>(sources).fill(0);
	}

	/** Source `index` has heard `silentMs` of silence since its last speech. */
	heard(index: number, silentMs: number): void {
		const silent = this.#silentMs;
		if (!silent) return;
		silent[index] = silentMs;
		if (silent.some((ms) => ms < this.#limitMs)) return;
		if (this.#action()) this.#silentMs = undefined;
	}
}
