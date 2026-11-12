import { sourceBridge, type AudioSource } from "./capture.js";
import { Engine } from "./engine.js";
import { SaysoError } from "./errors.js";
import { EventHub, LoopHold, WorkSet } from "./events.js";
import { bridge, native, settle, unwrap, type NativeSpeech } from "./native.js";
import type {
	Capability,
	Enhance,
	EnhanceReply,
	ErrorCode,
	ErrorReply,
	Event as Message,
	ModelInfo,
	Role,
	SegmentEvent,
	SessionReply,
	SessionStart,
	SpeakersEvent,
	TranscribeFile,
} from "./protocol/index.js";

/** The mode that an app asks for. */
export type Mode =
	/** Use the engine of the app. Never talk to Sayso. */
	| "local"
	/** Use Sayso when it runs, else the engine of the app. Never starts Sayso. */
	| "auto"
	/** Use Sayso, and fail when that is not possible. */
	| "sayso"
	/**
	 * Use Sayso. With `start`, start Sayso when it does not run. Sayso has hotkeys and a
	 * menu bar icon, so only do this when the user asked for it.
	 */
	| { sayso: { start?: boolean } };

/** The mode that a `Speech` uses now. */
export type ActiveMode = "sayso" | "local";

/**
 * The models of the app, for a request without a model in local mode. A field that is
 * missing takes the first suitable model on disk. In Sayso mode, a request without a model
 * gets the choice of the user, not these.
 */
export interface ModelChoices {
	finalModel?: string;
	previewModel?: string;
	/** A model id, or `basic` for Basic VAD. */
	vadModel?: string;
	diarizationModel?: string;
	/** A code such as `en`. The default is `auto`. */
	language?: string;
}

/** The times of the SDK in milliseconds. The defaults are the times of `docs/sdk.md`. */
export interface Timing {
	/** How often local mode looks for the socket of Sayso. Default 200. */
	watchMs?: number;
	/** After a `hello` that failed, local mode tries again after this time, or when the socket changes. Default 30000. */
	retryMs?: number;
	/** How long `{ sayso: { start: true } }` waits for Sayso. Default 10000. */
	startWaitMs?: number;
	/** How long the first reply to `hello` can take, unless Sayso said that it asks the user. Default 10000. */
	helloLimitMs?: number;
	/** How long local mode keeps a model in memory after the last work that used it. Default 60000. */
	keepModelMs?: number;
}

/** What `Speech.connect` needs. */
export interface SpeechOptions {
	mode: Mode;
	/** Sayso shows the name as a claim of the app, after the name from the system. */
	app: { name: string };
	/** What the app wants to use in Sayso. The approval question lists them. */
	capabilities?: Capability[];
	/** Needed for `local` and `auto`. */
	engine?: Engine;
	models?: ModelChoices;
	/** The socket of Sayso. The default is the rule of the protocol. */
	socket?: string;
	timing?: Timing;
	/**
	 * The sources of local mode instead of the capture of the SDK. A source that is
	 * missing is the real capture. Sayso mode does not use this, because Sayso captures there.
	 */
	capture?: { microphone?: AudioSource; systemAudio?: AudioSource };
	/** How `{ sayso: { start: true } }` starts Sayso. The default starts the installed Sayso. */
	start?: () => void | Promise<void>;
}

/** What the mode that the app has now can do. */
export interface Features {
	/** `enhance`: the providers and styles of the user. In Sayso mode only. */
	enhance: boolean;
	/** `subscribe`: `dictation_done`. In Sayso mode only. */
	subscribe: boolean;
	/** The user answers a question before a download. In Sayso mode only. */
	downloadQuestion: boolean;
	/** The source `system` of a live session can start. */
	systemAudio: boolean;
	/** In Sayso mode, what the user granted. In local mode, what the SDK can do without a grant. */
	capabilities: Capability[];
	/** In Sayso mode, what the user refused. */
	denied: Capability[];
	/** The optional parts of the Host API, in Sayso mode. */
	host: string[];
	/** The features of the engine, in local mode after the engine started. */
	engine: string[] | null;
}

