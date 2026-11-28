// The audio helper `sayso-audio` (sdk/protocol/audio-helper.md): a small MIT program that captures audio and decodes
// audio files for an SDK that cannot do it itself. The SDK starts it as a child process, one process for each command.

import { execFileSync, spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { statSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { StringDecoder } from "node:string_decoder";
import { platformKey, unpackedPath } from "../engine.js";
import { SaysoError } from "../errors.js";
import { decodeI16 } from "../protocol/codec.js";
import { LineSplitter } from "../transport.js";

/** The environment variable that names the helper program. */
export const AUDIO_HELPER_ENV = "SAYSO_AUDIO_HELPER";

function isFile(path: string): boolean {
	try {
		return statSync(path).isFile();
	} catch {
		return false;
	}
}

/**
 * The helper program: the one in the package `@justsayso/audio-<platform>` for this platform, else the file that
 * `SAYSO_AUDIO_HELPER` names. Undefined when there is none.
 */
export function findAudioHelper(): string | undefined {
	const exe = process.platform === "win32" ? "sayso-audio.exe" : "sayso-audio";
	try {
		const manifest = createRequire(import.meta.url).resolve(`@justsayso/audio-${platformKey()}/package.json`);
		const root = unpackedPath(dirname(manifest));
		for (const candidate of [join(root, exe), join(root, "bin", exe)]) if (isFile(candidate)) return candidate;
	} catch {
		// No package for this platform: look at the variable.
	}
	const named = process.env[AUDIO_HELPER_ENV];
	return named !== undefined && named !== "" && isFile(named) ? named : undefined;
}

/** True where `sayso-audio capture --source system` can work: it captures the system audio on Windows and Linux only. */
export function helperCapturesSystem(platform: NodeJS.Platform = process.platform): boolean {
	return platform === "win32" || platform === "linux";
}

/** The helper, or the error `unsupported` that says how to get it. */
export function requireAudioHelper(what: string): string {
	const helper = findAudioHelper();
	if (helper === undefined) {
		throw SaysoError.of("unsupported", `${what} needs the audio helper sayso-audio: install the package @justsayso/audio-${safeKey()} or set ${AUDIO_HELPER_ENV}`);
	}
	return helper;
}

function safeKey(): string {
	try {
		return platformKey();
	} catch {
		return "<platform>";
	}
}

/** A line of the helper. */
export interface HelperMessage {
	v?: number;
	type: string;
	[field: string]: unknown;
}

/** Error of a helper `error` message, as a `SaysoError`. */
export function helperError(message: HelperMessage): SaysoError {
	const code = typeof message.code === "string" ? message.code : "failed";
	const text = typeof message.message === "string" ? message.message : "the audio helper failed";
	return new SaysoError({ code: code === "unsupported" || code === "permission" || code === "bad_request" ? code : undefined, message: text });
}

/** Run a command that prints its answer and exits (`devices`, `permission`), and give its first message. */
export function runHelperSync(args: string[], what: string): HelperMessage {
	const helper = requireAudioHelper(what);
	let out: string;
	try {
		out = execFileSync(helper, args, { encoding: "utf8", timeout: 60_000, stdio: ["ignore", "pipe", "ignore"], windowsHide: true });
	} catch (e) {
		const stdout = (e as { stdout?: string }).stdout ?? "";
		const first = parseLines(stdout)[0];
		if (first?.type === "error") throw helperError(first);
		throw SaysoError.of("unavailable", `the audio helper failed: ${(e as Error).message}`);
	}
	const first = parseLines(out)[0];
	if (!first) throw SaysoError.of("unavailable", "the audio helper gave no answer");
	if (first.type === "error") throw helperError(first);
	return first;
}

function parseLines(text: string): HelperMessage[] {
	const out: HelperMessage[] = [];
	for (const line of text.split("\n")) {
		const message = parseLine(line);
		if (message) out.push(message);
	}
	return out;
}

function parseLine(line: string): HelperMessage | undefined {
	if (line.trim() === "") return undefined;
	try {
		const parsed: unknown = JSON.parse(line);
		if (typeof parsed === "object" && parsed !== null && typeof (parsed as HelperMessage).type === "string") return parsed as HelperMessage;
	} catch {
		// A line that is not JSON is not a message.
	}
	return undefined;
}

/** A helper process with its lines. */
export interface HelperProcess {
	child: ChildProcessWithoutNullStreams;
	/** The last lines of stderr. */
	stderr(): string[];
	/** Resolves when the process has exited. */
	exited: Promise<number | null>;
	/** Close the input and, after a time, stop the process. */
	stop(): void;
}

/** Start the helper with `args`. `onMessage` gets each message in order. */
export function spawnHelper(program: string, args: string[], onMessage: (message: HelperMessage) => void, env?: Record<string, string>): HelperProcess {
	const child = spawn(program, args, { stdio: ["pipe", "pipe", "pipe"], windowsHide: true, env: { ...process.env, ...env } });
	const decoder = new StringDecoder("utf8");
	const splitter = new LineSplitter(64 * 1024 * 1024);
	const tail: string[] = [];
	child.stdout.on("data", (chunk: Buffer) => {
		let lines: string[];
		try {
			lines = splitter.push(decoder.write(chunk));
		} catch {
			child.kill();
			return;
		}
		for (const line of lines) {
			const message = parseLine(line);
			if (message) onMessage(message);
		}
	});
	const errDecoder = new StringDecoder("utf8");
	child.stderr.on("data", (chunk: Buffer) => {
		for (const line of errDecoder.write(chunk).split("\n")) {
			if (line.trim() === "") continue;
			tail.push(line);
			if (tail.length > 20) tail.shift();
		}
	});
	child.stdin.on("error", () => undefined);
	const exited = new Promise<number | null>((resolve) => {
		child.once("error", () => resolve(null));
		child.once("close", (code) => resolve(code));
	});
	let timer: NodeJS.Timeout | undefined;
	return {
		child,
		stderr: () => [...tail],
		exited,
		stop() {
			child.stdin.end();
			timer ??= setTimeout(() => child.kill(), 2000);
			timer.unref();
			void exited.then(() => clearTimeout(timer));
		},
	};
}

/** Decode the base64 PCM of an `audio` message of the helper. */
export function helperPcm(message: HelperMessage): Int16Array {
	return decodeI16(String(message.pcm ?? ""));
}
