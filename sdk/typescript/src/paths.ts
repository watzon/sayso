// The rules of sdk/protocol/paths.md: the socket or pipe of the host, the data folder, the models folder,
// and the model store.

import { execFileSync } from "node:child_process";
import { homedir } from "node:os";
import { join } from "node:path";
import { SaysoError } from "./errors.js";

/** The environment variable with the path of the socket. */
export const SOCKET_ENV_VAR = "SAYSO_HOST_SOCKET";
export const SOCKET_FILE = "host.sock";
export const MODELS_DIR_ENV = "SAYSO_MODELS_DIR";
export const MODEL_STORE_ENV = "SAYSO_MODEL_STORE";
export const MODEL_STORE_FOLDER = "v1";

/** A system of the socket rules. */
export type System = "linux" | "macos" | "windows" | "other";

/** What the socket rules read from the system. */
export interface SocketEnv {
	/** An environment variable; "" is not set. */
	var(key: string): string;
	/** `confstr(_CS_DARWIN_USER_TEMP_DIR)` without its last `/` on macOS, or "". */
	darwinUserTempDir(): string;
	/** The Unix user id, or undefined where there is none. */
	uid(): number | undefined;
	/** The Windows SID of the user, or "". */
	userSid(): string;
}

/** The system that this process runs on. */
export function currentSystem(): System {
	switch (process.platform) {
		case "linux":
			return "linux";
		case "darwin":
			return "macos";
		case "win32":
			return "windows";
		default:
			return "other";
	}
}

let darwinTemp: string | undefined;
let windowsSid: string | undefined;

function run(program: string, args: string[]): string {
	try {
		return execFileSync(program, args, { encoding: "utf8", timeout: 5000, stdio: ["ignore", "pipe", "ignore"], windowsHide: true });
	} catch {
		return "";
	}
}

/**
 * The macOS user temp folder. `os.tmpdir()` follows `TMPDIR`, which a parent process can change,
 * so this asks `getconf`, as the host does with `confstr`. The answer is kept.
 */
function darwinUserTempDir(): string {
	darwinTemp ??= run("/usr/bin/getconf", ["DARWIN_USER_TEMP_DIR"]).trim().replace(/\/+$/, "");
	if (darwinTemp === "") darwinTemp = (process.env.TMPDIR ?? "").replace(/\/+$/, "");
	return darwinTemp;
}

/** The SID of the user from `whoami /user /fo csv /nh`, which prints `"DOMAIN\user","S-1-5-..."`. The answer is kept. */
export function parseWhoamiSid(output: string): string {
	const match = /"(S-1-[0-9-]+)"/.exec(output);
	return match?.[1] ?? "";
}

function userSid(): string {
	windowsSid ??= parseWhoamiSid(run("whoami", ["/user", "/fo", "csv", "/nh"]));
	return windowsSid;
}

/** The real environment. */
export const systemSocketEnv: SocketEnv = {
	var: (key) => process.env[key] ?? "",
	darwinUserTempDir: () => (process.platform === "darwin" ? darwinUserTempDir() : ""),
	uid: () => (typeof process.getuid === "function" ? process.getuid() : undefined),
	userSid: () => (process.platform === "win32" ? userSid() : ""),
};

function joinUnix(...parts: string[]): string {
	const [first = "", ...rest] = parts;
	return [first.replace(/\/+$/, ""), ...rest].join("/");
}

/** Where the host of this user binds its socket (the pipe name on Windows), by the rules of `system`. */
export function hostSocketPathFor(system: System, env: SocketEnv): string {
	const fixed = env.var(SOCKET_ENV_VAR);
	if (fixed !== "") return fixed;
	const inTmp = (): string => {
		const uid = env.uid();
		if (uid === undefined) throw SaysoError.of("host_not_running", "cannot find the user id");
		return `/tmp/sayso-${uid}/${SOCKET_FILE}`;
	};
	switch (system) {
		case "linux": {
			const dir = env.var("XDG_RUNTIME_DIR");
			if (dir === "") return inTmp();
			const app = env.var("FLATPAK_ID");
			return app !== "" ? joinUnix(dir, "app", app, SOCKET_FILE) : joinUnix(dir, "sayso", SOCKET_FILE);
		}
		case "macos": {
			const temp = env.darwinUserTempDir();
			return temp !== "" ? joinUnix(temp, "sayso", SOCKET_FILE) : inTmp();
		}
		case "windows": {
			const sid = env.userSid();
			if (sid === "") throw SaysoError.of("host_not_running", "cannot find the SID of the user");
			return `\\\\.\\pipe\\sayso-host-${sid}`;
		}
		default:
			return inTmp();
	}
}

/**
 * Where a client of this user looks for the host. It is the host path, except on Linux with
 * `XDG_RUNTIME_DIR` and without `SAYSO_HOST_SOCKET`: a client looks outside a Flatpak sandbox.
 */
export function clientSocketPathFor(system: System, env: SocketEnv): string {
	if (system === "linux" && env.var(SOCKET_ENV_VAR) === "") {
		const dir = env.var("XDG_RUNTIME_DIR");
		if (dir !== "") return joinUnix(dir, "sayso", SOCKET_FILE);
	}
	return hostSocketPathFor(system, env);
}

/** The socket or pipe where a client of this system looks for Sayso. */
export function clientSocketPath(): string {
	return clientSocketPathFor(currentSystem(), systemSocketEnv);
}

/** What the data folder rules read. */
export interface DataEnv {
	var(key: string): string;
	home(): string;
}

export const systemDataEnv: DataEnv = {
	var: (key) => process.env[key] ?? "",
	home: () => homedir() || "/",
};

/** The data folder of Sayso on `platform` (a `process.platform` name). It joins with the separator of the running system. */
export function dataDirFor(platform: string, env: DataEnv): string {
	const xdg = env.var("XDG_DATA_HOME");
	if (xdg !== "") return join(xdg, "sayso");
	const home = env.home();
	switch (platform) {
		case "darwin":
			return join(home, "Library", "Application Support", "Sayso");
		case "win32": {
			const local = env.var("LOCALAPPDATA") || join(home, "AppData", "Local");
			return join(local, "Sayso");
		}
		default:
			return join(home, ".local", "share", "sayso");
	}
}

/** The data folder of Sayso on this system. */
export function dataDir(): string {
	return dataDirFor(process.platform, systemDataEnv);
}

/** The models folder in a data folder (`SAYSO_MODELS_DIR`). */
export function modelsDirOf(data: string): string {
	return join(data, "models");
}

/** The model store in a models folder (`SAYSO_MODEL_STORE`). */
export function modelStoreOf(models: string): string {
	return join(models, MODEL_STORE_FOLDER);
}

/** The folder with a folder for each cleanup model, in a models folder. */
export function cleanupModelsDirOf(models: string): string {
	return join(models, "cleanup");
}

/** The models folder of Sayso on this system, which Local mode shares. */
export function modelsDir(): string {
	return modelsDirOf(dataDir());
}

/** The shared model store of this user: Sayso and the apps of the SDK keep their models there. */
export function modelStore(): string {
	return modelStoreOf(modelsDir());
}