/** What the app gets from `Speech.events()` and `Speech.on()`. */
export type SpeechEvent =
	/**
	 * A message of the Host API, with the ids of the SDK: `job_progress`, `segment`,
	 * `speakers`, `job_done`, `job_failed`, `partial`, `source_gap`, `source_ended`,
	 * `session_ended`, `model_state`, `download_progress`, or `dictation_done`.
	 */
	| { kind: "message"; message: Message }
	/** The mode changed. No work of the app is active at this moment. */
	| { kind: "mode_changed"; mode: ActiveMode; features: Features }
	/** An `error` that belongs to no request. */
	| { kind: "error"; error: ErrorReply }
	/** Sayso mode: the connection ended. Requests fail until `reconnect()`. */
	| { kind: "disconnected"; code: ErrorCode };

/** The result of `Speech.transcribe`. */
export interface Transcript {
	job: number;
	text: string;
	/** The `segment` events of the job, in order. */
	segments: SegmentEvent[];
	/** The `speakers` event of the job, when it was diarized. */
	speakers?: SpeakersEvent;
}

function messageKey(message: Message): { end?: string } {
	switch (message.type) {
		case "job_done":
		case "job_failed":
			return { end: `job:${message.job}` };
		case "session_ended":
			return { end: `session:${message.session}` };
		default:
			return {};
	}
}

/**
 * The app uses speech through this: with Sayso when it runs, and with the engine of the
 * app when it does not (`docs/sdk.md`). Call `close()` when done, or use `await using`.
 */
export class Speech implements AsyncDisposable {
	readonly #native: NativeSpeech;
	readonly #events = new EventHub<SpeechEvent>();
	readonly #hold = new LoopHold();
	readonly #work = new WorkSet(this.#hold);
	#closed: Promise<void> | undefined;

	private constructor(handle: NativeSpeech) {
		this.#native = handle;
	}

	/**
	 * Connect in the mode of the options:
	 *
	 * - `sayso` fails with `host_not_running` when Sayso does not run, and with `not_approved`
	 *   when the user refused the app.
	 * - `local` never talks to Sayso. It fails when the engine is not there.
	 * - `auto` uses Sayso when it can, else local mode.
	 *
	 * When Sayso asks the user about this app, this waits for the answer.
	 */
	static async connect(options: SpeechOptions): Promise<Speech> {
		const { capture, start, engine, ...rest } = options;
		const json = JSON.stringify({ ...rest, engine: engine?.toJSON() });
		const handle = new (native().NativeSpeech)();
		const microphone = capture?.microphone ? sourceBridge(capture.microphone) : undefined;
		const systemAudio = capture?.systemAudio ? sourceBridge(capture.systemAudio) : undefined;
		const startFn = start ? bridge({ start: async () => void (await start()) }) : undefined;
		await settle<string>(handle.connect(json, microphone, systemAudio, startFn));
		const speech = new Speech(handle);
		handle.listen((text) => speech.#deliver(text));
		return speech;
	}

	#deliver(text: string): void {
		let event: SpeechEvent;
		try {
			event = unwrap<SpeechEvent>(text);
		} catch (e) {
			event = { kind: "error", error: SaysoError.replyOf(e) };
		}
		if (event.kind === "message") {
			const { end } = messageKey(event.message);
			if (end) this.#work.ended(end);
		} else if (event.kind === "mode_changed" || event.kind === "disconnected") {
			// A change of mode happens only without work, and a lost connection ends all work.
			this.#work.clear();
		}
		this.#events.emit(event);
	}

