import { stat } from "node:fs/promises";
import { Client, startWaitMs, type ClientEvent, type HostEvent, type Launcher } from "./client.js";
import type { AudioSource } from "./capture.js";
import { findAudioHelper } from "./audio/helper.js";
import { Engine } from "./engine.js";
import { SaysoError } from "./errors.js";
import { EventHub } from "./events.js";
import { LocalBackend } from "./local/backend.js";
import type { ModelChoices } from "./local/models.js";
import { clientSocketPath } from "./paths.js";
import type { Capability, Enhance, EnhanceReply, ErrorCode, ErrorReply, Event as Message, ModelInfo, Role, SegmentEvent, SessionReply, SessionStart, SpeakersEvent, TranscribeFile } from "./protocol/index.js";

export type { ModelChoices } from "./local/models.js";

/** The mode that an app asks for. */
export type Mode =
	/** Use the engine of the app. Never talk to Sayso. */
	| "local"
	/** Use Sayso when it runs, else the engine of the app. Never starts Sayso. */
	| "auto"
	/** Use Sayso, and fail when that is not possible. */
	| "sayso"
	/**
	 * Use Sayso. With `start`, start Sayso when it does not run. Sayso has hotkeys and a menu bar icon, so only do this
	 * when the user asked for it.
	 */
	| { sayso: { start?: boolean } };

/** The mode that a `Speech` uses now. */
export type ActiveMode = "sayso" | "local";

/** The times of the SDK in milliseconds. */
export interface Timing {
	/** How often `auto` looks for Sayso while it works in Local mode. Default 200. */
	watchMs?: number;
	/** After a `hello` that failed, `auto` tries again after this time. Default 30000. */
	retryMs?: number;
	/** How long `{ sayso: { start: true } }` waits for Sayso. Default 10000. */
	startWaitMs?: number;
	/** How long the first reply to `hello` can take, unless Sayso said that it asks the user. Default 10000. */
	helloLimitMs?: number;
	/** How long Local mode keeps a model in memory after the last work that used it. Default 60000. */
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
	 * The sources of Local mode instead of the capture of the SDK. A source that is missing is the capture of the audio
	 * helper, when it is there. Sayso mode does not use this, because Sayso captures there.
	 */
	capture?: { microphone?: AudioSource; systemAudio?: AudioSource };
	/** How `{ sayso: { start: true } }` starts Sayso. The default starts the installed Sayso. */
	start?: () => void | Promise<void>;
	/** The ways to start Sayso for the default `start`. The default is `launchers()`. */
	launchers?: Launcher[];
	/** Called when Sayso asks the user about this app. */
	onApprovalWaiting?: () => void;
	/** The folder for the models of Local mode. Default: the models folder of Sayso, which Local mode shares. */
	modelsDir?: string;
	/** A function that gets a copy of what the engine of Local mode writes to stderr. */
	engineStderr?: (line: string) => void;
	/** More environment for the engine of Local mode. */
	engineEnv?: Record<string, string>;
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
	/** In Sayso mode, what the user granted. In Local mode, what the SDK can do without a grant. */
	capabilities: Capability[];
	/** In Sayso mode, what the user refused. */
	denied: Capability[];
	/** The optional parts of the Host API, in Sayso mode. Empty for a Sayso older than the list. */
	host: string[];
	/** The features of the engine, in Local mode. */
	engine: string[] | null;
}

/** What the app gets from `Speech.events()` and `Speech.on()`. */
export type SpeechEvent =
	/**
	 * A message of the Host API, with the ids of the SDK: `job_progress`, `segment`, `speakers`, `job_done`, `job_failed`,
	 * `partial`, `source_gap`, `source_ended`, `part_failed`, `session_ended`, `silence_limit`, `model_state`,
	 * `download_progress`, `dictation_done`, or `host_closing`.
	 */
	| { kind: "message"; message: HostEvent }
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

/** The mode of the app: one of the two backends. */
interface SaysoSide {
	kind: "sayso";
	client: Client;
}

interface LocalSide {
	kind: "local";
	local: LocalBackend;
}

type Side = SaysoSide | LocalSide;

const DEFAULT_WATCH_MS = 200;
const DEFAULT_RETRY_MS = 30_000;

/**
 * The app uses speech through this: with Sayso when it runs, and with the engine of the app when it does not. Call `close()`
 * when done, or use `await using`.
 */
