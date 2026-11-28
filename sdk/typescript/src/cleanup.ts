// Cleanup models: small local models that clean up text with fixed settings and read no prompt. This module holds all
// knowledge about the models (the catalog, the input format of each model, how long text splits, and the checks of an
// answer), downloads the model files, and talks to the cleanup runtime `sayso-cleanup` (NDJSON, protocol 1).

import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream, createWriteStream, mkdirSync, readFileSync, renameSync, rmSync, statSync, writeFileSync, existsSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { StringDecoder } from "node:string_decoder";
import { detectCpu, platformKey, unpackedPath, type CpuLevel } from "./engine.js";
import { SaysoError } from "./errors.js";
import { cleanupModelsDirOf } from "./paths.js";
import { LineSplitter } from "./transport.js";
import { VERSION } from "./version.js";

// ---- Types ----

export type CleanupTone = "very-casual" | "casual" | "formal";
export type CleanupLayout = "paragraphs" | "list";
export type CleanupFormat = "plain" | "email";

/**
 * A cleanup model that the SDK can download and run. Show S1-mini as "S1-mini" by "Superwhisper": its license asks for that
 * name.
 */
export interface CleanupModel {
	id: string;
	name: string;
	publisher: string;
	sizeBytes: number;
	/** Language codes, such as `en`. */
	languages: string[];
	/** True when the model has tones. */
	tones: boolean;
	layouts: CleanupLayout[];
	formats: CleanupFormat[];
	/** True when the model reads the app and the site of the text. */
	context: boolean;
}

/** How a cleanup writes the text. A missing value is the default: `formal`, `paragraphs`, `plain`. */
export interface CleanupSettings {
	tone?: CleanupTone;
	layout?: CleanupLayout;
	format?: CleanupFormat;
}

export interface CleanupRequest {
	/** The id of a model of `cleanup.catalog()`. */
	model: string;
	text: string;
	settings?: CleanupSettings;
	/** The name of the app of the text, for a model that reads context. */
	app?: string;
	/** The host of the web page of the text. */
	site?: string;
	/** One deadline for the whole text. Missing or 0: no deadline. */
	timeoutMs?: number;
}

export interface CleanupResponse {
	text: string;
	model: string;
	elapsedMs: number;
	/** The time to load the model in this request. 0 when it was loaded. */
	loadMs: number;
	parts: number;
	tokensIn: number;
	tokensOut: number;
}

/** How far a download of a cleanup model is. */
export interface CleanupProgress {
	bytesDone: number;
	bytesTotal: number;
}

/** Why an answer is not used, or why a cleanup gave no text. The caller keeps the text that went in. */
export class CleanupError extends SaysoError {
	readonly kind: "timeout" | "not_configured" | "runtime" | "invalid_output";

	constructor(kind: CleanupError["kind"], message: string) {
		super({ code: kind === "timeout" ? "timeout" : kind === "not_configured" ? "unsupported" : "unavailable", message });
		this.name = "CleanupError";
		this.kind = kind;
	}
}

// ---- The catalog ----

type PromptFormat = "s1_chat" | "key_value";

/** @internal A model of the catalog with its pinned files. */
export interface CatalogEntry extends CleanupModel {
	repo: string;
	revision: string;
	file: string;
	sha256: string;
	licenseFiles: string[];
	prompt: PromptFormat;
}

/** The context size that the runtime loads a model with. */
export const N_CTX = 2048;
/** The most text tokens in one part. 700 tokens of text, the prompt around it, and an answer of 1.3 × 700 + 32 tokens fit in `N_CTX`. */
export const MAX_PART_TOKENS = 700;

const S1_SYSTEM =
	"You are a text normalizer for speech-to-text transcripts. The input begins with a control line specifying the styling, structure, and context settings; clean the transcript to match those settings and output only the cleaned text.";

const CATALOG: readonly CatalogEntry[] = [
	{
		id: "s1-mini",
		name: "S1-mini",
		publisher: "Superwhisper",
		repo: "superwhisper/s1-mini-GGUF",
		revision: "34add00a48a2e5d24e5a4ee5405a99620a3a240c",
		file: "s1-mini-q4_k_m.gguf",
		sizeBytes: 484_219_808,
		sha256: "3b41ebe2502cbd03e811d5d16b022f5ab551eda58d62597d152f89535003c634",
		licenseFiles: ["LICENSE", "NOTICE"],
		languages: ["en"],
		tones: false,
		layouts: ["paragraphs", "list"],
		formats: ["plain", "email"],
		context: false,
		prompt: "s1_chat",
	},
	{
		id: "spokenly-cleanup-1",
		name: "Cleanup 1",
		publisher: "Spokenly",
		repo: "spokenly/cleanup-1-gguf",
		revision: "79936c95e5c0ea9162ae7a435e64cb3127020e36",
		file: "cleanup-1-q8_0.gguf",
		sizeBytes: 639_442_784,
		sha256: "66509e0fe1578d2ac4b0773c79a6c83e8d6f218618dfe11e4532beacd999b6e8",
		licenseFiles: ["LICENSE", "NOTICE"],
		languages: ["en", "de", "es", "fr", "zh", "ru", "ja"],
		tones: true,
		layouts: ["paragraphs"],
		formats: ["plain"],
		context: true,
		prompt: "key_value",
	},
];

const publicModel = (m: CatalogEntry): CleanupModel => ({ id: m.id, name: m.name, publisher: m.publisher, sizeBytes: m.sizeBytes, languages: [...m.languages], tones: m.tones, layouts: [...m.layouts], formats: [...m.formats], context: m.context });

/** What the catalog pins for a model: where the files are and what they hash to. */
export interface CleanupModelFiles {
	id: string;
	repo: string;
	revision: string;
	file: string;
	sha256: string;
	sizeBytes: number;
	licenseFiles: string[];
}

function entry(id: string): CatalogEntry | undefined {
	return CATALOG.find((m) => m.id === id);
}

/** The pinned files of a model, for the checks of a release. */
export function modelFiles(id: string): CleanupModelFiles | undefined {
	const m = entry(id);
	return m && { id: m.id, repo: m.repo, revision: m.revision, file: m.file, sha256: m.sha256, sizeBytes: m.sizeBytes, licenseFiles: [...m.licenseFiles] };
}