	/** The mode that a request uses now. */
	mode(): ActiveMode {
		return unwrap<ActiveMode>(this.#native.mode());
	}

	/** What the mode that the app has now can do. The event `mode_changed` says when that changes. */
	features(): Features {
		return unwrap<Features>(this.#native.features());
	}

	/**
	 * The events from now on: the events of work and the changes of the mode. Each iterator
	 * and each listener of `on` gets every event.
	 */
	events(options?: { signal?: AbortSignal }): AsyncIterableIterator<SpeechEvent> {
		return this.#events.iterate(options?.signal);
	}

	/** Call `listener` for each event from now on. The result removes it. */
	on(listener: (event: SpeechEvent) => void): () => void {
		return this.#events.on(listener);
	}

	/** The local models of this computer with their state. With `role`, only those that have it. */
	listModels(role?: Role): Promise<ModelInfo[]> {
		return settle<ModelInfo[]>(this.#native.listModels(role));
	}

	/**
	 * Download a model. In Sayso mode the user answers a question first, and can refuse
	 * (`declined`). `reason` is for that question. In local mode the download starts at once:
	 * the app asks its own user. `model_state` and `download_progress` come as events.
	 */
	async downloadModel(model: string, reason: string): Promise<void> {
		await settle<null>(this.#native.downloadModel(model, reason));
	}

	/**
	 * Start a job. It gives the id of the job, and the job ends with one `job_done` or
	 * `job_failed`. The path must be absolute.
	 */
	async transcribeFile(request: TranscribeFile): Promise<number> {
		const job = await settle<number>(this.#native.transcribeFile(JSON.stringify(request)));
		this.#work.started(`job:${job}`);
		return job;
	}

	/**
	 * Transcribe a file and wait for the end of the job: the text, the segments, and the
	 * speakers. A `job_failed` rejects with a `SaysoError`. The signal cancels the job and
	 * rejects with the reason of the signal (an `AbortError` by default).
	 */
	async transcribe(request: TranscribeFile, options?: { signal?: AbortSignal }): Promise<Transcript> {
		// The iterator starts before the job, so it has every event of the job.
		const events = this.#events.iterate(options?.signal);
		const segments: SegmentEvent[] = [];
		let speakers: SpeakersEvent | undefined;
		try {
			const job = await this.transcribeFile(request);
			const done = (message: Message): Transcript | undefined => {
				if (message.type === "segment" && message.job === job) segments.push(message);
				if (message.type === "speakers" && message.job === job) speakers = message;
				if (message.type === "job_failed" && message.job === job) {
					throw new SaysoError({ code: message.code ?? null, message: message.message });
				}
				if (message.type === "job_done" && message.job === job) {
					const transcript: Transcript = { job, text: message.text, segments };
					if (speakers) transcript.speakers = speakers;
					return transcript;
				}
				return undefined;
			};
			for await (const event of events) {
				if (event.kind !== "message") continue;
				const result = done(event.message);
				if (result) return result;
			}
			if (options?.signal?.aborted) {
				await this.cancelJob(job);
				throw options.signal.reason;
			}
			throw new SaysoError({ code: "connection_lost", message: "the Speech closed before the end of the job" });
		} finally {
			await events.return?.();
		}
	}

	/**
	 * Cancel a job. It ends with `job_failed`. A job of this app that ended already gives
	 * no error, so a cancel that crosses the end of the job is not an error.
	 */
	async cancelJob(job: number): Promise<void> {
		await settle<null>(this.#native.cancelJob(job));
	}

	/**
	 * Start a live session. It gives the id of the session, and what the capture of the
	 * source `system` leaves out. It ends with `stopSession`, or else with the event
	 * `session_ended`. A session with the source `client` gets its audio from
	 * `sessionAudio`. A refused capture gives the code `permission`.
	 */
	async startSession(request: SessionStart): Promise<SessionReply> {
		const reply = await settle<SessionReply>(this.#native.startSession(JSON.stringify(request)));
		this.#work.started(`session:${reply.session}`);
		return reply;
	}

	/** Audio of the source `client`: 16 kHz mono 16-bit samples. It only sends, so it does not wait. */
	sessionAudio(session: number, samples: Int16Array): void {
		unwrap<null>(this.#native.sessionAudio(session, samples));
	}

	/** End the audio, wait for the last parts, and give the text of the session. */
	async stopSession(session: number): Promise<string> {
		try {
			return await settle<string>(this.#native.stopSession(session));
		} finally {
			this.#work.ended(`session:${session}`);
		}
	}

	/** Sayso mode: clean up a text with the providers and styles of the user. */
	enhance(request: Enhance): Promise<EnhanceReply> {
		return settle<EnhanceReply>(this.#native.enhance(JSON.stringify(request)));
	}

	/** Sayso mode: get events such as `dictation_done`. Gives the events that Sayso will send. */
	subscribe(events: string[]): Promise<string[]> {
		return settle<string[]>(this.#native.subscribe(events));
	}

	/**
	 * Sayso mode: connect again after `disconnected`. Work does not survive the end of a
	 * connection. In the other modes the SDK does this by itself, and this does nothing.
	 */
	async reconnect(): Promise<void> {
		await settle<null>(this.#native.reconnect());
	}

	/**
	 * End the work, close the connection or stop the engine, and end the iterators of
	 * `events()` after the events that wait. A second call gives the same promise.
	 */
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
