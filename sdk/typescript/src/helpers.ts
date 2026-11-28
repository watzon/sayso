import { SaysoError } from "./errors.js";
import { findAudioHelper, helperError, requireAudioHelper, runHelperSync, spawnHelper, type HelperMessage } from "./audio/helper.js";
import { PROTOCOL } from "./protocol/names.js";

export { findEngine } from "./engine.js";
export { modelStore } from "./paths.js";

/** The version of the Host API of this SDK. */
export function protocolVersion(): number {
	return PROTOCOL;
}

/** The answer of the system about the microphone. */
export type MicrophonePermission = "granted" | "not_determined" | "denied";

function mapState(state: unknown): MicrophonePermission {
	return state === "granted" ? "granted" : state === "denied" ? "denied" : "not_determined";
}

/**
 * The permission of the system to use the microphone. A denied microphone on macOS gives silence, not an error, so ask
 * here before a capture. Windows and Linux tie no permission to the app when there is no audio helper: a refused
 * microphone there is an error of the capture. The permission belongs to the app that runs the helper.
 */
export const microphonePermission = {
	/** The state now. It does not ask the user. */
	status(): MicrophonePermission {
		if (findAudioHelper() === undefined) return process.platform === "darwin" ? "not_determined" : "granted";
		return mapState(runHelperSync(["permission", "mic"], "reading the microphone permission").state);
	},
	/** Show the question of the system when it has not asked yet. Gives true when the microphone is allowed. */
	async request(): Promise<boolean> {
		if (findAudioHelper() === undefined) return process.platform !== "darwin";
		const program = requireAudioHelper("asking for the microphone permission");
		const messages: HelperMessage[] = [];
		const run = spawnHelper(program, ["permission", "--request", "mic"], (m) => messages.push(m));
		run.child.stdin.end();
		await run.exited;
		const failure = messages.find((m) => m.type === "error");
		if (failure) throw helperError(failure);
		return mapState(messages.find((m) => m.type === "permission")?.state) === "granted";
	},
	/** The page of the system settings with this permission, for the app to open. Null when the system has none. */
	settingsUrl(): string | null {
		switch (process.platform) {
			case "darwin":
				return "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone";
			case "win32":
				return "ms-settings:privacy-microphone";
			default:
				return null;
		}
	},
};

/** An input device of this computer. */
export interface Device {
	id: string;
	name: string;
	isDefault: boolean;
}

/** The input devices of this computer, from the audio helper. Throws the error `unsupported` without it. */
export function devices(): Device[] {
	const reply = runHelperSync(["devices"], "listing the input devices");
	const list = Array.isArray(reply.devices) ? (reply.devices as Array<Record<string, unknown>>) : [];
	return list.map((d) => ({ id: String(d.id ?? ""), name: String(d.name ?? ""), isDefault: d.default === true }));
}

export { SaysoError };
