// Speech in Sayso mode and in auto mode, against a fake Sayso on a real socket.
// The scenarios follow crates/sayso-sdk/tests/one_end.rs.

import { test } from "node:test";
import assert from "node:assert/strict";
import { writeFileSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { join } from "node:path";
import { Speech, Engine, SaysoError } from "../dist/index.js";
import { fakeEngine, fakeHost, fakeWorld, FINAL, socketPath, tempDir, tone, until } from "./support.mjs";

const TIMING = { watchMs: 20, helloLimitMs: 2000, retryMs: 200 };
const CAPABILITIES = ["transcribe.file", "session.live"];

/** Connect in `mode`, record every event in `seen`, and close at the end of the test. */
async function connect(t, mode, socket, extra = {}) {
	const speech = await Speech.connect({ mode, app: { name: "Tests" }, capabilities: CAPABILITIES, socket, timing: TIMING, ...extra });
	const seen = [];
	speech.on((event) => seen.push(event));
	t.after(() => speech.close());
	return { speech, seen };
}

/** The engine of the fake world, for local mode and auto mode. */
function localEngine(world) {
	return Engine.fromProgram(fakeEngine).store(world.store).modelsDir(world.models);
}

const messagesOf = (seen, job) => seen.filter((e) => e.kind === "message" && e.message.job === job).map((e) => e.message);
const isMessage = (type) => (e) => e.kind === "message" && e.message.type === type;
const isKind = (kind) => (e) => e.kind === kind;

/** A host that replies to the jobs and sessions with fixed host ids. */
function jobHost(extra = () => undefined) {
	return (request, peer) => {
		const own = extra(request, peer);
		if (own) return own;
		switch (request.type) {
			case "transcribe_file":
				return { type: "ok", job: 77 };
			case "session_start":
				return { type: "ok", session: 50 };
			case "session_stop":
				return { type: "ok", text: "words" };
			case "list_models":
				return { type: "ok", models: [{ id: "host-model", name: "Host model", roles: ["final"], state: "downloaded" }] };
			default:
				return { type: "ok" };
		}
	};
}

test("sayso mode without Sayso gives host_not_running", async (t) => {
	const dir = tempDir(t);
	// A socket path that nobody listens on.
	await assert.rejects(Speech.connect({ mode: "sayso", app: { name: "Tests" }, socket: socketPath(dir, "none.sock"), timing: TIMING }), (e) => e instanceof SaysoError && e.code === "host_not_running");
	// No socket option: the SDK takes SAYSO_HOST_SOCKET. Point it at nothing, so a Sayso that runs on this computer is not touched.
	const before = process.env.SAYSO_HOST_SOCKET;
	process.env.SAYSO_HOST_SOCKET = socketPath(dir, "env-none.sock");
	t.after(() => {
		if (before === undefined) delete process.env.SAYSO_HOST_SOCKET;
		else process.env.SAYSO_HOST_SOCKET = before;
	});
	await assert.rejects(Speech.connect({ mode: "sayso", app: { name: "Tests" }, timing: TIMING }), (e) => e instanceof SaysoError && e.code === "host_not_running");
	await assert.rejects(Speech.connect({ mode: { sayso: { start: false } }, app: { name: "Tests" }, timing: TIMING }), (e) => e.code === "host_not_running");
});

test("sayso mode connects and shows the features from the hello reply", async (t) => {
	const host = await fakeHost(t, tempDir(t), jobHost(), {
		hello: () => ({ capabilities: ["transcribe.file", "audio.system"], denied: ["session.live"], features: ["diarization"] }),
	});
	const { speech } = await connect(t, "sayso", host.path);
	assert.equal(speech.mode(), "sayso");
	const features = speech.features();
	assert.deepEqual(features.capabilities, ["transcribe.file", "audio.system"]);
	assert.deepEqual(features.denied, ["session.live"]);
	assert.deepEqual(features.host, ["diarization"]);
	assert.equal(features.enhance, true);
	assert.equal(features.subscribe, true);
	assert.equal(features.downloadQuestion, true);
	assert.equal(features.systemAudio, true);
	assert.equal(features.engine, null);
	// The hello line has the app and its capabilities.
	const hello = host.requests.find((r) => r.type === "hello");
	assert.equal(hello.app.name, "Tests");
	assert.deepEqual(hello.capabilities, CAPABILITIES);
});

test("listModels asks the host, and the app gets the models of the host", async (t) => {
	const host = await fakeHost(t, tempDir(t), jobHost());
	const { speech } = await connect(t, "sayso", host.path);
	const models = await speech.listModels();
	assert.deepEqual(models.map((m) => m.id), ["host-model"]);
	await speech.listModels("final");
	const lines = host.requests.filter((r) => r.type === "list_models");
	assert.equal(lines.length, 2);
	assert.equal(lines[1].role, "final");
});

test("transcribeFile sends the request, gives its own job id, and the job ends with job_done", async (t) => {
	const host = await fakeHost(t, tempDir(t), jobHost());
	const { speech, seen } = await connect(t, "sayso", host.path);
	const job = await speech.transcribeFile({ path: "/tmp/talk.wav", model: "m1", diarize: true });
	const line = host.requests.find((r) => r.type === "transcribe_file");
	assert.equal(line.path, "/tmp/talk.wav");
	assert.equal(line.model, "m1");
	assert.equal(line.diarize, true);
	// The host calls the job 77. The app gets its own number, so ids of two hosts never meet.
	assert.equal(typeof job, "number");
	assert.notEqual(job, 77);
	const [peer] = host.peers;
	// Events of other jobs of the host are not for this app. A job_done comes once.
	peer.send({ type: "job_progress", job: 99, fraction: 0.1 });
	peer.send({ type: "job_progress", job: 77, fraction: 0.5 });
	peer.send({ type: "segment", job: 77, start_ms: 0, end_ms: 900, text: "Hello." });
	peer.send({ type: "job_done", job: 77, text: "Hello." });
	peer.send({ type: "job_done", job: 77, text: "twice" });
	await until(() => seen.some(isMessage("job_done")), 5000, "job_done");
	// A late second end must not reach the app. Give it a moment to show.
	await new Promise((r) => setTimeout(r, 100));
	assert.deepEqual(messagesOf(seen, job).map((m) => m.type), ["job_progress", "segment", "job_done"]);
	assert.equal(messagesOf(seen, job).at(-1).text, "Hello.");
	assert.ok(!seen.some((e) => e.kind === "message" && e.message.job === 99 || e.kind === "message" && e.message.job === 77), "no event has the id of the host");
	// A second job gets a new number.
	const second = await speech.transcribeFile({ path: "/tmp/talk.wav" });
	assert.ok(second > job);
});

test("transcribe waits for the job and gives the text and segments", async (t) => {
	const host = await fakeHost(t, tempDir(t), jobHost((request, peer) => {
		if (request.type !== "transcribe_file") return undefined;
		setTimeout(() => {
			peer.send({ type: "segment", job: 77, start_ms: 0, end_ms: 500, text: "One." });
			peer.send({ type: "segment", job: 77, start_ms: 500, end_ms: 900, text: "Two." });
			peer.send({ type: "job_done", job: 77, text: "One. Two." });
		}, 20);
		return { type: "ok", job: 77 };
	}));
	const { speech } = await connect(t, "sayso", host.path);
	const transcript = await speech.transcribe({ path: "/tmp/talk.wav" });
	assert.equal(transcript.text, "One. Two.");
	assert.deepEqual(transcript.segments.map((s) => s.text), ["One.", "Two."]);
	assert.notEqual(transcript.job, 77);
});

test("a job_failed from the host rejects transcribe with its code", async (t) => {
	const host = await fakeHost(t, tempDir(t), jobHost((request, peer) => {
		if (request.type !== "transcribe_file") return undefined;
		setTimeout(() => peer.send({ type: "job_failed", job: 77, message: "the model broke", code: "engine_failed" }), 20);
		return { type: "ok", job: 77 };
	}));
	const { speech } = await connect(t, "sayso", host.path);
	await assert.rejects(speech.transcribe({ path: "/tmp/talk.wav" }), (e) => e instanceof SaysoError && e.code === "engine_failed" && e.message === "the model broke");
});

test("cancelJob and the session calls use the ids of the host on the wire", async (t) => {
	const host = await fakeHost(t, tempDir(t), jobHost());
	const { speech } = await connect(t, "sayso", host.path);
	const job = await speech.transcribeFile({ path: "/tmp/talk.wav" });
	await speech.cancelJob(job);
	assert.equal(host.requests.find((r) => r.type === "cancel_job").job, 77);
	await assert.rejects(speech.cancelJob(job + 100), (e) => e.code === "bad_request");
	const { session } = await speech.startSession({ sources: ["client"] });
	assert.notEqual(session, 50);
	speech.sessionAudio(session, tone(100));
	await until(() => host.requests.some((r) => r.type === "session_audio"), 5000, "the audio line");
	const audio = host.requests.find((r) => r.type === "session_audio");
	assert.equal(audio.session, 50);
	assert.equal(audio.id, undefined);
	assert.equal(typeof audio.pcm, "string");
	assert.equal(await speech.stopSession(session), "words");
	assert.equal(host.requests.find((r) => r.type === "session_stop").session, 50);
	// The session ended, so its id is not valid any more.
	await assert.rejects(speech.stopSession(session), (e) => e.code === "bad_request");
});

test("an error reply becomes a SaysoError with the same code, message, and capability", async (t) => {
	const host = await fakeHost(t, tempDir(t), (request) =>
		request.type === "list_models" ? { type: "error", code: "capability", message: "models.read is not granted", capability: "models.read", reason: "denied" } : { type: "error", message: "no code here" },
	);
	const { speech } = await connect(t, "sayso", host.path);
	await assert.rejects(speech.listModels(), (e) => {
		assert.ok(e instanceof SaysoError);
		assert.equal(e.code, "capability");
		assert.equal(e.message, "models.read is not granted");
		assert.equal(e.capability, "models.read");
		assert.equal(e.reason, "denied");
		return true;
	});
	await assert.rejects(speech.transcribeFile({ path: "/tmp/talk.wav" }), (e) => e instanceof SaysoError && e.code === undefined && e.message === "no code here");
});

/** A server on `path` that answers every hello with `not_approved`. It counts the hellos. */
async function refusingServer(t, path) {
	const state = { hellos: 0 };
	const sockets = new Set();
	const server = createServer((socket) => {
		sockets.add(socket);
		socket.on("close", () => sockets.delete(socket));
		socket.on("data", (chunk) => {
			for (const text of String(chunk).split("\n").filter(Boolean)) {
				const line = JSON.parse(text);
				if (line.type === "hello") state.hellos += 1;
				socket.write(`${JSON.stringify({ v: 1, type: "error", id: line.id, code: "not_approved", message: "no" })}\n`);
			}
		});
		socket.on("error", () => {});
	});
	await new Promise((resolve) => server.listen(path, resolve));
	t.after(() => new Promise((resolve) => {
		for (const socket of sockets) socket.destroy();
		server.close(() => resolve());
	}));
	return state;
}

test("a refused app gets not_approved in sayso mode", async (t) => {
	const path = socketPath(tempDir(t), "refuse.sock");
	await refusingServer(t, path);
	await assert.rejects(Speech.connect({ mode: "sayso", app: { name: "Tests" }, socket: path, timing: TIMING }), (e) => e instanceof SaysoError && e.code === "not_approved");
});

test("a host that says nothing to hello counts as not running", async (t) => {
	const path = socketPath(tempDir(t), "silent.sock");
	const sockets = new Set();
	const server = createServer((socket) => {
		sockets.add(socket);
		socket.on("data", () => {});
		socket.on("error", () => {});
	});
	await new Promise((resolve) => server.listen(path, resolve));
	t.after(() => new Promise((resolve) => {
		for (const socket of sockets) socket.destroy();
		server.close(() => resolve());
	}));
	await assert.rejects(Speech.connect({ mode: "sayso", app: { name: "Tests" }, socket: path, timing: { ...TIMING, helloLimitMs: 150 } }), (e) => e.code === "host_not_running");
});

test("the host closes the socket during a job: job_failed, then disconnected, then requests fail until reconnect", async (t) => {
	const host = await fakeHost(t, tempDir(t), jobHost());
	const { speech, seen } = await connect(t, "sayso", host.path);
	const job = await speech.transcribeFile({ path: "/tmp/talk.wav" });
	for (const peer of host.peers) peer.socket.destroy();
	await until(() => seen.some(isKind("disconnected")), 5000, "disconnected");
	const kinds = seen.map((e) => (e.kind === "message" ? e.message.type : e.kind));
	assert.deepEqual(kinds, ["job_failed", "disconnected"]);
	const failed = messagesOf(seen, job)[0];
	assert.equal(failed.code, "connection_lost");
	assert.equal(seen.at(-1).code, "connection_lost");
	// Sayso mode does not change mode. It fails until reconnect.
	assert.equal(speech.mode(), "sayso");
	await assert.rejects(speech.listModels(), (e) => e.code === "host_not_running");
	await assert.rejects(speech.transcribeFile({ path: "/tmp/talk.wav" }), (e) => e.code === "host_not_running");
	const sent = host.requests.filter((r) => r.type === "list_models").length;
	assert.equal(sent, 0, "no request goes out on a lost connection");
	// The host still listens, so reconnect works, and work starts again.
	await speech.reconnect();
	assert.equal((await speech.listModels())[0].id, "host-model");
	const again = await speech.transcribeFile({ path: "/tmp/talk.wav" });
	assert.ok(again > job, "an id does not come back");
});

test("host_closing ends the job with host_closing and then disconnected", async (t) => {
	const host = await fakeHost(t, tempDir(t), jobHost());
	const { speech, seen } = await connect(t, "sayso", host.path);
	const job = await speech.transcribeFile({ path: "/tmp/talk.wav" });
	for (const peer of host.peers) {
		peer.send({ type: "host_closing" });
		peer.socket.end();
	}
	await until(() => seen.some(isKind("disconnected")), 5000, "disconnected");
	assert.equal(messagesOf(seen, job)[0].code, "host_closing");
	assert.equal(seen.at(-1).code, "host_closing");
});

test("a request that waits for a reply gets connection_lost when the host closes", async (t) => {
	const host = await fakeHost(t, tempDir(t), (request, peer) => {
		if (request.type === "list_models") {
			peer.socket.destroy();
			return undefined;
		}
		return { type: "ok" };
	});
	const { speech, seen } = await connect(t, "sayso", host.path);
	await assert.rejects(speech.listModels(), (e) => e.code === "connection_lost");
	await until(() => seen.some(isKind("disconnected")), 5000, "disconnected");
});

test("enhance and subscribe go to the host in sayso mode", async (t) => {
	const host = await fakeHost(t, tempDir(t), (request) => {
		if (request.type === "enhance") return { type: "ok", text: "Clean text.", provider: "fake", model: "fake-model" };
		if (request.type === "subscribe") return { type: "ok", events: ["dictation_done"] };
		return { type: "ok" };
	});
	const { speech } = await connect(t, "sayso", host.path);
	const reply = await speech.enhance({ text: "clean text", style: "plain" });
	assert.equal(reply.text, "Clean text.");
	assert.deepEqual(await speech.subscribe(["dictation_done"]), ["dictation_done"]);
	assert.deepEqual(host.requests.find((r) => r.type === "subscribe").events, ["dictation_done"]);
});

test("close in sayso mode sends bye and later calls fail", async (t) => {
	const host = await fakeHost(t, tempDir(t), jobHost());
	const { speech } = await connect(t, "sayso", host.path);
	await speech.close();
	await assert.rejects(speech.listModels(), (e) => e.code === "connection_lost");
	await speech.close();
});

test("enhance and subscribe fail with unsupported in local mode", async (t) => {
	const world = fakeWorld(t);
	const { speech } = await connect(t, "local", undefined, { engine: localEngine(world), models: { finalModel: FINAL, language: "en" } });
	await assert.rejects(speech.enhance({ text: "x" }), (e) => e instanceof SaysoError && e.code === "unsupported");
	await assert.rejects(speech.subscribe(["dictation_done"]), (e) => e instanceof SaysoError && e.code === "unsupported");
	assert.equal(speech.features().enhance, false);
	assert.equal(speech.features().subscribe, false);
});

test("enhance and subscribe fail with unsupported in auto mode without Sayso", async (t) => {
	const world = fakeWorld(t);
	const { speech } = await connect(t, "auto", socketPath(world.dir, "host.sock"), { engine: localEngine(world) });
	assert.equal(speech.mode(), "local");
	await assert.rejects(speech.enhance({ text: "x" }), (e) => e.code === "unsupported");
	await assert.rejects(speech.subscribe(["dictation_done"]), (e) => e.code === "unsupported");
});

test("auto mode uses Sayso when it runs at connect", async (t) => {
	const world = fakeWorld(t);
	const host = await fakeHost(t, world.dir, jobHost());
	const { speech, seen } = await connect(t, "auto", host.path, { engine: localEngine(world) });
	assert.equal(speech.mode(), "sayso");
	assert.equal((await speech.listModels())[0].id, "host-model");
	assert.deepEqual(seen, [], "the first mode is no change");
});

test("auto mode starts in local mode and changes to Sayso when a host appears", async (t) => {
	const world = fakeWorld(t);
	const socket = socketPath(world.dir, "host.sock");
	const { speech, seen } = await connect(t, "auto", socket, { engine: localEngine(world), models: { finalModel: FINAL, language: "en" } });
	assert.equal(speech.mode(), "local");
	assert.ok(speech.features().capabilities.includes("transcribe.file"));
	// Local mode works with the fake engine.
	assert.ok((await speech.listModels()).some((m) => m.id === FINAL));
	const host = await fakeHost(t, world.dir, jobHost(), { hello: () => ({ capabilities: ["models.read"] }) });
	assert.equal(host.path, socket);
	await until(() => seen.some(isKind("mode_changed")), 10000, "mode_changed");
	const change = seen.find(isKind("mode_changed"));
	assert.equal(change.mode, "sayso");
	assert.deepEqual(change.features.capabilities, ["models.read"]);
	assert.equal(speech.mode(), "sayso");
	assert.equal((await speech.listModels())[0].id, "host-model", "the request goes to Sayso now");
});

test("auto mode changes back to local mode when the host goes away", async (t) => {
	const world = fakeWorld(t);
	const host = await fakeHost(t, world.dir, jobHost());
	const { speech, seen } = await connect(t, "auto", host.path, { engine: localEngine(world), models: { finalModel: FINAL, language: "en" } });
	assert.equal(speech.mode(), "sayso");
	const job = await speech.transcribeFile({ path: world.wav });
	// Close the server too, so the SDK cannot connect again at once.
	await host.close();
	await until(() => seen.some(isKind("mode_changed")), 5000, "mode_changed");
	const kinds = seen.map((e) => (e.kind === "message" ? e.message.type : e.kind));
	assert.deepEqual(kinds, ["job_failed", "mode_changed"], "the end of the work comes first");
	assert.equal(messagesOf(seen, job)[0].code, "connection_lost");
	assert.equal(seen[1].mode, "local");
	assert.equal(speech.mode(), "local");
	assert.ok((await speech.listModels()).some((m) => m.id === FINAL), "the next request runs in local mode");
});

test("auto mode never changes to Sayso during a job", async (t) => {
	const world = fakeWorld(t);
	const hold = join(world.models, "hold-transcribe");
	writeFileSync(hold, "");
	const socket = socketPath(world.dir, "host.sock");
	const { speech, seen } = await connect(t, "auto", socket, { engine: localEngine(world), models: { finalModel: FINAL, language: "en" } });
	assert.equal(speech.mode(), "local");
	const job = await speech.transcribeFile({ path: world.wav, model: FINAL });
	const host = await fakeHost(t, world.dir, jobHost());
	// The SDK finds the host and sends hello, but the job holds the change.
	await until(() => host.requests.some((r) => r.type === "hello"), 5000, "the hello of the SDK");
	await new Promise((r) => setTimeout(r, 400));
	assert.equal(speech.mode(), "local", "still local during the job");
	assert.ok(!seen.some(isKind("mode_changed")), JSON.stringify(seen));
	// Let the job end. The change comes after its end.
	rmSync(hold);
	await until(() => seen.some(isKind("mode_changed")), 10000, "mode_changed");
	const kinds = seen.map((e) => (e.kind === "message" ? `${e.message.type}` : e.kind));
	const end = kinds.indexOf("job_done");
	assert.ok(end >= 0, `the job ended: ${kinds}`);
	assert.ok(end < kinds.indexOf("mode_changed"), `the job ends first: ${kinds}`);
	assert.equal(messagesOf(seen, job).at(-1).type, "job_done");
	assert.equal(speech.mode(), "sayso");
});

test("auto mode never changes during a live session", async (t) => {
	const world = fakeWorld(t);
	const socket = socketPath(world.dir, "host.sock");
	const { speech, seen } = await connect(t, "auto", socket, { engine: localEngine(world), models: { finalModel: FINAL, language: "en" } });
	const { session } = await speech.startSession({ sources: ["client"], model: FINAL, vad_model: "basic", language: "en" });
	const host = await fakeHost(t, world.dir, jobHost());
	await until(() => host.requests.some((r) => r.type === "hello"), 5000, "the hello of the SDK");
	await new Promise((r) => setTimeout(r, 400));
	assert.equal(speech.mode(), "local");
	assert.ok(!seen.some(isKind("mode_changed")));
	await speech.stopSession(session);
	await until(() => seen.some(isKind("mode_changed")), 10000, "mode_changed");
	assert.equal(speech.mode(), "sayso");
});

test("auto mode with a host that refuses stays local and tries again after the retry time", async (t) => {
	const world = fakeWorld(t);
	const path = socketPath(world.dir, "host.sock");
	const state = await refusingServer(t, path);
	const { speech, seen } = await connect(t, "auto", path, { engine: localEngine(world) });
	assert.equal(speech.mode(), "local");
	// It tries again after the retry time (200 ms here).
	await until(() => state.hellos >= 2, 5000, "a second hello");
	assert.equal(speech.mode(), "local");
	assert.deepEqual(seen, [], "a failed hello changes nothing for the app");
});