const KNOWN = {
	tone: ["formal", "casual", "very-casual"],
	layout: ["paragraphs", "list"],
	format: ["plain", "email"],
} as const;

/** The settings with their defaults, or undefined when a value is not known to this version. */
export function knownSettings(settings: CleanupSettings = {}): Required<CleanupSettings> | undefined {
	const tone = settings.tone ?? "formal";
	const layout = settings.layout ?? "paragraphs";
	const format = settings.format ?? "plain";
	if (!KNOWN.tone.includes(tone) || !KNOWN.layout.includes(layout) || !KNOWN.format.includes(format)) return undefined;
	return { tone, layout, format };
}

/** True when the model can run a style with these settings: every value is known, and the model has the layout and the format. */
export function runs(model: CleanupModel, settings: CleanupSettings = {}): boolean {
	const s = knownSettings(settings);
	return s !== undefined && model.layouts.includes(s.layout) && model.formats.includes(s.format);
}

// ---- The input of a model ----

/** One request to a cleanup model, for one part of the text. */
export interface RawPrompt {
	text: string;
	/** Strings that end the answer, beside the end-of-generation token. */
	end: string[];
	maxTokens: number;
}

/** The longest answer for a part of `textTokens` tokens. */
export function maxAnswerTokens(textTokens: number): number {
	return Math.floor((textTokens * 13) / 10) + 32;
}

/** Write the input of `model` for one part of the text. `app` and `site` are used only by a model that reads context. */
export function render(model: CleanupModel, text: string, settings: CleanupSettings = {}, app?: string | null, site?: string | null, textTokens = 0): RawPrompt {
	const entryOf = entry(model.id);
	if (!entryOf) throw SaysoError.of("unsupported", `no cleanup model ${model.id}`);
	const s = knownSettings(settings) ?? knownSettings({}) as Required<CleanupSettings>;
	const maxTokens = maxAnswerTokens(textTokens);
	if (entryOf.prompt === "s1_chat") {
		// The evaluation: casual styling keeps filler words, and semi-formal and formal give the same text. So always semi-formal.
		const structure = s.layout === "list" ? "lists" : "prose";
		const context = s.format === "email" ? "email" : "general";
		return {
			text: `<|im_start|>system\n${S1_SYSTEM}<|im_end|>\n<|im_start|>user\n[Styling: semi-formal] [Structure: ${structure}] [Context: ${context}]\n${text}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n`,
			end: ["<|im_end|>"],
			maxTokens,
		};
	}
	const oneLine = (value: string): string => value.split(/\s+/).filter(Boolean).join(" ");
	const lines: string[] = [];
	if (entryOf.context) {
		const a = app == null ? "" : oneLine(app);
		if (a !== "") lines.push(`app: ${a}`);
		const w = site == null ? "" : oneLine(site);
		if (w !== "") lines.push(`site: ${w}`);
	}
	if (entryOf.tones) lines.push(`tone: ${s.tone === "very-casual" ? "very casual" : s.tone}`);
	lines.push(`text: ${text}`, "out:");
	// The model ends with its end-of-text token. The strings guard against an answer that goes on with a new key.
	return { text: lines.join("\n"), end: ["\ntext:", "\nout:"], maxTokens };
}

// ---- Long text ----

const SENTENCE_END = new Set([".", "?", "!", "。", "？", "！", "\n"]);
const COMMA = new Set([",", "，", "、"]);
const isSpace = (c: string): boolean => /^\s$/u.test(c);

/** The index after each character for which `isCut` is true, with the white space that follows it. */
function cutPoints(text: string, isCut: (c: string) => boolean): number[] {
	const chars = [...text];
	const cuts: number[] = [];
	let at = 0;
	for (let i = 0; i < chars.length; i++) {
		const c = chars[i] as string;
		at += c.length;
		if (!isCut(c)) continue;
		let end = at;
		while (i + 1 < chars.length && isSpace(chars[i + 1] as string)) {
			i++;
			end += (chars[i] as string).length;
		}
		at = end;
		if (end < text.length && cuts.at(-1) !== end) cuts.push(end);
	}
	return cuts;
}

async function longestFit(text: string, cuts: number[], maxTokens: number, count: (text: string) => Promise<number>): Promise<number | undefined> {
	let lo = 0;
	let hi = cuts.length;
	let best: number | undefined;
	while (lo < hi) {
		const mid = Math.floor((lo + hi) / 2);
		const cut = cuts[mid] as number;
		if ((await count(text.slice(0, cut))) <= maxTokens) {
			best = cut;
			lo = mid + 1;
		} else {
			hi = mid;
		}
	}
	return best;
}

async function bestCut(text: string, maxTokens: number, count: (text: string) => Promise<number>): Promise<number> {
	for (const isCut of [(c: string) => SENTENCE_END.has(c), (c: string) => COMMA.has(c), isSpace]) {
		const cut = await longestFit(text, cutPoints(text, isCut), maxTokens, count);
		if (cut !== undefined) return cut;
	}
	const every: number[] = [];
	let at = 0;
	for (const c of text) {
		at += c.length;
		every.push(at);
	}
	// One character always goes, so the loop ends also when one character is over the limit.
	return (await longestFit(text, every, maxTokens, count)) ?? (every[0] as number);
}

/**
 * Split `text` into parts of at most `maxTokens` tokens. `count` gives the tokens of a text; its first error ends the split.
 * Every character of `text` is in exactly one part, in order. A part ends after a sentence end if it can, else after a comma,
 * else after a space, else after any character.
 */
export async function planParts(text: string, maxTokens: number, count: (text: string) => Promise<number>): Promise<string[]> {
	const parts: string[] = [];
	let rest = text;
	while (rest !== "") {
		if ((await count(rest)) <= maxTokens) {
			parts.push(rest);
			break;
		}
		const cut = await bestCut(rest, maxTokens, count);
		parts.push(rest.slice(0, cut));
		rest = rest.slice(cut);
	}
	return parts;
}

