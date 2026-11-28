import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { BasicVad } from "../src/audio/vad.js";
import { StreamResampler } from "../src/audio/resample.js";
import { Audio, WavWriter, decodeWav, encodeWav, readLiveWav } from "../src/audio/wav.js";
import { cleanup, check, render, restoreSpelling } from "../src/cleanup.js";
import { EngineStartError, SaysoError } from "../src/errors.js";
import { ECHO_LIMIT, EchoGate, echoWords, wordsEcho } from "../src/local/echo.js";
import { SilenceWatch } from "../src/local/silence.js";
import { cleanupModelsDirOf, clientSocketPathFor, dataDirFor, hostSocketPathFor, modelStoreOf, modelsDirOf, parseWhoamiSid, type System } from "../src/paths.js";
import { decodeI16, encodeI16, decodeF32, encodeF32 } from "../src/protocol/codec.js";
import { CORPUS, PACKAGE_DIR, speech, tempDir, passed } from "./support.js";

test("WAV: write, decode, and the header", () => {
	const samples = Int16Array.from([0, 1, -1, 32767, -32768, 1234]);
	const bytes = encodeWav(samples);
	assert.equal(bytes.length, 44 + samples.length * 2);
	assert.equal(bytes.toString("latin1", 0, 4), "RIFF");
	assert.equal(bytes.toString("latin1", 8, 12), "WAVE");
	const audio = decodeWav(bytes, "t.wav");
	assert.equal(audio.samples.length, samples.length);
	samples.forEach((v, i) => assert.ok(Math.abs(audio.samples[i]! - v) <= 1, `sample ${i}`));
	assert.throws(() => decodeWav(Buffer.from("nope"), "bad.wav"), /damaged/);
});

test("WavWriter writes a file that reads back", () => {
	const path = join(tempDir("wavw"), "a.wav");
	const w = new WavWriter(path);
	w.write(Float32Array.from([100, 200, 300].map((v) => v / 32768)));
	w.write(Float32Array.from([400 / 32768]));
	assert.equal(w.finish(), 4);
	assert.deepEqual([...readLiveWav(path).map((v) => Math.round(v * 32768))], [100, 200, 300, 400]);
});

test("Audio gives floats of a time range", () => {
	const a = new Audio(speech(300));
	assert.equal(a.lenMs, Math.floor(a.samples.length / 16));
	assert.equal(a.floats(0, 100).length, 1600);
});

test("PCM codecs round trip", () => {
	const i = Int16Array.from([0, 5, -5, 32767, -32768]);
	assert.deepEqual([...decodeI16(encodeI16(i))], [...i]);
	const f = Float32Array.from([0, 0.5, -0.25]);
	assert.deepEqual([...decodeF32(encodeF32(f))], [...f]);
});

test("the resampler gives the right length and keeps a tone", () => {
	for (const [from, to] of [
		[48000, 16000],
		[44100, 16000],
		[8000, 16000],
		[16000, 16000],
	] as const) {
		const r = new StreamResampler(from, to);
		const input = new Float32Array(from);
		for (let i = 0; i < input.length; i++) input[i] = Math.sin((2 * Math.PI * 440 * i) / from);
		const out: number[] = [];
		for (let i = 0; i < input.length; i += 1000) out.push(...r.process(input.subarray(i, i + 1000)));
		assert.ok(Math.abs(out.length - to) <= 64, `${from}->${to}: ${out.length}`);
		const mid = out.slice(Math.floor(to / 4), Math.floor((to * 3) / 4));
		const peak = Math.max(...mid.map(Math.abs));
		assert.ok(peak > 0.8 && peak < 1.1, `${from}->${to}: peak ${peak}`);
	}
});

test("the basic VAD finds the parts of speech", () => {
	const vad = new BasicVad();
	const s = speech(900, 600);
	const f = new Float32Array(s.length);
	s.forEach((v, i) => (f[i] = v / 32768));
	const parts = [...vad.accept(f), ...vad.finish()];
	assert.equal(parts.length, 2, JSON.stringify(parts));
	assert.ok(Math.abs(parts[0]!.start_ms - 500) <= 100 && Math.abs(parts[0]!.end_ms - 1400) <= 150, JSON.stringify(parts));
});

function pairs(): { kind: string; mic: string; system: string }[] {
	return readFileSync(join(PACKAGE_DIR, "tests", "fixtures", "echo_pairs.tsv"), "utf8")
		.split("\n")
		.filter((l) => l.trim() !== "" && !l.startsWith("#"))
		.map((l) => {
			const [kind = "", mic = "", system = ""] = l.split("\t");
			return { kind, mic, system };
		});
}

test("the echo rule drops the echoes and keeps what the user said", () => {
	const all = pairs();
	assert.ok(all.length > 10);
	let echoes = 0;
	let dropped = 0;
	let others = 0;
	let wrong = 0;
	for (const p of all) {
		const drop = wordsEcho(echoWords(p.mic), echoWords(p.system), ECHO_LIMIT);
		if (p.kind === "echo") {
			echoes++;
			if (drop) dropped++;
		} else {
			others++;
			if (drop) wrong++;
		}
	}
	assert.ok(dropped / echoes >= 0.9, `dropped ${dropped} of ${echoes}`);
	assert.ok(wrong / others <= 0.02, `dropped ${wrong} of ${others} real lines`);
});

