// The basic VAD: speech by its level above the noise floor of the last 5 s (a port of basic_vad.rs).

const MIN_SPEECH_MS = 250;
const MIN_PAUSE_MS = 500;
const MAX_PART_MS = 30_000;
const FRAME_MS = 10;
const FRAME = (16000 * FRAME_MS) / 1000;
const FLOOR_MS = 5_000;
const MARGIN_DB = 10;
const QUIETEST_DB = -55;
const SILENCE_DB = -100;

/** A part of audio with speech, in milliseconds from its start. */
export interface Part {
	start_ms: number;
	end_ms: number;
}

function levelDb(frame: Float32Array): number {
	let power = 0;
	for (const s of frame) power += Math.fround(s * s);
	power = Math.fround(power / frame.length);
	return power > 0 ? Math.max(10 * Math.log10(power), SILENCE_DB) : SILENCE_DB;
}

const framePart = (start: number, end: number): Part => ({ start_ms: start * FRAME_MS, end_ms: end * FRAME_MS });

export class BasicVad {
	#pending = new Float32Array(0);
	#frames = 0;
	#floor: Array<[number, number]> = [];
	#open = false;
	#openStart = 0;
	#continued = false;
	#voiced = 0;
	#speechEnd = 0;

	/** Take 16 kHz samples and give the parts that ended. */
	accept(samples: Float32Array): Part[] {
		const parts: Part[] = [];
		const all = new Float32Array(this.#pending.length + samples.length);
		all.set(this.#pending);
		all.set(samples, this.#pending.length);
		const n = Math.floor(all.length / FRAME);
		for (let i = 0; i < n; i++) this.#frame(levelDb(all.subarray(i * FRAME, (i + 1) * FRAME)), parts);
		this.#pending = all.slice(n * FRAME);
		return parts;
	}

	/** The part that is still open. */
	finish(): Part[] {
		const parts: Part[] = [];
		this.#close(parts);
		return parts;
	}

	/** True while a part is open. */
	get speaking(): boolean {
		return this.#open;
	}

	#frame(level: number, parts: Part[]): void {
		const index = this.#frames++;
		while (this.#floor.length > 0 && (this.#floor.at(-1) as [number, number])[1] >= level) this.#floor.pop();
		this.#floor.push([index, level]);
		while (this.#floor.length > 0 && (this.#floor[0] as [number, number])[0] + FLOOR_MS / FRAME_MS <= index) this.#floor.shift();
		const floor = this.#floor.length > 0 ? (this.#floor[0] as [number, number])[1] : level;
		const speech = level > Math.max(floor + MARGIN_DB, QUIETEST_DB);
		if (speech) {
			if (!this.#open) {
				this.#open = true;
				this.#openStart = index;
				this.#continued = false;
				this.#voiced = 0;
			}
			this.#voiced++;
			this.#speechEnd = index + 1;
			this.#cutLongPart(parts);
		} else if (this.#open && this.#frames - this.#speechEnd >= MIN_PAUSE_MS / FRAME_MS) {
			this.#close(parts);
		}
	}

	#cutLongPart(parts: Part[]): void {
		const maxFrames = MAX_PART_MS / FRAME_MS;
		while (this.#open) {
			const start = this.#openStart;
			if (this.#speechEnd < start + maxFrames + MIN_SPEECH_MS / FRAME_MS) break;
			parts.push(framePart(start, start + maxFrames));
			this.#openStart = start + maxFrames;
			this.#continued = true;
			this.#voiced = 0;
		}
	}

	#close(parts: Part[]): void {
		if (!this.#open) return;
		this.#open = false;
		const start = this.#openStart;
		const end = this.#speechEnd;
		if (!this.#continued && this.#voiced < MIN_SPEECH_MS / FRAME_MS) return;
		const maxFrames = MAX_PART_MS / FRAME_MS;
		if (end - start > maxFrames) {
			const cut = Math.min(start + maxFrames, end - MIN_SPEECH_MS / FRAME_MS);
			parts.push(framePart(start, cut), framePart(cut, end));
		} else {
			parts.push(framePart(start, end));
		}
	}
}

/** All the speech in `samples`, with a new `BasicVad`. */
export function detectSpeech(samples: Float32Array): Part[] {
	const vad = new BasicVad();
	return [...vad.accept(samples), ...vad.finish()];
}
