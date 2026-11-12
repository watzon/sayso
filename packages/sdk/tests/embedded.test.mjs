// EmbeddedSpeech with the fake engine: models, downloads, live sessions, a final pass in
// JavaScript, Deepgram, cancel, and a microphone in JavaScript.
// The scenarios follow crates/sayso-sdk/tests/embedded_session.rs.

import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { EmbeddedSpeech, DeepgramFinal, Deepgram, SaysoError } from "../dist/index.js";
import { concat, deepgramReply, fakeDeepgram, fakeEngine, fakeWorld, FINAL, silence, STREAMING, tempDir, until, voice } from "./support.mjs";

const index = new URL("../dist/index.js", import.meta.url).href;

/** Connect to the fake engine, and close at the end of the test. */
async function connect(t, world, extra = {}) {
	const speech = await EmbeddedSpeech.connect({
		app: { name: "Tests" },
		engine: { program: fakeEngine, modelsDir: world.models, store: world.store },
		tempDir: tempDir(t),
		...extra,
	});
	const seen = [];
	speech.on((event) => seen.push(event));
	t.after(() => speech.close());
	return { speech, seen };
}

/** Two parts of speech with silence around them. Basic VAD cuts two parts out of this. */
const talk = () => concat(silence(500), voice(1000), silence(800), voice(800));

/** Give float audio to a client session in chunks of 100 ms. */
function feed(speech, session, audio) {
	for (let at = 0; at < audio.length; at += 1600) speech.sessionAudio(session, audio.subarray(at, at + 1600));
}

const FAKE_TEXT = "Hello there. Bye. Later";
const session = (extra = {}) => ({ sources: ["client"], finalModel: FINAL, vad: "basic", language: "en", ...extra });

test("models lists the catalog of the fake world", async (t) => {
	const world = fakeWorld(t);
	const { speech } = await connect(t, world);
	assert.equal(speech.mode(), "local");
	const models = await speech.models();
	const byId = Object.fromEntries(models.map((m) => [m.id, m]));
	assert.deepEqual(byId[FINAL].roles, ["final"]);
	assert.deepEqual(byId[STREAMING].roles, ["streaming", "final"]);
	assert.deepEqual(byId["fake-vad"].roles, ["vad"]);
	assert.equal(byId[FINAL].state, "downloaded");
	assert.equal(byId[FINAL].size_bytes, 1000);
	assert.deepEqual(byId[FINAL].languages, ["en"]);
});

test("a missing engine program fails at the first call that needs it", async (t) => {
	const world = fakeWorld(t);
	const speech = await EmbeddedSpeech.connect({ app: { name: "Tests" }, engine: { program: "/no/such/engine", modelsDir: world.models, store: world.store }, tempDir: tempDir(t) });
	t.after(() => speech.close());
	await assert.rejects(speech.models(), (e) => e instanceof SaysoError);
});

test("download reports progress and the model is downloaded after it", async (t) => {
	const world = fakeWorld(t);
	rmSync(join(world.models, `downloaded-${FINAL}`));
	const { speech } = await connect(t, world);
	const before = (await speech.models()).find((m) => m.id === FINAL);
	assert.equal(before.state, "not_downloaded");
	const progress = [];
	await speech.download(FINAL, { sizeBytes: 1000, onProgress: (p) => progress.push(p) });
	// The fake engine reports 50 % at once, then waits, then says downloaded.
	assert.ok(progress.length >= 1, "onProgress was called");
	assert.equal(progress[0].model, FINAL);
	assert.equal(progress[0].fraction, 0.5);
	const after = (await speech.models()).find((m) => m.id === FINAL);
	assert.equal(after.state, "downloaded");
	// And the model can go again.
	await speech.delete(FINAL);
	assert.equal((await speech.models()).find((m) => m.id === FINAL).state, "not_downloaded");
});

test("a download without onProgress works", async (t) => {
	const world = fakeWorld(t);
	rmSync(join(world.models, `downloaded-${FINAL}`));
	const { speech } = await connect(t, world);
	await speech.download(FINAL);
	assert.equal((await speech.models()).find((m) => m.id === FINAL).state, "downloaded");
});

test("load and unload resolve for a model of the catalog", async (t) => {
	const world = fakeWorld(t);
	const { speech } = await connect(t, world);
	await speech.load(FINAL);
	await speech.unload(FINAL);
});