export class Speech implements AsyncDisposable {
	readonly #options: SpeechOptions;
	readonly #events = new EventHub<SpeechEvent>();
	readonly #jobs = new Set<number>();
	readonly #sessions = new Set<number>();
	#side: Side;
	#closed: Promise<void> | undefined;
	#isClosed = false;
	#watch: NodeJS.Timeout | undefined;
	#switching = false;
	#retryAfter = 0;
	#socketStamp: string | undefined;
	#unsubscribe: (() => void) | undefined;

	private constructor(options: SpeechOptions, side: Side) {
		this.#options = options;
		this.#side = side;
		this.#attach(side);
	}

	/**
	 * Connect in the mode of the options:
	 *
	 * - `sayso` fails with `host_not_running` when Sayso does not run, and with `not_approved` when the user refused the app.
	 * - `local` never talks to Sayso. It fails when the engine is not there.
	 * - `auto` uses Sayso when it can, else Local mode.
	 *
	 * When Sayso asks the user about this app, this waits for the answer.
	 */
	static async connect(options: SpeechOptions): Promise<Speech> {
		const mode = options.mode;
		let side: Side;
		if (mode === "local") {
			side = await Speech.#startLocal(options, undefined);
		} else if (mode === "auto") {
			try {
				side = await Speech.#startSayso(options, false);
			} catch (e) {
				if (!(e instanceof SaysoError) || e.code !== "host_not_running") throw e;
				side = await Speech.#startLocal(options, undefined);
			}
		} else {
			const start = typeof mode === "object" && mode.sayso.start === true;
			side = await Speech.#startSayso(options, start);
		}
		const speech = new Speech(options, side);
		if (mode === "auto") speech.#startWatching();
		return speech;
	}

