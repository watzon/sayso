// Deepgram against a fake Deepgram HTTP server.

import { test } from "node:test";
import assert from "node:assert/strict";
import { Deepgram, SaysoError } from "../dist/index.js";
import { fakeDeepgram, deepgramReply } from "./support.mjs";

test("the constants match the Rust crate", () => {
	assert.equal(Deepgram.MODEL, "deepgram-nova-3");
	assert.equal(Deepgram.CODE_KEY, "cloud_key");
	assert.equal(Deepgram.CODE_CONNECTION, "cloud_connection");
});

test("checkKey resolves for 200 and sends the Token header", async (t) => {
	const server = await fakeDeepgram(t, () => ({ status: 200, body: { projects: [] } }));
	await new Deepgram("secret-key", { baseUrl: server.baseUrl }).checkKey();
	assert.equal(server.requests.length, 1);
	assert.equal(server.requests[0].method, "GET");
	assert.equal(server.requests[0].url, "/v1/projects");
	assert.equal(server.requests[0].headers.authorization, "Token secret-key");
});

test("checkKey rejects with cloud_key for 401 and for 403", async (t) => {
	for (const status of [401, 403]) {
		const server = await fakeDeepgram(t, () => ({ status, body: { err_code: "INVALID_AUTH" } }));
		await assert.rejects(new Deepgram("bad", { baseUrl: server.baseUrl }).checkKey(), (e) => e instanceof SaysoError && e.code === "cloud_key");
	}
});

test("checkKey rejects with cloud_connection for a server error", async (t) => {
	const server = await fakeDeepgram(t, () => ({ status: 503, body: {} }));
	await assert.rejects(new Deepgram("k", { baseUrl: server.baseUrl }).checkKey(), (e) => e instanceof SaysoError && e.code === "cloud_connection");
});

test("transcribe gives the trimmed text and sends a WAV with the language", async (t) => {
	const server = await fakeDeepgram(t, () => ({ status: 200, body: deepgramReply("  Then push the fix. ") }));
	const dg = new Deepgram("secret-key", { baseUrl: server.baseUrl });
	const text = await dg.transcribe(new Float32Array(1600), "en");
	assert.equal(text, "Then push the fix.");
	const [seen] = server.requests;
	assert.equal(seen.method, "POST");
	assert.ok(seen.url.startsWith("/v1/listen?"), seen.url);
	assert.ok(seen.url.includes("model=nova-3") && seen.url.includes("language=en"), seen.url);
	assert.equal(seen.headers.authorization, "Token secret-key");
	assert.equal(seen.headers["content-type"], "audio/wav");
	// 1600 samples of 16 bits and a header of 44 bytes.
	assert.equal(seen.bytes, 44 + 3200);
	assert.equal(seen.body.subarray(0, 4).toString(), "RIFF");
	await dg.transcribe(new Float32Array(160), "auto");
	assert.ok(server.requests[1].url.includes("detect_language=true"), server.requests[1].url);
});

test("transcribe rejects with the right code for each failure", async (t) => {
	const replies = [
		[401, "{}", "cloud_key"],
		[429, "{}", "cloud_connection"],
		[500, "{}", "cloud_connection"],
		[400, "bad", undefined],
		[200, "{}", undefined],
	];
	let at = 0;
	const server = await fakeDeepgram(t, () => {
		const [status, body] = replies[at++];
		return { status, body };
	});
	const dg = new Deepgram("k", { baseUrl: server.baseUrl });
	for (const [, , code] of replies) {
		await assert.rejects(dg.transcribe(new Float32Array(160), "en"), (e) => e instanceof SaysoError && e.code === code, `code ${code}`);
	}
});

test("a server that is not there gives cloud_connection", async () => {
	const dg = new Deepgram("k", { baseUrl: "http://127.0.0.1:9/v1" });
	await assert.rejects(dg.transcribe(new Float32Array(160), "en"), (e) => e instanceof SaysoError && e.code === "cloud_connection");
});
