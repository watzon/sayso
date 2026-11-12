import { SaysoError, Speech, type AudioSource, type AudioWriter, type Engine, type OpenInfo } from "@justsayso/sdk";

declare const myCapture: {
	start(
		onSamples: (samples: Float32Array, timeMs: number) => void,
		onFail: (message: string) => void,
		options?: { excludePid?: number },
	): () => void;
};

declare function permissionIsGranted(): Promise<boolean>;

export const microphone: AudioSource = {
	open(writer: AudioWriter) {
		const stop = myCapture.start(
			(samples, timeMs) => {
				writer.push(samples, timeMs);
			},
			(message) => writer.end("failed", message),
		);
		writer.onClose(stop);
	},
};

export const systemAudio: AudioSource = {
	async open(writer: AudioWriter, info: OpenInfo) {
		if (!(await permissionIsGranted())) {
			throw new SaysoError({ code: "permission", message: "The user did not allow the capture of the system audio." });
		}
		const stop = myCapture.start(
			(samples, timeMs) => {
				writer.push(samples, timeMs);
			},
			(message) => writer.end("failed", message),
			{ excludePid: info.appPid },
		);
		writer.onClose(stop);
		return { excludes: info.appPid === undefined ? [] : ["app"] };
	},
};

export function toFloat32(samples: Int16Array): Float32Array {
	return Float32Array.from(samples, (sample) => sample / 32_768);
}

export function connectWithCapture(engine: Engine): Promise<Speech> {
	return Speech.connect({
		mode: "local",
		app: { name: "Notetaker" },
		engine,
		capture: { microphone, systemAudio },
	});
}

export function markGap(writer: AudioWriter, ms: number): void {
	if (!writer.gap(ms)) console.error("The session no longer takes audio.");
}
