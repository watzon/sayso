import { sourceBridge, type AudioSource } from "./capture.js";
import { Engine } from "./engine.js";
import { findEngine } from "./helpers.js";
import { SaysoError } from "./errors.js";
import { EventHub, LoopHold, WorkSet } from "./events.js";
import { bridge, native, settle, unwrap, type NativeEmbedded } from "./native.js";
import type { DownloadProgress, Event as Message, ModelState, Role, Source } from "./protocol/index.js";

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

/** What comes from `EmbeddedSpeech.events()`: `partial`, `segment`, or `silence_limit`. */
export type EmbeddedEvent = Extract<Message, { type: "partial" | "segment" | "silence_limit" }>;

/**
 * The final pass of the app: a function that gives the text of a part of the audio (16 kHz
 * mono samples from -1 to 1) for one of `models`. The other models stay with the engine.
 */
export interface FinalPassFunction {
	models: string[];
	transcribe(samples: Float32Array, model: string, language: string): Promise<string> | string;
}

/** Deepgram for the final pass of the model `deepgram-nova-3`. The key comes from the user. */
export class DeepgramFinal {
	readonly key: string;
	readonly baseUrl: string | undefined;

	constructor(key: string, options?: { baseUrl?: string }) {
		this.key = key;
		this.baseUrl = options?.baseUrl;
	}
}

/** The program and the folders of the engine, from an `Engine` or as they are. */
function programOf(engine: EmbeddedOptions["engine"]): { program: string; modelsDir: string; store?: string } {
	if (!(engine instanceof Engine)) return engine;
	const spec = engine.toJSON();
	if (!spec.modelsDir) {
		throw new SaysoError({ code: "bad_request", message: "EmbeddedSpeech needs the folder of the models: Engine.modelsDir(folder)" });
	}
	const program = spec.program ?? findEngine(spec.release ?? "");
	return { program, modelsDir: spec.modelsDir, ...(spec.store ? { store: spec.store } : {}) };
}

/** What `EmbeddedSpeech.connect` needs. */
export interface EmbeddedOptions {
	app: { name: string };
	/**
	 * The engine that the app ships: an `Engine` with `modelsDir` (for example
	 * `Engine.fromPackage().modelsDir(folder)`), or the program, the folder for the models, and
	 * the model store (default: `modelStore()`).
	 */
	engine: Engine | { program: string; modelsDir: string; store?: string };
	/** A folder for the temporary recordings of live sessions. The SDK makes it. */
	tempDir: string;
	/** The microphone of the app. Missing: the microphone of the SDK, with `setDevice` and the others. */
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

/**
 * A session in the process of the app, with an engine layer of the app, as Consort uses
 * it. `Speech` is the API for most apps. This one never talks to Sayso, and the app keeps
 * its models itself. Call `close()` when done, or use `await using`.
 */
export class EmbeddedSpeech implements AsyncDisposable {
	readonly #native: NativeEmbedded;
	readonly #events = new EventHub<EmbeddedEvent>();
	readonly #hold = new LoopHold();
	readonly #work = new WorkSet(this.#hold);
	#closed: Promise<void> | undefined;

	private constructor(handle: NativeEmbedded) {
		this.#native = handle;
	}

	/** Get ready. This starts no engine and loads no model. */
	static async connect(options: EmbeddedOptions): Promise<EmbeddedSpeech> {
		const { microphone, finalPass, engine, ...rest } = options;
		let finalJson: unknown;
		let finalFn;
		if (finalPass instanceof DeepgramFinal) {
			finalJson = { kind: "deepgram", key: finalPass.key, ...(finalPass.baseUrl ? { baseUrl: finalPass.baseUrl } : {}) };
		} else if (finalPass) {
			finalJson = { kind: "function", models: finalPass.models };
			finalFn = bridge({
				transcribe: async (json, samples) => {
					const { model, language } = json as { model: string; language: string };
					const text = await finalPass.transcribe(samples ?? new Float32Array(), model, language);
					if (typeof text !== "string") {
						throw new SaysoError({ message: "the final pass of the app must give a string" });
					}
					return text;
				},
			});
		}
		const handle = new (native().NativeEmbedded)();
		const json = JSON.stringify({ ...rest, engine: programOf(engine), ...(finalJson ? { finalPass: finalJson } : {}) });
		await settle<string>(handle.connect(json, microphone ? sourceBridge(microphone) : undefined, finalFn));
		const speech = new EmbeddedSpeech(handle);
		handle.listen((text) => speech.#deliver(text));
		return speech;
	}

	#deliver(text: string): void {
		let event: EmbeddedEvent;
		try {
			event = unwrap<EmbeddedEvent>(text);
		} catch {
			return;
		}
		this.#events.emit(event);
	}

