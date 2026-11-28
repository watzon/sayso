// The echo rule: without headphones the microphone hears the call too, so a session with the sources `mic` and `system`
// drops a microphone segment that a system segment covers in time and in words (a port of sayso-speech/src/live/echo.rs).
//
// A system segment can come after the microphone segment that it covers, because each source has its own final pass.
// So `EchoGate` holds each microphone segment until the system source has transcribed the audio of that time, and
// decides then. The segments of the system go out at once.

import { isUnspaced } from "../protocol/codec.js";
import type { Segment } from "../protocol/index.js";

/** The part of the words of a microphone segment that must be in the system audio of the same time. */
export const ECHO_LIMIT = 0.6;
/** A microphone segment with this many words or fewer is an echo only when it is also most of the system words of its time. */
export const ECHO_SHORT_WORDS = 3;
export const ECHO_SHORT_LIMIT = 0.5;
/** How far the times of the two sources can differ. */
export const ECHO_TIME_SLACK_MS = 1_000;
/** This part of a microphone segment must be in the time of system segments. */
export const ECHO_MIN_TIME_COVER = 0.5;
/** The most system audio that a microphone segment waits. */
export const ECHO_MAX_HOLD_MS = 60_000;
/** How much system text the gate keeps, in audio time. */
export const ECHO_KEEP_SYSTEM_MS = 600_000;

const ALNUM = /^[\p{Alphabetic}\p{N}]$/u;

/** The words of a text, in lower case and without punctuation. A character of a script without spaces counts as one word. */
export function echoWords(text: string): string[] {
	const out: string[] = [];
	let word = "";
	const flush = (): void => {
		if (word !== "") {
			out.push(word);
			word = "";
		}
	};
	for (const c of text) {
		if (isUnspaced(c)) {
			flush();
			out.push(c);
		} else if (ALNUM.test(c)) {
			word += c.toLowerCase();
		} else if (c !== "'" && c !== "’") {
			flush();
		}
	}
	flush();
	return out;
}

/** True for the same word, or a word with one wrong letter, as two transcripts of one sound often have. */
export function sameWord(a: string, b: string): boolean {
	if (a === b) return true;
	const x = [...a];
	const y = [...b];
	if (Math.min(x.length, y.length) < 4 || Math.abs(x.length - y.length) > 1) return false;
	let start = 0;
	while (start < x.length && start < y.length && x[start] === y[start]) start++;
	let end = 0;
	while (end < x.length - start && end < y.length - start && x[x.length - 1 - end] === y[y.length - 1 - end]) end++;
	return Math.max(x.length, y.length) - start - end <= 1;
}

/** The number of words of `a` that are in `b` in the same order. */
export function commonWords(a: string[], b: string[]): number {
	const row = new Array<number>(b.length + 1).fill(0);
	for (const word of a) {
		let diagonal = 0;
		b.forEach((other, j) => {
			const up = row[j + 1] as number;
			row[j + 1] = sameWord(word, other) ? diagonal + 1 : Math.max(up, row[j] as number);
			diagonal = up;
		});
	}
	return row[b.length] as number;
}

/** The decision on the words alone. */
export function wordsEcho(mic: string[], system: string[], limit: number): boolean {
	if (mic.length === 0 || system.length === 0) return false;
	const common = commonWords(mic, system);
	return common / mic.length >= limit && (mic.length > ECHO_SHORT_WORDS || common / system.length >= ECHO_SHORT_LIMIT);
}

interface TimedWord {
	at: number;
	text: string;
}

interface SystemSegment {
	start: number;
	end: number;
	words: TimedWord[];
}

