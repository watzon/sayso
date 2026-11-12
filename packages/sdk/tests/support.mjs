// What the tests share: the fake engine of crates/sayso-engine-client, temporary folders,
// WAV files, and a fake Sayso on a real socket.

import { mkdirSync, mkdtempSync, rmSync, writeFileSync, existsSync } from "node:fs";
import { createServer as createHttpServer } from "node:http";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const exe = process.platform === "win32" ? ".exe" : "";
export const fakeEngine = process.env.SAYSO_FAKE_ENGINE ?? join(root, "target", "debug", `sayso-fake-engine${exe}`);
if (!existsSync(fakeEngine)) {
	throw new Error(`build the fake engine first: cargo build -p sayso-engine-client --bin sayso-fake-engine (no ${fakeEngine})`);
}

/** A temporary folder that the test removes at its end. */
const tempDirs = new Set();
// Node runs the t.after hooks of a test in the order they were added, so a hook of
// tempDir would remove the folder before a later hook closes the Speech that writes
// into it. The folders go when the test process exits, after every hook.
process.on("exit", () => {
	for (const dir of tempDirs) rmSync(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
});

/** A new temporary folder. It is removed when the test process exits. */
export function tempDir(_t) {
	const dir = mkdtempSync(join(tmpdir(), "sayso-sdk-test-"));
	tempDirs.add(dir);
	return dir;
}

export const FINAL = "fake-final";
export const STREAMING = "fake-stream";
export const VAD = "fake-vad";
/** The fake engine answers `transcribe` of a `slow-*` model after 300 ms. */
export const SLOW = "slow-final";

function model(id, roles, timestamps = "word") {
	return { id, name: id, engine: {}, size_bytes: 1000, languages: ["en"], roles, timestamps, detects_language: false, recommended: false };
}

/**
 * The folders of the fake engine: its models folder with a catalog, and a model store. The
 * files of the folder steer the fake engine (see its source).
 */
export function fakeWorld(t) {
	const dir = tempDir(t);
	const models = join(dir, "models");
	const store = join(dir, "store");
	mkdirSync(models, { recursive: true });
	mkdirSync(store, { recursive: true });
	const catalog = { models: [model(FINAL, ["final"]), model(STREAMING, ["streaming", "final"]), model(VAD, ["vad"], "none"), model(SLOW, ["final"])], retired: [] };
	writeFileSync(join(models, "catalog.json"), JSON.stringify(catalog));
	writeFileSync(join(models, "more-features"), "catalog concurrent_streams");
	for (const id of [FINAL, STREAMING, VAD, SLOW]) writeFileSync(join(models, `downloaded-${id}`), "");
	const wav = join(dir, "talk.wav");
	writeWav(wav, tone(2000));
	return { dir, models, store, wav };
}

/** 16 kHz mono samples of a tone, `ms` long. */
export function tone(ms) {
	const n = Math.round((16000 * ms) / 1000);
	const samples = new Int16Array(n);
	for (let i = 0; i < n; i++) samples[i] = Math.round(Math.sin((2 * Math.PI * 440 * i) / 16000) * 8000);
	return samples;
}

export function writeWav(path, samples) {
	const data = Buffer.from(samples.buffer, samples.byteOffset, samples.byteLength);
	const header = Buffer.alloc(44);
	header.write("RIFF", 0);
	header.writeUInt32LE(36 + data.length, 4);
	header.write("WAVE", 8);
	header.write("fmt ", 12);
	header.writeUInt32LE(16, 16);
	header.writeUInt16LE(1, 20);
	header.writeUInt16LE(1, 22);
	header.writeUInt32LE(16000, 24);
	header.writeUInt32LE(32000, 28);
	header.writeUInt16LE(2, 32);
	header.writeUInt16LE(16, 34);
	header.write("data", 36);
	header.writeUInt32LE(data.length, 40);
	writeFileSync(path, Buffer.concat([header, data]));
}

/** Wait until `condition()` is true, or fail after `ms`. */
export async function until(condition, ms = 10000, what = "the condition") {
	const end = Date.now() + ms;
	while (!(await condition())) {
		if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
		await new Promise((r) => setTimeout(r, 10));
	}
}

/**
 * The socket `name` in `dir`. On Windows the SDK talks over a named pipe, so it is a pipe
 * with a name from the folder (a temporary folder, so the name is unique).
 */
export function socketPath(dir, name = "host.sock") {
	return process.platform === "win32" ? `\\\\.\\pipe\\${basename(dir)}-${name}` : join(dir, name);
}

/**
 * A fake Sayso on a socket in `dir`. `script(peer)` runs for each connection; the default
 * answers `hello` and replies to each request with `respond(request, peer)`.
 */
export async function fakeHost(t, dir, respond = () => ({ type: "ok" }), { hello } = {}) {
	const path = socketPath(dir);
	const peers = new Set();
	const requests = [];
	const server = createServer((socket) => {
		const peer = {
			socket,
			send(line) {
				socket.write(`${JSON.stringify({ v: 1, ...line })}\n`);
			},
		};
		peers.add(peer);
		socket.on("close", () => peers.delete(peer));
		let buffer = "";
		socket.on("data", (chunk) => {
			buffer += chunk;
			let at;
			while ((at = buffer.indexOf("\n")) >= 0) {
				const line = JSON.parse(buffer.slice(0, at));
				buffer = buffer.slice(at + 1);
				requests.push(line);
				if (line.type === "hello") {
					peer.send({ type: "ok", id: line.id, host_version: "fake", protocol: 1, capabilities: line.capabilities ?? [], denied: [], features: [], ...(hello ? hello(line, peer) : {}) });
					continue;
				}
				const reply = respond(line, peer);
				if (reply && line.id !== undefined) peer.send({ id: line.id, ...reply });
			}
		});
		socket.on("error", () => {});
	});
	await new Promise((resolve) => server.listen(path, resolve));
	const host = {
		path,
		requests,
		peers,
		close: () =>
			new Promise((resolve) => {
				for (const peer of peers) peer.socket.destroy();
				server.close(() => resolve());
			}),
	};
	t.after(() => host.close());
	return host;
}

/**
 * 16 kHz mono float samples that Basic VAD takes for speech: a voice of 140 Hz in syllables
 * (the signal of the Rust test `embedded_session`).
 */
export function voice(ms) {
	const n = Math.round((16000 * ms) / 1000);
	const out = new Float32Array(n);
	for (let i = 0; i < n; i++) {
		const t = i / 16000;
		let sum = 0;
		for (let h = 1; h <= 5; h++) sum += Math.sin(2 * Math.PI * 140 * h * t) / h;
		const phase = (t * 5) % 1;
		out[i] = sum * (phase < 0.7 ? 0.2 : 0.04);
	}
	return out;
}

/** Digital silence, `ms` long, as float samples. */
export function silence(ms) {
	return new Float32Array(Math.round((16000 * ms) / 1000));
}

/** Join float sample arrays into one. */
export function concat(...parts) {
	const out = new Float32Array(parts.reduce((n, p) => n + p.length, 0));
	let at = 0;
	for (const part of parts) {
		out.set(part, at);
		at += part.length;
	}
	return out;
}

/**
 * A fake Deepgram on a local HTTP port. `handler(request, body)` gives `{ status, body }`.
 * `server.requests` lists what it got: method, url, headers, and the size of the body.
 */
export async function fakeDeepgram(t, handler = () => ({ status: 200, body: deepgramReply("hello from deepgram") })) {
	const requests = [];
	const server = createHttpServer((req, res) => {
		const chunks = [];
		req.on("data", (c) => chunks.push(c));
		req.on("end", () => {
			const body = Buffer.concat(chunks);
			const seen = { method: req.method, url: req.url, headers: req.headers, bytes: body.length, body };
			requests.push(seen);
			const { status, body: reply } = handler(seen);
			res.writeHead(status, { "content-type": "application/json" });
			res.end(typeof reply === "string" ? reply : JSON.stringify(reply));
		});
	});
	await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
	t.after(() => new Promise((resolve) => {
		server.closeAllConnections();
		server.close(() => resolve());
	}));
	return { baseUrl: `http://127.0.0.1:${server.address().port}/v1`, requests };
}

/** The JSON that Deepgram sends for a transcript. */
export function deepgramReply(text) {
	return { results: { channels: [{ alternatives: [{ transcript: text }] }] } };
}
