import { native, unwrap } from "./native.js";

/** The engine program in the folder of an engine release file. Throws a `SaysoError` when it is not there. */
export function findEngine(folder: string): string {
	return unwrap<string>(native().findEngine(folder));
}

/** The shared model store of this user: Sayso and the apps of the SDK keep their models there. */
export function modelStore(): string {
	return native().modelStore();
}

/** The answer of the system about the microphone. */
export type MicrophonePermission = "granted" | "not_determined" | "denied";

/**
 * The permission of the system to use the microphone. A denied microphone on macOS gives
 * silence, not an error, so ask here before a capture. Windows and Linux tie no permission
 * to the app: a refused microphone there is an error of the capture.
 */
export const microphonePermission = {
	/** The state now. It does not ask the user. */
	status(): MicrophonePermission {
		return native().microphonePermissionStatus() as MicrophonePermission;
	},
	/**
	 * Show the question of the system when it has not asked yet. A process with no
	 * `NSMicrophoneUsageDescription` in its `Info.plist` (a script in a terminal, for example)
	 * gets no question, and the answer false.
	 */
	request(): Promise<boolean> {
		return native().microphonePermissionRequest();
	},
	/** The page of the system settings with this permission, for the app to open. Null when the system has none. */
	settingsUrl(): string | null {
		return native().microphoneSettingsUrl();
	},
};

/** An input device of this computer. */
export interface Device {
	id: string;
	name: string;
	isDefault: boolean;
}

/** The input devices of this computer. */
export function devices(): Device[] {
	return JSON.parse(native().devices()) as Device[];
}

/** The version of the Host API of this SDK. */
export function protocolVersion(): number {
	return native().protocolVersion();
}
