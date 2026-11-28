// Jobs: the transcription of a file in the background (a port of sayso-speech/src/jobs): read the file, find speech,
// cut chunks, transcribe each chunk, and assign speakers.

import { BasicVad } from "../audio/vad.js";
import { readAudioFile } from "../audio/decode.js";
import { Audio } from "../audio/wav.js";
import type { Event, Segment, SpeechPart, TimestampLevel, Turn } from "../protocol/index.js";
import { EngineGate, hasFeature, type LocalEngine, type RawTurn, ModelUse } from "./engine.js";
import { SaysoError } from "../errors.js";

/** The message of a job that the app cancelled. */
export const JOB_CANCELLED = "The job was cancelled.";

/** The part of the progress for the transcription when a job also diarizes. */
export const TRANSCRIPTION_SHARE = 0.9;
const MAX_CHUNK_MS = 30_000;
const MAX_CHUNK_MS_WITHOUT_TIMES = 10_000;
const MAX_JOIN_PAUSE_MS = 3_000;
const PAD_MS = 200;
const VAD_WINDOW_MS = 10 * 60 * 1000;
const REACHES_END_MS = 1_000;
const SMALL_SPEAKER_SHARE = 0.02;
const SMALL_SPEAKER_MS = 10_000;

/** The VAD of a job: Basic (energy) or a model. */
export interface VadRun {
	basic: boolean;
	model?: string;
}

/** A job that was checked: its models are known. */
export interface JobSpec {
	path: string;
	model: string;
	timestamps: TimestampLevel;
	vad: VadRun;
	diarization?: string;
	language: string;
	speakers?: number;
}

/** The models of the job, in the order of loading. */
export function jobModels(job: JobSpec): string[] {
	const out: string[] = [];
	if (!job.vad.basic && job.vad.model) out.push(job.vad.model);
	out.push(job.model);
	if (job.diarization) out.push(job.diarization);
	return out;
}

/** A part of the audio that one engine call transcribes. */
export interface Chunk {
	start: number;
	end: number;
}

const satSub = (a: number, b: number): number => (a < b ? 0 : a - b);

/** Join the speech parts into chunks of at most `maxMs`, with a pad. */
export function planChunks(parts: SpeechPart[], lenMs: number, maxMs: number): Chunk[] {
	const ps = parts
		.map((p) => ({ start: Math.min(p.start_ms, lenMs), end: Math.min(p.end_ms, lenMs) }))
		.filter((p) => p.end > p.start)
		.sort((a, b) => a.start - b.start);
	const joined: Chunk[] = [];
	for (const p of ps) {
		const last = joined.at(-1);
		if (last && p.start <= last.end + MAX_JOIN_PAUSE_MS && Math.max(p.end, last.end) - last.start <= maxMs) {
			last.end = Math.max(last.end, p.end);
			continue;
		}
		joined.push({ ...p });
	}
	const padded: Chunk[] = [];
	joined.forEach((c, i) => {
		const room = Math.floor(satSub(maxMs, c.end - c.start) / 2);
		const pad = Math.min(PAD_MS, room);
		const prevEnd = padded.at(-1)?.end ?? 0;
		const nextStart = i + 1 < joined.length ? (joined[i + 1] as Chunk).start : lenMs;
		padded.push({ start: Math.max(satSub(c.start, pad), prevEnd), end: Math.min(c.end + pad, nextStart, lenMs) });
	});
	return padded;
}

/** Give the turns of speakers with little speech to the speaker who overlaps them most. */
export function foldSmallSpeakers(turns: RawTurn[]): RawTurn[] {
	const speech = new Map<number, number>();
	for (const t of turns) speech.set(t.speaker, (speech.get(t.speaker) ?? 0) + satSub(t.end_ms, t.start_ms));
	let all = 0;
	for (const ms of speech.values()) all += ms;
	let largest: number | undefined;
	for (const [s, ms] of speech) {
		const best = largest === undefined ? undefined : (speech.get(largest) as number);
		if (best === undefined || ms > best || (ms === best && s < (largest as number))) largest = s;
	}
	const small = (s: number): boolean => {
		const ms = speech.get(s) ?? 0;
		return (largest === undefined || s !== largest) && ms < SMALL_SPEAKER_MS && ms < SMALL_SPEAKER_SHARE * all;
	};
	const kept = turns.filter((t) => !small(t.speaker));
	return turns
		.map((t) => {
			if (!small(t.speaker)) return t;
			const speaker = bestSpeaker(t.start_ms, t.end_ms, kept);
			return speaker === undefined ? t : { ...t, speaker };
		})
		.map((t, i) => ({ t, i }))
		.sort((a, b) => a.t.start_ms - b.t.start_ms || a.t.end_ms - b.t.end_ms || a.i - b.i)
		.map(({ t }) => t);
}

