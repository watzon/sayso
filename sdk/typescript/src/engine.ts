// The engine: a program that the app ships, and a client of its protocol (docs/engine-protocol.md, version 1),
// which runs over the standard input and output of the program.

import { execFileSync, spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { mkdirSync, readFileSync, readdirSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { arch, platform } from "node:os";
import { dirname, join, sep } from "node:path";
import { StringDecoder } from "node:string_decoder";
import { EngineError, EngineStartError, SaysoError } from "./errors.js";
import { EventHub } from "./events.js";
import { MODELS_DIR_ENV, MODEL_STORE_ENV, modelStoreOf, modelsDir as defaultModelsDir } from "./paths.js";
import { encodeI16 } from "./protocol/codec.js";
import { LineSplitter } from "./transport.js";
import type { engine as wire } from "./protocol/index.js";
import type { ModelStateEvent, SpeechPart, Segment } from "./protocol/index.js";

/** The engine protocol of this client. */
export const ENGINE_PROTOCOL = 1;

/** A message of the engine: a reply or an event. */
export interface EngineMessage {
	type: string;
	id?: string;
	[field: string]: unknown;
}

/** How to start an engine. */
export interface EngineStartOptions {
	/** The folder for the models (`SAYSO_MODELS_DIR`). Default: the models folder of Sayso. */
	modelsDir?: string;
	/** The model store (`SAYSO_MODEL_STORE`). Default `<modelsDir>/v1`. An engine without `store_v1` ignores it. */
	store?: string;
	/** More environment, on top of the environment of this process. */
	env?: Record<string, string>;
	/** Gets a copy of what the engine writes to stderr. */
	stderr?: (line: string) => void;
	/** How long the engine may take to answer `hello`. Default 30000. */
	helloTimeoutMs?: number;
}

const STDERR_LINES = 20;

const isReply = (type: string): boolean => type === "hello" || type === "ok" || type === "final" || type === "error";

interface PendingCall {
	resolve(message: EngineMessage): void;
	reject(error: Error): void;
	onEvent?: (message: EngineMessage) => void;
}

/** A running engine. */
export class EngineProcess {
	readonly program: string;
	readonly modelsDir: string;
	readonly store: string;
	#child: ChildProcessWithoutNullStreams;
	#pending = new Map<string, PendingCall>();
	#next = 0;
	#stderr: string[] = [];
	#exit: string | undefined;
	#exited = false;
	#hello: wire.Hello = { version: "", protocol: 0, features: [] };
	readonly #events = new EventHub<EngineMessage>();
	readonly #done: Promise<void>;

	private constructor(program: string, modelsDir: string, store: string, child: ChildProcessWithoutNullStreams, copyStderr?: (line: string) => void) {
		this.program = program;
		this.modelsDir = modelsDir;
		this.store = store;
		this.#child = child;
		let resolveDone!: () => void;
		this.#done = new Promise((resolve) => (resolveDone = resolve));
		const out = new StringDecoder("utf8");
		const outLines = new LineSplitter(64 * 1024 * 1024);
		child.stdout.on("data", (chunk: Buffer) => {
			let lines: string[];
			try {
				lines = outLines.push(out.write(chunk));
			} catch {
				child.kill();
				return;
			}
			for (const line of lines) this.#line(line);
		});
		const err = new StringDecoder("utf8");
		const errLines = new LineSplitter(1024 * 1024);
		child.stderr.on("data", (chunk: Buffer) => {
			try {
				for (const line of errLines.push(err.write(chunk))) {
					copyStderr?.(line);
					this.#stderr.push(line);
					if (this.#stderr.length > STDERR_LINES) this.#stderr.shift();
				}
			} catch {
				// A line of more than a megabyte on stderr is not worth keeping.
			}
		});
		child.stdin.on("error", () => undefined);
		child.once("error", (error) => this.#end(`it did not start: ${error.message}`));
		child.once("close", (code, signal) => this.#end(signal ? `it was stopped by ${signal}` : `it exited with ${code}`, resolveDone));
	}

	/** Start the program and wait for its `hello`. Fails with an `EngineStartError` that has the last lines of stderr. */
	static async start(program: string, options: EngineStartOptions = {}): Promise<EngineProcess> {
		const modelsDir = options.modelsDir ?? defaultModelsDir();
		const store = options.store ?? modelStoreOf(modelsDir);
		mkdirSync(modelsDir, { recursive: true });
		const child = spawn(program, [], {
			stdio: ["pipe", "pipe", "pipe"],
			windowsHide: true,
			env: { ...process.env, [MODELS_DIR_ENV]: modelsDir, [MODEL_STORE_ENV]: store, ...options.env },
		});
		const engine = new EngineProcess(program, modelsDir, store, child, options.stderr);
		const limit = options.helloTimeoutMs ?? 30_000;
		let timer: NodeJS.Timeout | undefined;
		try {
			const hello = await Promise.race([
				engine.call("hello", {}),
				new Promise<never>((_, reject) => {
					timer = setTimeout(() => reject(new Error(`it did not answer hello in ${limit} ms`)), limit);
				}),
			]);
			engine.#hello = { version: String(hello.version ?? ""), protocol: Number(hello.protocol ?? 0), features: Array.isArray(hello.features) ? hello.features.map(String) : [] };
			if (engine.#hello.protocol !== ENGINE_PROTOCOL) throw new Error(`the engine speaks protocol ${engine.#hello.protocol}, this client ${ENGINE_PROTOCOL}`);
		} catch (e) {
			engine.kill();
			await engine.#done;
			const why = e instanceof Error ? e.message : String(e);
			throw new EngineStartError(program, engine.#exit && /stopped|exited/.test(why) ? why : why, engine.stderrTail());
		} finally {
			clearTimeout(timer);
		}
		return engine;
	}

	#line(line: string): void {
		let parsed: unknown;
		try {
			parsed = JSON.parse(line);
		} catch {
			return;
		}
		if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return;
		const message = parsed as EngineMessage;
		if (typeof message.type !== "string") return;
		const id = typeof message.id === "string" ? message.id : undefined;
		const pending = id === undefined ? undefined : this.#pending.get(id);
		if (pending && id !== undefined) {
			if (isReply(message.type)) {
				this.#pending.delete(id);
				if (message.type === "error") {
					pending.reject(new EngineError({ message: String(message.message ?? "the engine failed"), code: typeof message.code === "string" ? message.code : null, reason: typeof message.reason === "string" ? message.reason : null }));
				} else {
					pending.resolve(message);
				}
				return;
			}
			pending.onEvent?.(message);
		}
		if (!isReply(message.type) || id === undefined) this.#events.emit(message);
	}

	#end(why: string, done?: () => void): void {
		if (!this.#exited) {
			this.#exited = true;
			this.#exit = why;
			const error = SaysoError.of("unavailable", `the engine stopped: ${why}`);
			for (const pending of this.#pending.values()) pending.reject(error);
			this.#pending.clear();
			this.#events.end();
		}
		done?.();
	}

	/** The `hello` of the engine. */
	get hello(): wire.Hello {
		return this.#hello;
	}

	/** True when the engine lists the feature. */
	has(feature: string): boolean {
		return this.#hello.features.includes(feature);
	}

	/** True when the engine has exited. */
	get exited(): boolean {
		return this.#exited;
	}

	/** Resolves when the engine has exited. */
	get done(): Promise<void> {
		return this.#done;
	}

	/** The last lines of stderr. */
	stderrTail(): string[] {
		return [...this.#stderr];
	}

	/** The events of the engine and its errors without an id, in order. They end when the engine exits. */
	events(options?: { signal?: AbortSignal }): AsyncIterableIterator<EngineMessage> {
		return this.#events.iterate(options?.signal);
	}

	/** Call `listener` for each event, as it comes. The result removes it. */
	on(listener: (message: EngineMessage) => void): () => void {
		return this.#events.on(listener);
	}

	#write(fields: Record<string, unknown>): void {
		if (this.#exited || !this.#child.stdin.writable) throw SaysoError.of("unavailable", "the engine is not running");
		this.#child.stdin.write(`${JSON.stringify(fields)}\n`);
	}

	/**
	 * Send a request and wait for its reply. `onEvent` gets the events with the id of the request, before the reply.
	 * An error reply is an `EngineError` with the `code` and the `reason` of the engine.
	 */
	call(kind: string, fields: Record<string, unknown> = {}, onEvent?: (message: EngineMessage) => void): Promise<EngineMessage> {
		if (this.#exited) return Promise.reject(SaysoError.of("unavailable", `the engine stopped: ${this.#exit ?? "it is not running"}`));
		const id = `r${++this.#next}`;
		return new Promise<EngineMessage>((resolve, reject) => {
			this.#pending.set(id, { resolve, reject, ...(onEvent ? { onEvent } : {}) });
			try {
				this.#write({ v: 1, type: kind, id, ...fields });
			} catch (e) {
				this.#pending.delete(id);
				reject(e);
			}
		});
	}

	// ---- The requests ----

	/** The state of models. Each one comes as a `model_state`. */
	async listModels(models: wire.ListedModel[]): Promise<ModelStateEvent[]> {
		const states: ModelStateEvent[] = [];
		await this.call("list_models", { models }, (m) => m.type === "model_state" && states.push(m as unknown as ModelStateEvent));
		return states;
	}

	/** The built-in catalog (feature `catalog`), the retired ids, and the state of each model. */
	async catalog(): Promise<{ models: wire.CatalogModel[]; retired: wire.Retired[]; states: ModelStateEvent[] }> {
		const states: ModelStateEvent[] = [];
		const reply = await this.call("list_models", {}, (m) => m.type === "model_state" && states.push(m as unknown as ModelStateEvent));
		return { models: (reply.models as wire.CatalogModel[] | undefined) ?? [], retired: (reply.retired as wire.Retired[] | undefined) ?? [], states };
	}

	/** Download a model. `onEvent` gets its `model_state` and `download_progress` events. */
	async download(model: string, options: { engine?: Record<string, unknown>; sizeBytes?: number; onEvent?: (message: EngineMessage) => void } = {}): Promise<void> {
		const fields: Record<string, unknown> = { model };
		if (options.engine) fields.engine = options.engine;
		if (options.sizeBytes) fields.size_bytes = options.sizeBytes;
		await this.call("download", fields, options.onEvent);
	}

	/** Delete the files of a model. */
	async delete(model: string, engine?: Record<string, unknown>): Promise<void> {
		await this.call("delete", { model, ...(engine ? { engine } : {}) });
	}

	/** Load a model, and give its load time. */
	async load(model: string, engine?: Record<string, unknown>): Promise<number> {
		let loadMs = 0;
		await this.call("load", { model, ...(engine ? { engine } : {}) }, (m) => {
			if (m.type === "model_state" && m.state === "ready" && typeof m.load_ms === "number") loadMs = m.load_ms;
		});
		return loadMs;
	}

	/** Take a model out of memory. */
	async unload(model: string, keepPreview = false): Promise<void> {
		await this.call("unload", { model, ...(keepPreview ? { keep_preview: true } : {}) });
	}

	/** The final pass on a 16 kHz mono 16-bit WAV file. */
	async transcribe(request: { model: string; path: string; language?: string; vocabulary?: string[]; segments?: boolean }): Promise<wire.Final> {
		const fields: Record<string, unknown> = { model: request.model, path: request.path, language: request.language ?? "auto", vocabulary: request.vocabulary ?? [] };
		if (request.segments) fields.segments = true;
		const reply = await this.call("transcribe", fields);
		return { text: String(reply.text ?? ""), elapsed_ms: Number(reply.elapsed_ms ?? 0), ...(Array.isArray(reply.segments) ? { segments: reply.segments as Segment[] } : {}), ...(typeof reply.language === "string" ? { language: reply.language } : {}) };
	}

	/** Open a stream. */
	async streamStart(session: number, model: string, language?: string): Promise<void> {
		await this.call("stream_start", { session, model, ...(language ? { language } : {}) });
	}

	/** Send audio to a stream or a VAD session. It has no reply. */
	streamAudio(session: number, samples: Int16Array): void {
		this.#write({ v: 1, type: "stream_audio", session, pcm: encodeI16(samples) });
	}

	/** Close a stream and give its text (empty for a VAD session). */
	async streamEnd(session: number): Promise<string> {
		const reply = await this.call("stream_end", { session });
		return typeof reply.text === "string" ? reply.text : "";
	}

	/** The speech in a WAV file. */
	async vad(model: string, path: string): Promise<SpeechPart[]> {
		const reply = await this.call("vad", { model, path });
		return (reply.speech as SpeechPart[] | undefined) ?? [];
	}

	/** Open a VAD session. */
	async vadStart(session: number, model: string): Promise<void> {
		await this.call("vad_start", { session, model });
	}

	/** Who speaks when in a WAV file. `speakers` 0 or missing lets the engine find the number. */
	async diarize(model: string, path: string, speakers?: number): Promise<wire.DiarizeReply> {
		const reply = await this.call("diarize", { model, path, ...(speakers ? { speakers } : {}) });
		return { speakers: Number(reply.speakers ?? 0), turns: (reply.turns as wire.SpeakerTurn[] | undefined) ?? [] };
	}

	/** Whether the language model of the system is there (feature `language_model`). */
	async languageModel(): Promise<wire.LanguageModelReply> {
		return (await this.call("language_model")) as unknown as wire.LanguageModelReply;
	}

	/** Run the language model of the system (feature `language_model`). */
	async generate(request: { instructions: string; prompt: string; timeoutMs: number; temperature?: number }): Promise<wire.GenerateReply> {
		const reply = await this.call("generate", { instructions: request.instructions, prompt: request.prompt, timeout_ms: request.timeoutMs, ...(request.temperature === undefined ? {} : { temperature: request.temperature }) });
		return { text: String(reply.text ?? ""), model: String(reply.model ?? "") };
	}

	/** Ask the engine to exit. */
	async shutdown(): Promise<void> {
		await this.call("shutdown");
	}

	/** Stop the engine at once. */
	kill(): void {
		this.#child.kill();
	}

	/** Close its input and wait up to 5 s for the engine to exit, then kill it. */
	async close(): Promise<void> {
		if (this.#exited) return;
		this.#child.stdin.end();
		const timer = setTimeout(() => this.kill(), 5000);
		try {
			await this.#done;
		} finally {
			clearTimeout(timer);
		}
	}
}

// ---- Finding the program ----

const FOLDER_PREFIX = "sayso-engine-";

/** The file name of the engine in a release folder. */
export function engineProgramName(): string {
	return platform() === "win32" ? "sayso-engine.exe" : "sayso-engine";
}

/** What an x86-64 processor can run: the three builds of a release folder, best first. */
export interface CpuLevel {
	avx: boolean;
	avx2: boolean;
}

let cpuCache: CpuLevel | undefined;

function flagsOf(text: string): Set<string> {
	return new Set(text.toLowerCase().split(/[\s,]+/).filter(Boolean));
}

/** The instruction sets of this processor. Where this cannot be read, it says that all are there. */
export function detectCpu(): CpuLevel {
	if (cpuCache) return cpuCache;
	let level: CpuLevel = { avx: true, avx2: true };
	if (arch() === "x64") {
		try {
			if (platform() === "linux") {
				const flags = flagsOf(/^flags\s*:(.*)$/m.exec(readFileSync("/proc/cpuinfo", "utf8"))?.[1] ?? "");
				if (flags.size > 0) {
					const avx = flags.has("avx") && flags.has("sse4_2");
					level = { avx, avx2: avx && flags.has("avx2") && flags.has("fma") && flags.has("bmi2") && flags.has("f16c") };
				}
			} else if (platform() === "darwin") {
				const flags = flagsOf(execFileSync("/usr/sbin/sysctl", ["-n", "machdep.cpu.features", "machdep.cpu.leaf7_features"], { encoding: "utf8", timeout: 5000 }));
				const avx = flags.has("avx1.0") && flags.has("sse4.2");
				level = { avx, avx2: avx && flags.has("avx2") && flags.has("fma") && flags.has("bmi2") && flags.has("f16c") };
			} else if (platform() === "win32") {
				const script = "$i='System.Runtime.Intrinsics.X86.';$a=[Type]::GetType($i+'Avx');$b=[Type]::GetType($i+'Avx2');$f=[Type]::GetType($i+'Fma');$m=[Type]::GetType($i+'Bmi2');$s=[Type]::GetType($i+'Sse42');($a::IsSupported -and $s::IsSupported).ToString()+','+($b::IsSupported -and $f::IsSupported -and $m::IsSupported).ToString()";
				const [avx, avx2] = execFileSync("powershell", ["-NoProfile", "-NonInteractive", "-Command", script], { encoding: "utf8", timeout: 15_000, windowsHide: true }).trim().toLowerCase().split(",");
				level = { avx: avx === "true", avx2: avx === "true" && avx2 === "true" };
			}
		} catch {
			// The answer stays "all there": the default build runs, or the engine says unsupported_cpu.
		}
	}
	cpuCache = level;
	return level;
}

function isFile(path: string): boolean {
	try {
		return statSync(path).isFile();
	} catch {
		return false;
	}
}

function bestBuild(dir: string, cpu: CpuLevel): string {
	const exe = platform() === "win32" ? ".exe" : "";
	const builds: Array<[string, boolean]> = [
		["sayso-engine", arch() !== "x64" || cpu.avx2],
		["sayso-engine-avx", arch() === "x64" && cpu.avx],
		["sayso-engine-baseline", true],
	];
	for (const [name, runs] of builds) {
		const path = join(dir, name + exe);
		if (runs && isFile(path)) return path;
	}
	return join(dir, engineProgramName());
}

/**
 * The engine program in `folder`, where a release file of the engine was unpacked (sdk/protocol/paths.md,
 * "Engine program"). `folder` holds the program, or folders `sayso-engine-<version>` that hold it; then the folder
 * whose name sorts last wins. On x86-64 it picks the best build that this processor runs. Throws a `SaysoError`
 * with the code `unsupported` when there is none.
 */
export function findEngine(folder: string, cpu: CpuLevel = detectCpu()): string {
	if (isFile(join(folder, engineProgramName()))) return bestBuild(folder, cpu);
	let entries: string[];
	try {
		entries = readdirSync(folder);
	} catch (e) {
		throw SaysoError.of("unsupported", `cannot read ${folder}: ${(e as Error).message}`);
	}
	const versions = entries.filter((name) => name.startsWith(FOLDER_PREFIX) && isFile(join(folder, name, engineProgramName()))).sort();
	const last = versions.at(-1);
	if (last === undefined) {
		throw SaysoError.of("unsupported", `there is no ${engineProgramName()} in ${folder}, and no ${FOLDER_PREFIX}<version> folder with one. Unpack a release file of the engine there`);
	}
	return bestBuild(join(folder, last), cpu);
}

// ---- The engine of an app ----

/** The engine as the SDK reads it. */
export interface EngineSpec {
	release?: string;
	program?: string;
	store?: string;
	modelsDir?: string;
}

/**
 * The engine that the app ships, for Local mode. The SDK never uses the engine inside an installed Sayso, because
 * an update of Sayso would then change the engine of the app. The methods give a new `Engine`.
 */
export class Engine {
	readonly #spec: EngineSpec;

	private constructor(spec: EngineSpec) {
		this.#spec = spec;
	}

	/** The folder where the app unpacked the release file of the engine. */
	static fromRelease(folder: string): Engine {
		return new Engine({ release: folder });
	}

	/** The engine program itself. */
	static fromProgram(program: string): Engine {
		return new Engine({ program });
	}

	/**
	 * The engine of the package `@justsayso/engine-<platform>` for this platform. In a packaged Electron app, the folder
	 * in `app.asar.unpacked`, because a program inside an asar file cannot start. Unpack the engine with the
	 * `asarUnpack` rule of your packager. The SDK reads the folder of the package and runs no code of it.
	 */
	static fromPackage(): Engine {
		const name = `@justsayso/engine-${platformKey()}`;
		let manifest: string;
		try {
			manifest = createRequire(import.meta.url).resolve(`${name}/package.json`);
		} catch (e) {
			throw SaysoError.of("unsupported", `Local mode needs the engine: install ${name} (${(e as Error).message})`);
		}
		const root = unpackedPath(dirname(manifest));
		for (const folder of [root, join(root, "engine"), join(root, "bin"), join(root, "release")]) {
			try {
				findEngine(folder);
				return Engine.fromRelease(folder);
			} catch {
				// Try the next place where a package can keep the release folder.
			}
		}
		throw SaysoError.of("unsupported", `the package ${name} has no engine program`);
	}

	/** Use another model store than `modelStore()`. */
	store(folder: string): Engine {
		return new Engine({ ...this.#spec, store: folder });
	}

	/** The folder for the models of an engine without the store, and for a model without a descriptor. */
	modelsDir(folder: string): Engine {
		return new Engine({ ...this.#spec, modelsDir: folder });
	}

	toJSON(): EngineSpec {
		return { ...this.#spec };
	}

	/** The engine program: the one named, or the best build in the release folder. */
	program(): string {
		const spec = this.#spec;
		if (spec.program) return spec.program;
		if (spec.release) return findEngine(spec.release);
		throw SaysoError.of("unsupported", "Local mode needs an engine: set the option `engine`");
	}

	/** Start the engine. The folders of this `Engine` are the defaults of `options`. */
	start(options: EngineStartOptions = {}): Promise<EngineProcess> {
		const spec = this.#spec;
		const merged: EngineStartOptions = { ...options };
		const modelsDir = options.modelsDir ?? spec.modelsDir;
		if (modelsDir !== undefined) merged.modelsDir = modelsDir;
		const store = options.store ?? spec.store;
		if (store !== undefined) merged.store = store;
		return EngineProcess.start(this.program(), merged);
	}
}

/** The platform part of the names of the engine and audio packages. */
export function platformKey(): string {
	const key = `${platform()}-${arch()}`;
	switch (key) {
		case "darwin-arm64":
		case "darwin-x64":
			return key;
		case "linux-x64":
		case "linux-arm64":
			return `${key}-gnu`;
		case "win32-x64":
			return "win32-x64-msvc";
		default:
			throw SaysoError.of("unsupported", `there is no engine package for ${key}`);
	}
}

/** `app.asar` in a path becomes `app.asar.unpacked`, in Electron only. */
export function unpackedPath(path: string, electron = process.versions.electron !== undefined): string {
	if (!electron) return path;
	const packed = `${sep}app.asar${sep}`;
	return path.includes(packed) ? path.replace(packed, `${sep}app.asar.unpacked${sep}`) : path;
}
