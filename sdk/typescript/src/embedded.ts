import { mkdirSync } from "node:fs";
import { AudioWriter, helperSource, type AudioSource, type OpenInfo, type Opened } from "./capture.js";
import { findAudioHelper } from "./audio/helper.js";
import { Deepgram, DeepgramFinal, FinalPassEngine, type FinalPassFunction } from "./deepgram.js";
import { Engine, EngineProcess } from "./engine.js";
import { SaysoError } from "./errors.js";
import { EventHub } from "./events.js";
import { EngineGate, EngineSpeech, type LocalEngine } from "./local/engine.js";
import { LiveSessions, type SessionSpec } from "./local/live.js";
import type { DownloadProgress, Event as Message, ModelState, Role, Source } from "./protocol/index.js";

export { DeepgramFinal, type FinalPassFunction } from "./deepgram.js";

/** A model of the engine and its state on disk. */
export interface EmbeddedModel {
	id: string;
	name: string;
	size_bytes: number;
	languages: string[];
	roles: Role[];
	state: ModelState;
	recommended: boolean;
}

/**
 * What comes from `EmbeddedSpeech.events()`: `partial`, `segment`, `silence_limit`, or `part_failed`. A `part_failed` event says that the
 * final pass of one part failed. The session has no segment for that audio and goes on.
 */
export type EmbeddedEvent = Extract<Message, { type: "partial" | "segment" | "silence_limit" | "part_failed" }>;

/** What `EmbeddedSpeech.connect` needs. */
export interface EmbeddedOptions {
	app: { name: string };
	/**
	 * The engine that the app ships: an `Engine` with `modelsDir` (for example `Engine.fromPackage().modelsDir(folder)`), or the program, the
	 * folder for the models, and the model store (default: `modelStore()`).
	 */
	engine: Engine | { program: string; modelsDir: string; store?: string };
	/** A folder for the temporary recordings of live sessions. The SDK makes it. */
	tempDir: string;
	/** The microphone of the app. Missing: the microphone of the SDK (the audio helper), with `setDevice` and the others. */
	microphone?: AudioSource;
	/** A layer for the final pass, around the engine. */
	finalPass?: DeepgramFinal | FinalPassFunction;
}

/** One live session. */
export interface EmbeddedSession {
	sources: Source[];
	/** The model of the final pass of each part. */
	finalModel: string;
	/** The streaming model of the `partial` events. Missing: no preview. */
	previewModel?: string;
	/** A VAD model id, or `basic` (the default). */
	vad?: string;
	/** A code such as `en`, or `auto` (the default). */
	language?: string;
	/** Stop the audio after this long with no speech. Missing: no limit. */
	silenceLimitMs?: number;
}

interface Program {
	program: string;
	modelsDir: string;
	store?: string;
}

function programOf(engine: EmbeddedOptions["engine"]): Program {
	if (!(engine instanceof Engine)) return engine;
	const spec = engine.toJSON();
	if (!spec.modelsDir) throw SaysoError.of("bad_request", "EmbeddedSpeech needs the folder of the models: Engine.modelsDir(folder)");
	return { program: engine.program(), modelsDir: spec.modelsDir, ...(spec.store ? { store: spec.store } : {}) };
}

/** The display level (0 to 1) of a frame: its power in dB between -60 and 0. */
export function displayLevel(samples: Float32Array): number {
	if (samples.length === 0) return 0;
	let power = 0;
	for (const s of samples) power += s * s;
	const db = 10 * Math.log10(power / samples.length + 1e-12);
	return Math.max(0, Math.min(1, (db + 60) / 60));
}

/** A capture that started before its session: it keeps what it hears until the session takes it. */
class EarlyCapture {
	target: AudioWriter | undefined;
	opened: Promise<void | Opened> = Promise.resolve();
	#buffered: Array<(writer: AudioWriter) => void> = [];

	push(samples: Float32Array, timeMs?: number): boolean {
		if (this.target) return this.target.push(samples, timeMs);
		this.#buffered.push((w) => void w.push(samples, timeMs));
		return true;
	}

	gap(ms: number): boolean {
		if (this.target) return this.target.gap(ms);
		this.#buffered.push((w) => void w.gap(ms));
		return true;
	}

	end(reason?: "permission" | "failed", message?: string): void {
		if (this.target) this.target.end(reason, message);
		else this.#buffered.push((w) => w.end(reason, message));
	}

	/** Give the session what was heard so far, and everything after it. */
	attach(writer: AudioWriter): void {
		this.target = writer;
		for (const replay of this.#buffered.splice(0)) replay(writer);
	}
}

/** The microphone of the SDK: the audio helper, with a device and a level meter that the app can change. */
class SdkMicrophone implements AudioSource {
	device: string | null = null;
	level: ((level: number) => void) | null = null;
	#early: { capture: EarlyCapture; writer: AudioWriter } | undefined;