test("a live session gives its segments before stopSession resolves, and the text", async (t) => {
	const world = fakeWorld(t);
	const { speech, seen } = await connect(t, world);
	const id = await speech.startSession(session());
	assert.equal(typeof id, "number");
	feed(speech, id, talk());
	const text = await speech.stopSession(id);
	// The segments are in `seen` already: no waiting after the stop.
	const segments = seen.filter((e) => e.type === "segment");
	assert.ok(segments.length > 0, JSON.stringify(seen));
	assert.ok(segments.every((e) => e.session === id && e.source === "client"));
	assert.equal(text, `${FAKE_TEXT} ${FAKE_TEXT}`, "two parts of speech, two final passes");
	// The second part comes later in the session than the first.
	const starts = segments.map((e) => e.start_ms);
	assert.deepEqual(starts, [...starts].sort((a, b) => a - b));
	assert.ok(starts.at(-1) >= 2000, `the second part starts after 2 s: ${starts}`);
});

test("audio with no speech gives no segment", async (t) => {
	const world = fakeWorld(t);
	const { speech, seen } = await connect(t, world);
	const id = await speech.startSession(session());
	feed(speech, id, silence(2000));
	assert.equal(await speech.stopSession(id), "");
	assert.ok(!seen.some((e) => e.type === "segment"), JSON.stringify(seen));
});

test("two iterators get the same events, and close ends them", async (t) => {
	const world = fakeWorld(t);
	const { speech } = await connect(t, world);
	const a = speech.events();
	const b = speech.events();
	const id = await speech.startSession(session());
	feed(speech, id, talk());
	await speech.stopSession(id);
	await speech.close();
	const read = async (iterator) => {
		const types = [];
		for await (const event of iterator) types.push(event.type);
		return types;
	};
	const [first, second] = await Promise.all([read(a), read(b)]);
	assert.deepEqual(first, second);
	assert.ok(first.includes("segment"));
});

test("a preview model gives partial events, and the silence limit gives one event", async (t) => {
	const world = fakeWorld(t);
	const { speech, seen } = await connect(t, world);
	const id = await speech.startSession(session({ previewModel: STREAMING, silenceLimitMs: 1000 }));
	feed(speech, id, concat(voice(1000), silence(2500)));
	await until(() => seen.some((e) => e.type === "silence_limit"), 5000, "the silence limit");
	const limit = seen.find((e) => e.type === "silence_limit");
	assert.equal(limit.session, id);
	assert.equal(limit.silence_ms, 1000);
	assert.ok(seen.some((e) => e.type === "partial"), "the preview model gives partials");
	await speech.stopSession(id);
	assert.equal(seen.filter((e) => e.type === "silence_limit").length, 1, "one event");
	// A new session can start after that.
	const again = await speech.startSession(session());
	assert.ok(again > id);
	speech.cancelSession();
});

test("only one live session runs at a time", async (t) => {
	const world = fakeWorld(t);
	const { speech } = await connect(t, world);
	const id = await speech.startSession(session());
	await assert.rejects(speech.startSession(session()), (e) => e instanceof SaysoError && /another live session/.test(e.message));
	await speech.stopSession(id);
});

test("audio for a session that does not exist fails", async (t) => {
	const world = fakeWorld(t);
	const { speech } = await connect(t, world);
	assert.throws(() => speech.sessionAudio(99, new Float32Array(160)), SaysoError);
	await assert.rejects(speech.stopSession(99), SaysoError);
});

test("cancelSession stops at once and forgets the parts that wait", async (t) => {
	const world = fakeWorld(t);
	const { speech, seen } = await connect(t, world);
	const id = await speech.startSession(session());
	feed(speech, id, voice(1000));
	const started = Date.now();
	speech.cancelSession();
	assert.ok(Date.now() - started < 200, "it returns at once");
	// The session takes no audio, and has nothing to stop.
	assert.throws(() => speech.sessionAudio(id, voice(100)), SaysoError);
	await assert.rejects(speech.stopSession(id), SaysoError);
	await new Promise((r) => setTimeout(r, 300));
	assert.ok(!seen.some((e) => e.type === "segment"), "the parts that waited are forgotten");
	// A new session starts soon after (the old one ends by itself).
	let again;
	await until(async () => {
		try {
			again = await speech.startSession(session());
			return true;
		} catch (e) {
			assert.match(e.message, /another live session/);
			return false;
		}
	}, 5000, "a new session after the cancel");
	assert.ok(again > id);
});

test("cancelSession with no session does nothing", async (t) => {
	const world = fakeWorld(t);
	const { speech } = await connect(t, world);
	speech.cancelSession();
	const id = await speech.startSession(session());
	await speech.stopSession(id);
});

