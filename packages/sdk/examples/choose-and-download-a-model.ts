import type { Engine, Speech, SpeechOptions } from "@justsayso/sdk";

export async function showModels(speech: Speech): Promise<void> {
	for (const model of await speech.listModels("final")) {
		const chosen = model.choice?.join(", ") ?? "";
		console.log(`${model.id} (${model.name}): ${model.state}, ${Math.round(model.size_bytes / 1_000_000)} MB, chosen for: ${chosen}`);
	}
}

export async function download(speech: Speech, model: string): Promise<void> {
	const stop = speech.on((event) => {
		if (event.kind === "message" && event.message.type === "download_progress") {
			console.error(`${event.message.model}: ${Math.round(event.message.fraction * 100)} %`);
		}
	});
	try {
		await speech.downloadModel(model, "To transcribe your recordings on this computer");
	} finally {
		stop();
	}
}

export function optionsWithModels(engine: Engine): SpeechOptions {
	return {
		mode: "local",
		app: { name: "Notetaker" },
		engine,
		models: { finalModel: "whisper-tiny", vadModel: "basic", language: "en" },
	};
}
