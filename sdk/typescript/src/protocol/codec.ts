// The codec of the Host API (sdk/protocol/host-api.md): one JSON object on one line,
// with "v": 1, a string `type`, and an optional string `id`.

import { endianness } from "node:os";
import type { ErrorReply, Event } from "./index.js";
import { PROTOCOL, REQUEST_TYPES, EVENT_TYPES } from "./names.js";

/** A line that is not a JSON object with a string `type`. */
export class NotAMessage extends Error {
	constructor(message: string) {
		super(message);
		this.name = "NotAMessage";
	}
}

/** A known type with a required field that is missing or has the wrong type. */
export class BadFields extends Error {
	readonly id: string | undefined;
	readonly kind: string;

	constructor(kind: string, id: string | undefined, message: string) {
		super(`bad ${kind} message: ${message}`);
		this.name = "BadFields";
		this.kind = kind;
		this.id = id;
	}
}

/** An event of a type that this version does not know. Its fields are as they came. */
export interface UnknownEvent {
	type: string;
	[field: string]: unknown;
}

/** A message of the host after decoding. */
export type Decoded =
	| { kind: "ok"; id?: string; fields: Record<string, unknown> }
	| { kind: "error"; id?: string; error: ErrorReply }
	| { kind: "event"; id?: string; event: Event }
	| { kind: "unknown"; id?: string; type: string; event: UnknownEvent };

/** A request after decoding. `body` has `type` and the fields. */
export type DecodedRequest = { kind: "request"; id?: string; body: { type: string; [field: string]: unknown } } | { kind: "unknown"; id?: string; type: string; fields: Record<string, unknown> };

type Check = "string" | "u53" | "number" | "array" | "boolean" | "object";

const REQUEST_FIELDS: Record<string, Record<string, Check>> = {
	hello: { protocol: "u53" },
	download_model: { model: "string", reason: "string" },
	transcribe_file: { path: "string" },
	cancel_job: { job: "u53" },
	session_start: { sources: "array" },
	session_audio: { session: "u53", pcm: "string" },
	session_stop: { session: "u53" },
	subscribe: { events: "array" },
};

const EVENT_FIELDS: Record<string, Record<string, Check>> = {
	model_state: { model: "string", state: "string" },
	download_progress: { model: "string", fraction: "number", bytes_done: "u53", bytes_total: "u53" },
	job_progress: { job: "u53", fraction: "number" },
	segment: { start_ms: "u53", end_ms: "u53", text: "string" },
	speakers: { turns: "array" },
	job_done: { job: "u53", text: "string" },
	job_failed: { job: "u53", message: "string" },
	partial: { session: "u53", committed: "string", tentative: "string" },
	source_gap: { session: "u53", source: "string", ms: "u53" },
	source_ended: { session: "u53", source: "string", reason: "string" },
	part_failed: { session: "u53", source: "string", start_ms: "u53", end_ms: "u53", message: "string" },
	session_ended: { session: "u53" },
	silence_limit: { session: "u53", silence_ms: "u53" },
	dictation_done: { text: "string", transcript: "string", time: "string" },
	approval_waiting: {},
	host_closing: {},
};

// Fields that are integers when they are there: JSON numbers above 2^53 lose digits, so they are refused.
const INTEGER_FIELDS = new Set(["job", "session", "start_ms", "end_ms", "ms", "silence_ms", "bytes_done", "bytes_total", "size_bytes", "load_ms", "silence_limit_ms", "speakers"]);