/** Join the answers of the parts. A part that ended with a new line keeps it between the answers; other parts get one space. */
export function joinAnswers(parts: string[], answers: string[]): string {
	let out = "";
	answers.forEach((answer, i) => {
		if (answer === "") return;
		if (out !== "") out += (parts[i - 1] ?? "").replace(/[ \t]+$/, "").endsWith("\n") ? "\n" : " ";
		out += answer;
	});
	return out;
}

// ---- The checks of an answer ----

/** Why a model stopped: at its end marker, at the limit of tokens, or at the end of its context. */
export type Stop = "end" | "limit" | "context";

export type CheckResult = { ok: true; text: string } | { ok: false; failure: "cut" | "empty" | "too_short" | "too_long"; message: string };

const CONTROL_TEXT = ["<|im_start|>", "<|im_end|>", "<|endoftext|>", "<think>", "</think>"];

const NUMBER_WORDS = new Set(["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety", "hundred", "thousand", "million", "billion", "point"]);

function isCjk(c: string): boolean {
	const n = c.codePointAt(0) ?? 0;
	return (n >= 0x3040 && n <= 0x30ff) || (n >= 0x3400 && n <= 0x4dbf) || (n >= 0x4e00 && n <= 0x9fff) || (n >= 0xac00 && n <= 0xd7af) || (n >= 0xf900 && n <= 0xfaff);
}

const ALNUM = /^[\p{Alphabetic}\p{N}]$/u;

/**
 * How much a text says, for the length check: one unit for a word, for each Chinese, Japanese, or Korean character, and for a
 * run of number words or digits. So "nine hundred and ninety nine" and "999" are one unit each.
 */
export function contentUnits(text: string): number {
	let units = 0;
	let inNumber = false;
	let word = "";
	const flush = (): void => {
		if (word === "") return;
		const lower = word.toLowerCase();
		const number = /\d/.test(lower) || NUMBER_WORDS.has(lower);
		const joins = inNumber && lower === "and";
		if (number || joins) {
			if (!inNumber) units++;
			inNumber = true;
		} else {
			units++;
			inNumber = false;
		}
		word = "";
	};
	for (const c of text) {
		if (isCjk(c)) {
			flush();
			units++;
			inNumber = false;
		} else if (ALNUM.test(c)) {
			word += c;
		} else {
			// Punctuation inside a number ("999,999", "5:30") does not end the number.
			flush();
		}
	}
	flush();
	return units;
}

const FILLERS = new Set([
	"um", "umm", "uh", "uhm", "uhh", "er", "erm", "ah", "hmm", "hm", "mm", "mhm", "like", "so", "well", "you", "know",
	"äh", "ähm", "öh", "halt", "also",
	"euh", "bah", "ben", "hein",
	"eh", "em", "este", "pues", "bueno",
	"э", "эм", "ну", "вот",
	"嗯", "呃", "啊", "那个", "就是",
	"えー", "えっと", "あの", "あのー", "まあ",
]);

/** True when every word of `text` is a filler word ("um uh you know"). A text with no words is not filler only. */
export function isFillerOnly(text: string): boolean {
	const words = text
		.split(/[^\p{Alphabetic}\p{N}ー]+/u)
		.filter((w) => w !== "")
		.map((w) => w.toLowerCase());
	// "you" and "know" count only together.
	const joined = ` ${words.join(" ")} `.replaceAll(" you know ", " ");
	return words.length > 0 && joined.split(/\s+/).filter(Boolean).every((w) => FILLERS.has(w) && w !== "you" && w !== "know");
}

/** Check the answer of a model for one part of the text, and clean it. Only an answer that stopped at its end marker is whole. */
export function check(input: string, stop: Stop, answer: string): CheckResult {
	if (stop !== "end") return { ok: false, failure: "cut", message: `the answer stopped early (${stop})` };
	let text = answer;
	for (const control of CONTROL_TEXT) text = text.replaceAll(control, "");
	text = text.trim();
	if (text === "") return isFillerOnly(input) ? { ok: true, text } : { ok: false, failure: "empty", message: "the answer is empty" };
	const before = contentUnits(input);
	const after = contentUnits(text);
	if (before >= 10 && after * 4 < before) return { ok: false, failure: "too_short", message: "the answer is much shorter than the text" };
	if ([...text].length > [...input].length * 3 + 200) return { ok: false, failure: "too_long", message: "the answer is much longer than the text" };
	return { ok: true, text };
}

// ---- Spelling ----

const escapeRegExp = (s: string): string => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

function wholeWord(term: string, anyCase: boolean): RegExp {
	const word = "[\\p{L}\\p{N}\\p{M}_]";
	const start = /^[\p{Alphabetic}\p{N}]/u.test(term) ? `(?<!${word})` : "";
	const end = /[\p{Alphabetic}\p{N}]$/u.test(term) ? `(?!${word})` : "";
	return new RegExp(`${start}${escapeRegExp(term)}${end}`, anyCase ? "giu" : "gu");
}

/**
 * Put back the spelling of `terms` that the model changed: a term that was in `sent` with its exact spelling (and in no other
 * case), and is in `answer` with the same letters in another case, as a whole word. Longer terms first, and a term inside a
 * term that was already put back is not changed.
 */
export function restoreSpelling(sent: string, answer: string, terms: string[]): string {
	const list = [...new Set(terms.map((t) => t.trim()).filter((t) => t !== ""))].sort((a, b) => [...b].length - [...a].length);
	let out = answer;
	let fixed: Array<[number, number]> = [];
	for (const term of list) {
		const exact = wholeWord(term, false);
		if (!exact.test(sent)) continue;
		const anyCase = wholeWord(term, true);
		// The text had the same word in another case too ("we may ship in May"): Sayso cannot tell which one the model changed.
		if ([...sent.matchAll(anyCase)].some((m) => m[0] !== term)) continue;
		const found = [...out.matchAll(anyCase)]
			.filter((m) => m[0] !== term && Buffer.byteLength(m[0]) === Buffer.byteLength(term) && m[0].length === term.length)
			.map((m): [number, number] => [m.index as number, (m.index as number) + m[0].length])
			.filter(([a, b]) => !fixed.some(([fa, fb]) => fa < b && a < fb));
		for (const [a, b] of found) out = out.slice(0, a) + term + out.slice(b);
		fixed = [...fixed, ...found, ...[...out.matchAll(wholeWord(term, false))].map((m): [number, number] => [m.index as number, (m.index as number) + m[0].length])];
	}
	return out;
}

