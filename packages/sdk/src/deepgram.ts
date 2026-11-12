import { native, settle, type NativeDeepgram } from "./native.js";

interface Constants {
	model: string;
	codeConnection: string;
	codeKey: string;
}

let constants: Constants | undefined;
function deepgramConstants(): Constants {
	constants ??= JSON.parse(native().deepgramConstants()) as Constants;
	return constants;
}

/**
 * A small client of the HTTP API of Deepgram. It keeps the key in memory only, and never
 * writes it in a log. For the final pass of a session, use `DeepgramFinal` with `EmbeddedSpeech`.
 */
export class Deepgram {
	/** The model id of the final pass with Deepgram. */
	static get MODEL(): string {
		return deepgramConstants().model;
	}

	/** The `code` of an error when Deepgram cannot be reached. */
	static get CODE_CONNECTION(): string {
		return deepgramConstants().codeConnection;
	}

	/** The `code` of an error when Deepgram refuses the key. */
	static get CODE_KEY(): string {
		return deepgramConstants().codeKey;
	}

	readonly #native: NativeDeepgram;

	/** `baseUrl`: another server, for tests. */
	constructor(key: string, options?: { baseUrl?: string }) {
		this.#native = new (native().NativeDeepgram)(key, options?.baseUrl ?? null);
	}

	/** Ask Deepgram if it knows the key. A refused key rejects with the code `CODE_KEY`. */
	async checkKey(): Promise<void> {
		await settle<null>(this.#native.checkKey());
	}

	/** The text of 16 kHz mono samples from -1 to 1. `language` is a code such as `en`, or `auto`. */
	transcribe(samples: Float32Array, language: string): Promise<string> {
		return settle<string>(this.#native.transcribe(samples, language));
	}
}