function isObject(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isU53(value: unknown): value is number {
	return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function matches(value: unknown, check: Check): boolean {
	switch (check) {
		case "string":
			return typeof value === "string";
		case "u53":
			return isU53(value);
		case "number":
			return typeof value === "number" && Number.isFinite(value);
		case "array":
			return Array.isArray(value);
		case "boolean":
			return typeof value === "boolean";
		case "object":
			return isObject(value);
	}
}

/** A field with the value `null` counts as missing, as in the Rust and Go SDKs. */
function dropNulls(value: Record<string, unknown>): Record<string, unknown> {
	const out: Record<string, unknown> = {};
	for (const [key, v] of Object.entries(value)) {
		if (v === null || v === undefined) continue;
		out[key] = isObject(v) ? dropNulls(v) : Array.isArray(v) ? v.map((x) => (isObject(x) ? dropNulls(x) : x)) : v;
	}
	return out;
}

function checkIntegers(kind: string, id: string | undefined, fields: Record<string, unknown>): void {
	for (const [key, value] of Object.entries(fields)) {
		if (INTEGER_FIELDS.has(key) && value !== undefined && !isU53(value)) {
			throw new BadFields(kind, id, `\`${key}\` is not an integer below 2^53`);
		}
	}
}

function checkRequired(kind: string, id: string | undefined, fields: Record<string, unknown>, required: Record<string, Check> | undefined): void {
	for (const [key, check] of Object.entries(required ?? {})) {
		if (!(key in fields)) throw new BadFields(kind, id, `missing field \`${key}\``);
		if (!matches(fields[key], check)) throw new BadFields(kind, id, `field \`${key}\` has the wrong type`);
	}
}

interface Envelope {
	type: string;
	id: string | undefined;
	fields: Record<string, unknown>;
}

function envelope(line: string): Envelope {
	let parsed: unknown;
	try {
		parsed = JSON.parse(line.trim());
	} catch (e) {
		throw new NotAMessage(`the line is not JSON: ${(e as Error).message}`);
	}
	if (!isObject(parsed)) throw new NotAMessage("the line is not a JSON object");
	const type = parsed.type;
	if (typeof type !== "string") throw new NotAMessage("the message has no string type");
	const rawId = parsed.id;
	const id = typeof rawId === "string" ? rawId : undefined;
	const { v: _v, id: _id, ...fields } = dropNulls(parsed);
	return { type, id, fields };
}

function withId<T extends object>(value: T, id: string | undefined): T & { id?: string } {
	return id === undefined ? value : { ...value, id };
}

/** Read one line of an app. */
export function decodeRequest(line: string): DecodedRequest {
	const { type, id, fields } = envelope(line);
	if (!(REQUEST_TYPES as readonly string[]).includes(type)) {
		const { type: _t, ...rest } = fields;
		return withId({ kind: "unknown" as const, type, fields: rest }, id);
	}
	checkRequired(type, id, fields, REQUEST_FIELDS[type]);
	checkIntegers(type, id, fields);
	return withId({ kind: "request" as const, body: { ...fields, type } }, id);
}

/** Read one line of the host: a reply or an event. */
export function decodeHost(line: string): Decoded {
	const { type, id, fields } = envelope(line);
	if (type === "ok") {
		const { type: _t, ...rest } = fields;
		checkIntegers(type, id, rest);
		return withId({ kind: "ok" as const, fields: rest }, id);
	}
	if (type === "error") {
		if (typeof fields.message !== "string") throw new BadFields(type, id, "missing field `message`");
		const error: ErrorReply = { message: fields.message };
		if (typeof fields.code === "string") error.code = fields.code;
		if (typeof fields.capability === "string") error.capability = fields.capability;
		if (typeof fields.reason === "string") error.reason = fields.reason;
		return withId({ kind: "error" as const, error }, id);
	}
	if (!(EVENT_TYPES as readonly string[]).includes(type)) {
		return withId({ kind: "unknown" as const, type, event: { ...fields, type } as UnknownEvent }, id);
	}
	checkRequired(type, id, fields, EVENT_FIELDS[type]);
	checkIntegers(type, id, fields);
	if (type === "speakers") {
		const turns = fields.turns as unknown[];
		fields.turns = turns.map((turn) => {
			if (!isObject(turn) || !isU53(turn.start_ms) || !isU53(turn.end_ms) || (typeof turn.speaker !== "string" && !isU53(turn.speaker))) {
				throw new BadFields(type, id, "a turn has a wrong field");
			}
			return { start_ms: turn.start_ms, end_ms: turn.end_ms, speaker: String(turn.speaker) };
		});
	}
	return withId({ kind: "event" as const, event: { ...fields, type } as unknown as Event }, id);
}

/**
 * One message as one line of JSON, without the new line. The keys come in the order of the
 * host: `type`, the fields, `v`, then `id`.
 */
export function encodeMessage(id: string | undefined, body: { type: string; [field: string]: unknown }): string {
	const { type, ...rest } = body;
	const out: Record<string, unknown> = { type };
	for (const [key, value] of Object.entries(rest)) {
		if (value !== undefined && value !== null) out[key] = value;
	}
	out.v = PROTOCOL;
	if (id !== undefined) out.id = id;
	return JSON.stringify(out);
}

// ---- PCM ----

/** The rate of PCM in the protocol. */
export const SAMPLE_RATE = 16000;

const LITTLE = endianness() === "LE";

/** Base64 of little-endian 16-bit samples. */
export function encodeI16(samples: Int16Array): string {
	if (LITTLE) return Buffer.from(samples.buffer, samples.byteOffset, samples.byteLength).toString("base64");
	const bytes = Buffer.alloc(samples.length * 2);
	samples.forEach((s, i) => bytes.writeInt16LE(s, i * 2));
	return bytes.toString("base64");
}

/** Floats in [-1, 1] as base64 16-bit PCM, with the scale 32768. */
export function encodeF32(samples: Float32Array): string {
	return encodeI16(floatsToI16(samples));
}

/** Read base64 of 16-bit PCM. */
export function decodeI16(pcm: string): Int16Array {
	if (!/^[A-Za-z0-9+/]*={0,2}$/.test(pcm) || pcm.length % 4 !== 0) throw new Error("pcm is not base64");
	const bytes = Buffer.from(pcm, "base64");
	if (bytes.length % 2 !== 0) throw new Error(`pcm has ${bytes.length} bytes, but 16-bit samples need an even number`);
	const out = new Int16Array(bytes.length / 2);
	for (let i = 0; i < out.length; i++) out[i] = bytes.readInt16LE(i * 2);
	return out;
}

/** Read base64 of 16-bit PCM as floats from -1 to 1. */
export function decodeF32(pcm: string): Float32Array {
	return i16ToFloats(decodeI16(pcm));
}

export function floatsToI16(samples: Float32Array): Int16Array {
	const out = new Int16Array(samples.length);
	for (let i = 0; i < samples.length; i++) {
		out[i] = Math.max(-32768, Math.min(32767, Math.round((samples[i] ?? 0) * 32768)));
	}
	return out;
}

export function i16ToFloats(samples: Int16Array): Float32Array {
	const out = new Float32Array(samples.length);
	for (let i = 0; i < samples.length; i++) out[i] = (samples[i] ?? 0) / 32768;
	return out;
}

// ---- Text of a partial ----

const UNSPACED_RANGES: Array<[number, number]> = [
	[0x3040, 0x30ff],
	[0x3400, 0x4dbf],
	[0x4e00, 0x9fff],
	[0xf900, 0xfaff],
	[0x0e00, 0x0e7f],
];

/** True for a character of a script that writes no spaces between words. */
export function isUnspaced(char: string): boolean {
	const code = char.codePointAt(0) ?? 0;
	return UNSPACED_RANGES.some(([from, to]) => code >= from && code <= to);
}

/** The committed and the tentative text of a `partial`, with a space where the script needs one. */
export function partialText(partial: { committed: string; tentative: string }): string {
	const committed = partial.committed.trimEnd();
	const tentative = partial.tentative.trimStart();
	if (committed === "" || tentative === "") return committed + tentative;
	const last = [...committed].at(-1) as string;
	const first = [...tentative][0] as string;
	const closes = ",.;:!?)]}%'’…、。，！？".includes(first);
	return committed + (closes || (isUnspaced(last) && isUnspaced(first)) ? "" : " ") + tentative;
}
