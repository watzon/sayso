// Speech in local mode with the fake engine: jobs, live sessions, events, errors, close.

import { test } from "node:test";
import assert from "node:assert/strict";
import { writeFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { execFileSync } from "node:child_process";
import { Speech, Engine, SaysoError } from "../dist/index.js";
import { concat, fakeEngine, fakeWorld, FINAL, silence, tone, until, voice } from "./support.mjs";

async function connect(t, world, extra = {}) {
	const speech = await Speech.connect({
		mode: "local",
		app: { name: "Tests" },
		engine: Engine.fromProgram(fakeEngine).store(world.store).modelsDir(world.models),
		models: { finalModel: FINAL, language: "en" },
		...extra,
	});
	t.after(() => speech.close());
	return speech;
}

test("local mode lists the models of the engine", async (t) => {
	const world = fakeWorld(t);
	const speech = await connect(t, world);
	assert.equal(speech.mode(), "local");
	const models = await speech.listModels();
	assert.ok(models.some((m) => m.id === FINAL && m.state === "downloaded"), JSON.stringify(models));
	const vad = await speech.listModels("vad");
	assert.deepEqual(vad.map((m) => m.id), ["fake-vad"]);
	assert.equal(speech.features().enhance, false);
});

test("a file job ends with job_done, and two listeners get the same events", async (t) => {
	const world = fakeWorld(t);
	const speech = await connect(t, world);
	const a = [];
	const b = [];
	speech.on((event) => a.push(event));
	const iterator = speech.events();
	const transcript = await speech.transcribe({ path: world.wav, model: FINAL });
	assert.ok(transcript.text.length > 0);
	assert.ok(transcript.segments.length > 0);
	for (;;) {
		const { value } = await iterator.next();
		b.push(value);
		if (value.kind === "message" && value.message.type === "job_done") break;
	}
	await iterator.return();
	const done = (list) => list.filter((e) => e.kind === "message" && e.message.job === transcript.job).map((e) => e.message.type);
	assert.deepEqual(done(a), done(b));
	assert.equal(done(a).at(-1), "job_done");
});

test("a request with a relative path fails with bad_request", async (t) => {
	const world = fakeWorld(t);
	const speech = await connect(t, world);
	await assert.rejects(speech.transcribeFile({ path: "talk.wav" }), (e) => e instanceof SaysoError && e.code === "bad_request" && /absolute/.test(e.message));
});

test("a cancelled job ends with job_failed", async (t) => {
	const world = fakeWorld(t);
	writeFileSync(join(world.models, "hold-transcribe"), "");
	const speech = await connect(t, world);
	const events = speech.events();
	const job = await speech.transcribeFile({ path: world.wav, model: FINAL });
	await speech.cancelJob(job);
	rmSync(join(world.models, "hold-transcribe"));
	for await (const event of events) {
		if (event.kind === "message" && event.message.job === job && event.message.type.startsWith("job_")) {
			if (event.message.type === "job_progress") continue;
			assert.equal(event.message.type, "job_failed");
			break;
		}
	}
});

test("a live session with the source client gives its text", async (t) => {
	const world = fakeWorld(t);
	const speech = await connect(t, world);
	const { session } = await speech.startSession({ sources: ["client"], model: FINAL, vad_model: "basic", language: "en" });
	const audio = tone(1500);
	for (let at = 0; at < audio.length; at += 1600) speech.sessionAudio(session, audio.subarray(at, at + 1600));
	const text = await speech.stopSession(session);
	assert.equal(typeof text, "string");
	assert.throws(() => speech.sessionAudio(session, new Int16Array(10)), SaysoError);
});

test("the segment events of a live session come before the text of stopSession", async (t) => {
	// As in embedded.test.mjs: a slow thread of the events shows the order.
	const world = fakeWorld(t);
	const script = `
import { Engine, Speech } from ${JSON.stringify(new URL("../dist/index.js", import.meta.url).href)};
const speech = await Speech.connect({
	mode: "local",
	app: { name: "Tests" },
	engine: Engine.fromProgram(${JSON.stringify(fakeEngine)}).store(${JSON.stringify(world.store)}).modelsDir(${JSON.stringify(world.models)}),
	models: { finalModel: ${JSON.stringify(FINAL)}, language: "en" },
});
const seen = [];
speech.on((event) => { if (event.kind === "message" && event.message.type === "segment") seen.push(event.message.text); });
const { session } = await speech.startSession({ sources: ["client"], model: ${JSON.stringify(FINAL)}, vad_model: "basic", language: "en" });
const audio = new Int16Array(${JSON.stringify(Array.from(concat(silence(500), voice(1000), silence(800), voice(800)), (v) => Math.round(v * 32767)))});
for (let at = 0; at < audio.length; at += 1600) speech.sessionAudio(session, audio.subarray(at, at + 1600));
const text = await speech.stopSession(session);
console.log(JSON.stringify({ text, segments: seen.join(" ") }));
await speech.close();
`;
	const file = join(world.dir, "order.mjs");
	writeFileSync(file, script);
	const out = execFileSync(process.execPath, [file], { env: { ...process.env, SAYSO_NODE_EVENT_DELAY_MS: "100" }, encoding: "utf8" });
	const { text, segments } = JSON.parse(out.trim().split("\n").pop());
	assert.ok(text.length > 0, "the session has text");
	assert.equal(segments, text);
});

test("a microphone in JavaScript gives the audio of a live session", async (t) => {
	const world = fakeWorld(t);
	let closed = false;
	let opened = 0;
	const microphone = {
		open(writer) {
			opened += 1;
			writer.onClose(() => {
				closed = true;
			});
			const audio = tone(1200);
			const samples = Float32Array.from(audio, (s) => s / 32768);
			let at = 0;
			const timer = setInterval(() => {
				if (writer.closed || at >= samples.length) return clearInterval(timer);
				writer.push(samples.subarray(at, at + 1600));
				at += 1600;
			}, 5);
			return { excludes: [] };
		},
	};
	const speech = await connect(t, world, { capture: { microphone } });
	const { session } = await speech.startSession({ sources: ["mic"], model: FINAL, vad_model: "basic", language: "en" });
	await new Promise((r) => setTimeout(r, 200));
	const text = await speech.stopSession(session);
	assert.equal(typeof text, "string");
	assert.equal(opened, 1);
	await until(() => closed, 5000, "the close of the writer");
});

test("a source that refuses gives the code permission", async (t) => {
	const world = fakeWorld(t);
	const microphone = {
		open() {
			throw new SaysoError({ code: "permission", message: "the user said no" });
		},
	};
	const speech = await connect(t, world, { capture: { microphone } });
	await assert.rejects(speech.startSession({ sources: ["mic"], model: FINAL, vad_model: "basic" }), (e) => e.code === "permission" && /said no/.test(e.message));
});

test("close ends the work, the iterators, and later calls, without a hang", async (t) => {
	const world = fakeWorld(t);
	writeFileSync(join(world.models, "hold-transcribe"), "");
	const speech = await connect(t, world);
	const events = speech.events();
	const job = await speech.transcribeFile({ path: world.wav, model: FINAL });
	const seen = [];
	const reading = (async () => {
		for await (const event of events) seen.push(event);
	})();
	await speech.close();
	await reading;
	await assert.rejects(speech.listModels(), (e) => e.code === "connection_lost");
	// A closed Speech sends nothing more: the job has no end event, as a drop in Rust.
	assert.ok(!seen.some((e) => e.kind === "message" && e.message.job === job && e.message.type === "job_done"), JSON.stringify(seen));
	await speech.close();
});

test("a number above 2^53 is refused", async (t) => {
	const world = fakeWorld(t);
	const speech = await connect(t, world);
	await assert.rejects(speech.cancelJob(2 ** 53), /2\^53/);
	await assert.rejects(speech.cancelJob(1.5), /integer/);
});

test("local mode without an engine fails with bad_request", async () => {
	await assert.rejects(Speech.connect({ mode: "local", app: { name: "Tests" } }), (e) => e.code === "bad_request");
});

test("a missing engine fails with unsupported", async () => {
	await assert.rejects(Speech.connect({ mode: "local", app: { name: "Tests" }, engine: Engine.fromProgram("/no/such/engine") }), (e) => e.code === "unsupported");
});

test("bad options are a TypeError of the addon", async () => {
	await assert.rejects(Speech.connect({ mode: "remote", app: { name: "Tests" } }), /remote/);
});

test("an aborted transcribe cancels the job and rejects with the reason of the signal", async (t) => {
	const world = fakeWorld(t);
	writeFileSync(join(world.models, "hold-transcribe"), "");
	const speech = await connect(t, world);
	const controller = new AbortController();
	const events = speech.events();
	const transcribing = speech.transcribe({ path: world.wav, model: FINAL }, { signal: controller.signal });
	for await (const event of events) {
		if (event.kind === "message" && event.message.type === "job_progress") break;
	}
	controller.abort();
	await assert.rejects(transcribing, (e) => e.name === "AbortError");
	rmSync(join(world.models, "hold-transcribe"));
});
