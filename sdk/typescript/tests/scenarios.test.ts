import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createServer as createHttpServer } from "node:http";
import { createServer, type Server } from "node:net";
import { dirname, join } from "node:path";
import { after, test } from "node:test";
import { decodeWav, encodeWav } from "../src/audio/wav.js";
import { Client } from "../src/client.js";
import { RuntimeCleaner, downloadEntry, modelFiles, type CatalogEntry } from "../src/cleanup.js";
import { Engine, engineProgramName, findEngine } from "../src/engine.js";
import type { SaysoError } from "../src/errors.js";
import { cleanupModelsDirOf } from "../src/paths.js";
import type { Capability } from "../src/protocol/index.js";
import { Speech, type SpeechEvent, type SpeechOptions } from "../src/speech.js";
import { VERSION } from "../src/version.js";
import { TestHost, next, passed, program, removeTemps, sleep, socketPath, speech, speechWav, tempDir } from "./support.js";

after(removeTemps);

const ALL: Capability[] = ["models.read", "models.download", "transcribe.file", "session.live", "audio.mic", "audio.system", "ai.enhance", "dictation.observe"];

const opened: { close(): Promise<void> }[] = [];
after(async () => {
	for (const o of opened.splice(0)) await o.close().catch(() => undefined);
});

async function connect(options: Partial<SpeechOptions> & { mode: SpeechOptions["mode"] }): Promise<Speech> {
	const s = await Speech.connect({
		app: { name: "Notes" },
		capabilities: ALL,
		engine: Engine.fromProgram(program("sayso-fake-engine")),
		modelsDir: join(tempDir("models"), "models"),
		...options,
	});
	opened.push(s);
	return s;
}

async function host(...args: string[]): Promise<TestHost> {
	const h = await TestHost.start(undefined, ...args);
	after(() => h.stop());
	return h;
}

type Msg = { type: string; [k: string]: unknown };
const message = (type: string, where: (m: Msg) => boolean = () => true) => (e: SpeechEvent): Msg | undefined =>
	e.kind === "message" && e.message.type === type && where(e.message as Msg) ? (e.message as Msg) : undefined;

/** Download the final model of the user, as Sayso picks it. */
async function ready(s: Speech): Promise<void> {
	const models = await s.listModels("final");
	const final = models.find((m) => (m.choice ?? []).includes("final"));
	assert.ok(final, "a final model of the user");
	await s.downloadModel(final.id, "tests");
}

/** All events from now on, in order. */
function record(s: Speech): SpeechEvent[] {
	const seen: SpeechEvent[] = [];
	s.on((e) => void seen.push(e));
	return seen;
}

const sixtyTones = () => speechWav(...new Array<number>(60).fill(900));

test("an unanswered question waits, and the callback runs once", async () => {
	const h = await host("--approve", "ignore");
	let calls = 0;
	let done = false;
	const connecting = connect({ mode: "sayso", socket: h.socket, onApprovalWaiting: () => calls++ }).finally(() => (done = true));
	await h.await("question", "approval");
	await sleep(300);
	assert.equal(done, false, "connect waits for the user");
	h.send({ approve: "allow" });
	const s = await connecting;
	assert.equal(calls, 1);
	assert.equal(s.mode(), "sayso");
	passed("H03");
});

test("a fifth waiting connection of one app is busy", async () => {
	const h = await host("--approve", "ignore");
	const waiting = [0, 1, 2, 3].map(() => Client.connect({ path: h.socket, app: { name: "Busy" } }).then((c) => (opened.push(c), c), (e: unknown) => e));
	await h.await("question", "approval");
	await sleep(800);
	const fifth = Client.connect({ path: h.socket, app: { name: "Busy" } });
	const timeout = sleep(10_000).then(() => "waits");
	const got = await Promise.race([fifth.then(() => "connected", (e: SaysoError) => e.code), timeout]);
	assert.equal(got, "busy");
	h.send({ approve: "deny" });
	await Promise.all(waiting);
	passed("H05");
});

test("the SDK refuses the sources that the host lacks", async () => {
	const h = await host();
	const s = await connect({ mode: "sayso", socket: h.socket });
	for (const [source, feature] of [["mic", "mic_capture"], ["system", "system_capture"]] as const) {
		await assert.rejects(s.startSession({ sources: [source] }), (e: SaysoError) => e.code === "unsupported" && e.message.includes(feature) && e.capability === undefined);
	}
	passed("H06");
});