test("a final pass in JavaScript gives the text of its model", async (t) => {
	const world = fakeWorld(t);
	const calls = [];
	const { speech, seen } = await connect(t, world, {
		finalPass: {
			models: ["js-final"],
			transcribe: async (samples, model, language) => {
				calls.push({ samples, model, language });
				return "from javascript";
			},
		},
	});
	const id = await speech.startSession(session({ finalModel: "js-final" }));
	feed(speech, id, talk());
	const text = await speech.stopSession(id);
	const segments = seen.filter((e) => e.type === "segment");
	assert.ok(segments.length >= 1);
	assert.ok(segments.every((e) => e.text === "from javascript"), JSON.stringify(segments));
	assert.equal(text, segments.map(() => "from javascript").join(" "));
	// One call for each part of speech, with samples as a Float32Array.
	assert.equal(calls.length, segments.length);
	for (const call of calls) {
		assert.ok(call.samples instanceof Float32Array);
		assert.ok(call.samples.length >= 16000 * 0.5, `a part of speech: ${call.samples.length} samples`);
		assert.ok(call.samples.some((s) => s !== 0));
		assert.equal(call.model, "js-final");
		assert.equal(call.language, "en");
	}
});

test("a final pass in JavaScript can be a plain function, and leaves other models to the engine", async (t) => {
	const world = fakeWorld(t);
	const { speech } = await connect(t, world, { finalPass: { models: ["js-final"], transcribe: () => "sync text" } });
	const own = await speech.startSession(session({ finalModel: "js-final" }));
	feed(speech, own, voice(1000));
	assert.equal(await speech.stopSession(own), "sync text");
	// The engine still serves its own models.
	const other = await speech.startSession(session({ finalModel: FINAL }));
	feed(speech, other, voice(1000));
	assert.equal(await speech.stopSession(other), FAKE_TEXT);
});

test("a final pass in JavaScript that throws drops the part and gives part_failed: no segment, an empty text", async (t) => {
	// The Rust layer logs a failed final pass of a part, sends part_failed, and goes on
	// (sayso-speech, live/source.rs, FinalPass::run).
	const world = fakeWorld(t);
	let calls = 0;
	const { speech, seen } = await connect(t, world, {
		finalPass: {
			models: ["js-final"],
			transcribe: async () => {
				calls += 1;
				throw new SaysoError({ code: "cloud_key", message: "boom" });
			},
		},
	});
	const id = await speech.startSession(session({ finalModel: "js-final" }));
	feed(speech, id, talk());
	assert.equal(await speech.stopSession(id), "");
	assert.ok(calls >= 1, "the function ran");
	await until(() => seen.some((e) => e.type === "part_failed"), 5000, "the part_failed event");
	const failed = seen.filter((e) => e.type === "part_failed");
	assert.equal(failed.length, calls, "one event for each failed part");
	for (const event of failed) {
		assert.equal(event.session, id);
		assert.equal(event.source, "client");
		assert.ok(event.message.includes("boom"), event.message);
		assert.ok(event.start_ms >= 0 && event.end_ms > event.start_ms, JSON.stringify(event));
	}
	assert.equal(seen.filter((e) => e.type === "segment").length, 0, "no segment");
});

test("a final pass that gives a value that is not a string drops the part and gives part_failed", async (t) => {
	const world = fakeWorld(t);
	let calls = 0;
	const { speech, seen } = await connect(t, world, {
		finalPass: {
			models: ["js-final"],
			transcribe: async () => {
				calls += 1;
				return 42;
			},
		},
	});
	const id = await speech.startSession(session({ finalModel: "js-final" }));
	feed(speech, id, voice(1000));
	assert.equal(await speech.stopSession(id), "");
	assert.ok(calls >= 1);
	await until(() => seen.some((e) => e.type === "part_failed"), 5000, "the part_failed event");
	assert.deepEqual(seen.map((e) => e.type), ["part_failed"]);
});

test("a session still works after a final pass failed once", async (t) => {
	const world = fakeWorld(t);
	let calls = 0;
	const { speech } = await connect(t, world, {
		finalPass: {
			models: ["js-final"],
			transcribe: async () => {
				calls += 1;
				if (calls === 1) throw new Error("first call fails");
				return "second works";
			},
		},
	});
	const id = await speech.startSession(session({ finalModel: "js-final" }));
	feed(speech, id, talk());
	assert.equal(await speech.stopSession(id), "second works");
});