/** The speaker who speaks most in [start, end], or the nearest one. */
export function bestSpeaker(start: number, end: number, turns: RawTurn[]): number | undefined {
	const overlap = new Map<number, number>();
	let nearestGap = 0;
	let nearest: number | undefined;
	for (const t of turns) {
		const shared = satSub(Math.min(end, t.end_ms), Math.max(start, t.start_ms));
		if (shared > 0) overlap.set(t.speaker, (overlap.get(t.speaker) ?? 0) + shared);
		const gap = Math.max(satSub(t.start_ms, end), satSub(start, t.end_ms));
		if (nearest === undefined || gap < nearestGap || (gap === nearestGap && t.speaker < nearest)) {
			nearestGap = gap;
			nearest = t.speaker;
		}
	}
	let most: number | undefined;
	let mostMs = 0;
	for (const [s, ms] of overlap) {
		if (most === undefined || ms > mostMs || (ms === mostMs && s < most)) {
			most = s;
			mostMs = ms;
		}
	}
	return most ?? nearest;
}

/** The speaker of each segment (undefined when there is none). */
export function assignSpeakers(segments: Array<[number, number]>, turns: RawTurn[]): Array<number | undefined> {
	return segments.map(([start, end]) => bestSpeaker(start, end, turns));
}

/** Turns of the segments, with labels "1", "2", ... in the order in which the speakers first speak. */
export function segmentTurns(segments: Array<[number, number]>, speakers: Array<number | undefined>): Turn[] {
	const labels = new Map<number, string>();
	const turns: Array<{ speaker: number; start: number; end: number }> = [];
	let last: number | undefined;
	segments.forEach(([start, end], i) => {
		const sp = speakers[i];
		const tail = turns.at(-1);
		if (sp !== undefined && tail && last !== undefined && last === sp) tail.end = Math.max(tail.end, end);
		else if (sp !== undefined) turns.push({ speaker: sp, start, end });
		last = sp;
	});
	return turns.map((t) => {
		let label = labels.get(t.speaker);
		if (label === undefined) {
			label = String(labels.size + 1);
			labels.set(t.speaker, label);
		}
		return { start_ms: t.start, end_ms: t.end, speaker: label };
	});
}

async function basicParts(audio: Audio): Promise<SpeechPart[]> {
	const vad = new BasicVad();
	const parts: SpeechPart[] = [];
	const rate = 16000;
	for (let i = 0; i < audio.samples.length; i += rate) {
		parts.push(...vad.accept(audio.floats((i * 1000) / rate, ((i + rate) * 1000) / rate)));
	}
	parts.push(...vad.finish());
	return parts;
}

/** A job that stopped: cancelled, or failed with a message. */
class Stopped extends Error {
	constructor(
		readonly cancelled: boolean,
		message: string,
	) {
		super(message);
	}
}

/** The running job as the runner sees it. */
export interface JobRun {
	readonly id: number;
	/** True after a cancel. */
	cancelled(): boolean;
	/** Send an event of the job. After `job_done` or `job_failed` the job sends nothing more. */
	event(event: Event): void;
	/** Wait for the live parts in the engine. False after a cancel. */
	beforeEngineCall(): Promise<boolean>;
}

/** Runs jobs on the engine of the app. */
export class JobRunner {
	readonly #engine: LocalEngine;
	readonly #models: ModelUse | undefined;
	readonly #hold: ((models: string[]) => () => void) | undefined;
	readonly #vadWindowMs: number;

	constructor(engine: LocalEngine, options: { hold?: (models: string[]) => () => void; models?: ModelUse; vadWindowMs?: number } = {}) {
		this.#engine = engine;
		this.#models = options.models;
		this.#hold = options.hold;
		this.#vadWindowMs = options.vadWindowMs ?? VAD_WINDOW_MS;
	}

	/** Run a job to its end. */
	async run(job: JobSpec, run: JobRun): Promise<void> {
		try {
			const text = await this.#runJob(job, run);
			run.event({ type: "job_done", job: run.id, text });
		} catch (e) {
			if (e instanceof Stopped) {
				run.event({ type: "job_failed", job: run.id, message: e.cancelled ? JOB_CANCELLED : e.message });
			} else {
				const error = SaysoError.from(e);
				run.event({ type: "job_failed", job: run.id, message: error.message, ...(error.code ? { code: error.code } : {}) });
			}
		}
	}

