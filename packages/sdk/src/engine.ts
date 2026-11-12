import { createRequire } from "node:module";
import { sep } from "node:path";
import { SaysoError } from "./errors.js";

/** The engine as the addon reads it. */
export interface EngineSpec {
	release?: string;
	program?: string;
	store?: string;
	modelsDir?: string;
}

/**
 * The engine that the app ships, for local mode. The SDK never uses the engine inside an
 * installed Sayso, because an update of Sayso would then change the engine of the app.
 * The methods give a new `Engine`.
 */
export class Engine {
	readonly #spec: EngineSpec;

	private constructor(spec: EngineSpec) {
		this.#spec = spec;
	}

	/** The folder where the app unpacked the release file of the engine. */
	static fromRelease(folder: string): Engine {
		return new Engine({ release: folder });
	}

	/** The engine program itself. */
	static fromProgram(program: string): Engine {
		return new Engine({ program });
	}

	/**
	 * The engine of the package `@justsayso/engine` for this platform. In a packaged
	 * Electron app, the folder in `app.asar.unpacked`, because a program inside an asar
	 * file cannot start. Unpack the engine with the `asarUnpack` rule of your packager.
	 */
	static fromPackage(): Engine {
		const require = createRequire(import.meta.url);
		let engine: { releaseFolder(): string };
		try {
			engine = require("@justsayso/engine") as { releaseFolder(): string };
		} catch (e) {
			throw new SaysoError({
				code: "unsupported",
				message: `local mode needs the engine: install @justsayso/engine (${(e as Error).message})`,
			});
		}
		let folder: string;
		try {
			folder = engine.releaseFolder();
		} catch (e) {
			throw new SaysoError({ code: "unsupported", message: (e as Error).message });
		}
		return Engine.fromRelease(unpackedPath(folder));
	}

	/** Use another model store than `modelStore()`. */
	store(folder: string): Engine {
		return new Engine({ ...this.#spec, store: folder });
	}

	/**
	 * The folder for the models of an engine without the store, and for a model without a
	 * descriptor. The default is a folder with the name of the app in the data folder of the system.
	 */
	modelsDir(folder: string): Engine {
		return new Engine({ ...this.#spec, modelsDir: folder });
	}

	toJSON(): EngineSpec {
		return { ...this.#spec };
	}
}

/** `app.asar` in a path becomes `app.asar.unpacked`, in Electron only. */
export function unpackedPath(path: string, electron = process.versions.electron !== undefined): string {
	if (!electron) return path;
	const packed = `${sep}app.asar${sep}`;
	return path.includes(packed) ? path.replace(packed, `${sep}app.asar.unpacked${sep}`) : path;
}