test("a host with no features gets each request and answers for itself", { skip: process.platform === "win32" && "the fake old host listens on a Unix socket" }, async () => {
	const socket = socketPath();
	const seen: string[] = [];
	const server: Server = createServer((conn) => {
		let buffer = "";
		conn.on("data", (chunk) => {
			buffer += chunk.toString("utf8");
			for (let i = buffer.indexOf("\n"); i >= 0; i = buffer.indexOf("\n")) {
				const req = JSON.parse(buffer.slice(0, i)) as { id: number; type: string };
				buffer = buffer.slice(i + 1);
				const reply =
					req.type === "hello"
						? { v: 1, type: "ok", id: req.id, host_version: "0.17.0", protocol: 1, features: [], capabilities: ["session.live", "audio.mic"] }
						: (seen.push(req.type), { v: 1, type: "error", id: req.id, code: "unsupported", message: "the host said so" });
				conn.write(`${JSON.stringify(reply)}\n`);
			}
		});
	});
	mkdirSync(dirname(socket), { recursive: true, mode: 0o700 });
	await new Promise<void>((res) => server.listen(socket, res));
	after(() => void server.close());
	const s = await connect({ mode: "sayso", socket, capabilities: ["session.live", "audio.mic"] });
	await assert.rejects(s.startSession({ sources: ["mic"], silence_limit_ms: 1000, record_to: tempDir("rec") }), (e: SaysoError) => e.code === "unsupported" && e.message === "the host said so");
	await assert.rejects(s.enhance({ text: "a" }));
	await assert.rejects(s.subscribe(["dictation_done"]));
	assert.deepEqual(seen, ["session_start", "enhance", "subscribe"]);
	await s.close();
	passed("H07");
});

test("the host says its version and the protocol, and the SDK its own", async () => {
	const h = await host();
	const c = await Client.connect({ path: h.socket, app: { name: "Notes", capabilities: ["models.read"] } });
	opened.push(c);
	const hello = c.hello();
	assert.equal(hello.host_version, VERSION);
	assert.equal(hello.protocol, 1);
	assert.ok((hello.features ?? []).length > 0);
	passed("H08");
});

test("list_models by role, and a download with the question", async () => {
	const h = await host("--download", "wait");
	const s = await connect({ mode: "sayso", socket: h.socket });
	const all = await s.listModels();
	assert.ok(all.length > 0);
	const vads = await s.listModels("vad");
	assert.ok(vads.length > 0 && vads.every((m) => m.roles.includes("vad")), JSON.stringify(vads));
	passed("M01");

	const model = all[0]!.id;
	const declined = s.downloadModel(model, "tests");
	const q = await h.await("question", "download");
	assert.equal(q.model, model);
	assert.equal(q.reason, "tests");
	h.send({ download_answer: false });
	await assert.rejects(declined, (e: SaysoError) => e.code === "declined");
	passed("M03");

	const allowed = s.downloadModel(model, "tests");
	await h.await("question", "download");
	h.send({ download_answer: true });
	await allowed;
	assert.ok((await s.listModels()).some((m) => m.id === model && m.state === "downloaded"));
	await assert.rejects(s.downloadModel("no-such-model", "tests"), (e: SaysoError) => e.code === "unknown_model");
	passed("M02");
});

test("a live session of the client source in Sayso mode has partials and a final text", async () => {
	const h = await host();
	const s = await connect({ mode: "sayso", socket: h.socket });
	await ready(s);
	const it = s.events();
	const sess = await s.startSession({ sources: ["client"] });
	const samples = speech(900, 900);
	for (let i = 0; i < samples.length; i += 3200) s.sessionAudio(sess.session, samples.subarray(i, i + 3200));
	await next(it, message("partial", (m) => m.session === sess.session));
	const text = await s.stopSession(sess.session);
	assert.ok(text.includes("alpha"), text);
	passed("L01");
});

test("a live session of the microphone of the host gives segments of the source mic", async () => {
	const h = await host("--mic-wav", speechWav(900, 900), "--speed", "4");
	const s = await connect({ mode: "sayso", socket: h.socket });
	assert.ok(s.features().host.includes("mic_capture"));
	await ready(s);
	const it = s.events();
	const sess = await s.startSession({ sources: ["mic"] });
	const seg = await next(it, message("segment", (m) => m.session === sess.session));
	assert.equal(seg.source, "mic");
	const text = await s.stopSession(sess.session);
	assert.ok(text.includes("alpha"), text);
	passed("L02");
});

