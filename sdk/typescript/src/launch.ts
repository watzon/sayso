// Start Sayso: the ways to do it on each system, and a way to run one.

import { spawn } from "node:child_process";
import { existsSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, join } from "node:path";
import { SaysoError } from "./errors.js";

/** The bundle id of Sayso on macOS, and its app id in Flatpak. */
export const APP_ID = "dev.sayso.Sayso";

/** One way to start Sayso: a program and its arguments. */
export interface Launcher {
	program: string;
	args: string[];
	/** Wait for the program to exit with status 0 before the start counts as done (`open` on macOS). */
	wait?: boolean;
}

function isFile(path: string): boolean {
	try {
		return statSync(path).isFile();
	} catch {
		return false;
	}
}

function onPath(name: string): string | undefined {
	const exts = process.platform === "win32" ? (process.env.PATHEXT ?? ".EXE").split(";") : [""];
	for (const dir of (process.env.PATH ?? "").split(delimiter)) {
		for (const ext of exts) {
			const candidate = join(dir, name + ext);
			if (dir !== "" && isFile(candidate)) return candidate;
		}
	}
	return undefined;
}

/**
 * The ways to start Sayso on this system, in the order to try them: LaunchServices on macOS; the install
 * folders on Windows; on Linux the folders of the tarball and the packages, then the Flatpak, then `sayso`
 * on the PATH (only `/app/bin/sayso` inside the Flatpak of Sayso). Empty when Sayso is not installed in a
 * place that these rules know.
 */
export function launchers(): Launcher[] {
	switch (process.platform) {
		case "darwin":
			return [{ program: "/usr/bin/open", args: ["-g", "-b", APP_ID], wait: true }];
		case "win32": {
			const out: Launcher[] = [];
			const local = process.env.LOCALAPPDATA;
			if (local) out.push({ program: join(local, "Programs", "Sayso", "Sayso.exe"), args: [] });
			const files = process.env.ProgramFiles;
			if (files) out.push({ program: join(files, "Sayso", "Sayso.exe"), args: [] });
			return out.filter((l) => isFile(l.program));
		}
		default: {
			if (process.env.FLATPAK_ID === APP_ID) return [{ program: "/app/bin/sayso", args: [] }].filter((l) => isFile(l.program));
			const home = homedir();
			const out: Launcher[] = [join(home, ".local", "lib", "sayso"), "/usr/local/lib/sayso", "/usr/lib/sayso"].map((dir) => ({ program: join(dir, "sayso"), args: [] }));
			const installed = ["/var/lib/flatpak/app", join(home, ".local", "share", "flatpak", "app")].some((dir) => existsSync(join(dir, APP_ID)));
			const flatpak = onPath("flatpak");
			if (installed && flatpak) out.push({ program: flatpak, args: ["run", APP_ID] });
			const sayso = onPath("sayso");
			if (sayso) out.push({ program: sayso, args: [] });
			return out.filter((l) => isFile(l.program));
		}
	}
}

/** Run one launcher: Sayso gets a process of its own, without the input, the output, or the `SAYSO_HOST_SOCKET` of this app. */
function run(launcher: Launcher): Promise<void> {
	return new Promise((resolve, reject) => {
		const { SAYSO_HOST_SOCKET: _socket, ...env } = process.env;
		const child = spawn(launcher.program, launcher.args, { detached: true, stdio: "ignore", env, windowsHide: true });
		child.once("error", reject);
		if (launcher.wait) {
			child.once("exit", (code) => (code === 0 ? resolve() : reject(new Error(`${launcher.program} exited with ${code}`))));
		} else {
			child.once("spawn", () => {
				child.unref();
				resolve();
			});
		}
	});
}

/**
 * Start Sayso with the first launcher that works (default `launchers()`). It does not wait for the host.
 * Fails with `host_not_running` when none works.
 */
export async function startSayso(list: Launcher[] = launchers()): Promise<void> {
	const errors: string[] = [];
	for (const launcher of list) {
		try {
			await run(launcher);
			return;
		} catch (e) {
			errors.push(`${launcher.program}: ${(e as Error).message}`);
		}
	}
	throw SaysoError.of("host_not_running", `cannot start Sayso: ${errors.length > 0 ? errors.join("; ") : "it is not installed in a place that the SDK knows"}`);
}
