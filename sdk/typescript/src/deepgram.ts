import { encodeWav, floatsToI16 } from "./audio/wav.js";
import { SaysoError } from "./errors.js";
import type { LocalEngine, LocalEngineEvent, RawTurn } from "./local/engine.js";
import type { Segment, SpeechPart } from "./protocol/index.js";

const MODEL_ID = "deepgram-nova-3";
const API_MODEL = "nova-3";
const BASE_URL = "https://api.deepgram.com/v1";
const TIMEOUT_MS = 30_000;
const CODE_CONNECTION = "cloud_connection";
const CODE_KEY = "cloud_key";

/**
 * A small client of the HTTP API of Deepgram. It keeps the key in memory only, and never writes it in a log. For the final pass of a
 * session, use `DeepgramFinal` with `EmbeddedSpeech`.
 */
export class Deepgram {
	/** The model id of the final pass with Deepgram. */
	static get MODEL(): string {
		return MODEL_ID;
	}

	/** The `code` of an error when Deepgram cannot be reached. */
	static get CODE_CONNECTION(): string {
		return CODE_CONNECTION;
	}

	/** The `code` of an error when Deepgram refuses the key. */
	static get CODE_KEY(): string {
		return CODE_KEY;
	}

	readonly #key: string;
	readonly #baseUrl: string;

	/** `baseUrl`: another server, for tests. */
	constructor(key: string, options?: { baseUrl?: string }) {
		this.#key = key;
		this.#baseUrl = (options?.baseUrl ?? BASE_URL).replace(/\/+$/, "");
	}

	toJSON(): { baseUrl: string } {
		return { baseUrl: this.#baseUrl };
	}

	/** Ask Deepgram if it knows the key. A refused key rejects with the code `CODE_KEY`. */
	async checkKey(): Promise<void> {
		const response = await this.#fetch(`${this.#baseUrl}/projects`, { method: "GET" });
		await checkStatus(response);
	}

	/** The text of 16 kHz mono samples from -1 to 1. `language` is a code such as `en`, or `auto`. */
	async transcribe(samples: Float32Array, language: string): Promise<string> {
		const wav = encodeWav(floatsToI16(samples));
		const response = await this.#fetch(this.#listenUrl(language), { method: "POST", headers: { "Content-Type": "audio/wav" }, body: new Uint8Array(wav) });
		const body = await checkStatus(response);
		let json: unknown;
		try {
			json = JSON.parse(body);
		} catch (e) {
			throw new SaysoError({ message: `Deepgram sent a reply that is not JSON: ${(e as Error).message}` });
		}
		const transcript = (json as { results?: { channels?: Array<{ alternatives?: Array<{ transcript?: unknown }> }> } }).results?.channels?.[0]?.alternatives?.[0]?.transcript;
		if (typeof transcript !== "string") throw new SaysoError({ message: "Deepgram sent a reply with no transcript" });
		return transcript.trim();
	}

	#listenUrl(language: string): string {
		let url = `${this.#baseUrl}/listen?model=${API_MODEL}&smart_format=true&punctuate=true`;
		if (language === "" || language === "auto") url += "&detect_language=true";
		else url += `&language=${[...language].filter((c) => /[A-Za-z0-9-]/.test(c)).join("")}`;
		return url;
	}

	async #fetch(url: string, init: { method: string; headers?: Record<string, string>; body?: Uint8Array }): Promise<Response> {
		try {
			return await fetch(url, { ...init, headers: { Authorization: `Token ${this.#key}`, ...init.headers }, signal: AbortSignal.timeout(TIMEOUT_MS) });
		} catch (e) {
			throw new SaysoError({ code: CODE_CONNECTION, message: `cannot reach Deepgram: ${(e as Error).message}` });
		}
	}
}

async function checkStatus(response: Response): Promise<string> {
	const status = response.status;
	const body = await response.text().catch(() => "");
	if (status >= 200 && status <= 299) return body;
	if (status === 401 || status === 403) throw new SaysoError({ code: CODE_KEY, message: `Deepgram refused the key (HTTP ${status})` });
	if (status === 408 || status === 429 || (status >= 500 && status <= 599)) throw new SaysoError({ code: CODE_CONNECTION, message: `Deepgram answered HTTP ${status}` });
	throw new SaysoError({ message: `Deepgram answered HTTP ${status}: ${body.slice(0, 200)}` });
}

/** Deepgram for the final pass of the model `deepgram-nova-3`. The key comes from the user. */
export class DeepgramFinal {
	readonly key: string;
	readonly baseUrl: string | undefined;

	constructor(key: string, options?: { baseUrl?: string }) {
		this.key = key;
		this.baseUrl = options?.baseUrl;
	}
}

/** The final pass of the app: a function that gives the text of a part of the audio for one of `models`. The other models stay with the engine. */
export interface FinalPassFunction {
	models: string[];
	transcribe(samples: Float32Array, model: string, language: string): Promise<string> | string;
}

/** An engine whose final pass for some models comes from elsewhere. Everything else is the engine below. */
export class FinalPassEngine implements LocalEngine {
	readonly #local: LocalEngine;
	readonly #handles: (model: string) => boolean;
	readonly #pass: (samples: Float32Array, model: string, language: string) => Promise<string>;

	constructor(local: LocalEngine, handles: (model: string) => boolean, pass: (samples: Float32Array, model: string, language: string) => Promise<string>) {
		this.#local = local;
		this.#handles = handles;
		this.#pass = pass;
	}

	features = (): string[] => this.#local.features();
	isUsable = (model: string): boolean => this.#handles(model) || this.#local.isUsable(model);
	load = (model: string): Promise<void> => (this.#handles(model) ? Promise.resolve() : this.#local.load(model));
	transcribe = (samples: Float32Array, model: string, language: string): Promise<string> => (this.#handles(model) ? this.#pass(samples, model, language) : this.#local.transcribe(samples, model, language));

	async transcribeSegments(samples: Float32Array, model: string, language: string): Promise<Segment[]> {
		if (!this.#handles(model)) return this.#local.transcribeSegments(samples, model, language);
		const text = await this.#pass(samples, model, language);
		return [{ start_ms: 0, end_ms: Math.floor((samples.length * 1000) / 16000), text }];
	}

	detectSpeech = (model: string, samples: Float32Array): Promise<SpeechPart[]> => this.#local.detectSpeech(model, samples);
	diarize = (model: string, samples: Float32Array, speakers?: number): Promise<RawTurn[]> => this.#local.diarize(model, samples, speakers);
	startStream = (session: number, model: string, language: string): Promise<void> => this.#local.startStream(session, model, language);
	startVad = (session: number, model: string): Promise<void> => this.#local.startVad(session, model);
	pushAudio = (session: number, samples: Float32Array): void => this.#local.pushAudio(session, samples);
	endStream = (session: number): Promise<void> => this.#local.endStream(session);
	endVad = (session: number): Promise<void> => this.#local.endVad(session);
	onEvent = (listener: (event: LocalEngineEvent) => void): (() => void) => this.#local.onEvent(listener);
}
