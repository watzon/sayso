import assert from "node:assert/strict";
import { existsSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { after, test } from "node:test";
import { Client } from "../src/client.js";
import { Cleanup } from "../src/cleanup.js";
import { Engine } from "../src/engine.js";
import { EngineStartError, SaysoError } from "../src/errors.js";
import { Speech, type SpeechEvent, type SpeechOptions } from "../src/speech.js";
import { encodeI16 } from "../src/protocol/codec.js";
import type { Capability } from "../src/protocol/index.js";
import { TestHost, next, program, removeTemps, sleep, socketPath, speech, speechWav, tempDir, passed } from "./support.js";

after(removeTemps);

const ALL: Capability[] = ["models.read", "models.download", "transcribe.file", "session.live", "audio.mic", "ai.enhance", "dictation.observe"];

const opened: { close(): Promise<void> }[] = [];
after(async () => {
	for (const o of opened.splice(0)) await o.close().catch(() => undefined);
});

async function connect(options: Partial<SpeechOptions> & { mode: SpeechOptions["mode"] }, env?: Record<string, string>): Promise<Speech> {
	const s = await Speech.connect({
		app: { name: "Notes" },
		capabilities: ALL,
		engine: Engine.fromProgram(program("sayso-fake-engine")),
		modelsDir: join(tempDir("models"), "models"),
		...(env ? { engineEnv: env } : {}),
		...options,
	});
	opened.push(s);
	return s;
}

type Msg = { type: string; [k: string]: unknown };
const message = (type: string) => (e: SpeechEvent): Msg | undefined => (e.kind === "message" && e.message.type === type ? (e.message as Msg) : undefined);

test("Local mode transcribes a file", async () => {
	const s = await connect({ mode: "local" });
	const f = s.features();
	assert.equal(s.mode(), "local");
	assert.ok(!f.enhance && !f.subscribe && f.engine?.includes("segments"));
	const models = await s.listModels();
	assert.equal(models.length, 4);
	assert.equal(models[0]!.state, "not_downloaded");
	const path = speechWav(900, 600);
	await assert.rejects(s.transcribeFile({ path }), (e: SaysoError) => e.code === "model_not_downloaded");
	await s.downloadModel("fake-final", "tests");
	await assert.rejects(s.downloadModel("nothing", "tests"), (e: SaysoError) => e.code === "unknown_model");
	const it = s.events();
	const job = await s.transcribeFile({ path });
	const seen: string[] = [];
	const done = await next(it, message("job_done"), seen);
	assert.equal(done.job, job);
	assert.equal(done.text, "alpha bravo charlie delta echo");
	assert.deepEqual(seen.filter((t) => t !== "model_state"), ["job_progress", "segment", "segment", "job_progress", "job_done"]);
	await assert.rejects(s.transcribeFile({ path: "relative.wav" }), (e: SaysoError) => e.code === "bad_request");
	const t = await s.transcribe({ path });
	assert.equal(t.text, "alpha bravo charlie delta echo");
	assert.equal(t.segments.length, 2);
	passed("G01");
	passed("J04");
});

test("Local mode diarizes", async () => {
	const s = await connect({ mode: "local" });
	await s.downloadModel("fake-final", "tests");
	await s.downloadModel("fake-diar", "tests");
	const it = s.events();
	await s.transcribeFile({ path: speechWav(600, 600), diarize: true, speakers: 2 });
	const sp = await next(it, message("speakers"));
	const turns = sp.turns as { speaker: string }[];
	assert.deepEqual(turns.map((t) => t.speaker), ["1", "2"]);
	await next(it, message("job_done"));
	passed("J02");
});

test("Local mode cancels a job", async () => {
	const s = await connect({ mode: "local" }, { SAYSO_FAKE_SCRIPT: '{"delay_ms":{"transcribe":1500}}' });
	await s.downloadModel("fake-final", "tests");
	const it = s.events();
	const job = await s.transcribeFile({ path: speechWav(600) });
	await s.cancelJob(job);
	const failed = await next(it, message("job_failed"));
	assert.equal(failed.message, "The job was cancelled.");
	await sleep(1800);
	let extra = false;
	s.on((e) => {
		if (e.kind === "message") extra = true;
	});
	await sleep(100);
	assert.equal(extra, false);
	passed("J03");
});

test("Local mode runs a live session of the client, and records it", async () => {
	const s = await connect({ mode: "local" });
	await s.downloadModel("fake-stream", "tests");
	await assert.rejects(s.startSession({ sources: ["mic"] }), (e: SaysoError) => e.code === "unsupported");
	const rec = join(tempDir("rec"), "rec");
	const it = s.events();
	const sess = await s.startSession({ sources: ["client"], model: "fake-stream", record_to: rec });
	const samples = speech(900);
	for (let i = 0; i < samples.length; i += 2560) s.sessionAudio(sess.session, samples.subarray(i, i + 2560));
	const p = await next(it, message("partial"));
	assert.equal(p.session, sess.session);
	assert.equal(p.source, "client");
	assert.ok(Number.isSafeInteger(sess.session) && sess.session > 0);
	const text = await s.stopSession(sess.session);
	assert.equal(text, "alpha bravo charlie");
	assert.ok(existsSync(join(rec, "client.wav")));
	passed("G02");
});

test("Local mode needs Sayso for enhance and subscribe", async () => {
	const s = await connect({ mode: "local" });
	await assert.rejects(s.enhance({ text: "hi" }), (e: SaysoError) => e.code === "unsupported");
	await assert.rejects(s.subscribe(["dictation_done"]), (e: SaysoError) => e.code === "unsupported");
});

test("an engine that does not start says why", async () => {
	await assert.rejects(
		Speech.connect({ mode: "local", app: { name: "n" }, modelsDir: tempDir("m"), engine: Engine.fromProgram(program("sayso-fake-engine")), engineEnv: { SAYSO_FAKE_SCRIPT: '{"stderr":["cannot load libwhisper"],"exit_at_start":4}' } }),
		(e: unknown) => e instanceof EngineStartError && e.message.includes("cannot load libwhisper"),
	);
	await assert.rejects(Speech.connect({ mode: "local", app: { name: "n" } }), (e: SaysoError) => e.code === "unsupported");
	passed("G04");
});

test("Sayso mode without Sayso is host_not_running", async () => {
	await assert.rejects(Speech.connect({ mode: "sayso", app: { name: "n" }, socket: socketPath() }), (e: SaysoError) => e.code === "host_not_running");
	passed("A04");
});

test("Sayso mode uses the host", async () => {
	const h = await TestHost.start();
	after(() => h.stop());
	const s = await connect({ mode: "sayso", socket: h.socket });
	assert.equal(s.mode(), "sayso");
	const f = s.features();
	assert.ok(f.enhance && f.subscribe && f.downloadQuestion && f.host.includes("record_to"));
	await assert.rejects(s.startSession({ sources: ["mic"] }), (e: SaysoError) => e.code === "unsupported" && e.message.includes("mic_capture"));
	const r = await s.enhance({ text: "um hello", prompt: "Fix." });
	assert.equal(r.text, "[app-prompt] um hello");
	const it = s.events();
	assert.deepEqual(await s.subscribe(["dictation_done"]), ["dictation_done"]);
	h.send({ dictation_done: { text: "Hi.", transcript: "hi" } });
	const d = await next(it, message("dictation_done"));
	assert.equal(d.text, "Hi.");
	passed("H01");
});

test("a denied app is not approved", async () => {
	const h = await TestHost.start(undefined, "--approve", "deny");
	after(() => h.stop());
	await assert.rejects(Speech.connect({ mode: "auto", app: { name: "n" }, socket: h.socket, engine: Engine.fromProgram(program("sayso-fake-engine")), modelsDir: tempDir("m") }), (e: SaysoError) => e.code === "not_approved");
	passed("H02");
});

test("Sayso mode runs the whole lifecycle through the Client", async () => {
	const h = await TestHost.start();
	after(() => h.stop());
	let waiting = 0;
	const c = await Client.connect({ path: h.socket, app: { name: "Lifecycle", capabilities: ALL }, onApprovalWaiting: () => waiting++ });
	opened.push(c);
	assert.ok(c.hello().capabilities.includes("models.read"));
	assert.ok(c.supports("record_to"));
	const it = c.events();

	const models = (await c.request("list_models", {})) as { models: { id: string; state: string }[] };
	assert.ok(models.models.length > 0);
	const wav = speechWav(900, 600);
	const final = models.models.find((m) => ((m as { choice?: string[] }).choice ?? []).includes("final"));
	assert.ok(final, "a final model of the user");
	await c.request("download_model", { model: final.id, reason: "tests" });
	const job = (await c.request("transcribe_file", { path: wav })) as { job: number };
	const done = await next(it, (e) => (e.kind === "host" && (e.event as Msg).type === "job_done" ? (e.event as Msg) : undefined));
	assert.equal(done.job, job.job);
	assert.ok(String(done.text).includes("alpha"));

	const sess = (await c.request("session_start", { sources: ["client"] })) as { session: number };
	const pcm = speech(900, 900);
	for (let i = 0; i < pcm.length; i += 2560) c.send("session_audio", { session: sess.session, pcm: encodeI16(pcm.subarray(i, i + 2560)) });
	await sleep(200);
	const stop = (await c.request("session_stop", { session: sess.session })) as { text: string };
	assert.ok(stop.text.includes("alpha"), stop.text);
	await assert.rejects(c.request("session_start", { sources: ["mic"] }), (e: SaysoError) => e.code === "unsupported");

	await assert.rejects(c.request("cancel_job", { job: 12345 }), (e: SaysoError) => e.code !== undefined);

	h.send({ close: true });
	const gone = await next(it, (e) => (e.kind === "host_gone" ? e : undefined));
	assert.ok(gone.code === "host_closing" || gone.code === "connection_lost");
	await assert.rejects(c.request("list_models", {}), (e: SaysoError) => e.code === "host_closing" || e.code === "connection_lost");
	void waiting;
	passed("J01");
});

test("a capability that the app did not ask for is refused", async () => {
	const h = await TestHost.start();
	after(() => h.stop());
	const c = await Client.connect({ path: h.socket, app: { name: "Narrow", capabilities: ["models.read"] } });
	opened.push(c);
	await assert.rejects(c.request("transcribe_file", { path: speechWav(300) }), (e: SaysoError) => e.code === "capability" && e.capability === "transcribe.file");
	await c.request("bye", {}).catch(() => undefined);
	passed("H04");
});

test("a killed host is connection_lost", async () => {
	const h = await TestHost.start();
	const c = await Client.connect({ path: h.socket, app: { name: "Killed", capabilities: ALL } });
	opened.push(c);
	const it = c.events();
	h.send({ kill: true });
	const gone = await next(it, (e) => (e.kind === "host_gone" ? e : undefined));
	assert.equal(gone.code, "connection_lost");
	await h.exited;
});

test("Auto follows Sayso while idle, and never starts it", async () => {
	const socket = socketPath();
	const s = await connect({ mode: "auto", socket, timing: { watchMs: 50 } });
	assert.equal(s.mode(), "local");
	await s.downloadModel("fake-stream", "tests");
	const sess = await s.startSession({ sources: ["client"], model: "fake-stream" });
	const h = await TestHost.start(socket);
	after(() => h.stop());
	await sleep(400);
	assert.equal(s.mode(), "local", "a running session keeps Local");
	await s.stopSession(sess.session);
	const it = s.events();
	await s.listModels();
	const mc = await next(it, (e) => (e.kind === "mode_changed" ? e : undefined), [], 10_000).catch(async () => {
		await s.listModels();
		return next(it, (e) => (e.kind === "mode_changed" ? e : undefined));
	});
	assert.equal(mc.mode, "sayso");
	assert.ok(mc.features.host.length > 0);
	assert.equal(s.mode(), "sayso");
	await h.stop();
	const back = await next(it, (e) => (e.kind === "mode_changed" ? e : undefined), [], 15_000).catch(async () => {
		await s.listModels().catch(() => undefined);
		return next(it, (e) => (e.kind === "mode_changed" ? e : undefined));
	});
	assert.equal(back.mode, "local");
	assert.equal(s.mode(), "local");
	passed("A01");
	passed("A03");
});

test("Sayso mode starts Sayso with a launcher", async () => {
	const socket = socketPath();
	let started = false;
	const hosts: TestHost[] = [];
	const failing = () => Promise.reject(new Error("not installed here"));
	const launch = async () => {
		started = true;
		hosts.push(await TestHost.start(socket));
	};
	const s = await connect({ mode: { sayso: { start: true } }, socket, timing: { startWaitMs: 15_000 }, launchers: [failing, launch] as never, start: launch });
	assert.equal(s.mode(), "sayso");
	assert.ok(started);
	for (const h of hosts) await h.stop();
	passed("S01");
});

test("the Sayso mode reconnects after the host came back", async () => {
	const socket = socketPath();
	const h = await TestHost.start(socket);
	const s = await connect({ mode: "sayso", socket });
	await h.stop();
	await assert.rejects(s.listModels(), (e: SaysoError) => e.code === "host_not_running" || e.code === "host_closing" || e.code === "connection_lost");
	const h2 = await TestHost.start(socket);
	after(() => h2.stop());
	await s.reconnect();
	assert.ok((await s.listModels()).length > 0);
	passed("X03");
});

test("the cleanup client talks to the fake runtime", async () => {
	const c = Cleanup.start(program("sayso-fake-cleanup"));
	try {
		const hello = await c.hello();
		assert.equal(hello.version, 1);
		const model = join(tempDir("cl"), "m.gguf");
		writeFileSync(model, "gguf");
		assert.ok((await c.load("s1-mini", model, 64)) >= 0);
		assert.equal(await c.count("s1-mini", "um so hello"), 3);
		const done = await c.complete("s1-mini", "text: um so hello there\nout:", ["\ntext:"], 20);
		assert.equal(done.stop, "end");
		assert.equal(done.text, " So hello there.");
		const cut = await c.complete("s1-mini", "text: one two three four\nout:", [], 2);
		assert.equal(cut.stop, "limit");
		await c.unload();
		await assert.rejects(c.count("s1-mini", "x"), (e: Error) => e.name !== "");
		passed("C03");
	} finally {
		await c.close();
	}
});

test("the cleanup client reports a runtime that crashes", async () => {
	const c = Cleanup.start(program("sayso-fake-cleanup"), { env: { SAYSO_FAKE_SCRIPT: '{"stderr":["no llama"],"exit_at_start":3}' } });
	await assert.rejects(c.hello(), (e: SaysoError) => /stopped/.test(e.message));
	assert.deepEqual(c.stderrTail(), ["no llama"]);
	await c.close();
});