// ---- The language ----

/** The language setting that asks the model to detect the language. */
export const AUTO_LANGUAGE = "auto";

/**
 * The language of a dictation, when Sayso knows it. 1. The setting is a language code: that language. 2. The setting is "auto"
 * and the final model knows one language: that language. 3. Else `detect` on the text, which gives a language only when it is sure.
 */
export function dictationLanguage(setting: string, finalModelLanguages: string[], text: string, detect: (text: string) => string | undefined = detectLanguage): string | undefined {
	if (setting !== AUTO_LANGUAGE && setting !== "") return setting;
	if (finalModelLanguages.length === 1) return finalModelLanguages[0];
	return detect(text);
}

const STOPWORDS: Record<string, string[]> = {
	en: ["the", "and", "to", "of", "is", "in", "that", "it", "for", "we", "on", "with", "this", "be", "are", "before", "need", "fix", "update"],
	de: ["der", "die", "das", "und", "ist", "nicht", "den", "ein", "zu", "wir", "müssen", "mit", "auf", "für", "vor", "dem", "von", "beheben", "die"],
	es: ["el", "la", "de", "que", "y", "en", "los", "se", "del", "las", "por", "un", "para", "con", "no", "una", "su", "es"],
	fr: ["le", "la", "les", "de", "des", "et", "est", "en", "un", "une", "que", "pour", "dans", "pas", "sur", "nous", "vous"],
	it: ["il", "di", "che", "la", "e", "è", "per", "un", "in", "con", "non", "una", "sono", "del", "le", "della"],
	pt: ["o", "de", "que", "e", "do", "da", "em", "um", "para", "com", "não", "uma", "os", "no", "se", "na", "por"],
	nl: ["de", "het", "een", "van", "en", "is", "dat", "op", "te", "voor", "met", "niet", "zijn", "we", "moeten"],
};

/**
 * The language of `text` as a Sayso language code, when the detector is sure. A short or mixed text is often not sure, and then
 * this gives undefined. This detector reads scripts (Japanese, Chinese, Korean, Russian) and the common words of the Latin
 * languages of the catalog.
 */
export function detectLanguage(text: string): string | undefined {
	const letters = [...text].filter((c) => /\p{L}/u.test(c));
	if (letters.length < 12) return undefined;
	const count = (re: RegExp): number => letters.filter((c) => re.test(c)).length;
	const kana = count(/[぀-ヿ]/u);
	const han = count(/[㐀-鿿]/u);
	const hangul = count(/[가-힯]/u);
	const cyrillic = count(/[Ѐ-ӿ]/u);
	const share = (n: number): number => n / letters.length;
	if (share(kana) > 0.1) return "ja";
	if (share(han) > 0.5) return "zh";
	if (share(hangul) > 0.5) return "ko";
	if (share(cyrillic) > 0.5) return "ru";
	const words = text.toLowerCase().split(/[^\p{L}]+/u).filter(Boolean);
	if (words.length < 5) return undefined;
	const scores = Object.entries(STOPWORDS).map(([lang, list]) => [lang, words.filter((w) => list.includes(w)).length] as const).sort((a, b) => b[1] - a[1]);
	const [best, second] = scores;
	if (!best || best[1] < 2 || best[1] < (second?.[1] ?? 0) * 1.5 + 1) return undefined;
	return best[0];
}

// ---- The files of a model ----

/** The default server of the model files. */
export const DEFAULT_ENDPOINT = "https://huggingface.co";
const READY = "ready.json";
const ATTEMPTS = 3;
const RETRY_PAUSE_MS = 500;
const PROGRESS_INTERVAL_MS = 125;

const plainName = (name: string): boolean => name !== "" && name !== "." && name !== ".." && /^[A-Za-z0-9._-]+$/.test(name);

function checkNames(m: CatalogEntry): void {
	if (!plainName(m.id) || !plainName(m.file) || !m.licenseFiles.every(plainName)) throw new CleanupError("not_configured", `the catalog names a path that is not a plain name (${m.id})`);
}

function modelDir(modelsDir: string, m: CatalogEntry): string {
	return join(cleanupModelsDirOf(modelsDir), m.id);
}

function requireEntry(id: string): CatalogEntry {
	const m = entry(id);
	if (!m) throw SaysoError.of("unknown_model", `no cleanup model ${id}`);
	return m;
}

function fileLength(path: string): number {
	try {
		return statSync(path).size;
	} catch {
		return 0;
	}
}

/** True when the model is in `modelsDir/cleanup/<id>` and complete: `ready.json` names the revision and the hash, and the file is there. */
function isReady(modelsDir: string, id: string): boolean {
	const m = entry(id);
	return m !== undefined && isReadyEntry(modelsDir, m);
}

function isReadyEntry(modelsDir: string, m: CatalogEntry): boolean {
	const dir = modelDir(modelsDir, m);
	try {
		const ready = JSON.parse(readFileSync(join(dir, READY), "utf8")) as Record<string, unknown>;
		return ready.revision === m.revision && ready.sha256 === m.sha256 && statSync(join(dir, m.file)).isFile();
	} catch {
		return false;
	}
}

/** The model file when the model is ready, else null. */
function modelPath(modelsDir: string, id: string): string | null {
	const m = entry(id);
	return m && isReady(modelsDir, id) ? join(modelDir(modelsDir, m), m.file) : null;
}

const active = new Set<string>();

function cancelled(): SaysoError {
	return SaysoError.of("declined", "The download was cancelled.");
}

async function pause(ms: number, signal: AbortSignal | undefined): Promise<void> {
	await new Promise<void>((resolve) => setTimeout(resolve, ms));
	if (signal?.aborted) throw cancelled();
}