test("the echo gate holds a microphone segment until the system source has caught up", () => {
	const out: string[] = [];
	const gate = new EchoGate((s) => out.push(s.text));
	gate.micSegment({ start_ms: 0, end_ms: 2000, text: "we ship the release on friday" }, 0);
	gate.micSegment({ start_ms: 5000, end_ms: 6000, text: "my own words entirely" }, 0);
	assert.deepEqual(out, []);
	gate.systemSegment({ start_ms: 0, end_ms: 2000, text: "we ship the release on friday" }, 0);
	gate.systemDoneUntil(10_000);
	assert.deepEqual(out, ["my own words entirely"]);
	gate.systemEnded();
	assert.equal(gate.keptSegments().length, 1);
});

test("the silence watch fires once when every source passed the limit", () => {
	let fired = 0;
	const w = new SilenceWatch(1000, 2, () => {
		fired++;
		return true;
	});
	w.heard(0, 1500);
	assert.equal(fired, 0);
	w.heard(1, 900);
	assert.equal(fired, 0);
	w.heard(1, 1000);
	assert.equal(fired, 1);
	w.heard(0, 3000);
	assert.equal(fired, 1);
});

test("paths follow paths.json", () => {
	const table = JSON.parse(readFileSync(join(CORPUS, "paths.json"), "utf8")) as {
		socket: { case: string; system: System; env: Record<string, string>; uid?: number; darwin_temp?: string; sid?: string; host?: string; client?: string; error?: boolean }[];
		data: { case: string; goos: string; home: string; env: Record<string, string>; data: string; models: string; store: string; cleanup: string }[];
	};
	for (const c of table.socket) {
		const env = { var: (k: string) => c.env[k] ?? "", darwinUserTempDir: () => c.darwin_temp ?? "", uid: () => c.uid, userSid: () => c.sid ?? "" };
		if (c.error) {
			assert.throws(() => hostSocketPathFor(c.system, env), SaysoError, c.case);
			continue;
		}
		assert.equal(hostSocketPathFor(c.system, env), c.host, c.case);
		assert.equal(clientSocketPathFor(c.system, env), c.client, c.case);
	}
	for (const c of table.data) {
		const platform = c.goos === "windows" ? "win32" : c.goos;
		const dir = dataDirFor(platform, { var: (k) => c.env[k] ?? "", home: () => c.home });
		assert.equal(dir.replaceAll("\\", "/"), c.data, c.case);
		assert.equal(modelsDirOf(dir).replaceAll("\\", "/"), c.models, c.case);
		assert.equal(modelStoreOf(modelsDirOf(dir)).replaceAll("\\", "/"), c.store, c.case);
		assert.equal(cleanupModelsDirOf(modelsDirOf(dir)).replaceAll("\\", "/"), c.cleanup, c.case);
	}
	assert.equal(parseWhoamiSid('"PC\\\\user","S-1-5-21-1-2-3-1001"\r\n'), "S-1-5-21-1-2-3-1001");
	assert.equal(parseWhoamiSid("garbage"), "");
	passed("P02");
});

test("errors keep the code, capability, reason, and unknown codes", () => {
	const e = new SaysoError({ code: "capability", capability: "audio.mic", reason: "no", message: "m" });
	assert.deepEqual(e.toReply(), { code: "capability", capability: "audio.mic", reason: "no", message: "m" });
	const u = new SaysoError({ code: "quota_exceeded" as never, message: "later" });
	assert.equal(u.code, "quota_exceeded");
	assert.equal(SaysoError.from(new Error("x")).message, "x");
	assert.equal(SaysoError.from(new Error("x")).code, undefined);
	const s = new EngineStartError("/e", "exit 4", ["cannot load libwhisper"]);
	assert.match(s.message, /cannot load libwhisper/);
	assert.deepEqual(s.stderr, ["cannot load libwhisper"]);
});

test("the cleanup catalog renders the prompt of the model", () => {
	const catalog = cleanup.catalog();
	assert.deepEqual(catalog.map((m) => m.id).sort(), ["s1-mini", "spokenly-cleanup-1"]);
	const s1 = cleanup.find("s1-mini")!;
	const p = render(s1, "so um send the report by uh friday", {}, null, null, 10);
	assert.equal(
		p.text,
		"<|im_start|>system\nYou are a text normalizer for speech-to-text transcripts. The input begins with a control line specifying the styling, structure, and context settings; clean the transcript to match those settings and output only the cleaned text.<|im_end|>\n<|im_start|>user\n[Styling: semi-formal] [Structure: prose] [Context: general]\nso um send the report by uh friday<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n",
	);
	assert.deepEqual(p.end, ["<|im_end|>"]);
	assert.equal(p.maxTokens, 45);
	assert.deepEqual(check("hi", "end", " Hi.<|im_end|>"), { ok: true, text: "Hi." });
	assert.equal(check("hi", "limit", " H").ok, false);
	passed("C01");
});

test("restoreSpelling puts the spelling of the terms back", () => {
	assert.equal(restoreSpelling("buy an iPhone today", "Buy an iphone today.", ["iPhone"]), "Buy an iPhone today.");
	assert.equal(restoreSpelling("we may ship in May", "We may ship in May.", ["May"]), "We may ship in May.");
	assert.equal(restoreSpelling("no terms here", "No terms here.", []), "No terms here.");
});