test("with the microphone and the system audio, the echo rule drops the microphone segments", async () => {
	const wav = speechWav(900, 900);
	const h = await host("--mic-wav", wav, "--system-wav", wav, "--speed", "4");
	const s = await connect({ mode: "sayso", socket: h.socket });
	assert.ok(s.features().host.includes("system_capture") && s.features().host.includes("echo_rule"), s.features().host.join(","));
	await ready(s);
	const seen = record(s);
	const it = s.events();
	const sess = await s.startSession({ sources: ["mic", "system"] });
	await next(it, message("segment", (m) => m.source === "system"));
	await sleep(1500);
	await s.stopSession(sess.session);
	const mic = seen.filter((e) => e.kind === "message" && e.message.type === "segment" && (e.message as Msg).source === "mic");
	assert.equal(mic.length, 0, "the microphone heard what the system audio played");
	passed("L03");
});

test("the silence limit gives one event", async () => {
	const h = await host("--mic-wav", speechWav(600), "--speed", "8");
	const s = await connect({ mode: "sayso", socket: h.socket });
	await ready(s);
	const seen = record(s);
	const it = s.events();
	const sess = await s.startSession({ sources: ["mic"], silence_limit_ms: 1000 });
	const limit = await next(it, message("silence_limit", (m) => m.session === sess.session));
	assert.equal(limit.silence_ms, 1000);
	await sleep(500);
	await s.stopSession(sess.session);
	assert.equal(seen.filter((e) => e.kind === "message" && e.message.type === "silence_limit").length, 1);
	passed("L04");
});

test("record_to writes a WAV file for each source", async () => {
	const h = await host("--mic-wav", speechWav(600), "--system-wav", speechWav(900), "--speed", "8");
	const s = await connect({ mode: "sayso", socket: h.socket });
	await ready(s);
	const folder = join(tempDir("rec"), "rec");
	mkdirSync(folder);
	const sess = await s.startSession({ sources: ["mic", "system"], record_to: folder });
	await sleep(1500);
	await s.stopSession(sess.session);
	for (const name of ["mic.wav", "system.wav"]) {
		const audio = decodeWav(readFileSync(join(folder, name)), name);
		assert.ok(audio.samples.some((v) => v !== 0), `${name} has ${audio.samples.length} samples and no sound`);
	}
	passed("L05");
});

test("a second live session while one runs is refused", async () => {
	const h = await host();
	const s = await connect({ mode: "sayso", socket: h.socket });
	await ready(s);
	const first = await s.startSession({ sources: ["client"] });
	await assert.rejects(s.startSession({ sources: ["client"] }), (e: SaysoError) => e.code === "busy" || e.code === "too_many_streams");
	await s.stopSession(first.session);
	passed("L06");
});

test("enhance runs a prompt, and an empty text is bad_request", async () => {
	const h = await host();
	const s = await connect({ mode: "sayso", socket: h.socket });
	const r = await s.enhance({ text: "um so hello", prompt: "Translate." });
	assert.deepEqual([r.text, r.provider, r.model], ["[app-prompt] um so hello", "testhost", "echo"]);
	await assert.rejects(s.enhance({ text: " " }), (e: SaysoError) => e.code === "bad_request");
	passed("E01");
});

test("subscribe gets dictation_done, and never an incognito one", async () => {
	const h = await host();
	const s = await connect({ mode: "sayso", socket: h.socket });
	assert.deepEqual(await s.subscribe(["dictation_done", "future_event"]), ["dictation_done"]);
	const it = s.events();
	h.send({ dictation_done: { text: "Hello.", transcript: "hello", app: "Mail" } });
	const d = await next(it, message("dictation_done"));
	assert.equal(d.text, "Hello.");
	assert.equal(d.app, "Mail");
	assert.ok(String(d.time).length > 0);
	h.send({ dictation_done: { text: "secret", incognito: true } });
	await h.await("sent", "dictation_done");
	h.send({ dictation_done: { text: "after" } });
	assert.equal((await next(it, message("dictation_done"))).text, "after", "an incognito dictation was sent");
	passed("E02");
});