async function fetchTo(url: string, dest: string, signal: AbortSignal | undefined): Promise<void> {
	const partial = `${dest}.partial`;
	for (let failures = 1; ; failures++) {
		if (signal?.aborted) throw cancelled();
		try {
			const response = await fetch(url, { headers: { "Accept-Encoding": "identity", "User-Agent": `Sayso/${VERSION}` }, ...(signal ? { signal } : {}) });
			if (response.status >= 500) throw new Error(`the server answered with status ${response.status}`);
			if (response.status !== 200) throw new SaysoError({ message: `The server answered with status ${response.status}.` });
			writeFileSync(partial, Buffer.from(await response.arrayBuffer()));
			renameSync(partial, dest);
			return;
		} catch (e) {
			if (signal?.aborted) throw cancelled();
			if (e instanceof SaysoError || failures >= ATTEMPTS) {
				rmSync(partial, { force: true });
				throw e instanceof SaysoError ? e : new SaysoError({ message: `The network failed: ${(e as Error).message}` });
			}
			await pause(RETRY_PAUSE_MS * failures, signal);
		}
	}
}

function contentRangeStart(value: string): number | undefined {
	const match = /^\s*bytes\s+(\d+)-/.exec(value);
	return match ? Number(match[1]) : undefined;
}

async function fetchModel(url: string, partial: string, m: CatalogEntry, signal: AbortSignal | undefined, report: (done: number, force: boolean) => void): Promise<void> {
	let best = fileLength(partial);
	let failures = 0;
	for (;;) {
		if (signal?.aborted) throw cancelled();
		try {
			await modelAttempt(url, partial, m.sizeBytes, signal, report);
			return;
		} catch (e) {
			if (signal?.aborted) throw cancelled();
			if (e instanceof SaysoError) throw e;
			const now = fileLength(partial);
			if (now > best) {
				best = now;
				failures = 0;
			} else {
				failures++;
			}
			if (failures >= ATTEMPTS) throw new SaysoError({ message: `The network failed: ${(e as Error).message}` });
			await pause(RETRY_PAUSE_MS * Math.max(failures, 1), signal);
		}
	}
}

async function modelAttempt(url: string, partial: string, size: number, signal: AbortSignal | undefined, report: (done: number, force: boolean) => void): Promise<void> {
	let have = fileLength(partial);
	if (have > size) {
		rmSync(partial, { force: true });
		have = 0;
	}
	const response = await fetch(url, { headers: { "Accept-Encoding": "identity", "User-Agent": `Sayso/${VERSION}`, ...(have > 0 ? { Range: `bytes=${have}-` } : {}) }, ...(signal ? { signal } : {}) });
	let append = false;
	switch (response.status) {
		case 206:
			if (have === 0 || contentRangeStart(response.headers.get("content-range") ?? "") !== have) {
				rmSync(partial, { force: true });
				throw new Error("the server resumed at the wrong place");
			}
			append = true;
			break;
		case 200:
			have = 0;
			break;
		case 416:
			if (have === size && have > 0) return;
			rmSync(partial, { force: true });
			throw new Error("the server answered with status 416");
		default:
			if (response.status >= 500) throw new Error(`the server answered with status ${response.status}`);
			throw new SaysoError({ message: `The server answered with status ${response.status}.` });
	}
	if (!response.body) throw new Error("the server sent no body");
	const out = createWriteStream(partial, { flags: append ? "a" : "w" });
	report(have, false);
	try {
		const reader = response.body.getReader();
		for (;;) {
			const { done, value } = await reader.read();
			if (done) break;
			if (!out.write(value)) await new Promise<void>((resolve) => out.once("drain", resolve));
			have += value.length;
			if (have > size) break;
			report(have, false);
		}
	} finally {
		await new Promise<void>((resolve) => out.end(resolve));
	}
	if (have < size) throw new Error(`the connection closed after ${have} of ${size} bytes`);
}

async function sha256File(path: string): Promise<string> {
	const hash = createHash("sha256");
	for await (const chunk of createReadStream(path)) hash.update(chunk as Buffer);
	return hash.digest("hex");
}

/**
 * Download the model file and its license files to `<modelsDir>/cleanup/<id>`, and give the path of the model file. The signal
 * cancels it, with the code `declined`; the partial file stays for a later resume. The app asks its own user first.
 */
async function download(modelsDir: string, id: string, options: DownloadOptions = {}): Promise<string> {
	return downloadEntry(modelsDir, requireEntry(id), options);
}

interface DownloadOptions {
	endpoint?: string;
	signal?: AbortSignal;
	onProgress?: (progress: CleanupProgress) => void;
}

/** @internal The download of `download`, for an entry that a test can give. */
export async function downloadEntry(modelsDir: string, m: CatalogEntry, options: DownloadOptions = {}): Promise<string> {
	checkNames(m);
	const dir = modelDir(modelsDir, m);
	if (active.has(dir)) throw new SaysoError({ code: "busy", message: "This model is already downloading or being deleted." });
	active.add(dir);
	try {
		const path = join(dir, m.file);
		const size = m.sizeBytes;
		if (isReadyEntry(modelsDir, m)) {
			options.onProgress?.({ bytesDone: size, bytesTotal: size });
			return path;
		}
		mkdirSync(dir, { recursive: true });
		const endpoint = (options.endpoint ?? DEFAULT_ENDPOINT).replace(/\/+$/, "");
		const url = (name: string): string => `${endpoint}/${m.repo}/resolve/${m.revision}/${name}`;
		for (const name of m.licenseFiles) await fetchTo(url(name), join(dir, name), options.signal);
		const partial = `${path}.partial`;
		const info = `${path}.partial.json`;
		const wanted = JSON.stringify({ revision: m.revision, sha256: m.sha256 });
		let current: string | undefined;
		try {
			current = JSON.stringify(JSON.parse(readFileSync(info, "utf8")));
		} catch {
			current = undefined;
		}
		if (current !== wanted) {
			rmSync(partial, { force: true });
			writeFileSync(info, wanted);
		}
		let highest = 0;
		let last = 0;
		const report = (done: number, force: boolean): void => {
			highest = Math.max(highest, Math.min(done, size));
			const now = Date.now();
			if (force || now - last >= PROGRESS_INTERVAL_MS) {
				last = now;
				options.onProgress?.({ bytesDone: highest, bytesTotal: size });
			}
		};
		await fetchModel(url(m.file), partial, m, options.signal, report);
		const got = fileLength(partial);
		if (got !== size) {
			rmSync(partial, { force: true });
			rmSync(info, { force: true });
			throw new SaysoError({ message: `The file has ${got} bytes, but it must have ${size}.` });
		}
		const hash = await sha256File(partial);
		if (hash.toLowerCase() !== m.sha256.toLowerCase()) {
			rmSync(partial, { force: true });
			rmSync(info, { force: true });
			throw new SaysoError({ message: `The file is not the expected file. Its hash is ${hash}, but it must be ${m.sha256}.` });
		}
		renameSync(partial, path);
		rmSync(info, { force: true });
		const ready = join(dir, READY);
		writeFileSync(`${ready}.tmp`, JSON.stringify({ revision: m.revision, sha256: m.sha256, file: m.file }));
		renameSync(`${ready}.tmp`, ready);
		report(size, true);
		return path;
	} finally {
		active.delete(dir);
	}
}