test("a Deepgram client in a final pass function: the text arrives, and a refused key gives part_failed", async (t) => {
	const world = fakeWorld(t);
	let status = 200;
	const server = await fakeDeepgram(t, () => (status === 200 ? { status, body: deepgramReply("deepgram says hi") } : { status, body: {} }));
	const deepgram = new Deepgram("key", { baseUrl: server.baseUrl });
	let refused;
	const { speech, seen } = await connect(t, world, {
		finalPass: {
			models: [Deepgram.MODEL],
			transcribe: async (samples, model, language) => {
				try {
					return await deepgram.transcribe(samples, language);
				} catch (e) {
					refused = e;
					throw e;
				}
			},
		},
	});
	const ok = await speech.startSession(session({ finalModel: Deepgram.MODEL }));
	feed(speech, ok, voice(1000));
	assert.equal(await speech.stopSession(ok), "deepgram says hi");
	assert.equal(server.requests[0].headers.authorization, "Token key");
	seen.length = 0;
	status = 401;
	const bad = await speech.startSession(session({ finalModel: Deepgram.MODEL }));
	feed(speech, bad, voice(1000));
	assert.equal(await speech.stopSession(bad), "");
	assert.equal(refused.code, "cloud_key", "the function saw the code");
	await until(() => seen.some((e) => e.type === "part_failed"), 5000, "the part_failed event");
	assert.deepEqual(seen.map((e) => e.type), ["part_failed"], "the session shows the failed part");
});


test("the segment events of a session come before the text of stopSession", async (t) => {
	// The text and the events cross from the addon in two ways, with no order between them
	// in Node. stopSession waits for the events. A slow thread of the events (a busy
	// computer) shows it: without the wait, the text comes before the last segment.
	const world = fakeWorld(t);
	const script = `
import { EmbeddedSpeech } from ${JSON.stringify(index)};
const speech = await EmbeddedSpeech.connect({
	app: { name: "Tests" },
	engine: { program: ${JSON.stringify(fakeEngine)}, modelsDir: ${JSON.stringify(world.models)}, store: ${JSON.stringify(world.store)} },
	tempDir: ${JSON.stringify(tempDir(t))},
});
const seen = [];
speech.on((event) => seen.push(event));
const samples = new Float32Array(${JSON.stringify(Array.from(talk()))});
const id = await speech.startSession({ sources: ["client"], finalModel: ${JSON.stringify(FINAL)}, vad: "basic", language: "en" });
for (let at = 0; at < samples.length; at += 1600) speech.sessionAudio(id, samples.subarray(at, at + 1600));
const text = await speech.stopSession(id);
const segments = seen.filter((e) => e.type === "segment").map((e) => e.text).join(" ");
console.log(JSON.stringify({ text, segments }));
await speech.close();
`;
	const file = join(tempDir(t), "order.mjs");
	writeFileSync(file, script);
	const out = execFileSync(process.execPath, [file], { env: { ...process.env, SAYSO_NODE_EVENT_DELAY_MS: "100" }, encoding: "utf8" });
	const { text, segments } = JSON.parse(out.trim().split("\n").pop());
	assert.ok(text.length > 0, "the session has text");
	assert.equal(segments, text);
});

test("DeepgramFinal gives the text of Deepgram in the segments", async (t) => {
	const world = fakeWorld(t);
	const server = await fakeDeepgram(t, () => ({ status: 200, body: deepgramReply("deepgram says hi") }));
	const { speech, seen } = await connect(t, world, { finalPass: new DeepgramFinal("key", { baseUrl: server.baseUrl }) });
	const id = await speech.startSession(session({ finalModel: "deepgram-nova-3" }));
	feed(speech, id, talk());
	const text = await speech.stopSession(id);
	const segments = seen.filter((e) => e.type === "segment");
	assert.ok(segments.length >= 1);
	assert.ok(segments.every((e) => e.text === "deepgram says hi"));
	assert.equal(text, segments.map(() => "deepgram says hi").join(" "));
	assert.equal(server.requests.length, segments.length);
	const [request] = server.requests;
	assert.equal(request.method, "POST");
	assert.ok(request.url.startsWith("/v1/listen?") && request.url.includes("model=nova-3") && request.url.includes("language=en"), request.url);
	assert.equal(request.headers.authorization, "Token key");
	assert.equal(request.headers["content-type"], "audio/wav");
});

