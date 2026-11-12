// The loader of the native addon (crates/sayso-node) and the outcome of its calls.

import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync } from "node:fs";
import { SaysoError, type ErrorReplyLike } from "./errors.js";

/** A call of Rust into JavaScript: `(call, kind, json, samples)`. `call` is 0 when Rust does not wait. */
export type BridgeFn = (call: number, kind: string, json: string, samples: Float32Array | null) => void;

export interface NativeSpeech {
	connect(options: string, microphone?: BridgeFn, systemAudio?: BridgeFn, start?: BridgeFn): Promise<string>;
	listen(callback: (outcome: string) => void): void;
	mode(): string;
	features(): string;
	listModels(role?: string): Promise<string>;
	downloadModel(model: string, reason: string): Promise<string>;
	transcribeFile(request: string): Promise<string>;
	cancelJob(job: number): Promise<string>;
	startSession(request: string): Promise<string>;
	sessionAudio(session: number, samples: Int16Array): string;
	stopSession(session: number): Promise<string>;
	enhance(request: string): Promise<string>;
	subscribe(events: string[]): Promise<string>;
	reconnect(): Promise<string>;
	close(): Promise<string>;
}

export interface NativeEmbedded {
	connect(options: string, microphone?: BridgeFn, finalPass?: BridgeFn): Promise<string>;
	listen(callback: (outcome: string) => void): void;
	models(): Promise<string>;
	download(model: string, sizeBytes: number | undefined | null, onProgress: (progress: string) => void): Promise<string>;
	delete(model: string): Promise<string>;
	load(model: string): Promise<string>;
	unload(model: string): Promise<string>;
	startSession(options: string): Promise<string>;
	sessionAudio(session: number, samples: Float32Array): string;
	stopSession(session: number): Promise<string>;
	cancelSession(): void;
	setDevice(device?: string | null): void;
	setLevel(onLevel?: ((level: number) => void) | null): void;
	openEarly(): Promise<string>;
	closeEarly(): void;
	close(): Promise<string>;
}

export interface NativeClient {
	connect(how: string, start?: BridgeFn): Promise<string>;
	listen(callback: (outcome: string) => void): void;
	hello(): string;
	request(kind: string, fields: string): Promise<string>;
	send(kind: string, fields: string): string;
	reconnect(): Promise<string>;
	close(): Promise<string>;
}

export interface NativeDeepgram {
	checkKey(): Promise<string>;
	transcribe(samples: Float32Array, language: string): Promise<string>;
}

export interface NativeAddon {
	addonVersion(): string;
	protocolVersion(): number;
	settleCall(call: number, ok: boolean, json: string): void;
	writerPush(writer: number, samples: Float32Array, timeMs?: number | null): boolean;
	writerGap(writer: number, ms: number): boolean;
	writerEnd(writer: number, reason: string, message?: string | null): void;
	findEngine(folder: string): string;
	modelStore(): string;
	microphonePermissionStatus(): string;
	microphonePermissionRequest(): Promise<boolean>;
	microphoneSettingsUrl(): string | null;
	devices(): string;
	startSayso(): string;
	launchers(): string;
	clientConstants(): string;
	deepgramConstants(): string;
	NativeSpeech: new () => NativeSpeech;
	NativeEmbedded: new () => NativeEmbedded;
	NativeClient: new () => NativeClient;
	NativeDeepgram: new (key: string, baseUrl?: string | null) => NativeDeepgram;
}

/** The name of this platform in the names of the packages: `darwin-arm64`, `linux-x64-gnu`, and so on. */
export function platformName(): string {
	const { platform, arch } = process;
	if (platform === "linux") {
		const report = process.report?.getReport() as { header?: { glibcVersionRuntime?: string } } | undefined;
		const libc = report?.header?.glibcVersionRuntime ? "gnu" : "musl";
		return `linux-${arch}-${libc}`;
	}
	if (platform === "win32") {
		return `win32-${arch}-msvc`;
	}
	return `${platform}-${arch}`;
}

const SUPPORTED = ["darwin-arm64", "darwin-x64", "linux-x64-gnu", "linux-arm64-gnu", "win32-x64-msvc"];

function load(): NativeAddon {
	const require = createRequire(import.meta.url);
	const name = platformName();
	const tried: string[] = [];
	// SAYSO_NODE_ADDON: a build of crates/sayso-node, for the development of the SDK.
	const own = process.env.SAYSO_NODE_ADDON;
	const local = join(dirname(fileURLToPath(import.meta.url)), "..", `sayso.${name}.node`);
	for (const path of [own, local]) {
		if (path && existsSync(path)) {
			return require(path) as NativeAddon;
		}
		if (path) tried.push(path);
	}
	if (!SUPPORTED.includes(name)) {
		throw new SaysoError({
			code: "unsupported",
			message: `the Sayso SDK has no native addon for ${name}; it supports ${SUPPORTED.join(", ")}`,
		});
	}
	const pkg = `@justsayso/sdk-${name}`;
	try {
		return require(pkg) as NativeAddon;
	} catch (e) {
		throw new SaysoError({
			code: "unsupported",
			message: `cannot load ${pkg}, the native addon of the Sayso SDK for this platform. Install @justsayso/sdk again, without --no-optional or --omit=optional. (${(e as Error).message})`,
		});
	}
}

let addon: NativeAddon | undefined;

/** The addon, loaded at the first use. */
export function native(): NativeAddon {
	addon ??= load();
	return addon;
}

interface Outcome<T> {
	ok?: T;
	err?: ErrorReplyLike;
}

/** The value of an outcome of the addon, or a `SaysoError`. */
export function unwrap<T>(text: string): T {
	const outcome = JSON.parse(text) as Outcome<T>;
	if (outcome.err) {
		throw new SaysoError(outcome.err);
	}
	return outcome.ok as T;
}

/** As `unwrap`, for a promise of the addon. */
export async function settle<T>(promise: Promise<string>): Promise<T> {
	return unwrap<T>(await promise);
}

/**
 * A function that Rust calls from its threads. Each kind has a handler; Rust waits for
 * the answer of a call that has a number.
 */
export function bridge(handlers: Record<string, (json: unknown, samples: Float32Array | undefined) => unknown>): BridgeFn {
	return (call, kind, json, samples) => {
		const handler = handlers[kind];
		const run = async () => {
			if (!handler) {
				throw new SaysoError({ code: "unsupported", message: `no handler for ${kind}` });
			}
			return handler(JSON.parse(json), samples ?? undefined);
		};
		run().then(
			(value) => {
				if (call) native().settleCall(call, true, JSON.stringify(value ?? null));
			},
			(error: unknown) => {
				if (call) native().settleCall(call, false, JSON.stringify(SaysoError.replyOf(error)));
			},
		);
	};
}