	/** The mode of the work now. */
	mode(): "local" {
		return "local";
	}

	/** The models of the engine and their state on disk. Starts the engine. */
	models(): Promise<EmbeddedModel[]> {
		return settle<EmbeddedModel[]>(this.#native.models());
	}

	/** Download a model at once. The app asks its own user first. `sizeBytes` helps an engine that knows only a fraction. */
	async download(model: string, options?: { sizeBytes?: number; onProgress?: (progress: DownloadProgress) => void }): Promise<void> {
		const onProgress = options?.onProgress;
		this.#hold.add();
		try {
			await settle<null>(
				this.#native.download(model, options?.sizeBytes ?? null, (text) => {
					onProgress?.(JSON.parse(text) as DownloadProgress);
				}),
			);
		} finally {
			this.#hold.remove();
		}
	}

	/** Delete the files of a model. */
	async delete(model: string): Promise<void> {
		await settle<null>(this.#native.delete(model));
	}

	/** Read a model into memory. Waits while it loads. */
	async load(model: string): Promise<void> {
		await settle<null>(this.#native.load(model));
	}

	/** Take a model out of memory. */
	async unload(model: string): Promise<void> {
		await settle<null>(this.#native.unload(model));
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
		const id = await settle<number>(this.#native.startSession(JSON.stringify(session)));
		this.#work.started(`session:${id}`);
		return id;
	}

	/** Audio of the source `client`: 16 kHz mono samples from -1 to 1. */
	sessionAudio(session: number, samples: Float32Array): void {
		unwrap<null>(this.#native.sessionAudio(session, samples));
	}

	/** End the audio, wait for the last parts, and give the text of the session. The `segment` events come before this returns. */
	async stopSession(session: number): Promise<string> {
		try {
			return await settle<string>(this.#native.stopSession(session));
		} finally {
			this.#work.ended(`session:${session}`);
		}
	}

	/** Stop the session and forget the parts that wait. Returns at once. */
	cancelSession(): void {
		this.#native.cancelSession();
		this.#work.clear();
	}

	/** The microphone of the SDK: use this input device from the next session on. Null: the default device. */
	setDevice(device: string | null): void {
		this.#native.setDevice(device);
	}

	/** The microphone of the SDK: call `onLevel` with the display level (0 to 1) of each frame of the next sessions. */
	setLevel(onLevel: ((level: number) => void) | null): void {
		this.#native.setLevel(onLevel);
	}

	/**
	 * The microphone of the SDK: start the capture now, before the models of a session load,
	 * so the first words are not lost. A session that does not come needs `closeEarly`.
	 */
	async openEarly(): Promise<void> {
		await settle<null>(this.#native.openEarly());
	}

	/** Stop a capture of `openEarly` that no session took. */
	closeEarly(): void {
		this.#native.closeEarly();
	}

	/** Stop the session, stop the engine, and end the iterators after the events that wait. */
	close(): Promise<void> {
		this.#closed ??= (async () => {
			try {
				await settle<null>(this.#native.close());
			} finally {
				this.#work.clear();
				this.#hold.clear();
				this.#events.end();
			}
		})();
		return this.#closed;
	}

	[Symbol.asyncDispose](): Promise<void> {
		return this.close();
	}
}