test("a host that closes ends the open job, and the connection ends with host_closing", async () => {
	const h = await host();
	const s = await connect({ mode: "sayso", socket: h.socket });
	await ready(s);
	const it = s.events();
	const job = await s.transcribeFile({ path: sixtyTones() });
	h.send({ close: true });
	const failed = await next(it, message("job_failed", (m) => m.job === job));
	assert.ok(failed.code === undefined || failed.code === "host_closing", String(failed.code));
	const gone = await next(it, (e) => (e.kind === "disconnected" ? e : undefined));
	assert.equal(gone.code, "host_closing");
	await assert.rejects(s.listModels(), (e: SaysoError) => e.code === "host_closing");
	passed("X01");
});

test("a killed host is connection_lost, with one end for each job", async () => {
	const h = await host("--speed", "0.5");
	const s = await connect({ mode: "sayso", socket: h.socket });
	await ready(s);
	const seen = record(s);
	const it = s.events();
	const job = await s.transcribeFile({ path: sixtyTones() });
	h.send({ kill: true });
	await h.exited;
	const failed = await next(it, message("job_failed", (m) => m.job === job));
	assert.equal(failed.code, "connection_lost");
	await next(it, (e) => (e.kind === "disconnected" && e.code === "connection_lost" ? e : undefined));
	assert.equal(seen.filter((e) => e.kind === "message" && (e.message.type === "job_done" || e.message.type === "job_failed")).length, 1);
	passed("X02");
});

test("Auto: a host killed while a job runs ends the job once, and Auto goes back to Local mode", async () => {
	const socket = socketPath();
	const h = await TestHost.start(socket, "--speed", "0.5");
	after(() => h.stop());
	const s = await connect({ mode: "auto", socket, timing: { watchMs: 50 } });
	assert.equal(s.mode(), "sayso", "Sayso runs, so Auto uses it");
	await ready(s);
	const seen = record(s);
	const it = s.events();
	const job = await s.transcribeFile({ path: sixtyTones() });
	h.send({ kill: true });
	await h.exited;
	const failed = await next(it, message("job_failed", (m) => m.job === job));
	assert.equal(failed.code, "connection_lost");
	const changed = await next(it, (e) => (e.kind === "mode_changed" ? e : undefined), [], 10_000).catch(async () => {
		await s.listModels();
		return next(it, (e) => (e.kind === "mode_changed" ? e : undefined));
	});
	assert.equal(changed.mode, "local");
	assert.equal(s.mode(), "local");
	assert.equal(seen.filter((e) => e.kind === "message" && (e.message.type === "job_done" || e.message.type === "job_failed")).length, 1);
	passed("A02");
});

test("Local mode cuts a file where the voice stops for long", async () => {
	const s = await connect({ mode: "local" });
	await s.downloadModel("fake-final", "tests");
	// Two tones of 3 words with 5 s of silence between them: longer than the longest pause inside one chunk, so the
	// final pass runs on two chunks, and the fake engine counts its words from alpha in each.
	const tone = speech(900).subarray(8000, 8000 + 900 * 16);
	const parts: number[] = new Array<number>(8000).fill(0);
	for (let n = 0; n < 2; n++) parts.push(...tone, ...new Array<number>(5 * 16000).fill(0));
	const path = join(tempDir("cut"), "cut.wav");
	writeFileSync(path, encodeWav(Int16Array.from(parts)));
	const it = s.events();
	const job = await s.transcribeFile({ path, model: "fake-final", vad_model: "basic" });
	const done = await next(it, message("job_done", (m) => m.job === job));
	assert.equal(done.text, "alpha bravo charlie alpha bravo charlie");
	passed("G03");
});

test("findEngine takes the last version folder and the best build for the processor", () => {
	const touch = (path: string) => {
		mkdirSync(dirname(path), { recursive: true });
		writeFileSync(path, "");
	};
	const dir = tempDir("eng");
	const exe = engineProgramName();
	touch(join(dir, "sayso-engine-0.11.0", exe));
	touch(join(dir, "sayso-engine-0.12.0", exe));
	touch(join(dir, "sayso-engine-0.13.0", "README.md"));
	touch(join(dir, "other", exe));
	assert.equal(findEngine(dir), join(dir, "sayso-engine-0.12.0", exe));
	assert.equal(findEngine(join(dir, "sayso-engine-0.11.0")), join(dir, "sayso-engine-0.11.0", exe));
	assert.throws(() => findEngine(tempDir("empty")), (e: SaysoError) => e.code === "unsupported" && e.message.includes("Unpack a release file"));

	const builds = tempDir("builds");
	const ext = process.platform === "win32" ? ".exe" : "";
	for (const name of ["sayso-engine", "sayso-engine-avx", "sayso-engine-baseline"]) touch(join(builds, name + ext));
	if (process.arch === "x64") {
		assert.equal(findEngine(builds, { avx: true, avx2: false }), join(builds, `sayso-engine-avx${ext}`));
		assert.equal(findEngine(builds, { avx: false, avx2: false }), join(builds, `sayso-engine-baseline${ext}`));
	}
	assert.equal(findEngine(builds, { avx: true, avx2: true }), join(builds, `sayso-engine${ext}`));
	passed("G05");
});

