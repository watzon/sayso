import { join } from "node:path";
import { Deepgram, DeepgramFinal, EmbeddedSpeech, Engine, SaysoError, type FinalPassFunction } from "@justsayso/sdk";

export function connectWithDeepgram(dataFolder: string, deepgramKey: string): Promise<EmbeddedSpeech> {
	return EmbeddedSpeech.connect({
		app: { name: "Notetaker" },
		engine: Engine.fromPackage().modelsDir(join(dataFolder, "models")),
		tempDir: join(dataFolder, "tmp"),
		finalPass: new DeepgramFinal(deepgramKey),
	});
}

export async function keyIsValid(key: string): Promise<boolean> {
	try {
		await new Deepgram(key).checkKey();
		return true;
	} catch (error) {
		if (error instanceof SaysoError && error.code === Deepgram.CODE_KEY) return false;
		throw error;
	}
}

declare function sendToMyService(samples: Float32Array, model: string, language: string): Promise<string>;

export const finalPass: FinalPassFunction = {
	models: ["my-service"],
	async transcribe(samples, model, language) {
		return sendToMyService(samples, model, language);
	},
};

export function connectWithFunction(dataFolder: string): Promise<EmbeddedSpeech> {
	return EmbeddedSpeech.connect({
		app: { name: "Notetaker" },
		engine: Engine.fromPackage().modelsDir(join(dataFolder, "models")),
		tempDir: join(dataFolder, "tmp"),
		finalPass,
	});
}

export async function prepareModel(speech: EmbeddedSpeech, id: string): Promise<void> {
	const model = (await speech.models()).find((found) => found.id === id);
	if (!model) throw new SaysoError({ code: "unknown_model", message: `the engine has no model ${id}` });
	if (model.state === "not_downloaded") {
		await speech.download(id, {
			sizeBytes: model.size_bytes,
			onProgress: (progress) => console.error(`${progress.model}: ${Math.round(progress.fraction * 100)} %`),
		});
	}
	await speech.load(id);
}

export async function dictate(speech: EmbeddedSpeech, model: string, ms: number): Promise<string> {
	const session = await speech.startSession({ sources: ["mic"], finalModel: model, language: "en" });
	const stop = speech.on((event) => {
		if (event.type === "partial") console.log(`${event.committed} ${event.tentative}`);
		if (event.type === "segment") console.log(event.text);
	});
	try {
		await new Promise((resolve) => setTimeout(resolve, ms));
		return await speech.stopSession(session);
	} finally {
		stop();
	}
}

export function sendAudio(speech: EmbeddedSpeech, session: number, samples: Float32Array): void {
	speech.sessionAudio(session, samples);
}
