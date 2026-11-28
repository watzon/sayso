// Live sessions: audio that is transcribed for an app while it arrives (a port of sayso-speech/src/live).
//
// A session has one or more sources (`mic`, `system`, `client`), and each source is a pipeline of its own: the audio goes
// to the recording (a WAV file), to a preview stream of the engine when the session has a streaming model (`partial`
// events), and to a VAD (a VAD session of the engine, or the basic VAD). Each speech that ends is a part, and the final
// model transcribes it (`segment` events, with times from the start of the session). `stop` ends the audio, waits for the
// last parts, runs diarization on the recording of each source, sends `speakers` events, and only then gives the text, so
// the text comes after the last events.
//
// The times of all sources are from the start of the session. A source that starts late or loses audio gets silence there,
// so its recording and its times stay on the time of the other sources.

import { mkdirSync, rmSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import { tmpdir } from "node:os";
import { AudioWriter, type AudioSource, type WriterEndReason } from "../capture.js";
import { BasicVad } from "../audio/vad.js";
import { WavWriter, readLiveWav } from "../audio/wav.js";
import { SaysoError } from "../errors.js";
import type { ErrorCode, Event, Segment, Source, Turn } from "../protocol/index.js";
import { EchoGate } from "./echo.js";
import { EngineGate, engineCode, hasFeature, type LocalEngine, type LocalEngineEvent } from "./engine.js";
import { assignSpeakers, foldSmallSpeakers, segmentTurns, type VadRun } from "./jobs.js";
import { SilenceWatch } from "./silence.js";

/** The first engine session id of live sessions: 2^40, under 2^53, so an engine that reads JSON numbers as doubles keeps the ids exact. */
export const ENGINE_SESSION_BASE = 2 ** 40;

const RATE = 16000;
/** Audio before and after the speech of a part, so the first and last words are complete. */
const PAD_MS = 200;
/** How much audio the intake keeps while nobody speaks, for a speech start that the engine reports late. */
const LOOKBACK_MS = 10_000;
/** The most silence that a source that starts late or loses audio gets. */
const MAX_SILENCE_MS = 60_000;
/** How long after the audio a VAD can report the start of a speech. */
const ECHO_VAD_LAG_MS = 1_000;
/** How often the system source tells the echo rule how far it is. */
const ECHO_MARK_MS = 250;

const sampleAt = (ms: number): number => Math.floor((ms * RATE) / 1000);
const satSub = (a: number, b: number): number => (a < b ? 0 : a - b);

/** The options of a live session, with the models chosen. */
export interface SessionSpec {
	sources: Source[];
	finalModel: string;
	previewModel?: string;
	vad: VadRun;
	language: string;
	diarization?: string;
	recordTo?: string;
	silenceLimitMs?: number;
}

/** What a live session needs. */
export interface LiveDeps {
	engine: LocalEngine;
	gate: EngineGate;
	/** Hold models while a session runs. */
	hold?: (models: string[]) => () => void;
	microphone?: AudioSource;
	systemAudio?: AudioSource;
	/** The folder of the recordings of sessions without `record_to`. */
	recordingDir?: string;
	/** The process of the app, for the capture of the system audio to leave out. */
	appPid?: number;
	emit: (event: Event) => void;
}

type FeedItem = { kind: "audio"; samples: Float32Array; timeMs: number | undefined } | { kind: "gap"; ms: number } | { kind: "ended"; reason: WriterEndReason; message: string | undefined };

/** The audio of one capture. It keeps the items until the pipeline of the source takes them. */
class FeedQueue {
	#items: FeedItem[] = [];
	#target: ((item: FeedItem) => void) | undefined;
	#closed = false;

	push(item: FeedItem): void {
		if (this.#closed) return;
		if (this.#target) this.#target(item);
		else this.#items.push(item);
	}

	attach(target: (item: FeedItem) => void): void {
		this.#target = target;
		const items = this.#items;
		this.#items = [];
		for (const item of items) target(item);
	}

	close(): void {
		this.#closed = true;
	}
}

/** A queue of messages that an async handler takes one at a time. */
class SerialQueue<T> {
	readonly #handler: (message: T) => Promise<void> | void;
	#items: T[] = [];
	#running = false;
	#closed = false;
	#idle: Array<() => void> = [];

	constructor(handler: (message: T) => Promise<void> | void) {
		this.#handler = handler;
	}

	push(message: T): void {
		if (this.#closed) return;
		this.#items.push(message);
		void this.#run();
	}

	async #run(): Promise<void> {
		if (this.#running) return;
		this.#running = true;
		try {
			for (let next = this.#items.shift(); next !== undefined; next = this.#items.shift()) {
				try {
					await this.#handler(next);
				} catch {
					// A handler reports its own failures as events.
				}
			}
		} finally {
			this.#running = false;
			if (this.#items.length === 0) for (const done of this.#idle.splice(0)) done();
		}
	}

	/** No message comes any more. Resolves after the last one was handled. */
	close(): Promise<void> {
		this.#closed = true;
		if (!this.#running && this.#items.length === 0) return Promise.resolve();
		return new Promise((resolve) => this.#idle.push(resolve));
	}
}

/** The audio that a part can still need. */
class KeptAudio {
	#chunks: Float32Array[] = [];
	#from = 0;
	#end = 0;

	get from(): number {
		return this.#from;
	}

	append(samples: Float32Array): void {
		this.#chunks.push(samples);
		this.#end += samples.length;
	}

	/** Forget the audio before the absolute sample `sample`. */
	dropBefore(sample: number): void {
		sample = Math.min(sample, this.#end);
		if (sample <= this.#from) return;
		let at = this.#from;
		while (this.#chunks.length > 0) {
			const chunk = this.#chunks[0] as Float32Array;
			if (at + chunk.length <= sample) {
				at += chunk.length;
				this.#chunks.shift();
			} else {
				this.#chunks[0] = chunk.subarray(sample - at);
				at = sample;
				break;
			}
		}
		this.#from = sample;
	}

	/** A copy of the samples from `from` to `to` (absolute, inside what is kept). */
	slice(from: number, to: number): Float32Array {
		const out = new Float32Array(to - from);
		let at = this.#from;
		let filled = 0;
		for (const chunk of this.#chunks) {
			const chunkEnd = at + chunk.length;
			if (chunkEnd > from && at < to) {
				const a = Math.max(from, at) - at;
				const b = Math.min(to, chunkEnd) - at;
				out.set(chunk.subarray(a, b), filled);
				filled += b - a;
			}
			at = chunkEnd;
			if (at >= to) break;
		}
		return out;
	}
}

/** The time of the first audio of a session on the capture clock, which all its sources share. */
class SessionClock {
	#origin = Number.POSITIVE_INFINITY;

	/** The first audio of a source has the time `ms`. Gives how late it is after the first audio of the session. */
	firstAudio(ms: number): number {
		this.#origin = Math.min(this.#origin, ms);
		return ms - this.#origin;
	}
}

type EchoRole = "none" | "mic" | "system";

interface SourcePlan {
	session: number;
	source: Source;
	finalModel: string;
	language: string;
	preview?: number;
	vad?: number;
	wav: WavWriter;
	clock: SessionClock;
	echo?: EchoGate;
	echoRole: EchoRole;
	silence?: SilenceWatch;
	silenceIndex: number;
}

interface PartAudio {
	offsetMs: number;
	samples: Float32Array;
	clockOffset: number;
}

type PartMessage = { part: PartAudio } | { doneUntil: number };

/** What a source gives when it ends. */
export interface SourceResult {
	source: Source;
	segments: Segment[];
	wav: string;
	wavError?: string;
}

function shiftSegment(s: Segment, offsetMs: number): Segment {
	const out: Segment = { start_ms: s.start_ms + offsetMs, end_ms: s.end_ms + offsetMs, text: s.text };
	if (s.words && s.words.length > 0) out.words = s.words.map((w) => ({ start_ms: w.start_ms + offsetMs, end_ms: w.end_ms + offsetMs, text: w.text }));
	return out;
}

/** The segments of a part. An engine without segments gives one segment for the whole part. */
export async function liveTranscribe(engine: LocalEngine, samples: Float32Array, model: string, language: string): Promise<Segment[]> {
	if (!hasFeature(engine.features(), "segments")) {
		const text = await engine.transcribe(samples, model, language);
		return [{ start_ms: 0, end_ms: Math.floor((samples.length * 1000) / RATE), text: text.trim() }];
	}
	const found = await engine.transcribeSegments(samples, model, language);
	return found.map((s) => {
		const out: Segment = { start_ms: s.start_ms, end_ms: s.end_ms, text: s.text.trim() };
		if (s.words && s.words.length > 0) out.words = [...s.words];
		return out;
	});
}

/** The pipeline of one source. */
class SourcePipeline {
	readonly #plan: SourcePlan;
	readonly #engine: LocalEngine;
	readonly #deps: LiveDeps;
	readonly #cancelled: () => boolean;
	readonly #basic: BasicVad | undefined;
	readonly #kept = new KeptAudio();
	readonly #parts: SerialQueue<PartMessage>;
	readonly #segments: Segment[] = [];
	readonly #unwatch: () => void;
	#wav: WavWriter | undefined;
	#wavError: string | undefined;
	#heard = 0;
	#lastEndMs = 0;
	#timed = false;
	#clockOffset = 0;
	#open = false;
	#openFrom = 0;
	#lastSpeechMs = 0;
	#lastTrim = Date.now();
	#lastMark = Date.now();
	#ended: { reason: WriterEndReason; message: string | undefined } | undefined;
	#finishing: Promise<SourceResult> | undefined;
	#stopped = false;

	constructor(plan: SourcePlan, deps: LiveDeps, cancelled: () => boolean) {
		this.#plan = plan;
		this.#engine = deps.engine;
		this.#deps = deps;
		this.#cancelled = cancelled;
		this.#wav = plan.wav;
		if (plan.vad === undefined) this.#basic = new BasicVad();
		this.#parts = new SerialQueue<PartMessage>((m) => this.#finalPass(m));
		this.#unwatch = deps.engine.onEvent((e) => this.#event(e));
	}

	/** Take what a capture gives, in order. */
	feed(item: FeedItem): void {
		if (this.#stopped) return;
		switch (item.kind) {
			case "audio":
				this.#captured(item.samples, item.timeMs);
				break;
			case "gap":
				this.#silenceFor(item.ms);
				this.#deps.emit({ type: "source_gap", session: this.#plan.session, source: this.#plan.source, ms: item.ms });
				break;
			case "ended":
				this.#ended = { reason: item.reason, message: item.message };
				void this.finish();
				break;
		}
	}

	#heardMs(): number {
		return Math.floor((this.#heard * 1000) / RATE);
	}

	#captured(samples: Float32Array, timeMs: number | undefined): void {
		if (timeMs !== undefined) {
			if (!this.#timed) {
				this.#timed = true;
				this.#silenceFor(this.#plan.clock.firstAudio(timeMs));
			}
			this.#clockOffset = timeMs - this.#heardMs();
		}
		this.#hear(samples);
		const now = Date.now();
		if (now - this.#lastTrim >= 1000) {
			this.#trimIdle();
			this.#lastTrim = now;
		}
		if (this.#plan.echoRole === "system" && now - this.#lastMark >= ECHO_MARK_MS) {
			this.#echoMark();
			this.#lastMark = now;
		}
	}

	#silenceFor(ms: number): void {
		const zeros = new Float32Array(RATE / 50);
		for (let i = 0; i < Math.floor(Math.min(ms, MAX_SILENCE_MS) / 20); i++) this.#hear(zeros);
	}

	/** Tell the echo rule how far the system source is. The message goes through the queue of parts, so the parts before it are done when it arrives. */
	#echoMark(): void {
		const echo = this.#plan.echo;
		if (!echo) return;
		const now = this.#heardMs() + this.#clockOffset;
		echo.systemHeard(now);
		if (!this.#speaking()) this.#parts.push({ doneUntil: now - ECHO_VAD_LAG_MS });
	}

	#speaking(): boolean {
		return this.#basic ? this.#basic.speaking : this.#open;
	}

	#hear(samples: Float32Array): void {
		if (samples.length === 0) return;
		if (this.#wav) {
			try {
				this.#wav.write(samples);
			} catch (e) {
				this.#wavError = `cannot write ${this.#wav.path}: ${(e as Error).message}`;
				this.#wav.abandon();
				this.#wav = undefined;
			}
		}
		this.#kept.append(samples);
		this.#heard += samples.length;
		if (this.#plan.preview !== undefined) this.#engine.pushAudio(this.#plan.preview, samples);
		if (this.#basic) {
			for (const p of this.#basic.accept(samples)) this.#cut(p.start_ms, p.end_ms);
		} else if (this.#plan.vad !== undefined) {
			this.#engine.pushAudio(this.#plan.vad, samples);
		}
		this.#watchSilence();
	}

	#watchSilence(): void {
		const silence = this.#plan.silence;
		if (!silence) return;
		const heardMs = this.#heardMs();
		if (this.#speaking()) this.#lastSpeechMs = heardMs;
		silence.heard(this.#plan.silenceIndex, satSub(heardMs, this.#lastSpeechMs));
	}

	#event(e: LocalEngineEvent): void {
		const plan = this.#plan;
		if (e.kind === "partial") {
			if (plan.preview !== undefined && e.session === plan.preview) {
				this.#deps.emit({ type: "partial", session: plan.session, source: plan.source, committed: e.committed, tentative: e.tentative });
			}
		} else if (e.kind === "speech_start") {
			if (!this.#basic && e.session === plan.vad) {
				this.#open = true;
				this.#openFrom = e.start_ms;
			}
		} else if (!this.#basic && e.session === plan.vad) {
			this.#open = false;
			this.#lastSpeechMs = Math.max(this.#lastSpeechMs, e.end_ms);
			this.#cut(e.start_ms, e.end_ms);
		}
	}

	/** Send the audio of a speech from `startMs` to `endMs` that ended to the final pass. */
	#cut(startMs: number, endMs: number): void {
		startMs = Math.max(startMs, this.#lastEndMs);
		if (endMs <= startMs) return;
		const padBefore = Math.min(PAD_MS, startMs - this.#lastEndMs);
		const from = Math.max(sampleAt(startMs - padBefore), this.#kept.from);
		const to = Math.min(sampleAt(endMs + PAD_MS), this.#heard);
		if (from >= to) return;
		const samples = this.#kept.slice(from, to);
		this.#lastEndMs = endMs;
		this.#kept.dropBefore(sampleAt(endMs));
		this.#parts.push({ part: { offsetMs: Math.floor((from * 1000) / RATE), samples, clockOffset: this.#clockOffset } });
	}

	/** Forget the audio older than the lookback while nobody speaks. */
	#trimIdle(): void {
		let keepFrom: number;
		if (this.#basic?.speaking) return;
		if (!this.#basic && this.#open) keepFrom = sampleAt(satSub(this.#openFrom, PAD_MS));
		else keepFrom = satSub(this.#heard, sampleAt(LOOKBACK_MS));
		this.#kept.dropBefore(Math.max(keepFrom, sampleAt(this.#lastEndMs)));
	}

	async #finalPass(m: PartMessage): Promise<void> {
		const plan = this.#plan;
		if (!("part" in m)) {
			if (plan.echoRole === "system") plan.echo?.systemDoneUntil(m.doneUntil);
			return;
		}
		if (this.#cancelled()) return;
		const part = m.part;
		const release = this.#deps.gate.livePart();
		let found: Segment[];
		try {
			found = await liveTranscribe(this.#engine, part.samples, plan.finalModel, plan.language);
		} catch (e) {
			if (!this.#cancelled()) {
				const { code, message } = engineCode(e);
				this.#deps.emit({
					type: "part_failed",
					session: plan.session,
					source: plan.source,
					start_ms: part.offsetMs,
					end_ms: part.offsetMs + Math.floor((part.samples.length * 1000) / RATE),
					message,
					...(code ? { code: code as ErrorCode } : {}),
				});
			}
			return;
		} finally {
			release();
		}
		for (const raw of found) {
			const s = shiftSegment(raw, part.offsetMs);
			if (s.text.trim() === "") continue;
			if (plan.echoRole === "mic") {
				// The gate sends it when the system source has caught up, unless it is an echo.
				plan.echo?.micSegment(s, part.clockOffset);
				continue;
			}
			this.#deps.emit({ type: "segment", session: plan.session, source: plan.source, start_ms: s.start_ms, end_ms: s.end_ms, text: s.text, ...(s.words ? { words: s.words } : {}) });
			if (plan.echoRole === "system") plan.echo?.systemSegment(s, part.clockOffset);
			this.#segments.push(s);
		}
	}

	/** End the audio, close the engine sessions, cut the last part, and wait for the final pass. Asking again gives the same answer. */
	finish(): Promise<SourceResult> {
		this.#finishing ??= this.#finish();
		return this.#finishing;
	}

	async #finish(): Promise<SourceResult> {
		const plan = this.#plan;
		this.#stopped = true;
		if (plan.preview !== undefined) await this.#engine.endStream(plan.preview);
		if (this.#basic) {
			for (const p of this.#basic.finish()) this.#cut(p.start_ms, p.end_ms);
		} else if (plan.vad !== undefined) {
			// The engine sends the last `speech_end` before its reply, so it is cut when the reply comes.
			try {
				await this.#engine.endVad(plan.vad);
			} catch {
				// The speech that is open ends at the end of the audio.
			}
			if (this.#open) this.#cut(this.#openFrom, this.#heardMs());
		}
		this.#unwatch();
		if (this.#wav) {
			try {
				this.#wav.finish();
			} catch (e) {
				this.#wavError = `cannot write ${this.#wav.path}: ${(e as Error).message}`;
			}
			this.#wav = undefined;
		}
		await this.#parts.close();
		if (plan.echoRole === "system") plan.echo?.systemEnded();
		if (this.#ended) this.#deps.emit({ type: "source_ended", session: plan.session, source: plan.source, reason: this.#ended.reason });
		return { source: plan.source, segments: this.#segments, wav: plan.wav.path, ...(this.#wavError ? { wavError: this.#wavError } : {}) };
	}
}

interface Running {
	session: number;
	sources: Array<{ source: Source; pipeline: SourcePipeline; feed: FeedQueue; writer?: AudioWriter }>;
	client: SourcePipeline | undefined;
	clientOpen: boolean;
	excludes: string[];
	echo: EchoGate | undefined;
	diarization: string | undefined;
	temporary: string[];
	release: (() => void) | undefined;
	stopping: boolean;
	cancelled: { value: boolean };
}

function liveError(code: ErrorCode | undefined, message: string): SaysoError {
	return new SaysoError({ ...(code ? { code } : {}), message });
}

/** Runs the one live session of an app. */
export class LiveSessions {
	readonly #deps: LiveDeps;
	#state: "free" | "starting" | "running" = "free";
	#run: Running | undefined;
	#nextSession = 0;
	#nextEngineSession = ENGINE_SESSION_BASE;
	#closed = false;

	constructor(deps: LiveDeps) {
		this.#deps = deps;
	}

	/** True while a session starts or runs. */
	get busy(): boolean {
		return this.#state !== "free";
	}

	/** The id of the session that runs. */
	get current(): number | undefined {
		return this.#run?.session;
	}

	#checkSources(sources: Source[]): void {
		if (sources.length === 0) throw liveError("bad_request", "a live session needs at least one source");
		sources.forEach((source, i) => {
			if (sources.slice(0, i).includes(source)) throw liveError("bad_request", `the source ${source} is in the list twice`);
			if (source === "mic" && !this.#deps.microphone) throw liveError("unsupported", `this program cannot capture the source ${source}`);
			if (source === "system" && !this.#deps.systemAudio) throw liveError("unsupported", `this program cannot capture the source ${source}`);
			if (source !== "mic" && source !== "system" && source !== "client") throw liveError("bad_request", `unknown source ${source}`);
		});
	}

	#recordings(session: number, spec: SessionSpec): string[] {
		let dir = this.#deps.recordingDir ?? join(tmpdir(), "sayso-live");
		if (spec.recordTo !== undefined && spec.recordTo !== "") {
			if (!isAbsolute(spec.recordTo)) throw liveError("bad_request", `record_to must be an absolute path, not ${spec.recordTo}`);
			dir = spec.recordTo;
		}
		try {
			mkdirSync(dir, { recursive: true, mode: 0o700 });
		} catch (e) {
			throw liveError("bad_request", `cannot make the folder ${dir}: ${(e as Error).message}`);
		}
		return spec.sources.map((source) => (spec.recordTo ? join(dir, `${source}.wav`) : join(dir, `sayso-live-${process.pid}-${session}-${source}.wav`)));
	}

	/** Start a session. It waits while the models load, and while the user answers a question of the system. */
	async start(spec: SessionSpec): Promise<{ session: number; excludes: string[] }> {
		this.#checkSources(spec.sources);
		if (this.#closed) throw liveError(undefined, "the live session cannot start, because the Speech closed");
		if (this.#state !== "free") throw liveError("busy", "another live session runs, and only one can run at a time");
		this.#state = "starting";
		let run: Running | undefined;
		try {
			const session = ++this.#nextSession;
			run = await this.#open(session, spec);
			if (this.#closed) throw liveError(undefined, "the start of the live session was cancelled, because the Speech closed");
			this.#run = run;
			this.#state = "running";
			return { session, excludes: run.excludes };
		} catch (e) {
			this.#state = "free";
			this.#run = undefined;
			throw e;
		}
	}

	async #open(session: number, spec: SessionSpec): Promise<Running> {
		const deps = this.#deps;
		const engine = deps.engine;
		const recordings = this.#recordings(session, spec);
		const models = [spec.finalModel, ...(spec.previewModel ? [spec.previewModel] : []), ...(!spec.vad.basic && spec.vad.model ? [spec.vad.model] : []), ...(spec.diarization ? [spec.diarization] : [])];
		const release = deps.hold?.(models);
		const writers: AudioWriter[] = [];
		const writerOf = new Map<Source, AudioWriter>();
		const feeds = new Map<Source, FeedQueue>();
		const streams: number[] = [];
		const vads: number[] = [];
		const pipelines: SourcePipeline[] = [];
		const written: string[] = [];
		const cancelled = { value: false };
		let ok = false;
		const undo = async (): Promise<void> => {
			cancelled.value = true;
			for (const feed of feeds.values()) feed.close();
			for (const w of writers) w._close();
			await Promise.all(pipelines.map((p) => p.finish().catch(() => undefined)));
			for (const id of streams) await engine.endStream(id).catch(() => undefined);
			for (const id of vads) await engine.endVad(id).catch(() => undefined);
			for (const p of written) rmSync(p, { force: true });
			release?.();
		};
		try {
			const load = [spec.finalModel, ...(spec.previewModel ? [spec.previewModel] : []), ...(!spec.vad.basic && spec.vad.model ? [spec.vad.model] : [])];
			for (const m of load) {
				try {
					if (!engine.isUsable(m)) await engine.load(m);
				} catch (e) {
					const { code, message } = engineCode(e);
					throw liveError(code as ErrorCode | undefined, `cannot load ${m}: ${message}`);
				}
			}
			const clock = new SessionClock();
			let echo: EchoGate | undefined;
			if (spec.sources.includes("mic") && spec.sources.includes("system")) {
				echo = new EchoGate((s) => deps.emit({ type: "segment", session, source: "mic", start_ms: s.start_ms, end_ms: s.end_ms, text: s.text, ...(s.words ? { words: s.words } : {}) }));
			}
			// The captures first, and the system audio first of them. It can wait for the answer of the user, and the
			// microphone must not run during that wait. A refusal stops the start with no microphone and no recording.
			const excludes: string[] = [];
			const openCapture = async (source: "mic" | "system"): Promise<void> => {
				const feed = new FeedQueue();
				const writer = new AudioWriter({
					audio: (samples, timeMs) => feed.push({ kind: "audio", samples, timeMs }),
					gap: (ms) => feed.push({ kind: "gap", ms }),
					ended: (reason, message) => feed.push({ kind: "ended", reason, message }),
				});
				writers.push(writer);
				writerOf.set(source, writer);
				feeds.set(source, feed);
				const capture = source === "mic" ? deps.microphone : deps.systemAudio;
				try {
					const opened = await capture?.open(writer, deps.appPid === undefined ? {} : { appPid: deps.appPid });
					if (source === "system") excludes.push(...(opened?.excludes ?? []));
				} catch (e) {
					const error = SaysoError.from(e);
					const code = error.code === "permission" || error.code === "unsupported" ? error.code : undefined;
					throw liveError(code, `the source ${source} did not start: ${error.message}`);
				}
			};
			if (spec.sources.includes("system")) await openCapture("system");
			if (spec.sources.includes("mic")) await openCapture("mic");
			if (this.#closed) throw liveError(undefined, "the start of the live session was cancelled, because the Speech closed");

			const run: Running = { session, sources: [], client: undefined, clientOpen: false, excludes, echo, diarization: spec.diarization, temporary: spec.recordTo ? [] : [...recordings], release, stopping: false, cancelled };
			let silence: SilenceWatch | undefined;
			if (spec.silenceLimitMs !== undefined && spec.silenceLimitMs > 0) {
				const limit = spec.silenceLimitMs;
				silence = new SilenceWatch(limit, spec.sources.length, () => {
					if (this.#state === "starting") return false;
					if (this.#state !== "running" || this.#run !== run) return true;
					run.clientOpen = false;
					for (const w of writers) w._close();
					deps.emit({ type: "silence_limit", session, silence_ms: limit });
					return true;
				});
			}
			for (const [index, source] of spec.sources.entries()) {
				const plan: SourcePlan = { session, source, finalModel: spec.finalModel, language: spec.language, wav: undefined as unknown as WavWriter, clock, echoRole: "none", silenceIndex: index };
				if (silence) plan.silence = silence;
				if (spec.previewModel) {
					const id = this.#nextEngineSession++;
					await engine.startStream(id, spec.previewModel, spec.language).catch((e: unknown) => {
						const { code, message } = engineCode(e);
						throw liveError(code as ErrorCode | undefined, `cannot open the preview stream: ${message}`);
					});
					streams.push(id);
					plan.preview = id;
				}
				if (!spec.vad.basic && spec.vad.model) {
					const id = this.#nextEngineSession++;
					await engine.startVad(id, spec.vad.model).catch((e: unknown) => {
						const { code, message } = engineCode(e);
						throw liveError(code as ErrorCode | undefined, `cannot open the VAD session: ${message}`);
					});
					vads.push(id);
					plan.vad = id;
				}
				const path = recordings[index] as string;
				try {
					plan.wav = new WavWriter(path);
				} catch (e) {
					throw liveError("bad_request", `cannot write ${path}: ${(e as Error).message}`);
				}
				written.push(path);
				if (echo && source === "mic") {
					plan.echo = echo;
					plan.echoRole = "mic";
				} else if (echo && source === "system") {
					plan.echo = echo;
					plan.echoRole = "system";
				}
				const pipeline = new SourcePipeline(plan, deps, () => cancelled.value || run.cancelled.value);
				pipelines.push(pipeline);
				// The source owns its engine sessions now.
				streams.length = 0;
				vads.length = 0;
				const feed = feeds.get(source);
				if (feed) {
					feed.attach((item) => pipeline.feed(item));
					run.sources.push({ source, pipeline, feed, ...(writerOf.has(source) ? { writer: writerOf.get(source) as AudioWriter } : {}) });
				} else {
					run.client = pipeline;
					run.clientOpen = true;
					run.sources.push({ source, pipeline, feed: new FeedQueue() });
				}
			}
			ok = true;
			return run;
		} finally {
			if (!ok) await undo();
		}
	}

	/** Audio of the source `client`: 16 kHz mono samples from -1 to 1. */
	audio(session: number, samples: Float32Array): void {
		const run = this.#run;
		if (this.#state !== "running" || !run || run.session !== session) throw liveError("bad_request", `no live session ${session} of this connection runs`);
		if (!run.client || !run.clientOpen) {
			throw liveError("bad_request", run.stopping ? "the live session stops" : run.client ? "the live session stopped taking audio" : "the live session has no source client");
		}
		run.client.feed({ kind: "audio", samples, timeMs: undefined });
	}

	/** End the audio, wait for the last parts, diarize, and give the text. The events go out before it returns. */
	async stop(session: number): Promise<string> {
		const run = this.#run;
		if (this.#state !== "running" || !run || run.session !== session) throw liveError("bad_request", `no live session ${session} of this connection runs`);
		if (run.stopping) throw liveError("bad_request", "the live session stops already");
		run.stopping = true;
		try {
			return await this.#finish(run, false);
		} finally {
			this.#free(run);
		}
	}

	/** Stop the session without diarization, and stop new sessions. */
	async close(): Promise<void> {
		this.#closed = true;
		await this.cancel();
	}

	/** Stop the session without diarization and forget its parts. A new session can start after it. */
	async cancel(): Promise<void> {
		const run = this.#run;
		if (!run || run.stopping) return;
		run.stopping = true;
		run.cancelled.value = true;
		try {
			await this.#finish(run, true);
		} finally {
			this.#free(run);
		}
	}

	#free(run: Running): void {
		if (this.#run === run) {
			this.#run = undefined;
			this.#state = "free";
		}
	}

	async #finish(run: Running, cancelled: boolean): Promise<string> {
		run.clientOpen = false;
		for (const s of run.sources) {
			s.feed.close();
			s.writer?._close();
		}
		const results = await Promise.all(run.sources.map((s) => s.pipeline.finish()));
		try {
			if (run.echo) {
				run.echo.systemEnded();
				for (const r of results) if (r.source === "mic") r.segments = run.echo.keptSegments();
			}
			return cancelled ? "" : await this.#speakersAndText(run, results);
		} finally {
			for (const p of run.temporary) rmSync(p, { force: true });
			run.release?.();
		}
	}

	async #speakersAndText(run: Running, results: SourceResult[]): Promise<string> {
		const { engine, gate, emit } = this.#deps;
		for (const r of results) if (r.wavError) throw liveError(undefined, r.wavError);
		if (run.diarization) {
			for (const r of results) {
				try {
					if (!engine.isUsable(run.diarization)) await engine.load(run.diarization);
				} catch (e) {
					const { code, message } = engineCode(e);
					throw liveError(code as ErrorCode | undefined, `cannot load ${run.diarization}: ${message}`);
				}
				const samples = readLiveWav(r.wav);
				const release = gate.livePart();
				let found;
				try {
					found = await engine.diarize(run.diarization, samples);
				} catch (e) {
					const { code, message } = engineCode(e);
					throw liveError(code as ErrorCode | undefined, `diarization failed: ${message}`);
				} finally {
					release();
				}
				// A session has no number of speakers, so small speakers fold.
				found = foldSmallSpeakers(found);
				const times = r.segments.map((s): [number, number] => [s.start_ms, s.end_ms]);
				const turns: Turn[] = segmentTurns(times, assignSpeakers(times, found));
				emit({ type: "speakers", session: run.session, source: r.source, turns });
			}
		}
		return sessionText(results);
	}
}

/** The text of the session: the segments of all sources in the order of time. */
export function sessionText(results: SourceResult[]): string {
	const pieces: Array<{ start: number; source: number; text: string }> = [];
	results.forEach((r, i) => {
		for (const s of r.segments) pieces.push({ start: s.start_ms, source: i, text: s.text.trim() });
	});
	pieces.sort((a, b) => a.start - b.start || a.source - b.source);
	return pieces
		.map((p) => p.text)
		.filter((t) => t !== "")
		.join(" ");
}