/** Delete the files of the model. A model that is not on disk gives no error. */
async function deleteModel(modelsDir: string, id: string): Promise<void> {
	const m = requireEntry(id);
	checkNames(m);
	const dir = modelDir(modelsDir, m);
	if (active.has(dir)) throw new SaysoError({ code: "busy", message: "This model is already downloading or being deleted." });
	active.add(dir);
	try {
		for (let attempt = 0; ; attempt++) {
			try {
				rmSync(dir, { recursive: true, force: true });
				return;
			} catch (e) {
				if (process.platform !== "win32" || attempt >= 20) throw e;
				await new Promise((resolve) => setTimeout(resolve, 100));
			}
		}
	} finally {
		active.delete(dir);
	}
}

// ---- The runtime ----

const PROGRAM = "sayso-cleanup";
/** How long the runtime may take to load a model. */
export const LOAD_LIMIT_MS = 15_000;
/** The runtime stops after this long without use. */
export const IDLE_LIMIT_MS = 10 * 60 * 1000;
const SHORT_LIMIT_MS = 5_000;
/** A small model takes time for each token, so a long text gets more time than the timeout of the style. */
export const TIME_PER_TOKEN_MS = 15;

/** The answer of the runtime to `hello`. */
export interface CleanupHello {
	version: number;
	llama_cpp: string;
	backend: string;
	libraries?: string;
}

/** A completion of the runtime. */
export interface Completion {
	text: string;
	stop: Stop;
	tokens_in: number;
	tokens_out: number;
	ms: number;
}

interface PendingReply {
	resolve(message: Record<string, unknown>): void;
	reject(error: Error): void;
}

/** A failure of the runtime: an `error` reply (`code` is one of its codes), a timeout, or an end of the process. */
export class RuntimeError extends SaysoError {
	readonly kind: "timeout" | "stopped" | "failed" | "not_installed" | "unexpected" | "closed";

	constructor(kind: RuntimeError["kind"], message: string, code?: string) {
		super({ code: code ?? (kind === "timeout" ? "timeout" : "unavailable"), message });
		this.name = "RuntimeError";
		this.kind = kind;
	}
}

/**
 * A client of the cleanup runtime (protocol 1): a process that talks NDJSON over its standard input and output. One request
 * at a time is sent, and each waits for its reply.
 */
export class Cleanup {
	readonly program: string;
	#child: ChildProcessWithoutNullStreams;
	#pending = new Map<string, PendingReply>();
	#next = 0;
	#stderr: string[] = [];
	#exit: string | undefined;
	readonly #done: Promise<void>;