	static async #startSayso(options: SpeechOptions, start: boolean): Promise<SaysoSide> {
		const timing = options.timing ?? {};
		const client = await Client.connect({
			...(options.socket ? { path: options.socket } : {}),
			app: { name: options.app.name, ...(options.capabilities ? { capabilities: options.capabilities } : {}) },
			start,
			waitMs: timing.startWaitMs ?? startWaitMs(),
			...(options.start ? { startSayso: options.start } : {}),
			...(options.launchers ? { launchers: options.launchers } : {}),
			...(timing.helloLimitMs ? { helloLimitMs: timing.helloLimitMs } : {}),
			...(options.onApprovalWaiting ? { onApprovalWaiting: options.onApprovalWaiting } : {}),
		});
		return { kind: "sayso", client };
	}

	static async #startLocal(options: SpeechOptions, emit: ((event: Message) => void) | undefined): Promise<LocalSide> {
		if (!options.engine) throw SaysoError.of("unsupported", "Local mode needs an engine: set the option `engine`");
		// The events of the backend go to the hub of the Speech that gets this side; until then they wait here.
		const early: Message[] = [];
		let forward: (event: Message) => void = emit ?? ((event) => void early.push(event));
		const local = await LocalBackend.start(
			{
				engine: options.engine,
				...(options.models ? { models: options.models } : {}),
				...(options.modelsDir ? { modelsDir: options.modelsDir } : {}),
				...(options.capture ? { capture: options.capture } : {}),
				...(options.timing?.keepModelMs ? { keepModelMs: options.timing.keepModelMs } : {}),
				...(options.engineStderr ? { stderr: options.engineStderr } : {}),
				...(options.engineEnv ? { env: options.engineEnv } : {}),
			},
			(event) => forward(event),
		);
		const side: LocalSide & { setForward?: (f: (event: Message) => void) => void } = { kind: "local", local };
		side.setForward = (f) => {
			forward = f;
			for (const event of early.splice(0)) f(event);
		};
		return side;
	}

	/** Make `side` the active one and route its events. */
	#attach(side: Side): void {
		this.#unsubscribe?.();
		this.#unsubscribe = undefined;
		if (side.kind === "sayso") {
			this.#unsubscribe = side.client.on((event) => this.#clientEvent(side, event));
		} else {
			(side as LocalSide & { setForward?: (f: (event: Message) => void) => void }).setForward?.((message) => this.#deliver(message));
		}
	}

	#clientEvent(side: SaysoSide, event: ClientEvent): void {
		if (side !== this.#side) return;
		switch (event.kind) {
			case "host":
				this.#deliver(event.event);
				break;
			case "error":
				this.#events.emit({ kind: "error", error: event.error });
				break;
			case "host_gone":
				this.#jobs.clear();
				this.#sessions.clear();
				this.#events.emit({ kind: "disconnected", code: event.code ?? "connection_lost" });
				break;
		}
	}

	#deliver(message: HostEvent): void {
		switch (message.type) {
			case "job_done":
			case "job_failed":
				this.#jobs.delete((message as { job: number }).job);
				break;
			case "session_ended":
				this.#sessions.delete((message as { session: number }).session);
				break;
		}
		this.#events.emit({ kind: "message", message });
	}

	// ---- The mode ----

	/** The mode that a request uses now. */
	mode(): ActiveMode {
		return this.#side.kind;
	}

	/** What the mode that the app has now can do. The event `mode_changed` says when that changes. */
	features(): Features {
		return this.#featuresOf(this.#side);
	}

	#featuresOf(side: Side): Features {
		if (side.kind === "sayso") {
			const hello = side.client.hello();
			const host = side.client.hostFeatures();
			return {
				enhance: host.length === 0 || host.includes("enhance"),
				subscribe: true,
				downloadQuestion: host.length === 0 || host.includes("download_question"),
				systemAudio: host.includes("system_capture"),
				capabilities: hello.capabilities ?? [],
				denied: hello.denied ?? [],
				host,
				engine: null,
			};
		}
		const local = side.local;
		const capabilities: Capability[] = ["models.read", "models.download", "transcribe.file", "session.live"];
		if (local.microphone) capabilities.push("audio.mic");
		if (local.systemAudio) capabilities.push("audio.system");
		return { enhance: false, subscribe: false, downloadQuestion: false, systemAudio: local.systemAudio !== undefined, capabilities, denied: [], host: [], engine: [...local.engineFeatures] };
	}

	#idle(): boolean {
		return this.#jobs.size === 0 && this.#sessions.size === 0 && !(this.#side.kind === "local" && this.#side.local.busy);
	}

	/** The side for new work. In `auto` it changes the mode while idle. */
	async #current(): Promise<Side> {
		if (this.#isClosed) throw SaysoError.of("connection_lost", "the Speech is closed");
		if (this.#options.mode === "auto") await this.#reconsider();
		return this.#side;
	}

	// ---- Auto: Sayso when it runs, else Local ----

	#startWatching(): void {
		const every = this.#options.timing?.watchMs ?? DEFAULT_WATCH_MS;
		this.#watch = setInterval(() => void this.#reconsider().catch(() => undefined), every);
		this.#watch.unref();
	}

	/** The change of the mode in `auto`: only while no work of the app is active. */
	async #reconsider(): Promise<void> {
		if (this.#switching || this.#isClosed || !this.#idle()) return;
		this.#switching = true;
		try {
			const side = this.#side;
			let next: Side | undefined;
			if (side.kind === "local") {
				if (Date.now() < this.#retryAfter && (await this.#socketChanged()) === false) return;
				try {
					const sayso = await Speech.#startSayso(this.#options, false);
					if (!this.#idle() || this.#isClosed) {
						await sayso.client.close();
						return;
					}
					next = sayso;
				} catch (e) {
					if (e instanceof SaysoError && e.code !== "host_not_running") this.#retryAfter = Date.now() + (this.#options.timing?.retryMs ?? DEFAULT_RETRY_MS);
					if (side.local.alive) return;
					next = await Speech.#startLocal(this.#options, undefined);
				}
			} else if (!side.client.connected) {
				next = await Speech.#startLocal(this.#options, undefined);
			} else {
				return;
			}
			if (this.#isClosed || !this.#idle()) {
				await Speech.#closeSide(next);
				return;
			}
			const old = this.#side;
			this.#side = next;
			this.#attach(next);
			if (old.kind === "sayso") await old.client.close().catch(() => undefined);
			else if (next.kind === "sayso") await old.local.close().catch(() => undefined);
			this.#events.emit({ kind: "mode_changed", mode: next.kind, features: this.#featuresOf(next) });
		} finally {
			this.#switching = false;
		}
	}

	/** True when the socket of Sayso changed since the last look, so a failed `hello` is worth another try. */
	async #socketChanged(): Promise<boolean> {
		let stamp: string;
		try {
			const info = await stat(this.#options.socket ?? clientSocketPath());
			stamp = `${info.mtimeMs}:${info.ino}`;
		} catch {
			stamp = "";
		}
		const changed = stamp !== "" && stamp !== this.#socketStamp;
		this.#socketStamp = stamp;
		return changed;
	}

	static async #closeSide(side: Side): Promise<void> {
		if (side.kind === "sayso") await side.client.close().catch(() => undefined);
		else await side.local.close().catch(() => undefined);
	}

	// ---- The events ----

	/** The events from now on: the events of work and the changes of the mode. Each iterator and each listener gets every event. */
	events(options?: { signal?: AbortSignal }): AsyncIterableIterator<SpeechEvent> {
		return this.#events.iterate(options?.signal);
	}

	/** Call `listener` for each event from now on. The result removes it. */
	on(listener: (event: SpeechEvent) => void): () => void {
		return this.#events.on(listener);
	}

	// ---- The requests ----

	/** The local models of this computer with their state. With `role`, only those that have it. */
	async listModels(role?: Role): Promise<ModelInfo[]> {
		const side = await this.#current();
		if (side.kind === "local") return side.local.listModels(role);
		const reply = await side.client.request<{ models: ModelInfo[] }>("list_models", role === undefined ? {} : { role });
		return reply.models;
	}

	/**
	 * Download a model. In Sayso mode the user answers a question first, and can refuse (`declined`). `reason` is for that
	 * question. In Local mode the download starts at once: the app asks its own user. `model_state` and `download_progress`
	 * come as events.
	 */
	async downloadModel(model: string, reason: string): Promise<void> {
		const side = await this.#current();
		if (side.kind === "local") return side.local.downloadModel(model);
		await side.client.request("download_model", { model, reason });
	}

	/** Start a job. It gives the id of the job, and the job ends with one `job_done` or `job_failed`. The path must be absolute. */
	async transcribeFile(request: TranscribeFile): Promise<number> {
		const side = await this.#current();
		let job: number;
		if (side.kind === "local") job = await side.local.transcribeFile(request);
		else job = (await side.client.request<{ job: number }>("transcribe_file", { ...request })).job;
		this.#jobs.add(job);
		return job;
	}

	/**
	 * Transcribe a file and wait for the end of the job: the text, the segments, and the speakers. A `job_failed` rejects with
	 * a `SaysoError`. The signal cancels the job and rejects with the reason of the signal (an `AbortError` by default).
	 */
	async transcribe(request: TranscribeFile, options?: { signal?: AbortSignal }): Promise<Transcript> {
		// The iterator starts before the job, so it has every event of the job.
		const events = this.#events.iterate(options?.signal);
		const segments: SegmentEvent[] = [];
		let speakers: SpeakersEvent | undefined;
		try {
			const job = await this.transcribeFile(request);
			for await (const event of events) {
				if (event.kind === "disconnected") throw SaysoError.of(event.code, "the connection to Sayso ended");
				if (event.kind !== "message") continue;
				const message = event.message as Message;
				if (message.type === "segment" && message.job === job) segments.push(message as SegmentEvent);
				if (message.type === "speakers" && message.job === job) speakers = message as SpeakersEvent;
				if (message.type === "job_failed" && message.job === job) throw new SaysoError({ code: message.code ?? null, message: message.message });
				if (message.type === "job_done" && message.job === job) {
					const transcript: Transcript = { job, text: message.text, segments };
					if (speakers) transcript.speakers = speakers;
					return transcript;
				}
			}
			if (options?.signal?.aborted) {
				await this.cancelJob(job);
				throw options.signal.reason;
			}
			throw SaysoError.of("connection_lost", "the Speech closed before the end of the job");
		} finally {
			await events.return?.();
		}
	}

	/** Cancel a job. It ends with `job_failed`. A job of this app that ended already gives no error. */
	async cancelJob(job: number): Promise<void> {
		if (!this.#jobs.has(job)) return;
		const side = this.#side;
		try {
			if (side.kind === "local") side.local.cancelJob(job);
			else await side.client.request("cancel_job", { job });
		} catch (e) {
			if (this.#jobs.has(job)) throw e;
		}
	}

	/**
	 * Start a live session. It gives the id of the session, and what the capture of the source `system` leaves out. It ends
	 * with `stopSession`, or else with the event `session_ended`. A session with the source `client` gets its audio from
	 * `sessionAudio`. A refused capture gives the code `permission`.
	 */
	async startSession(request: SessionStart): Promise<SessionReply> {
		const side = await this.#current();
		const req: SessionStart = { ...request, sources: request.sources.length > 0 ? request.sources : ["client"] };
		let reply: SessionReply;
		if (side.kind === "local") {
			reply = await side.local.startSession(req);
		} else {
			this.#gateSession(side.client, req);
			const raw = await side.client.request<{ session: number; excludes?: string[] }>("session_start", { ...req });
			reply = { session: raw.session, excludes: raw.excludes ?? [] };
		}
		this.#sessions.add(reply.session);
		return reply;
	}

	/** Refuse what the host says it lacks. A host with no features is older than the list, and gets the request. */
	#gateSession(client: Client, req: SessionStart): void {
		const need: Array<[boolean, string]> = [
			[req.sources.includes("mic"), "mic_capture"],
			[req.sources.includes("system"), "system_capture"],
			[req.record_to !== undefined, "record_to"],
			[req.silence_limit_ms !== undefined && req.silence_limit_ms > 0, "silence_limit"],
		];
		for (const [wanted, feature] of need) {
			if (wanted && !client.supports(feature)) throw SaysoError.of("unsupported", `this Sayso does not serve ${feature}`);
		}
	}

	/** Audio of the source `client`: 16 kHz mono 16-bit samples. It only sends, so it does not wait. */
	sessionAudio(session: number, samples: Int16Array): void {
		const side = this.#side;
		if (this.#isClosed) throw SaysoError.of("connection_lost", "the Speech is closed");
		if (side.kind === "local") side.local.sessionAudio(session, samples);
		else side.client.send("session_audio", { session, pcm: encodePcm(samples) });
	}

	/** End the audio, wait for the last parts, and give the text of the session. The last events of the session come before it. */
	async stopSession(session: number): Promise<string> {
		const side = this.#side;
		try {
			if (side.kind === "local") return await side.local.stopSession(session);
			const reply = await side.client.request<{ text: string }>("session_stop", { session });
			side.client.forgetSession(session);
			return reply.text;
		} finally {
			this.#sessions.delete(session);
		}
	}

	/** Sayso mode: clean up a text with the providers and styles of the user. */
	async enhance(request: Enhance): Promise<EnhanceReply> {
		const side = await this.#current();
		if (side.kind === "local") throw SaysoError.of("unsupported", "Local mode cannot enhance; it needs Sayso");
		if (!side.client.supports("enhance")) throw SaysoError.of("unsupported", "this version of Sayso cannot enhance");
		return side.client.request<EnhanceReply>("enhance", { ...request });
	}

	/** Sayso mode: get events such as `dictation_done`. Gives the events that Sayso will send. */
	async subscribe(events: string[]): Promise<string[]> {
		const side = await this.#current();
		if (side.kind === "local") throw SaysoError.of("unsupported", "Local mode cannot subscribe; it needs Sayso");
		if (events.includes("dictation_done") && !side.client.supports("dictation_done")) throw SaysoError.of("unsupported", "this version of Sayso cannot subscribe to dictation_done");
		return (await side.client.request<{ events: string[] }>("subscribe", { events })).events;
	}

	/**
	 * Sayso mode: connect again after `disconnected`. Work does not survive the end of a connection. In the other modes the SDK
	 * does this by itself, and this does nothing.
	 */
	async reconnect(): Promise<void> {
		const side = this.#side;
		if (side.kind !== "sayso" || this.#options.mode === "auto") return;
		await side.client.reconnect();
	}

	/**
	 * End the work, close the connection or stop the engine, and end the iterators of `events()` after the events that wait.
	 * A second call gives the same promise.
	 */
	close(): Promise<void> {
		this.#closed ??= (async () => {
			this.#isClosed = true;
			clearInterval(this.#watch);
			try {
				while (this.#switching) await new Promise((resolve) => setTimeout(resolve, 5));
				this.#unsubscribe?.();
				await Speech.#closeSide(this.#side);
			} finally {
				this.#jobs.clear();
				this.#sessions.clear();
				this.#events.end();
			}
		})();
		return this.#closed;
	}

	[Symbol.asyncDispose](): Promise<void> {
		return this.close();
	}
}

function encodePcm(samples: Int16Array): string {
	return Buffer.from(samples.buffer, samples.byteOffset, samples.byteLength).toString("base64");
}

export { findAudioHelper };
