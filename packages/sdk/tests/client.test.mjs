// The plain Client against a fake Sayso on a real socket.

import { test } from "node:test";
import assert from "node:assert/strict";
import { join } from "node:path";
import { Client, SaysoError } from "../dist/index.js";
import { fakeHost, tempDir, until } from "./support.mjs";

const APP = { name: "Tests", capabilities: ["transcribe.file"] };

/** Connect a Client to `host` and close it when the test ends. */
async function connect(t, host, extra = {}) {
	const client = await Client.connect({ path: host.path, app: APP, ...extra });
	t.after(() => client.close());
	return client;
}

/** The next event of `iterator` that has `kind`. */
async function nextOfKind(iterator, kind) {
	for (;;) {
		const { value, done } = await iterator.next();
		assert.ok(!done, `the events ended before a ${kind}`);
		if (value.kind === kind) return value;
	}
}

test("hello gives the reply of the host", async (t) => {
	const host = await fakeHost(t, tempDir(t));
	const client = await connect(t, host);
	const hello = client.hello();
	assert.equal(hello.host_version, "fake");
	assert.equal(hello.protocol, 1);
	// The fake host grants what the app asked for.
	assert.deepEqual(hello.capabilities, ["transcribe.file"]);
	assert.deepEqual(hello.denied, []);
	// The app told Sayso its name and what it wants.
	const sent = host.requests.find((r) => r.type === "hello");
	assert.equal(sent.app.name, "Tests");
	assert.deepEqual(sent.capabilities, ["transcribe.file"]);
});

test("request gives the fields of the ok reply", async (t) => {
	const models = [{ id: "m1", name: "M1" }];
	const host = await fakeHost(t, tempDir(t), (request) => (request.type === "list_models" ? { type: "ok", models } : { type: "ok" }));
	const client = await connect(t, host);
	const reply = await client.request("list_models", {});
	assert.deepEqual(reply.models, models);
	// The fields of the call go into the line.
	await client.request("cancel_job", { job: 4 });
	const line = host.requests.find((r) => r.type === "cancel_job");
	assert.equal(line.job, 4);
	assert.equal(typeof line.id, "string");
});

test("an error reply rejects with a SaysoError that has its code", async (t) => {
	const host = await fakeHost(t, tempDir(t), () => ({ type: "error", code: "capability", message: "not granted", capability: "transcribe.file", reason: "why" }));
	const client = await connect(t, host);
	await assert.rejects(client.request("transcribe_file", { path: "/a.wav" }), (e) => {
		assert.ok(e instanceof SaysoError);
		assert.equal(e.code, "capability");
		assert.equal(e.message, "not granted");
		assert.equal(e.capability, "transcribe.file");
		assert.equal(e.reason, "why");
		return true;
	});
});

test("send writes a line without an id and does not wait", async (t) => {
	const host = await fakeHost(t, tempDir(t));
	const client = await connect(t, host);
	client.send("session_audio", { session: 3, pcm: "AAAA" });
	await until(() => host.requests.some((r) => r.type === "session_audio"), 5000, "the audio line");
	const line = host.requests.find((r) => r.type === "session_audio");
	assert.equal(line.id, undefined);
	assert.equal(line.session, 3);
	assert.equal(line.pcm, "AAAA");
});

test("an error without an id comes as an error event", async (t) => {
	const host = await fakeHost(t, tempDir(t), (request, peer) => {
		if (request.type === "session_audio") peer.send({ type: "error", code: "bad_request", message: "no such session" });
		return undefined;
	});
	const client = await connect(t, host);
	const events = client.events();
	client.send("session_audio", { session: 9, pcm: "AAAA" });
	const event = await nextOfKind(events, "error");
	assert.equal(event.error.code, "bad_request");
	assert.equal(event.error.message, "no such session");
	await events.return();
});

test("an event of the host with an id comes as a host event", async (t) => {
	const host = await fakeHost(t, tempDir(t), (request, peer) => {
		if (request.type === "transcribe_file") {
			peer.send({ type: "job_progress", id: request.id, job: 5, fraction: 0.25 });
			return { type: "ok", job: 5 };
		}
		return { type: "ok" };
	});
	const client = await connect(t, host);
	const events = client.events();
	const reply = await client.request("transcribe_file", { path: "/a.wav" });
	assert.equal(reply.job, 5);
	const event = await nextOfKind(events, "host");
	const sent = host.requests.find((r) => r.type === "transcribe_file");
	assert.deepEqual(event, { kind: "host", id: sent.id, event: { type: "job_progress", job: 5, fraction: 0.25 } });
	await events.return();
});