test("DeepgramFinal with a refused key gives part_failed like any failed final pass", async (t) => {
	const world = fakeWorld(t);
	const server = await fakeDeepgram(t, () => ({ status: 401, body: {} }));
	const { speech, seen } = await connect(t, world, { finalPass: new DeepgramFinal("bad", { baseUrl: server.baseUrl }) });
	const id = await speech.startSession(session({ finalModel: "deepgram-nova-3" }));
	feed(speech, id, voice(1000));
	assert.equal(await speech.stopSession(id), "");
	assert.ok(server.requests.length >= 1);
	await until(() => seen.some((e) => e.type === "part_failed"), 5000, "the part_failed event");
	assert.deepEqual(seen.map((e) => e.type), ["part_failed"]);
});

test("DeepgramFinal without a baseUrl connects, and leaves other models to the engine", async (t) => {
	const world = fakeWorld(t);
	const { speech } = await connect(t, world, { finalPass: new DeepgramFinal("key") });
	const id = await speech.startSession(session({ finalModel: FINAL }));
	feed(speech, id, voice(1000));
	assert.equal(await speech.stopSession(id), FAKE_TEXT);
});

test("a microphone in JavaScript feeds a session with the source mic", async (t) => {
	const world = fakeWorld(t);
	let opened = 0;
	let closed = false;
	let info;
	const microphone = {
		open(writer, openInfo) {
			opened += 1;
			info = openInfo;
			writer.onClose(() => {
				closed = true;
			});
			const audio = concat(silence(300), voice(1000), silence(600));
			let at = 0;
			const timer = setInterval(() => {
				if (writer.closed || at >= audio.length) return clearInterval(timer);
				writer.push(audio.subarray(at, at + 1600));
				at += 1600;
			}, 5);
			return { excludes: [] };
		},
	};
	const { speech, seen } = await connect(t, world, { microphone });
	const id = await speech.startSession(session({ sources: ["mic"] }));
	// The microphone gives audio in real time of the test, so wait until the speech is through.
	await new Promise((r) => setTimeout(r, 500));
	const text = await speech.stopSession(id);
	assert.equal(opened, 1);
	assert.deepEqual(info, {});
	assert.equal(text, FAKE_TEXT);
	const segments = seen.filter((e) => e.type === "segment");
	assert.ok(segments.length > 0 && segments.every((e) => e.source === "mic" && e.session === id));
	await until(() => closed, 5000, "the close of the writer");
});

test("a microphone that refuses gives the code permission", async (t) => {
	const world = fakeWorld(t);
	const microphone = {
		open() {
			throw new SaysoError({ code: "permission", message: "the user said no" });
		},
	};
	const { speech } = await connect(t, world, { microphone });
	await assert.rejects(speech.startSession(session({ sources: ["mic"] })), (e) => e instanceof SaysoError && e.code === "permission" && /said no/.test(e.message));
	// The refused start leaves no session behind.
	const id = await speech.startSession(session());
	await speech.stopSession(id);
});

test("close ends the session, the iterators, and later calls fail", async (t) => {
	const world = fakeWorld(t);
	const { speech } = await connect(t, world);
	const events = speech.events();
	const id = await speech.startSession(session());
	feed(speech, id, voice(500));
	const reading = (async () => {
		for await (const _ of events);
	})();
	await speech.close();
	await reading;
	await assert.rejects(speech.models(), (e) => e instanceof SaysoError && e.code === "connection_lost");
	await assert.rejects(speech.startSession(session()), (e) => e.code === "connection_lost");
	await speech.close();
});

test("the microphone settings fail when the app gave its own microphone", async (t) => {
	const world = fakeWorld(t);
	const microphone = { open: () => ({ excludes: [] }) };
	const { speech } = await connect(t, world, { microphone });
	assert.throws(() => speech.setDevice("x"), /microphone/);
	assert.throws(() => speech.closeEarly(), /microphone/);
});

test("the engine can be an Engine with modelsDir, and must have modelsDir", async (t) => {
	const { Engine } = await import("../dist/index.js");
	const world = fakeWorld(t);
	const tempDir = join(world.dir, "tmp-engine");
	const speech = await EmbeddedSpeech.connect({ app: { name: "Tests" }, engine: Engine.fromProgram(fakeEngine).store(world.store).modelsDir(world.models), tempDir });
	t.after(() => speech.close());
	assert.ok((await speech.models()).some((m) => m.id === FINAL));
	await assert.rejects(EmbeddedSpeech.connect({ app: { name: "Tests" }, engine: Engine.fromProgram(fakeEngine), tempDir }), (e) => e.code === "bad_request" && /modelsDir/.test(e.message));
});
