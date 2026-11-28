import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { decodeHost, decodeRequest, encodeMessage } from "../src/protocol/codec.js";
import { names } from "../src/protocol/names.js";
import { CORPUS } from "./support.js";

interface Case {
	name: string;
	line: string;
}

function cases(file: string): Case[] {
	const out: Case[] = [];
	let name = "";
	for (const raw of readFileSync(join(CORPUS, file), "utf8").split("\n")) {
		const line = raw.trim();
		if (line === "") continue;
		if (line.startsWith("#")) name = line.slice(1).trim();
		else out.push({ name, line });
	}
	return out;
}

/** Drop the fields with a null value: a null counts as missing. */
function clean(value: unknown): unknown {
	if (Array.isArray(value)) return value.map(clean);
	if (value !== null && typeof value === "object") {
		const out: Record<string, unknown> = {};
		for (const [k, v] of Object.entries(value)) if (v !== null) out[k] = clean(v);
		return out;
	}
	return value;
}

function same(name: string, original: string, encoded: string): void {
	assert.deepEqual(clean(JSON.parse(encoded)), clean(JSON.parse(original)), name);
}

test("host requests decode and encode again", () => {
	for (const { name, line } of cases("host/requests.ndjson")) {
		const d = decodeRequest(line);
		assert.equal(d.kind, "request", name);
		if (d.kind !== "request") continue;
		same(name, line, encodeMessage(d.id, d.body));
	}
});

test("host replies and events decode and encode again", () => {
	for (const file of ["host/replies.ndjson", "host/events.ndjson"]) {
		for (const { name, line } of cases(file)) {
			const d = decodeHost(line);
			const decodeOnly = name.startsWith("decode:");
			let body: { type: string; [k: string]: unknown };
			switch (d.kind) {
				case "ok":
					body = { type: "ok", ...d.fields };
					break;
				case "error":
					body = { type: "error", ...d.error };
					break;
				case "event":
					body = d.event as unknown as typeof body;
					break;
				default:
					throw new Error(`${name}: unknown`);
			}
			if (!decodeOnly) same(name, line, encodeMessage(d.id, body));
		}
	}
});

test("edge cases: unknown names are kept, large numbers stay exact, numeric speakers become strings", () => {
	for (const { name, line } of cases("host/edge.ndjson")) {
		const raw = JSON.parse(line) as Record<string, unknown>;
		if (name.startsWith("a request type")) {
			const d = decodeRequest(line);
			assert.equal(d.kind, "unknown", name);
			continue;
		}
		if (raw.type === "hello") {
			const d = decodeRequest(line);
			assert.equal(d.kind, "request", name);
			if (d.kind === "request") same(name, line, encodeMessage(d.id, d.body));
			continue;
		}
		const d = decodeHost(line);
		if (name.startsWith("an event type")) {
			assert.equal(d.kind, "unknown", name);
			if (d.kind === "unknown") assert.equal(d.event.text, "Short.");
			continue;
		}
		if (name.startsWith("a numeric speaker") || name.includes("numeric speaker")) {
			assert.equal(d.kind, "event");
			if (d.kind === "event") assert.equal((d.event as unknown as { turns: { speaker: string }[] }).turns[0]?.speaker, "3");
			continue;
		}
		if (d.kind === "error") assert.equal(d.error.code, "quota_exceeded", name);
		if (d.kind === "event" && "job" in d.event && name.includes("2^53")) assert.equal(d.event.job, 9007199254740991);
		if (d.kind === "event" && "session" in d.event) assert.equal(d.event.session, 1099511627776);
		if (d.kind === "event" && "text" in d.event && name.includes("escapes")) assert.equal(d.event.text, '"quote" \\ tab\t new\n line é 日本 😀');
	}
});

test("a request that lacks a field is refused", () => {
	assert.throws(() => decodeRequest('{"type":"cancel_job","v":1,"id":"1"}'), /bad cancel_job/);
	assert.throws(() => decodeRequest("not json"));
	assert.throws(() => decodeHost("[1]"));
});

test("engine and cleanup corpus lines are JSON objects with a known type", () => {
	const all = names() as Record<string, Record<string, string[]>>;
	for (const [dir, files] of [
		["engine", ["requests", "replies", "events"]],
		["cleanup", ["requests", "replies"]],
	] as const) {
		const known = new Set<string>();
		for (const key of Object.keys(all[dir]!)) if (key.endsWith("_types")) for (const t of all[dir]![key]!) known.add(t);
		for (const f of files) {
			const lines = cases(`${dir}/${f}.ndjson`);
			assert.ok(lines.length > 0, `${dir}/${f}`);
			for (const { name, line } of lines) {
				const m = JSON.parse(line) as { type?: string; v?: number };
				assert.equal(typeof m.type, "string", name);
				if (!name.startsWith("decode:") && !name.includes("does not know")) assert.ok(known.has(m.type!), `${dir}: ${name}: ${m.type}`);
				assert.equal(m.v, 1, name);
				same(name, line, JSON.stringify(JSON.parse(line)));
			}
		}
	}
});

test("names.json matches the names of the SDK", () => {
	const file = JSON.parse(readFileSync(join(CORPUS, "names.json"), "utf8")) as Record<string, Record<string, unknown>>;
	const mine = JSON.parse(JSON.stringify(names())) as typeof file;
	assert.deepEqual(mine, file);
});

test("the corpus has every folder", () => {
	assert.deepEqual(readdirSync(CORPUS).filter((n) => !n.includes(".")).sort(), ["cleanup", "engine", "host"]);
});