test("a cleanup model download checks the hash and writes ready.json last", async () => {
	const content = Buffer.from("0123456789abcdef".repeat(10_000));
	const sha = createHash("sha256").update(content).digest("hex");
	const modelsDir = tempDir("cl");
	let readyBeforeModel: boolean | undefined;
	const entry = (id: string, sha256: string): CatalogEntry => ({
		id, name: "Test", publisher: "Tests", sizeBytes: content.length, languages: ["en"], tones: false, layouts: ["paragraphs"], formats: ["plain"], context: false,
		repo: "test/repo", revision: "rev1", file: "test.gguf", sha256, licenseFiles: ["LICENSE"], prompt: "key_value",
	});
	const server = createHttpServer((req, res) => {
		if (req.url === "/test/repo/resolve/rev1/LICENSE") return void res.end("MIT");
		if (req.url === "/test/repo/resolve/rev1/test.gguf") {
			readyBeforeModel ??= existsSync(join(cleanupModelsDirOf(modelsDir), "good", "ready.json"));
			res.writeHead(200, { "Content-Length": content.length });
			return void res.end(content);
		}
		res.writeHead(404).end();
	});
	await new Promise<void>((res) => server.listen(0, "127.0.0.1", res));
	after(() => {
		server.closeAllConnections();
		server.close();
	});
	const address = server.address() as { port: number };
	const endpoint = `http://127.0.0.1:${address.port}`;

	const path = await downloadEntry(modelsDir, entry("good", sha), { endpoint });
	const dir = join(cleanupModelsDirOf(modelsDir), "good");
	assert.equal(path, join(dir, "test.gguf"));
	assert.ok(readFileSync(path).equals(content));
	assert.equal(readyBeforeModel, false, "ready.json comes after the model file");
	assert.deepEqual(JSON.parse(readFileSync(join(dir, "ready.json"), "utf8")), { revision: "rev1", sha256: sha, file: "test.gguf" });
	assert.equal(readFileSync(join(dir, "LICENSE"), "utf8"), "MIT");

	await assert.rejects(downloadEntry(modelsDir, entry("bad", "0".repeat(64)), { endpoint }), (e: SaysoError) => e.message.includes("not the expected file"));
	const bad = join(cleanupModelsDirOf(modelsDir), "bad");
	assert.ok(!existsSync(join(bad, "ready.json")) && !existsSync(join(bad, "test.gguf")) && !existsSync(join(bad, "test.gguf.partial")));
	passed("C02");
});

test("a long text goes to the cleanup runtime in parts and comes back as one text", async () => {
	const modelsDir = tempDir("cl");
	const files = modelFiles("spokenly-cleanup-1")!;
	const dir = join(cleanupModelsDirOf(modelsDir), files.id);
	mkdirSync(dir, { recursive: true });
	writeFileSync(join(dir, files.file), "gguf");
	writeFileSync(join(dir, "ready.json"), JSON.stringify({ revision: files.revision, sha256: files.sha256, file: files.file }));
	const cleaner = new RuntimeCleaner({ runtime: program("sayso-fake-cleanup"), modelsDir });
	try {
		const text = "um so we met on monday and we talked about the plan for the release. ".repeat(160);
		const r = await cleaner.clean({ model: files.id, text, timeoutMs: 60_000 });
		assert.ok(r.parts >= 2, `${r.parts} part`);
		assert.equal(r.model, files.id);
		assert.ok(!r.text.includes("um so") && r.text.includes("we met on monday"), r.text.slice(0, 200));
		assert.ok(r.tokensIn > 0 && r.tokensOut > 0);
		const short = await cleaner.clean({ model: files.id, text: "um hello there", timeoutMs: 60_000 });
		assert.deepEqual([short.parts, short.loadMs, short.text.trim()], [1, 0, "Hello there."]);
		passed("C04");
	} finally {
		cleaner.stop();
	}
});
