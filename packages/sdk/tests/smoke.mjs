// A short check of the SDK for runtimes without node:test (Bun, Deno): a file job and a
// live session with the fake engine, and a JavaScript microphone. Exit code 0 when it passes.
//   bun packages/sdk/tests/smoke.mjs
//   deno run --allow-all packages/sdk/tests/smoke.mjs

import { Speech, Engine, SaysoError } from "../dist/index.js";
import { fakeEngine, fakeWorld, FINAL, tone } from "./support.mjs";

const cleanups = [];
const t = { after: (fn) => cleanups.push(fn) };
const check = (ok, what) => {
	if (!ok) throw new Error(`failed: ${what}`);
	console.log(`ok: ${what}`);
};

try {
	const world = fakeWorld(t);
	const microphone = {
		open(writer) {
			const samples = Float32Array.from(tone(800), (s) => s / 32768);
			writer.push(samples);
			return {};
		},
	};
	await using speech = await Speech.connect({
		mode: "local",
		app: { name: "Smoke" },
		engine: Engine.fromProgram(fakeEngine).store(world.store).modelsDir(world.models),
		capture: { microphone },
	});
	check(speech.mode() === "local", "local mode");
	const models = await speech.listModels();
	check(models.some((m) => m.id === FINAL), "the models of the engine");
	const transcript = await speech.transcribe({ path: world.wav, model: FINAL });
	check(transcript.text.length > 0 && transcript.segments.length > 0, "a file job");
	const { session } = await speech.startSession({ sources: ["mic"], model: FINAL, vad_model: "basic" });
	check(typeof (await speech.stopSession(session)) === "string", "a live session with a JavaScript microphone");
	let refused = false;
	try {
		await speech.transcribeFile({ path: "relative.wav" });
	} catch (e) {
		refused = e instanceof SaysoError && e.code === "bad_request";
	}
	check(refused, "a SaysoError with a code");
	console.log(`passed in ${typeof Bun !== "undefined" ? "Bun" : typeof Deno !== "undefined" ? "Deno" : "Node"}`);
} finally {
	for (const fn of cleanups) fn();
}