	#source(): AudioSource {
		return helperSource("mic", this.device ? { device: this.device } : {});
	}

	#metered(target: { push(samples: Float32Array, timeMs?: number): boolean; gap(ms: number): boolean; end(reason?: "permission" | "failed", message?: string): void }): AudioWriter {
		const meter = this.level;
		return new AudioWriter({
			audio: (samples, timeMs) => {
				meter?.(displayLevel(samples));
				target.push(samples, timeMs);
			},
			gap: (ms) => void target.gap(ms),
			ended: (reason, message) => target.end(reason, message),
		});
	}

	/** Start the capture now, before the models of a session load. */
	async openEarly(): Promise<void> {
		if (this.#early) return;
		const capture = new EarlyCapture();
		const early = { capture, writer: this.#metered(capture) };
		this.#early = early;
		capture.opened = Promise.resolve(this.#source().open(early.writer, {}));
		try {
			await capture.opened;
		} catch (e) {
			this.#early = undefined;
			throw e;
		}
	}

	/** Stop a capture of `openEarly` that no session took. */
	closeEarly(): void {
		const early = this.#early;
		this.#early = undefined;
		if (early && !early.capture.target) early.writer._close();
	}

	async open(writer: AudioWriter, info: OpenInfo): Promise<void | Opened> {
		const early = this.#early;
		if (early) {
			this.#early = undefined;
			const opened = await early.capture.opened;
			writer.onClose(() => early.writer._close());
			early.capture.attach(writer);
			return opened;
		}
		const inner = this.#metered(writer);
		writer.onClose(() => inner._close());
		return this.#source().open(inner, info);
	}
}

/**
 * A session in the process of the app, with an engine layer of the app. `Speech` is the API for most apps. This one never talks to Sayso, and the
 * app keeps its models itself. Call `close()` when done, or use `await using`.
 */
export class EmbeddedSpeech implements AsyncDisposable {
	readonly #program: Program;
	readonly #options: EmbeddedOptions;
	readonly #events = new EventHub<EmbeddedEvent>();
	readonly #mic = new SdkMicrophone();
	readonly #gate = new EngineGate();
	#ready: Promise<{ process: EngineProcess; speech: EngineSpeech; live: LiveSessions }> | undefined;
	#closed: Promise<void> | undefined;

	private constructor(options: EmbeddedOptions, program: Program) {
		this.#options = options;
		this.#program = program;
	}

	/** Get ready. This starts no engine and loads no model. */
	static async connect(options: EmbeddedOptions): Promise<EmbeddedSpeech> {
		return new EmbeddedSpeech(options, programOf(options.engine));
	}

	/** The mode of the work now. */
	mode(): "local" {
		return "local";
	}

	#start(): Promise<{ process: EngineProcess; speech: EngineSpeech; live: LiveSessions }> {
		this.#ready ??= (async () => {
			const process = await EngineProcess.start(this.#program.program, { modelsDir: this.#program.modelsDir, ...(this.#program.store ? { store: this.#program.store } : {}) });
			const speech = new EngineSpeech(process);
			let engine: LocalEngine = speech;
			const pass = this.#options.finalPass;
			if (pass instanceof DeepgramFinal) {
				const deepgram = new Deepgram(pass.key, pass.baseUrl ? { baseUrl: pass.baseUrl } : {});
				engine = new FinalPassEngine(speech, (m) => m === Deepgram.MODEL, (samples, _model, language) => deepgram.transcribe(samples, language));
			} else if (pass) {
				const models = new Set(pass.models);
				engine = new FinalPassEngine(speech, (m) => models.has(m), async (samples, model, language) => {
					const text = await pass.transcribe(samples, model, language);
					if (typeof text !== "string") throw new SaysoError({ message: "the final pass of the app must give a string" });
					return text;
				});
			}
			mkdirSync(this.#options.tempDir, { recursive: true });
			const microphone: AudioSource | undefined = this.#options.microphone ?? (findAudioHelper() ? this.#mic : undefined);
			const live = new LiveSessions({
				engine,
				gate: this.#gate,
				hold: (models) => speech.hold(models),
				emit: (event) => {
					if (event.type === "partial" || event.type === "segment" || event.type === "silence_limit" || event.type === "part_failed") this.#events.emit(event);
				},
				recordingDir: this.#options.tempDir,
				appPid: globalThis.process.pid,
				...(microphone ? { microphone } : {}),
			});
			return { process, speech, live };
		})();
		return this.#ready;
	}

	/** The models of the engine and their state on disk. Starts the engine. */
	async models(): Promise<EmbeddedModel[]> {
		const { process } = await this.#start();
		const catalog = await process.catalog();
		const states = new Map(catalog.states.map((s) => [s.model, s.state] as const));
		return catalog.models.map((m) => ({ id: m.id, name: m.name, size_bytes: m.size_bytes, languages: m.languages, roles: m.roles, state: (states.get(m.id) ?? "not_downloaded") as ModelState, recommended: m.recommended }));
	}

	/** Download a model at once. The app asks its own user first. `sizeBytes` helps an engine that knows only a fraction. */
	async download(model: string, options?: { sizeBytes?: number; onProgress?: (progress: DownloadProgress) => void }): Promise<void> {
		const { process } = await this.#start();
		const onProgress = options?.onProgress;
		await process.download(model, {
			...(options?.sizeBytes ? { sizeBytes: options.sizeBytes } : {}),
			onEvent: (m) => {
				if (m.type === "download_progress") onProgress?.({ model: String(m.model), fraction: Number(m.fraction ?? 0), bytes_done: Number(m.bytes_done ?? 0), bytes_total: Number(m.bytes_total ?? 0) });
			},
		});
	}

	/** Delete the files of a model. */
	async delete(model: string): Promise<void> {
		await (await this.#start()).process.delete(model);
	}

	/** Read a model into memory. Waits while it loads. */
	async load(model: string): Promise<void> {
		await (await this.#start()).speech.load(model);
	}

	/** Take a model out of memory. */
	async unload(model: string): Promise<void> {
		await (await this.#start()).process.unload(model);
	}

	/** The events of the sessions from now on. Each iterator and each listener gets every event. */
	events(options?: { signal?: AbortSignal }): AsyncIterableIterator<EmbeddedEvent> {
		return this.#events.iterate(options?.signal);
	}

	/** Call `listener` for each event from now on. The result removes it. */
	on(listener: (event: EmbeddedEvent) => void): () => void {
		return this.#events.on(listener);
	}

	/** Start a live session. Waits while its models load. At most one session runs at a time. */
	async startSession(session: EmbeddedSession): Promise<number> {
		const { live } = await this.#start();
		const vad = session.vad ?? "basic";
		const spec: SessionSpec = { sources: session.sources, finalModel: session.finalModel, vad: vad === "basic" ? { basic: true } : { basic: false, model: vad }, language: session.language ?? "auto" };
		if (session.previewModel) spec.previewModel = session.previewModel;
		if (session.silenceLimitMs !== undefined) spec.silenceLimitMs = session.silenceLimitMs;
		return (await live.start(spec)).session;
	}

	/** Audio of the source `client`: 16 kHz mono samples from -1 to 1. */
	sessionAudio(session: number, samples: Float32Array): void {
		const ready = this.#ready;
		if (!ready) throw SaysoError.of("bad_request", `no live session ${session} runs`);
		void ready.then(({ live }) => live.audio(session, samples)).catch(() => undefined);
	}

	/** End the audio, wait for the last parts, and give the text of the session. The `segment` events come before this returns. */
	async stopSession(session: number): Promise<string> {
		const { live } = await this.#start();
		return live.stop(session);
	}

	/** Stop the session and forget the parts that wait. Returns at once. */
	cancelSession(): void {
		void this.#ready?.then(({ live }) => live.cancel()).catch(() => undefined);
	}

	/** The microphone of the SDK: use this input device from the next session on. Null: the default device. */
	setDevice(device: string | null): void {
		this.#mic.device = device;
	}

	/** The microphone of the SDK: call `onLevel` with the display level (0 to 1) of each frame of the next sessions. */
	setLevel(onLevel: ((level: number) => void) | null): void {
		this.#mic.level = onLevel;
	}

	/** The microphone of the SDK: start the capture now, before the models of a session load, so the first words are not lost. A session that does not come needs `closeEarly`. */
	async openEarly(): Promise<void> {
		await this.#mic.openEarly();
	}

	/** Stop a capture of `openEarly` that no session took. */
	closeEarly(): void {
		this.#mic.closeEarly();
	}

	/** Stop the session, stop the engine, and end the iterators after the events that wait. */
	close(): Promise<void> {
		this.#closed ??= (async () => {
			try {
				this.#mic.closeEarly();
				const ready = await this.#ready?.catch(() => undefined);
				if (ready) {
					await ready.live.close().catch(() => undefined);
					await ready.speech.close();
				}
			} finally {
				this.#events.end();
			}
		})();
		return this.#closed;
	}

	[Symbol.asyncDispose](): Promise<void> {
		return this.close();
	}
}

