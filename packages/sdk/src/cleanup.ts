import { createRequire } from "node:module";
import { unpackedPath } from "./engine.js";
import { SaysoError } from "./errors.js";
import { native, settle, unwrap } from "./native.js";

/**
 * A cleanup model that the SDK can download and run. Show S1-mini as "S1-mini" by
 * "Superwhisper": its license asks for that name.
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

export type CleanupTone = "very-casual" | "casual" | "formal";
export type CleanupLayout = "paragraphs" | "list";
export type CleanupFormat = "plain" | "email";

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

interface NativeCleanup {
	cleanupCatalog(): string;
	cleanupIsReady(modelsDir: string, model: string): string;
	cleanupModelPath(modelsDir: string, model: string): string;
	cleanupDelete(modelsDir: string, model: string): Promise<string>;
	cleanupBestRuntime(folder: string): string | null;
	cleanupDownload(modelsDir: string, model: string, endpoint: string | null, onProgress: ((progress: string) => void) | null): [{ cancel(): void }, Promise<string>];
	NativeCleaner: new (runtime: string, modelsDir: string) => NativeCleanerHandle;
}

interface NativeCleanerHandle {
	clean(request: string): Promise<string>;
	preload(model: string): void;
	isRunning(): boolean;
	stop(): void;
}

function addon(): NativeCleanup {
	return native() as unknown as NativeCleanup;
}

/** The local cleanup models: the catalog, the downloads, and the runtime. */
export const cleanup = {
	/** The cleanup models that the SDK can download and run. */
	catalog(): CleanupModel[] {
		return JSON.parse(addon().cleanupCatalog()) as CleanupModel[];
	},

	find(id: string): CleanupModel | undefined {
		return cleanup.catalog().find((model) => model.id === id);
	},

	/** True when the model is in `modelsDir` and complete. */
	isReady(modelsDir: string, model: string): boolean {
		return unwrap<boolean>(addon().cleanupIsReady(modelsDir, model));
	},

	/** The model file when the model is ready, else null. */
	modelPath(modelsDir: string, model: string): string | null {
		return unwrap<string | null>(addon().cleanupModelPath(modelsDir, model));
	},

	/**
	 * Download the model file and its license files to `<modelsDir>/cleanup/<id>`, and give the
	 * path of the model file. The signal cancels it, with the code `declined`; the partial file
	 * stays for a later resume. The app asks its own user first.
	 */
	async download(modelsDir: string, model: string, options?: { endpoint?: string; signal?: AbortSignal; onProgress?: (progress: CleanupProgress) => void }): Promise<string> {
		const onProgress = options?.onProgress;
		const [handle, promise] = addon().cleanupDownload(modelsDir, model, options?.endpoint ?? null, onProgress ? (text) => onProgress(JSON.parse(text) as CleanupProgress) : null);
		const signal = options?.signal;
		const abort = () => handle.cancel();
		if (signal?.aborted) abort();
		signal?.addEventListener("abort", abort, { once: true });
		// A download holds the event loop of Node until it ends.
		const hold = setInterval(() => {}, 1 << 30);
		try {
			return await settle<string>(promise);
		} finally {
			clearInterval(hold);
			signal?.removeEventListener("abort", abort);
		}
	},

	/** Delete the files of the model. A model that is not on disk gives no error. */
	async delete(modelsDir: string, model: string): Promise<void> {
		await settle<null>(addon().cleanupDelete(modelsDir, model));
	},

	/**
	 * The best build of the runtime `sayso-cleanup` in `folder` that this processor runs. On
	 * x64 a folder can have three builds for three CPU levels.
	 */
	bestRuntime(folder: string): string | null {
		return addon().cleanupBestRuntime(folder);
	},

	/** The runtime of the package `@justsayso/cleanup` for this platform. */
	runtimeFromPackage(): string {
		const require = createRequire(import.meta.url);
		let pkg: { runtimeFolder(): string };
		try {
			pkg = require("@justsayso/cleanup") as { runtimeFolder(): string };
		} catch (e) {
			throw new SaysoError({ code: "unsupported", message: `the local cleanup needs its runtime: install @justsayso/cleanup (${(e as Error).message})` });
		}
		const folder = unpackedPath(pkg.runtimeFolder());
		const runtime = cleanup.bestRuntime(folder);
		if (!runtime) {
			throw new SaysoError({ code: "unsupported", message: `no build of the cleanup runtime in ${folder} runs on this processor` });
		}
		return runtime;
	},
};

/**
 * Runs cleanup models in the runtime `sayso-cleanup`, a separate process. It starts at the
 * first cleanup, and stops after a time without use, or with `stop()`.
 */
export class RuntimeCleaner implements Disposable {
	readonly #native: NativeCleanerHandle;

	/** `runtime`: the program, for example `cleanup.runtimeFromPackage()`. `modelsDir`: the folder of `cleanup.download`. */
	constructor(options: { runtime: string; modelsDir: string }) {
		this.#native = new (addon().NativeCleaner)(options.runtime, options.modelsDir);
	}

	/** Clean a text. A failure keeps nothing: use the text that went in. */
	clean(request: CleanupRequest): Promise<CleanupResponse> {
		return settle<CleanupResponse>(this.#native.clean(JSON.stringify(request)));
	}

	/** Load the model before the text is ready, while the microphone records. A hint: it gives no error. */
	preload(model: string): void {
		this.#native.preload(model);
	}

	isRunning(): boolean {
		return this.#native.isRunning();
	}

	/** Stop the runtime, which frees the memory of the model. The next cleanup starts it again. */
	stop(): void {
		this.#native.stop();
	}

	[Symbol.dispose](): void {
		this.stop();
	}
}
