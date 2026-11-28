// The speech engine as the jobs and the live sessions of Local mode see it, and the engine program behind it.

import { mkdtempSync, rmSync, unlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { writeWav, floatsToI16 } from "../audio/wav.js";
import type { EngineProcess } from "../engine.js";
import { EngineError, SaysoError } from "../errors.js";
import type { Segment, SpeechPart } from "../protocol/index.js";


/** A turn of a speaker as the engine gives it: an integer from 0. */
export interface RawTurn {
	start_ms: number;
	end_ms: number;
	speaker: number;
}

/** What a stream or a VAD session of the engine reports. */
export type LocalEngineEvent =
	| { kind: "partial"; session: number; committed: string; tentative: string }
	| { kind: "speech_start"; session: number; start_ms: number }
	| { kind: "speech_end"; session: number; start_ms: number; end_ms: number };

/** The speech engine of Local mode. `EngineSpeech` is the one over an engine program. */
export interface LocalEngine {
	/** The features of the engine. An engine with none is older than the list, and has them all. */
	features(): string[];
	/** True when the model is in memory. */
	isUsable(model: string): boolean;
	load(model: string): Promise<void>;
	transcribe(samples: Float32Array, model: string, language: string): Promise<string>;
	transcribeSegments(samples: Float32Array, model: string, language: string): Promise<Segment[]>;
	detectSpeech(model: string, samples: Float32Array): Promise<SpeechPart[]>;
	diarize(model: string, samples: Float32Array, speakers?: number): Promise<RawTurn[]>;
	startStream(session: number, model: string, language: string): Promise<void>;
	startVad(session: number, model: string): Promise<void>;
	pushAudio(session: number, samples: Float32Array): void;
	endStream(session: number): Promise<void>;
	endVad(session: number): Promise<void>;
	onEvent(listener: (event: LocalEngineEvent) => void): () => void;
}

/** True when the engine names the feature, or names none (an old engine has them all). */
export function hasFeature(features: string[], feature: string): boolean {
	return features.length === 0 || features.includes(feature);
}

/** The code and the message of an error of the engine. */
export function engineCode(error: unknown): { code: string | undefined; message: string } {
	if (error instanceof SaysoError) return { code: error.code, message: error.message };
	return { code: undefined, message: error instanceof Error ? error.message : String(error) };
}

// ---- Which models jobs and sessions hold ----

/** Says which models jobs and live sessions hold, and since when the others are free. */
export class ModelUse {
	readonly #models = new Map<string, { holders: number; released?: number }>();

	/** Hold the models until the function that it gives runs. */
	hold(models: string[]): () => void {
		for (const id of models) {
			const use = this.#models.get(id) ?? { holders: 0 };
			use.holders++;
			delete use.released;
			this.#models.set(id, use);
		}
		let done = false;
		return () => {
			if (done) return;
			done = true;
			for (const id of models) {
				const use = this.#models.get(id);
				if (!use) continue;
				if (use.holders > 0) use.holders--;
				if (use.holders === 0) use.released = Date.now();
			}
		};
	}

	/** True while a job or a live session holds the model. */
	held(model: string): boolean {
		return (this.#models.get(model)?.holders ?? 0) > 0;
	}

	/** The models that nobody holds since `keepMs`. They leave the list. */
	idle(keepMs: number): string[] {
		const out: string[] = [];
		for (const [id, use] of this.#models) {
			if (use.holders === 0 && use.released !== undefined && Date.now() - use.released >= keepMs) {
				out.push(id);
				this.#models.delete(id);
			}
		}
		return out;
	}

	/** The models that are held or were held. */
	known(): string[] {
		return [...this.#models.keys()];
	}
}

// ---- Jobs wait for the parts of a live session in the engine ----

/** Holds jobs while a live session transcribes a part, so a job does not slow the final pass of a session. */
export class EngineGate {
	#parts = 0;
	#waiters: Array<() => void> = [];

	/** Mark a part of a live session in the engine, until the function that it gives runs. */
	livePart(): () => void {
		this.#parts++;
		let done = false;
		return () => {
			if (done) return;
			done = true;
			if (this.#parts > 0) this.#parts--;
			this.wake();
		};
	}

	/** Make the waiters look again. */
	wake(): void {
		const waiters = this.#waiters;
		this.#waiters = [];
		for (const waiter of waiters) waiter();
	}

	/** Wait until no live part is in the engine. False when `stop` turned true. */
	async waitForJob(stop: () => boolean): Promise<boolean> {
		for (;;) {
			if (stop()) return false;
			if (this.#parts === 0) return true;
			await new Promise<void>((resolve) => this.#waiters.push(resolve));
		}
	}
}

// ---- The engine program ----

export interface EngineSpeechOptions {
	/** How long a model that nobody uses stays in memory. Default 60000. */
	keepModelMs?: number;
}

/** A `LocalEngine` over an engine program. It writes 16 kHz mono WAV files for the calls that take a path. */
export class EngineSpeech implements LocalEngine {
	readonly process: EngineProcess;
	readonly models = new ModelUse();
	readonly #dir: string;
	readonly #loaded = new Set<string>();
	readonly #loading = new Map<string, Promise<void>>();
	readonly #keepMs: number;
	#files = 0;
	#sweep: NodeJS.Timeout | undefined;
	#closed = false;

	constructor(process: EngineProcess, options: EngineSpeechOptions = {}) {
		this.process = process;
		this.#keepMs = options.keepModelMs ?? 60_000;
		this.#dir = mkdtempSync(join(tmpdir(), "sayso-local-"));
		void process.done.then(() => this.#loaded.clear());
	}

	features(): string[] {
		return this.process.hello.features;
	}

	isUsable(model: string): boolean {
		return this.#loaded.has(model);
	}

	load(model: string): Promise<void> {
		if (this.#loaded.has(model)) return Promise.resolve();
		let loading = this.#loading.get(model);
		if (!loading) {
			loading = this.process
				.load(model)
				.then(() => void this.#loaded.add(model))
				.finally(() => this.#loading.delete(model));
			this.#loading.set(model, loading);
		}
		return loading;
	}

	/** Mark models as held by work that runs, and schedule the unload of the ones that nobody uses for `keepModelMs`. */
	hold(models: string[]): () => void {
		const release = this.models.hold(models);
		return () => {
			release();
			this.#schedule();
		};
	}

	#schedule(): void {
		if (this.#closed || this.#sweep !== undefined) return;
		this.#sweep = setTimeout(() => {
			this.#sweep = undefined;
			for (const model of this.models.idle(this.#keepMs)) {
				if (!this.#loaded.delete(model)) continue;
				this.process.unload(model, false).catch(() => undefined);
			}
			if (this.models.known().length > 0) this.#schedule();
		}, Math.max(50, Math.min(this.#keepMs, 5000)));
		this.#sweep.unref();
	}

	#tempWav(samples: Float32Array): string {
		const path = join(this.#dir, `part-${++this.#files}.wav`);
		writeWav(path, floatsToI16(samples));
		return path;
	}

	async #withWav<T>(samples: Float32Array, use: (path: string) => Promise<T>): Promise<T> {
		const path = this.#tempWav(samples);
		try {
			return await use(path);
		} finally {
			try {
				unlinkSync(path);
			} catch {
				// Gone already.
			}
		}
	}

	async transcribe(samples: Float32Array, model: string, language: string): Promise<string> {
		return this.#withWav(samples, async (path) => (await this.process.transcribe({ model, path, language })).text);
	}

	async transcribeSegments(samples: Float32Array, model: string, language: string): Promise<Segment[]> {
		return this.#withWav(samples, async (path) => {
			const final = await this.process.transcribe({ model, path, language, segments: true });
			if (final.segments) return final.segments;
			return [{ start_ms: 0, end_ms: Math.floor((samples.length * 1000) / 16000), text: final.text.trim() }];
		});
	}

	detectSpeech(model: string, samples: Float32Array): Promise<SpeechPart[]> {
		return this.#withWav(samples, (path) => this.process.vad(model, path));
	}

	async diarize(model: string, samples: Float32Array, speakers?: number): Promise<RawTurn[]> {
		return this.#withWav(samples, async (path) => (await this.process.diarize(model, path, speakers)).turns);
	}

	startStream(session: number, model: string, language: string): Promise<void> {
		return this.process.streamStart(session, model, language === "auto" ? undefined : language);
	}

	startVad(session: number, model: string): Promise<void> {
		return this.process.vadStart(session, model);
	}

	pushAudio(session: number, samples: Float32Array): void {
		try {
			this.process.streamAudio(session, floatsToI16(samples));
		} catch {
			// The engine is gone: the next call with a reply says so.
		}
	}

	async endStream(session: number): Promise<void> {
		await this.process.streamEnd(session).catch(() => undefined);
	}

	async endVad(session: number): Promise<void> {
		await this.process.streamEnd(session).catch((e: unknown) => {
			throw e instanceof EngineError || e instanceof SaysoError ? e : new SaysoError({ message: String(e) });
		});
	}

	onEvent(listener: (event: LocalEngineEvent) => void): () => void {
		return this.process.on((m) => {
			if (m.type === "partial" && typeof m.session === "number") {
				listener({ kind: "partial", session: m.session, committed: String(m.committed ?? ""), tentative: String(m.tentative ?? "") });
			} else if (m.type === "speech_start" && typeof m.session === "number") {
				listener({ kind: "speech_start", session: m.session, start_ms: Number(m.start_ms ?? 0) });
			} else if (m.type === "speech_end" && typeof m.session === "number") {
				listener({ kind: "speech_end", session: m.session, start_ms: Number(m.start_ms ?? 0), end_ms: Number(m.end_ms ?? 0) });
			}
		});
	}

	/** Stop the engine and delete the temporary files. */
	async close(): Promise<void> {
		this.#closed = true;
		clearTimeout(this.#sweep);
		try {
			await this.process.close();
		} finally {
			rmSync(this.#dir, { recursive: true, force: true });
		}
	}
}
