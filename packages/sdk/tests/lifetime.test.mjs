// The lifetime of a process with the SDK (docs/sdk-typescript.md, "Events", "Threads"):
// work holds the event loop of Node, no work does not, and calls that wait do not take
// the threads of libuv.

import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import { createServer } from "node:net";
import { join } from "node:path";
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { Speech } from "../dist/index.js";
import { fakeEngine, fakeWorld, SLOW, socketPath, tempDir } from "./support.mjs";

const index = new URL("../dist/index.js", import.meta.url).href;

/** Run a script in a new Node and give its output and the time it took. */
function run(t, dir, source, limitMs = 20000) {
	const file = join(dir, "script.mjs");
	writeFileSync(file, source);
	return new Promise((resolve, reject) => {
		const started = Date.now();
		const child = spawn(process.execPath, [file], { stdio: ["ignore", "pipe", "pipe"] });
		let out = "";
		child.stdout.on("data", (chunk) => (out += chunk));
		child.stderr.on("data", (chunk) => (out += chunk));
		const timer = setTimeout(() => {
			child.kill("SIGKILL");
			reject(new Error(`the process did not end in ${limitMs} ms; output: ${out}`));
		}, limitMs);
		child.on("exit", (code) => {
			clearTimeout(timer);
			resolve({ code, out, ms: Date.now() - started });
		});
	});
}

function connectSource(world) {
	return `
import { Speech, Engine } from ${JSON.stringify(index)};
const speech = await Speech.connect({
	mode: "local",
	app: { name: "Tests" },
	engine: Engine.fromProgram(${JSON.stringify(fakeEngine)}).store(${JSON.stringify(world.store)}).modelsDir(${JSON.stringify(world.models)}),
});
`;
}

test("a process with no work ends, also without close", async (t) => {
	const world = fakeWorld(t);
	const { code, out } = await run(t, world.dir, `${connectSource(world)}
await speech.listModels();
console.log("listed");
`);
	assert.equal(code, 0, out);
	assert.match(out, /listed/);
});

test("a process with a job ends after the last event of the job", async (t) => {
	const world = fakeWorld(t);
	const { code, out } = await run(t, world.dir, `${connectSource(world)}
speech.on((event) => {
	if (event.kind === "message" && event.message.type === "job_done") console.log("done", event.message.job);
});
const job = await speech.transcribeFile({ path: ${JSON.stringify(world.wav)}, model: ${JSON.stringify(SLOW)} });
console.log("started", job);
`);
	assert.equal(code, 0, out);
	assert.match(out, /started 1[\s\S]*done 1/);
});

// Node 22 cannot parse `await using` in JavaScript. TypeScript compiles it for Node 22.
let awaitUsing = true;
try {
	new Function("return async () => { await using x = null; }");
} catch {
	awaitUsing = false;
}

test("a process ends after close with await using", { skip: !awaitUsing && "this Node cannot parse await using" }, async (t) => {
	const world = fakeWorld(t);
	const { code, out } = await run(t, world.dir, `
import { Speech, Engine } from ${JSON.stringify(index)};
{
	await using speech = await Speech.connect({
		mode: "local",
		app: { name: "Tests" },
		engine: Engine.fromProgram(${JSON.stringify(fakeEngine)}).store(${JSON.stringify(world.store)}).modelsDir(${JSON.stringify(world.models)}),
	});
	const job = await speech.transcribeFile({ path: ${JSON.stringify(world.wav)}, model: ${JSON.stringify(SLOW)} });
	console.log("started", job);
}
console.log("closed");
`);
	assert.equal(code, 0, out);
	assert.match(out, /started 1\s+closed/);
});

test("calls that wait do not stop the file I/O of Node", async (t) => {
	const dir = tempDir(t);
	// A Sayso that takes connections and never answers hello.
	const path = socketPath(dir, "silent.sock");
	const sockets = [];
	const server = createServer((socket) => sockets.push(socket));
	await new Promise((resolve) => server.listen(path, resolve));
	t.after(() => {
		for (const socket of sockets) socket.destroy();
		server.close();
	});
	const waits = Array.from({ length: 6 }, () =>
		Speech.connect({ mode: "sayso", app: { name: "Tests" }, socket: path, timing: { helloLimitMs: 3000 } }).then(
			() => "connected",
			(e) => e.code,
		),
	);
	await new Promise((resolve) => setTimeout(resolve, 200));
	const started = Date.now();
	const self = fileURLToPath(import.meta.url);
	for (let i = 0; i < 8; i++) await readFile(self);
	const ms = Date.now() - started;
	assert.ok(ms < 1000, `8 reads took ${ms} ms while 6 calls waited`);
	assert.deepEqual(await Promise.all(waits), Array(6).fill("host_not_running"));
});