function systemSegment(s: Segment, clockOffset: number): SystemSegment {
	const start = s.start_ms + clockOffset;
	const end = s.end_ms + clockOffset;
	const words: TimedWord[] = [];
	if (s.words && s.words.length > 0) {
		for (const w of s.words) for (const text of echoWords(w.text)) words.push({ at: w.start_ms + clockOffset, text });
	} else {
		const all = echoWords(s.text);
		const count = Math.max(all.length, 1);
		all.forEach((text, i) => words.push({ at: start + Math.trunc(((end - start) * i) / count), text }));
	}
	return { start, end, words };
}

/** True when the system segments cover a microphone segment in time and in words. */
export function covered(start: number, end: number, mic: string[], system: SystemSegment[]): boolean {
	const near = system.filter((s) => s.start - ECHO_TIME_SLACK_MS < end && s.end + ECHO_TIME_SLACK_MS > start);
	const spans = near.map((s): [number, number] => [Math.max(s.start - ECHO_TIME_SLACK_MS, start), Math.min(s.end + ECHO_TIME_SLACK_MS, end)]).sort((a, b) => a[0] - b[0] || a[1] - b[1]);
	let inTime = 0;
	let reached = start;
	for (const [from, to] of spans) {
		inTime += Math.max(to - Math.max(from, reached), 0);
		reached = Math.max(reached, to);
	}
	if (inTime < Math.max(end - start, 1) * ECHO_MIN_TIME_COVER) return false;
	const heard: string[] = [];
	for (const s of near) {
		for (const w of s.words) if (w.at >= start - ECHO_TIME_SLACK_MS && w.at <= end + ECHO_TIME_SLACK_MS) heard.push(w.text);
	}
	return wordsEcho(mic, heard, ECHO_LIMIT);
}

interface Held {
	segment: Segment;
	start: number;
	end: number;
}

/** Decides on the microphone segments of one session. */
export class EchoGate {
	readonly #emit: (segment: Segment) => void;
	#system: SystemSegment[] = [];
	#doneUntil = Number.NEGATIVE_INFINITY;
	#heard = Number.NEGATIVE_INFINITY;
	#ended = false;
	#held: Held[] = [];
	#kept: Segment[] = [];

	/** `emit` sends the event of a microphone segment that is not an echo. */
	constructor(emit: (segment: Segment) => void) {
		this.#emit = emit;
	}

	/** A microphone segment, with `clockOffset` from its times to the capture clock. */
	micSegment(s: Segment, clockOffset: number): void {
		this.#held.push({ segment: s, start: s.start_ms + clockOffset, end: s.end_ms + clockOffset });
		this.#release();
	}

	/** A segment of the system source, after its event went out. */
	systemSegment(s: Segment, clockOffset: number): void {
		this.#system.push(systemSegment(s, clockOffset));
		const newest = (this.#system.at(-1) as SystemSegment).end;
		this.#system = this.#system.filter((x) => x.end >= newest - ECHO_KEEP_SYSTEM_MS);
	}

	/** The system final pass has finished all the speech before `clockMs`. */
	systemDoneUntil(clockMs: number): void {
		this.#doneUntil = Math.max(this.#doneUntil, clockMs);
		this.#release();
	}

	/** The system source has heard the audio up to `clockMs`. */
	systemHeard(clockMs: number): void {
		this.#heard = Math.max(this.#heard, clockMs);
		this.#release();
	}

	/** No more system segments come. It can be called more than once. */
	systemEnded(): void {
		this.#ended = true;
		this.#release();
	}

	/** The microphone segments that stayed. */
	keptSegments(): Segment[] {
		return [...this.#kept];
	}

	#release(): void {
		for (let front = this.#held[0]; front; front = this.#held[0]) {
			const ready = this.#ended || this.#doneUntil >= front.end + ECHO_TIME_SLACK_MS || this.#heard >= front.end + ECHO_MAX_HOLD_MS;
			if (!ready) return;
			this.#held.shift();
			if (covered(front.start, front.end, echoWords(front.segment.text), this.#system)) continue;
			this.#emit(front.segment);
			this.#kept.push(front.segment);
		}
	}
}
