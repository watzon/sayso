import assert from "node:assert/strict";
import { join } from "node:path";
import { after, test } from "node:test";
import { Engine } from "../src/engine.js";
import type { SaysoError } from "../src/errors.js";
import { Speech } from "../src/speech.js";
import { passed, program, removeTemps, tempDir } from "./support.js";

after(removeTemps);

test("with sayso-audio on macOS, Local mode has no system audio", { skip: process.platform !== "darwin" && "sayso-audio captures the system audio on Windows and Linux" }, async () => {
	const before = process.env.SAYSO_AUDIO_HELPER;
	process.env.SAYSO_AUDIO_HELPER = program("sayso-audio");
	const s = await Speech.connect({
		mode: "local",
		app: { name: "Notes" },
		capabilities: ["models.read", "models.download", "session.live", "audio.mic", "audio.system"],
		engine: Engine.fromProgram(program("sayso-fake-engine")),
		modelsDir: join(tempDir("models"), "models"),
	});
	try {
		const f = s.features();
		assert.equal(f.systemAudio, false);
		assert.ok(!f.capabilities.includes("audio.system"), f.capabilities.join(","));
		assert.ok(f.capabilities.includes("audio.mic"), "the helper still captures the microphone");
		await s.downloadModel("fake-stream", "tests");
		await assert.rejects(s.startSession({ sources: ["system"], model: "fake-stream" }), (e: SaysoError) => e.code === "unsupported");
		passed("AU01");
	} finally {
		await s.close();
		if (before === undefined) delete process.env.SAYSO_AUDIO_HELPER;
		else process.env.SAYSO_AUDIO_HELPER = before;
	}
});
