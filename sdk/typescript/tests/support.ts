import { execFileSync, spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { appendFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { encodeWav } from "../src/audio/wav.js";

const here = dirname(fileURLToPath(import.meta.url));
// dist-test/tests -> sdk/typescript
export const PACKAGE_DIR = resolve(here, "..", "..");
export const SDK_DIR = resolve(PACKAGE_DIR, "..");
export const REPO_DIR = resolve(SDK_DIR, "..");
export const CORPUS = join(SDK_DIR, "protocol", "testdata");

const envOf: Record<string, string> = { "sayso-fake-engine": "SAYSO_FAKE_ENGINE", "sayso-fake-cleanup": "SAYSO_FAKE_CLEANUP", "sayso-testhost": "SAYSO_TESTHOST" };
const built = new Map<string, string>();
let binDir: string | undefined;
const temps: string[] = [];

/** A fresh folder that the run removes at its end. */
export function tempDir(prefix = "sdkts"): string {
	const dir = mkdtempSync(join(tmpdir(), `${prefix}-`));
	temps.push(dir);
	return dir;
}

export async function removeTemps(): Promise<void> {
	for (const h of Array.from(hosts)) await h.stop().catch(() => undefined);
	const rm = (dir: string) => {
		try {
			rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
		} catch {
			// Best effort on Windows where an exiting child or AV may hold a handle briefly.
		}
	};
	for (const dir of temps.splice(0)) rm(dir);
	if (binDir) {
		rm(binDir);
		binDir = undefined;
	}
}

/** Build a Go test program once per process. An environment variable can name one that is built already. */
export function program(name: "sayso-fake-engine" | "sayso-fake-cleanup" | "sayso-testhost" | "sayso-audio"): string {
	const variable = envOf[name];
	const named = variable === undefined ? undefined : process.env[variable];
	if (named) return named;
	const have = built.get(name);
	if (have) return have;
	binDir ??= mkdtempSync(join(tmpdir(), "sayso-sdkts-bin-"));
	const out = join(binDir, process.platform === "win32" ? `${name}.exe` : name);
	const cwd = name === "sayso-testhost" ? REPO_DIR : join(SDK_DIR, "go");
	execFileSync("go", ["build", "-o", out, `./cmd/${name}`], { cwd, env: { ...process.env, CGO_ENABLED: "0" }, stdio: "pipe" });
	built.set(name, out);
	return out;
}

/** 16 kHz mono audio: 500 ms of silence, then a tone for each length in ms, with 1 s of silence after each. */
export function speech(...lengths: number[]): Int16Array {
	const out: number[] = new Array(8000).fill(0);
	for (const ms of lengths) {
		for (let i = 0; i < ms * 16; i++) out.push(Math.trunc(9000 * Math.sin((2 * Math.PI * 220 * i) / 16000)));
		for (let i = 0; i < 16000; i++) out.push(0);
	}
	return Int16Array.from(out);
}

export function speechWav(...lengths: number[]): string {
	const path = join(tempDir("wav"), "speech.wav");
	writeFileSync(path, encodeWav(speech(...lengths)));
	return path;
}

let hostCount = 0;
export function socketPath(): string {
	hostCount++;
	if (process.platform === "win32") return `\\\\.\\pipe\\sayso-sdkts-${process.pid}-${hostCount}`;
	const dir = mkdtempSync(join("/tmp", "ssts"));
	temps.push(dir);
	mkdirSync(join(dir, "s"), { recursive: true, mode: 0o700 });
	return join(dir, "s", "host.sock");
}

const hosts = new Set<TestHost>();

export class TestHost {
	readonly socket: string;
	readonly #child: ChildProcessWithoutNullStreams;
	readonly #lines: Record<string, unknown>[] = [];
	readonly #waiters: (() => void)[] = [];
	readonly exited: Promise<void>;
	#exited = false;

	private constructor(socket: string, child: ChildProcessWithoutNullStreams) {
		this.socket = socket;
		this.#child = child;
		hosts.add(this);
		child.stdin.on("error", () => undefined);
		createInterface({ input: child.stdout }).on("line", (line) => {
			try {
				this.#lines.push(JSON.parse(line) as Record<string, unknown>);
			} catch {
				return;
			}
			for (const w of this.#waiters.splice(0)) w();
		});
		child.stderr.resume();
		this.exited = new Promise((res) =>
			child.on("exit", () => {
				this.#exited = true;
				hosts.delete(this);
				for (const w of this.#waiters.splice(0)) w();
				res();
			}),
		);
	}

	static async start(socket = socketPath(), ...args: string[]): Promise<TestHost> {
		const all = ["--socket", socket, "--engine", program("sayso-fake-engine"), "--data", tempDir("host"), "--engine-env", `SAYSO_FAKE_SCRIPT={"accept_any":true}`, ...args];
		const child = spawn(program("sayso-testhost"), all, { stdio: ["pipe", "pipe", "pipe"] });
		const host = new TestHost(socket, child);
		await host.await("ready");
		return host;
	}

	async await(key: string, value?: string, ms = 20_000): Promise<Record<string, unknown>> {
		const end = Date.now() + ms;
		for (;;) {
			const i = this.#lines.findIndex((m) => key in m && (value === undefined || String(m[key]) === value));
			if (i >= 0) return this.#lines.splice(i, 1)[0]!;
			if (this.#exited) throw new Error(`the test host exited while waiting for ${key}`);
			const left = end - Date.now();
			if (left <= 0) throw new Error(`the test host printed no ${key}`);
			await new Promise<void>((res) => {
				const t = setTimeout(res, left);
				this.#waiters.push(() => {
					clearTimeout(t);
					res();
				});
			});
		}
	}

	send(cmd: Record<string, unknown>): void {
		this.#child.stdin.write(`${JSON.stringify(cmd)}\n`);
	}

	async stop(): Promise<void> {
		if (this.#exited) return;
		try {
			if (this.#child.stdin.writable) this.#child.stdin.write(`{"close":true}\n`);
		} catch {
			// Pipe already closed.
		}
		const t = setTimeout(() => this.#child.kill("SIGKILL"), 10_000);
		await this.exited;
		clearTimeout(t);
	}
}

/** The next event of an async iterator for which `pick` gives a value, in 20 s. */
export async function next<T, R>(it: AsyncIterator<T>, pick: (event: T) => R | undefined, seen: string[] = [], ms = 20_000): Promise<R> {
	const end = Date.now() + ms;
	for (;;) {
		const left = end - Date.now();
		if (left <= 0) throw new Error(`no matching event; seen ${seen.join(",")}`);
		let timer: NodeJS.Timeout | undefined;
		const r = await Promise.race([it.next(), new Promise<never>((_, rej) => (timer = setTimeout(() => rej(new Error(`no matching event; seen ${seen.join(",")}`)), left)))]).finally(() => clearTimeout(timer));
		if (r.done) throw new Error(`the events ended; seen ${seen.join(",")}`);
		const e = r.value as { kind?: string; message?: { type?: string }; event?: { type?: string } };
		seen.push(e.message?.type ?? e.event?.type ?? String(e.kind));
		const v = pick(r.value);
		if (v !== undefined) return v;
	}
}

export const sleep = (ms: number) => new Promise<void>((res) => setTimeout(res, ms));

/** Write the id of a scenario of sdk/conformance/scenarios.json that passed to `SAYSO_SCENARIO_REPORT`, when it is set. */
export function passed(id: string): void {
	const report = process.env.SAYSO_SCENARIO_REPORT;
	if (report) appendFileSync(report, `${id}\n`);
}
