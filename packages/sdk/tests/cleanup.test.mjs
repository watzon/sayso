// The local cleanup models: the catalog, a download that the app cancels, and errors of the runtime.

import { test } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { cleanup, RuntimeCleaner, SaysoError } from "../dist/index.js";
import { tempDir } from "./support.mjs";

test("the catalog has S1-mini by Superwhisper", () => {
	const s1 = cleanup.find("s1-mini");
	assert.equal(s1?.name, "S1-mini");
	assert.equal(s1?.publisher, "Superwhisper");
	assert.ok(s1.sizeBytes > 1e8);
	assert.deepEqual(s1.layouts, ["paragraphs", "list"]);
	assert.ok(cleanup.catalog().length >= 2);
});

test("a model that is not downloaded is not ready", (t) => {
	const dir = tempDir(t);
	assert.equal(cleanup.isReady(dir, "s1-mini"), false);
	assert.equal(cleanup.modelPath(dir, "s1-mini"), null);
	assert.throws(() => cleanup.isReady(dir, "no-such-model"), (e) => e instanceof SaysoError && e.code === "unknown_model");
});

test("a signal cancels a download with declined, and progress comes", async (t) => {
	const dir = tempDir(t);
	// A server that sends the license files at once and the model file slowly, so the
	// download is running at the abort.
	const server = createServer((req, res) => {
		if (!req.url.endsWith(".gguf")) {
			res.end("license text");
			return;
		}
		res.writeHead(200, { "content-length": String(484_219_808) });
		const timer = setInterval(() => res.write(Buffer.alloc(64 * 1024)), 20);
		res.on("close", () => clearInterval(timer));
	});
	await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
	t.after(() => server.close());
	const endpoint = `http://127.0.0.1:${server.address().port}`;
	const controller = new AbortController();
	const progress = [];
	const download = cleanup.download(dir, "s1-mini", {
		endpoint,
		signal: controller.signal,
		onProgress: (p) => {
			progress.push(p);
			if (progress.length === 2) controller.abort();
		},
	});
	await assert.rejects(download, (e) => e instanceof SaysoError && e.code === "declined");
	assert.ok(progress.length >= 2);
	assert.ok(progress[0].bytesTotal > 0);
	await cleanup.delete(dir, "s1-mini");
});

test("a cleanup without the model fails with unavailable", async (t) => {
	const dir = tempDir(t);
	// Not `using`: Node 22 cannot parse it in a .mjs file.
	const cleaner = new RuntimeCleaner({ runtime: "/no/such/runtime", modelsDir: dir });
	t.after(() => cleaner[Symbol.dispose]());
	await assert.rejects(cleaner.clean({ model: "s1-mini", text: "um so hello" }), (e) => e.code === "unavailable");
	await assert.rejects(cleaner.clean({ model: "s1-mini", text: "x", settings: { tone: "angry" } }), (e) => e.code === "bad_request");
	assert.equal(cleaner.isRunning(), false);
});