test("an event of the host without an id has a null id", async (t) => {
	const host = await fakeHost(t, tempDir(t));
	const client = await connect(t, host);
	const events = client.events();
	for (const peer of host.peers) peer.send({ type: "job_done", job: 8, text: "late" });
	const event = await nextOfKind(events, "host");
	assert.equal(event.id, null);
	assert.deepEqual(event.event, { type: "job_done", job: 8, text: "late" });
	await events.return();
});

test("two listeners get the same events", async (t) => {
	const host = await fakeHost(t, tempDir(t));
	const client = await connect(t, host);
	const a = [];
	const off = client.on((event) => a.push(event));
	const events = client.events();
	for (const peer of host.peers) peer.send({ type: "job_progress", job: 1, fraction: 0.5 });
	const b = await nextOfKind(events, "host");
	await until(() => a.length === 1, 5000, "the listener");
	assert.deepEqual(a[0], b);
	off();
	await events.return();
});

test("the close of the host socket gives host_gone, and requests fail until reconnect", async (t) => {
	const host = await fakeHost(t, tempDir(t));
	const client = await connect(t, host);
	const events = client.events();
	for (const peer of host.peers) peer.socket.destroy();
	const gone = await nextOfKind(events, "host_gone");
	assert.deepEqual(gone, { kind: "host_gone" });
	await assert.rejects(client.request("list_models", {}), (e) => e instanceof SaysoError && ["connection_lost", "host_not_running"].includes(e.code), "a request on a lost connection");
	// The host is still listening, so the client can connect again.
	const hello = await client.reconnect();
	assert.equal(hello.host_version, "fake");
	assert.deepEqual((await client.request("list_models", {})), {});
	await events.return();
});

test("close sends bye and ends the iterators", async (t) => {
	const host = await fakeHost(t, tempDir(t));
	const client = await Client.connect({ path: host.path, app: APP });
	const events = client.events();
	const reading = (async () => {
		for await (const _ of events);
	})();
	await client.close();
	await reading;
	await until(() => host.requests.some((r) => r.type === "bye"), 5000, "the bye line");
	// A second close is fine.
	await client.close();
});

test("a socket that does not exist fails with host_not_running", async (t) => {
	const dir = tempDir(t);
	await assert.rejects(Client.connect({ path: join(dir, "no-such.sock") }), (e) => e instanceof SaysoError && e.code === "host_not_running");
	await assert.rejects(Client.connect({ path: "/no/such/socket" }), (e) => e instanceof SaysoError && e.code === "host_not_running");
});

test("start: true calls the JavaScript startSayso and connects when the host appears", async (t) => {
	const dir = tempDir(t);
	const path = join(dir, "host.sock");
	let calls = 0;
	let host;
	const client = await Client.connect({
		path,
		app: APP,
		start: true,
		waitMs: 2000,
		startSayso: async () => {
			calls += 1;
			host = await fakeHost(t, dir);
		},
	});
	t.after(() => client.close());
	assert.equal(calls, 1);
	assert.equal(client.hello().host_version, "fake");
	assert.equal(host.path, path);
});

test("start: true does not call startSayso when the host runs already", async (t) => {
	const host = await fakeHost(t, tempDir(t));
	let calls = 0;
	const client = await connect(t, host, {
		start: true,
		waitMs: 500,
		startSayso: () => {
			calls += 1;
		},
	});
	assert.equal(calls, 0);
	assert.equal(client.hello().protocol, 1);
});

test("start: true fails with host_not_running when the host never comes", async (t) => {
	const dir = tempDir(t);
	let calls = 0;
	await assert.rejects(
		Client.connect({
			path: join(dir, "host.sock"),
			start: true,
			waitMs: 300,
			startSayso: () => {
				calls += 1;
			},
		}),
		(e) => e instanceof SaysoError && e.code === "host_not_running",
	);
	assert.equal(calls, 1);
});

test("a startSayso that throws fails the connect", async (t) => {
	const dir = tempDir(t);
	await assert.rejects(
		Client.connect({
			path: join(dir, "host.sock"),
			start: true,
			waitMs: 300,
			startSayso: () => {
				throw new Error("cannot launch");
			},
		}),
		(e) => e instanceof SaysoError && /cannot launch/.test(e.message),
	);
});
