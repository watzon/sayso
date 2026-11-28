// Local mode: the engine of the app, with jobs and live sessions of the SDK.

import { isAbsolute } from "node:path";
import { helperSource, type AudioSource } from "../capture.js";
import { findAudioHelper, helperCapturesSystem } from "../audio/helper.js";
import { i16ToFloats } from "../audio/wav.js";
import { Engine, type EngineProcess } from "../engine.js";
import { SaysoError } from "../errors.js";
import type { Event, ModelInfo, ModelState, Role, SessionReply, SessionStart, TimestampLevel, TranscribeFile } from "../protocol/index.js";
import { EngineGate, EngineSpeech } from "./engine.js";
import { JobQueue, JobRunner, type JobSpec, type VadRun } from "./jobs.js";
import { LiveSessions, type SessionSpec } from "./live.js";
import type { ModelChoices } from "./models.js";

/** What Local mode needs. */
export interface LocalOptions {
	engine: Engine;
	models?: ModelChoices;
	/** The folder for the models. Default: the models folder of Sayso. */
	modelsDir?: string;
	/** The sources of live sessions instead of the capture of the SDK. */
	capture?: { microphone?: AudioSource; systemAudio?: AudioSource };
	/** How long a model that nobody uses stays in memory. Default 60000. */
	keepModelMs?: number;
	/** A folder for the recordings of live sessions without `record_to`. */
	tempDir?: string;
	/** A function that gets a copy of what the engine writes to stderr. */
	stderr?: (line: string) => void;
	/** More environment for the engine. */
	env?: Record<string, string>;
	/** How long the engine may take to answer `hello`. */
	helloTimeoutMs?: number;
}

const NOT_USABLE: ModelState[] = ["not_downloaded", "downloading", "failed"];

function asModelState(value: unknown): ModelState {
	return typeof value === "string" ? (value as ModelState) : "not_downloaded";
}

/** Local mode over an engine program. */
export class LocalBackend {
	readonly speech: EngineSpeech;
	readonly #process: EngineProcess;
	readonly #emit: (event: Event) => void;
	readonly #choices: ModelChoices;
	readonly #gate = new EngineGate();
	readonly #queue: JobQueue;
	readonly #live: LiveSessions;
	readonly microphone: AudioSource | undefined;
	readonly systemAudio: AudioSource | undefined;
	#closed = false;