	private constructor(program: string, child: ChildProcessWithoutNullStreams) {
		this.program = program;
		this.#child = child;
		let finish!: () => void;
		this.#done = new Promise((resolve) => (finish = resolve));
		const out = new StringDecoder("utf8");
		const lines = new LineSplitter(64 * 1024 * 1024);
		child.stdout.on("data", (chunk: Buffer) => {
			try {
				for (const line of lines.push(out.write(chunk))) this.#line(line);
			} catch {
				child.kill();
			}
		});
		const err = new StringDecoder("utf8");
		child.stderr.on("data", (chunk: Buffer) => {
			for (const line of err.write(chunk).split("\n")) {
				if (line.trim() === "") continue;
				this.#stderr.push(line);
				if (this.#stderr.length > 20) this.#stderr.shift();
			}
		});
		child.stdin.on("error", () => undefined);
		const end = (why: string): void => {
			if (this.#exit === undefined) {
				this.#exit = why;
				const error = new RuntimeError("stopped", `the cleanup runtime stopped: ${why}${this.#stderr.length > 0 ? ` (${this.#stderr.at(-1)})` : ""}`);
				for (const pending of this.#pending.values()) pending.reject(error);
				this.#pending.clear();
			}
		};
		child.once("error", (error) => end(error.message));
		child.once("close", (code, signal) => {
			end(signal ? `it was stopped by ${signal}` : `it exited with ${code}`);
			finish();
		});
	}

	/** Start the runtime program. It sends nothing until the first request. */
	static start(program: string, options: { env?: Record<string, string> } = {}): Cleanup {
		const child = spawn(program, [], { stdio: ["pipe", "pipe", "pipe"], windowsHide: true, env: { ...process.env, ...options.env } });
		return new Cleanup(program, child);
	}

	#line(line: string): void {
		let reply: Record<string, unknown>;
		try {
			const parsed: unknown = JSON.parse(line);
			if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) return;
			reply = parsed as Record<string, unknown>;
		} catch {
			return;
		}
		if (reply.v !== 1 || typeof reply.id !== "string") return;
		const pending = this.#pending.get(reply.id);
		if (!pending) return;
		this.#pending.delete(reply.id);
		if (reply.type === "error") pending.reject(new RuntimeError("failed", `the cleanup runtime failed: ${String(reply.code)}: ${String(reply.message)}`, String(reply.code)));
		else pending.resolve(reply);
	}

	get exited(): boolean {
		return this.#exit !== undefined;
	}

	/** Why the process ended, when it did. */
	get exitReason(): string | undefined {
		return this.#exit;
	}

	stderrTail(): string[] {
		return [...this.#stderr];
	}

	/** Send a request and wait for its reply. A reply that is late kills the process and gives the error `timeout`. */
	async call(type: string, fields: Record<string, unknown> = {}, timeoutMs?: number): Promise<Record<string, unknown>> {
		if (this.#exit !== undefined) throw new RuntimeError("stopped", `the cleanup runtime stopped: ${this.#exit}`);
		const id = String(++this.#next);
		let timer: NodeJS.Timeout | undefined;
		const reply = new Promise<Record<string, unknown>>((resolve, reject) => {
			this.#pending.set(id, { resolve, reject });
			if (timeoutMs !== undefined) {
				timer = setTimeout(() => {
					this.#pending.delete(id);
					this.kill();
					reject(new RuntimeError("timeout", "the cleanup runtime did not answer in time"));
				}, Math.max(timeoutMs, 1));
			}
		});
		try {
			this.#child.stdin.write(`${JSON.stringify({ v: 1, id, type, ...fields })}\n`);
		} catch (e) {
			this.#pending.delete(id);
			clearTimeout(timer);
			throw new RuntimeError("stopped", `the cleanup runtime stopped: ${(e as Error).message}`);
		}
		try {
			return await reply;
		} finally {
			clearTimeout(timer);
		}
	}

	async hello(timeoutMs = SHORT_LIMIT_MS * 6): Promise<CleanupHello> {
		const r = await this.call("hello", {}, timeoutMs);
		return { version: Number(r.version), llama_cpp: String(r.llama_cpp ?? ""), backend: String(r.backend ?? ""), ...(typeof r.libraries === "string" ? { libraries: r.libraries } : {}) };
	}

	/** Load a model. Gives the load time in milliseconds. */
	async load(modelId: string, path: string, nCtx = N_CTX, timeoutMs = LOAD_LIMIT_MS): Promise<number> {
		const r = await this.call("load", { model_id: modelId, path, n_ctx: nCtx }, timeoutMs);
		return Number(r.load_ms ?? 0);
	}

	/** The tokens of a text. */
	async count(modelId: string, text: string, timeoutMs = SHORT_LIMIT_MS): Promise<number> {
		const r = await this.call("count", { model_id: modelId, text }, timeoutMs);
		return Number(r.tokens ?? 0);
	}

	async complete(modelId: string, prompt: string, end: string[], maxTokens: number, timeoutMs?: number): Promise<Completion> {
		const r = await this.call("complete", { model_id: modelId, prompt, end, max_tokens: maxTokens }, timeoutMs);
		return { text: String(r.text ?? ""), stop: r.stop as Stop, tokens_in: Number(r.tokens_in ?? 0), tokens_out: Number(r.tokens_out ?? 0), ms: Number(r.ms ?? 0) };
	}

	async unload(timeoutMs = SHORT_LIMIT_MS): Promise<void> {
		await this.call("unload", {}, timeoutMs);
	}

	kill(): void {
		this.#child.kill();
	}

	/** Close its input and wait up to 2 s for the runtime to exit, then kill it. */
	async close(): Promise<void> {
		if (this.#exit !== undefined) return;
		this.#child.stdin.end();
		const timer = setTimeout(() => this.kill(), 2000);
		try {
			await this.#done;
		} finally {
			clearTimeout(timer);
		}
	}

	/** Resolves when the process has exited. */
	get done(): Promise<void> {
		return this.#done;
	}
}

// ---- Finding the runtime ----

/** The best build of the runtime `sayso-cleanup` in `folder` that this processor runs. On x64 a folder can have three builds for three CPU levels. */
function bestRuntime(folder: string, cpu: CpuLevel = detectCpu()): string | null {
	const exe = process.platform === "win32" ? ".exe" : "";
	const x64 = process.arch === "x64";
	const builds: Array<[string, boolean]> = [
		[PROGRAM, !x64 || cpu.avx2],
		[`${PROGRAM}-avx`, x64 && cpu.avx],
		[`${PROGRAM}-baseline`, x64],
	];
	for (const [name, ok] of builds) {
		const path = join(folder, name + exe);
		if (ok && existsSync(path)) return path;
	}
	const fallback = join(folder, PROGRAM + exe);
	return existsSync(fallback) ? fallback : null;
}

/** The runtime of the package `@justsayso/cleanup-<platform>` for this platform. The SDK reads the folder of the package and runs no code of it. */
function runtimeFromPackage(): string {
	const name = `@justsayso/cleanup-${platformKey()}`;
	let manifest: string;
	try {
		manifest = createRequire(import.meta.url).resolve(`${name}/package.json`);
	} catch (e) {
		throw SaysoError.of("unsupported", `the local cleanup needs its runtime: install ${name} (${(e as Error).message})`);
	}
	const root = unpackedPath(dirname(manifest));
	for (const folder of [root, join(root, "llama"), join(root, "bin")]) {
		const runtime = bestRuntime(folder);
		if (runtime) return runtime;
	}
	throw SaysoError.of("unsupported", `no build of the cleanup runtime in ${root} runs on this processor`);
}

/** The local cleanup models: the catalog, the downloads, and the runtime. */
export const cleanup = {
	/** The cleanup models that the SDK can download and run. */
	catalog(): CleanupModel[] {
		return CATALOG.map(publicModel);
	},
	find(id: string): CleanupModel | undefined {
		return cleanup.catalog().find((model) => model.id === id);
	},
	/** True when the model is in `modelsDir` and complete. */
	isReady,
	/** The model file when the model is ready, else null. */
	modelPath,
	download,
	delete: deleteModel,
	bestRuntime,
	runtimeFromPackage,
	runs,
	render,
	planParts,
	joinAnswers,
	check,
	isFillerOnly,
	restoreSpelling,
	dictationLanguage,
	detectLanguage,
	contentUnits,
	maxAnswerTokens,
	files: modelFiles,
};

// ---- The cleaner ----

/**
 * Runs cleanup models in the runtime `sayso-cleanup`, a separate process. It starts at the first cleanup, and stops after a time
 * without use, or with `stop()`.
 */
export class RuntimeCleaner implements Disposable {
	readonly #runtime: string;
	readonly #modelsDir: string;
	#process: Cleanup | undefined;
	#loaded: { id: string; path: string } | undefined;
	#lock: Promise<unknown> = Promise.resolve();
	#lastUse = Date.now();
	#idle: NodeJS.Timeout | undefined;

	/** `runtime`: the program, for example `cleanup.runtimeFromPackage()`. `modelsDir`: the folder of `cleanup.download`. */
	constructor(options: { runtime: string; modelsDir: string }) {
		this.#runtime = options.runtime;
		this.#modelsDir = options.modelsDir;
	}

	isRunning(): boolean {
		return this.#process !== undefined && !this.#process.exited;
	}

	/** Stop the runtime, which frees the memory of the model. The next cleanup starts it again. */
	stop(): void {
		clearInterval(this.#idle);
		this.#idle = undefined;
		const process = this.#process;
		this.#process = undefined;
		this.#loaded = undefined;
		process?.kill();
	}

	/** Run `work` with the process loaded with `id`, one at a time. */
	#exclusive<T>(work: () => Promise<T>): Promise<T> {
		const run = this.#lock.then(work, work);
		this.#lock = run.catch(() => undefined);
		return run;
	}

	#watchIdle(): void {
		this.#lastUse = Date.now();
		if (this.#idle) return;
		this.#idle = setInterval(() => {
			if (Date.now() - this.#lastUse >= IDLE_LIMIT_MS) this.stop();
		}, 30_000);
		this.#idle.unref();
	}

	async #ensure(info: CatalogEntry, path: string): Promise<{ process: Cleanup; loadMs: number }> {
		this.#watchIdle();
		if (!this.#process || this.#process.exited) {
			this.#process = Cleanup.start(this.#runtime);
			this.#loaded = undefined;
		}
		const process = this.#process;
		if (this.#loaded?.id === info.id && this.#loaded.path === path) return { process, loadMs: 0 };
		this.#loaded = undefined;
		try {
			const loadMs = await process.load(info.id, path, N_CTX, LOAD_LIMIT_MS);
			this.#loaded = { id: info.id, path };
			return { process, loadMs };
		} catch (e) {
			if (e instanceof RuntimeError && e.kind === "timeout") this.stop();
			throw e;
		}
	}

	/** Load the model before the text is ready, while the microphone records. A hint: it gives no error. */
	preload(model: string): void {
		const info = entry(model);
		const path = info ? modelPath(this.#modelsDir, model) : null;
		if (!info || !path) return;
		void this.#exclusive(() => this.#ensure(info, path)).catch(() => undefined);
	}

	/** Clean a text. A failure keeps nothing: use the text that went in. */
	clean(request: CleanupRequest): Promise<CleanupResponse> {
		const started = Date.now();
		const info = entry(request.model);
		if (!info) return Promise.reject(new CleanupError("not_configured", `no cleanup model ${request.model}`));
		const path = modelPath(this.#modelsDir, request.model);
		if (!path) return Promise.reject(new CleanupError("not_configured", `${info.name} is not downloaded`));
		return this.#exclusive(async () => {
			try {
				return await this.#clean(info, path, request, started);
			} catch (e) {
				if (e instanceof CleanupError) throw e;
				if (e instanceof RuntimeError) {
					if (e.kind === "timeout") throw new CleanupError("timeout", `timed out after ${request.timeoutMs ?? 0} ms`);
					if (e.kind === "not_installed" || e.kind === "closed") throw new CleanupError("not_configured", e.message);
					throw new CleanupError("runtime", e.message);
				}
				throw new CleanupError("runtime", (e as Error).message);
			}
		});
	}

	async #clean(info: CatalogEntry, path: string, request: CleanupRequest, started: number): Promise<CleanupResponse> {
		let loaded: { process: Cleanup; loadMs: number };
		try {
			loaded = await this.#ensure(info, path);
		} catch (e) {
			if (e instanceof Error && "code" in e && (e as NodeJS.ErrnoException).code === "ENOENT") throw new RuntimeError("not_installed", `the cleanup runtime is not installed: ${this.#runtime}`);
			throw e;
		}
		const { process, loadMs } = loaded;
		// One budget for the whole cleanup, from here: the split, the counts, and the answers.
		const budgetStart = Date.now();
		const timeout = request.timeoutMs ?? 0;
		const planUntil = timeout > 0 ? budgetStart + timeout : undefined;
		const left = (until: number | undefined, cap?: number): number | undefined => {
			const ms = until === undefined ? cap : Math.min(Math.max(until - Date.now(), 1), cap ?? Number.POSITIVE_INFINITY);
			return ms;
		};
		const count = (text: string): Promise<number> => process.count(info.id, text, left(planUntil, SHORT_LIMIT_MS) ?? SHORT_LIMIT_MS);
		const parts = await planParts(request.text, MAX_PART_TOKENS, count);
		const counts: number[] = [];
		for (const part of parts) counts.push(await count(part));
		const total = counts.reduce((a, b) => a + b, 0);
		const deadline = timeout > 0 ? budgetStart + timeout + TIME_PER_TOKEN_MS * total : undefined;
		const answers: string[] = [];
		let tokensIn = 0;
		let tokensOut = 0;
		for (const [i, part] of parts.entries()) {
			const prompt = render(info, part, request.settings ?? {}, request.app, request.site, counts[i] ?? 0);
			const completion = await process.complete(info.id, prompt.text, prompt.end, prompt.maxTokens, left(deadline));
			tokensIn += completion.tokens_in;
			tokensOut += completion.tokens_out;
			// A part that fails fails the whole text: the text after replacements goes in, not a mix of cleaned and not cleaned parts.
			const checked = check(part, completion.stop, completion.text);
			if (!checked.ok) throw new CleanupError("invalid_output", checked.message);
			answers.push(checked.text);
		}
		this.#lastUse = Date.now();
		return { text: joinAnswers(parts, answers), model: info.id, elapsedMs: Date.now() - started, loadMs, parts: parts.length, tokensIn, tokensOut };
	}

	[Symbol.dispose](): void {
		this.stop();
	}
}