	async #before(run: JobRun): Promise<void> {
		if (!(await run.beforeEngineCall())) throw new Stopped(true, JOB_CANCELLED);
	}

	async #runJob(job: JobSpec, run: JobRun): Promise<string> {
		let audio: Audio;
		try {
			audio = await readAudioFile(job.path);
		} catch (e) {
			throw new Stopped(false, SaysoError.from(e).message);
		}
		if (run.cancelled()) throw new Stopped(true, JOB_CANCELLED);
		const release = this.#hold?.(jobModels(job)) ?? this.#models?.hold(jobModels(job));
		try {
			for (const m of jobModels(job)) {
				await this.#before(run);
				try {
					if (!this.#engine.isUsable(m)) await this.#engine.load(m);
				} catch (e) {
					throw new Stopped(false, `cannot load the model ${m}: ${SaysoError.from(e).message}`);
				}
			}
			const parts = await this.#speechParts(audio, job.vad, run);
			const maxMs = job.timestamps === "none" ? MAX_CHUNK_MS_WITHOUT_TIMES : MAX_CHUNK_MS;
			const chunks = planChunks(parts, audio.lenMs, maxMs);
			const share = job.diarization ? TRANSCRIPTION_SHARE : 1;
			const lenMs = Math.max(audio.lenMs, 1);
			const times: Array<[number, number]> = [];
			const texts: string[] = [];
			for (const [i, c] of chunks.entries()) {
				await this.#before(run);
				for (const s of await this.#transcribe(audio, c, job)) {
					times.push([s.start_ms, s.end_ms]);
					texts.push(s.text);
					run.event({ type: "segment", job: run.id, start_ms: s.start_ms, end_ms: s.end_ms, text: s.text, ...(s.words ? { words: s.words } : {}) });
				}
				const done = i + 1 === chunks.length ? lenMs : Math.min(c.end, lenMs);
				run.event({ type: "job_progress", job: run.id, fraction: (share * done) / lenMs });
			}
			if (job.diarization) {
				await this.#before(run);
				let turns: RawTurn[];
				try {
					turns = await this.#engine.diarize(job.diarization, audio.allFloats(), job.speakers);
				} catch (e) {
					throw new Stopped(false, `Diarization failed: ${SaysoError.from(e).message}`);
				}
				if (job.speakers === undefined) turns = foldSmallSpeakers(turns);
				run.event({ type: "speakers", job: run.id, turns: segmentTurns(times, assignSpeakers(times, turns)) });
			}
			return texts.join(" ");
		} finally {
			release?.();
		}
	}

	async #transcribe(audio: Audio, c: Chunk, job: JobSpec): Promise<Segment[]> {
		const samples = audio.floats(c.start, c.end);
		try {
			if (job.timestamps === "none") {
				const text = (await this.#engine.transcribe(samples, job.model, job.language)).trim();
				return text === "" ? [] : [{ start_ms: c.start, end_ms: c.end, text }];
			}
			const found = hasFeature(this.#engine.features(), "segments")
				? await this.#engine.transcribeSegments(samples, job.model, job.language)
				: [{ start_ms: 0, end_ms: Math.floor((samples.length * 1000) / 16000), text: (await this.#engine.transcribe(samples, job.model, job.language)).trim() }];
			const out: Segment[] = [];
			for (const s of found) {
				const text = s.text.trim();
				if (text === "") continue;
				const seg: Segment = { start_ms: s.start_ms + c.start, end_ms: s.end_ms + c.start, text };
				if (s.words) seg.words = s.words.map((w) => ({ start_ms: w.start_ms + c.start, end_ms: w.end_ms + c.start, text: w.text }));
				out.push(seg);
			}
			return out;
		} catch (e) {
			throw new Stopped(false, `Transcription failed: ${SaysoError.from(e).message}`);
		}
	}

	async #speechParts(audio: Audio, vad: VadRun, run: JobRun): Promise<SpeechPart[]> {
		if (vad.basic || !vad.model) return basicParts(audio);
		const lenMs = audio.lenMs;
		const window = Math.max(this.#vadWindowMs, 1);
		const parts: SpeechPart[] = [];
		let start = 0;
		while (start < lenMs) {
			await this.#before(run);
			const end = Math.min(start + window, lenMs);
			let found: SpeechPart[];
			try {
				found = await this.#engine.detectSpeech(vad.model, audio.floats(start, end));
			} catch (e) {
				throw new Stopped(false, `Voice detection failed: ${SaysoError.from(e).message}`);
			}
			let shifted = found.map((p) => ({ start_ms: start + p.start_ms, end_ms: Math.min(start + p.end_ms, end) }));
			let next = end;
			const last = shifted.at(-1);
			if (end < lenMs && last && last.end_ms + REACHES_END_MS >= end && last.start_ms > start) {
				next = last.start_ms;
				shifted = shifted.slice(0, -1);
			}
			parts.push(...shifted);
			start = next;
		}
		return parts;
	}
}

// ---- The queue: one job at a time, in the order they came ----

interface Queued {
	id: number;
	job: JobSpec;
	sink: (event: Event) => void;
}

class RunningJob implements JobRun {
	cancelFlag = false;
	ended = false;

	constructor(
		readonly id: number,
		readonly sink: (event: Event) => void,
		readonly gate: EngineGate | undefined,
	) {}

	cancelled(): boolean {
		return this.cancelFlag;
	}

	event(event: Event): void {
		if (this.ended) return;
		if (event.type === "job_done" || event.type === "job_failed") this.ended = true;
		this.sink(event);
	}

	beforeEngineCall(): Promise<boolean> {
		return this.gate ? this.gate.waitForJob(() => this.cancelFlag) : Promise.resolve(!this.cancelFlag);
	}

	cancel(message: string): boolean {
		if (this.cancelFlag) return false;
		this.cancelFlag = true;
		this.event({ type: "job_failed", job: this.id, message });
		return true;
	}
}

/** Runs one job at a time, in the order they came. */
export class JobQueue {
	readonly #runner: JobRunner;
	readonly #gate: EngineGate | undefined;
	#waiting: Queued[] = [];
	#running: RunningJob | undefined;
	#next = 0;
	#closed = false;
	#working = false;

	constructor(runner: JobRunner, gate?: EngineGate) {
		this.#runner = runner;
		this.#gate = gate;
	}

	/** Queue a job. Its events go to `sink`, the first (`job_progress` 0) after this returns. */
	submit(job: JobSpec, sink: (event: Event) => void): number {
		if (this.#closed) throw SaysoError.of("connection_lost", "the Speech is closed");
		const id = ++this.#next;
		this.#waiting.push({ id, job, sink });
		setImmediate(() => {
			sink({ type: "job_progress", job: id, fraction: 0 });
			void this.#work();
		});
		return id;
	}

	/** True when a job runs or waits. */
	get busy(): boolean {
		return this.#running !== undefined || this.#waiting.length > 0;
	}

	/** Cancel a job. It ends with `job_failed`. False when there is no such job. */
	cancel(job: number): boolean {
		const queued = this.#waiting.findIndex((w) => w.id === job);
		if (queued >= 0) {
			const [w] = this.#waiting.splice(queued, 1) as [Queued];
			w.sink({ type: "job_failed", job: w.id, message: JOB_CANCELLED });
			return true;
		}
		if (this.#running?.id === job) {
			const cancelled = this.#running.cancel(JOB_CANCELLED);
			this.#gate?.wake();
			return cancelled;
		}
		return false;
	}

	/** End every job: with `message` and `code` for the job that runs and for the ones that wait. */
	endAll(message: string, code?: string): void {
		const waiting = this.#waiting;
		this.#waiting = [];
		for (const w of waiting) w.sink({ type: "job_failed", job: w.id, message, ...(code ? { code } : {}) });
		this.#running?.cancel(message);
		this.#gate?.wake();
	}

	/** Cancel each job and stop the worker. */
	close(): void {
		this.#closed = true;
		this.endAll(JOB_CANCELLED);
	}

	/** Resolves when no job runs. */
	async idle(): Promise<void> {
		while (this.#working) await new Promise((resolve) => setTimeout(resolve, 5));
	}

	async #work(): Promise<void> {
		if (this.#working) return;
		this.#working = true;
		try {
			for (let next = this.#waiting.shift(); next; next = this.#waiting.shift()) {
				const running = new RunningJob(next.id, next.sink, this.#gate);
				this.#running = running;
				await this.#runner.run(next.job, running);
				this.#running = undefined;
			}
		} finally {
			this.#running = undefined;
			this.#working = false;
		}
	}
}