	private constructor(process: EngineProcess, options: LocalOptions, emit: (event: Event) => void) {
		this.#process = process;
		this.#emit = emit;
		this.#choices = options.models ?? {};
		this.speech = new EngineSpeech(process, { ...(options.keepModelMs === undefined ? {} : { keepModelMs: options.keepModelMs }) });
		const helper = findAudioHelper() !== undefined;
		this.microphone = options.capture?.microphone ?? (helper ? helperSource("mic") : undefined);
		this.systemAudio = options.capture?.systemAudio ?? (helper && helperCapturesSystem() ? helperSource("system") : undefined);
		const runner = new JobRunner(this.speech, { hold: (models) => this.speech.hold(models) });
		this.#queue = new JobQueue(runner, this.#gate);
		this.#live = new LiveSessions({
			engine: this.speech,
			gate: this.#gate,
			hold: (models) => this.speech.hold(models),
			emit,
			appPid: globalThis.process.pid,
			...(this.microphone ? { microphone: this.microphone } : {}),
			...(this.systemAudio ? { systemAudio: this.systemAudio } : {}),
			...(options.tempDir ? { recordingDir: options.tempDir } : {}),
		});
		void process.done.then(() => this.#engineStopped());
	}

	/** Start the engine of the app. */
	static async start(options: LocalOptions, emit: (event: Event) => void): Promise<LocalBackend> {
		const process = await options.engine.start({
			...(options.modelsDir ? { modelsDir: options.modelsDir } : {}),
			...(options.stderr ? { stderr: options.stderr } : {}),
			...(options.env ? { env: options.env } : {}),
			...(options.helloTimeoutMs ? { helloTimeoutMs: options.helloTimeoutMs } : {}),
		});
		return new LocalBackend(process, options, emit);
	}

	get engineFeatures(): string[] {
		return this.#process.hello.features;
	}

	get alive(): boolean {
		return !this.#closed && !this.#process.exited;
	}

	/** True while a job or a live session runs. */
	get busy(): boolean {
		return this.#queue.busy || this.#live.busy;
	}

	#engineStopped(): void {
		if (this.#closed) return;
		this.#queue.endAll("the engine stopped", "unavailable");
		const session = this.#live.current;
		if (session !== undefined) this.#emit({ type: "session_ended", session, text: "", code: "unavailable" });
	}

	// ---- Models ----

	async #catalog(): Promise<ModelInfo[]> {
		let catalog: Awaited<ReturnType<EngineProcess["catalog"]>>;
		try {
			catalog = await this.#process.catalog();
		} catch (e) {
			const error = SaysoError.from(e);
			throw error.code === "unavailable" ? error : SaysoError.of("unsupported", `the engine has no catalog: ${error.message}`);
		}
		const states = new Map<string, ModelState>();
		for (const s of catalog.states) states.set(s.model, asModelState(s.state));
		return catalog.models.map((m) => {
			const info: ModelInfo = {
				id: m.id,
				name: m.name,
				roles: m.roles,
				state: states.get(m.id) ?? "not_downloaded",
				languages: m.languages ?? [],
				timestamps: m.timestamps as TimestampLevel,
				size_bytes: m.size_bytes,
			};
			const choice = this.#choiceRoles(m.id);
			if (choice.length > 0) info.choice = choice;
			return info;
		});
	}

	#choiceRoles(id: string): Role[] {
		const c = this.#choices;
		const out: Role[] = [];
		if (c.finalModel === id) out.push("final");
		if (c.previewModel === id) out.push("streaming");
		if (c.vadModel === id) out.push("vad");
		if (c.diarizationModel === id) out.push("diarization");
		return out;
	}

	async listModels(role?: Role): Promise<ModelInfo[]> {
		const all = await this.#catalog();
		return role === undefined ? all : all.filter((m) => m.roles.includes(role));
	}

	async downloadModel(model: string): Promise<void> {
		const all = await this.#catalog();
		const info = all.find((m) => m.id === model);
		if (!info) throw SaysoError.of("unknown_model", `the engine has no model ${model}`);
		await this.#process.download(model, {
			sizeBytes: info.size_bytes,
			onEvent: (m) => {
				if (m.type === "download_progress") {
					this.#emit({ type: "download_progress", model: String(m.model), fraction: Number(m.fraction ?? 0), bytes_done: Number(m.bytes_done ?? 0), bytes_total: Number(m.bytes_total ?? 0) });
				} else if (m.type === "model_state") {
					this.#emit({
						type: "model_state",
						model: String(m.model),
						state: asModelState(m.state),
						...(typeof m.load_ms === "number" ? { load_ms: m.load_ms } : {}),
						...(typeof m.preview === "boolean" ? { preview: m.preview } : {}),
						...(typeof m.message === "string" ? { message: m.message } : {}),
						...(typeof m.reason === "string" ? { reason: m.reason } : {}),
					});
				}
			},
		});
	}

	/** The model of a request: the one named, else the one of the app's choices, else the first one of the role that is on disk. */
	async #pick(named: string | undefined, role: Role, chosen: string | undefined): Promise<ModelInfo> {
		const all = await this.#catalog();
		const want = named ?? chosen;
		if (want !== undefined) {
			const m = all.find((x) => x.id === want);
			if (!m) throw SaysoError.of("unknown_model", `the engine has no model ${want}`);
			if (!m.roles.includes(role)) throw SaysoError.of("wrong_role", `the model ${m.id} cannot do ${role}`);
			if (NOT_USABLE.includes(m.state)) throw SaysoError.of("model_not_downloaded", `the model ${m.id} is not downloaded`);
			return m;
		}
		const found = all.find((m) => m.roles.includes(role) && !NOT_USABLE.includes(m.state));
		if (!found) throw SaysoError.of("model_not_downloaded", `no ${role} model is downloaded`);
		return found;
	}

	async #vad(named: string | undefined): Promise<VadRun> {
		const want = named ?? this.#choices.vadModel;
		if (want === undefined || want === "basic") return { basic: true };
		if (!this.#process.has("vad")) throw SaysoError.of("unsupported", "the engine has no VAD models");
		return { basic: false, model: (await this.#pick(want, "vad", undefined)).id };
	}

	// ---- Jobs ----

	async transcribeFile(req: TranscribeFile): Promise<number> {
		if (!isAbsolute(req.path)) throw SaysoError.of("bad_request", `path must be absolute: ${req.path}`);
		if (req.speakers !== undefined && req.speakers === 0) throw SaysoError.of("bad_request", "speakers must be 1 or more");
		const model = await this.#pick(req.model, "final", this.#choices.finalModel);
		const vad = await this.#vad(req.vad_model);
		const job: JobSpec = { path: req.path, model: model.id, timestamps: model.timestamps, vad, language: req.language ?? this.#choices.language ?? "auto" };
		if (req.diarize === true || req.diarization_model !== undefined) {
			if (!this.#process.has("diarization")) throw SaysoError.of("unsupported", "the engine has no diarization");
			job.diarization = (await this.#pick(req.diarization_model, "diarization", this.#choices.diarizationModel)).id;
			if (req.speakers !== undefined) job.speakers = req.speakers;
		}
		return this.#queue.submit(job, this.#emit);
	}

	cancelJob(job: number): void {
		if (!this.#queue.cancel(job)) throw SaysoError.of("bad_request", `this connection has no job ${job}`);
	}

	// ---- Live sessions ----

	async startSession(req: SessionStart): Promise<SessionReply> {
		const final = await this.#pick(req.model, "final", this.#choices.finalModel);
		let previewModel: string | undefined;
		const wanted = req.preview_model ?? this.#choices.previewModel;
		if (wanted !== undefined) previewModel = (await this.#pick(wanted, "streaming", undefined)).id;
		else if (final.roles.includes("streaming")) previewModel = final.id;
		const spec: SessionSpec = { sources: req.sources, finalModel: final.id, vad: await this.#vad(req.vad_model), language: req.language ?? this.#choices.language ?? "auto" };
		if (previewModel) spec.previewModel = previewModel;
		if (req.diarize === true || req.diarization_model !== undefined) {
			if (!this.#process.has("diarization")) throw SaysoError.of("unsupported", "the engine has no diarization");
			spec.diarization = (await this.#pick(req.diarization_model, "diarization", this.#choices.diarizationModel)).id;
		}
		if (req.record_to !== undefined) spec.recordTo = req.record_to;
		if (req.silence_limit_ms !== undefined) spec.silenceLimitMs = req.silence_limit_ms;
		return this.#live.start(spec);
	}

	sessionAudio(session: number, samples: Int16Array): void {
		this.#live.audio(session, i16ToFloats(samples));
	}

	stopSession(session: number): Promise<string> {
		return this.#live.stop(session);
	}

	async close(): Promise<void> {
		if (this.#closed) return;
		this.#closed = true;
		this.#queue.close();
		await this.#live.close().catch(() => undefined);
		await this.speech.close();
	}
}
