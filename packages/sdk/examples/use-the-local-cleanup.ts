import { cleanup, RuntimeCleaner, SaysoError, type CleanupModel } from "@justsayso/sdk";

export function label(model: CleanupModel): string {
	return `${model.name} by ${model.publisher}`;
}

export function showCatalog(): void {
	for (const model of cleanup.catalog()) {
		console.log(`${label(model)}: ${Math.round(model.sizeBytes / 1_000_000)} MB, languages: ${model.languages.join(", ")}`);
	}
}

export async function download(modelsDir: string, id: string, signal: AbortSignal): Promise<string> {
	const ready = cleanup.modelPath(modelsDir, id);
	if (ready) return ready;
	try {
		return await cleanup.download(modelsDir, id, {
			signal,
			onProgress: ({ bytesDone, bytesTotal }) => console.error(`${Math.round((bytesDone / bytesTotal) * 100)} %`),
		});
	} catch (error) {
		if (error instanceof SaysoError && error.code === "declined") console.error("The download stopped. A later call goes on from the partial file.");
		throw error;
	}
}

export async function cleanText(modelsDir: string, text: string): Promise<string> {
	using cleaner = new RuntimeCleaner({ runtime: cleanup.runtimeFromPackage(), modelsDir });
	try {
		const result = await cleaner.clean({ model: "s1-mini", text, settings: { layout: "paragraphs", format: "plain" } });
		return result.text;
	} catch (error) {
		console.error(`The cleanup failed: ${error instanceof Error ? error.message : error}`);
		return text;
	}
}

export function loadWhileRecording(cleaner: RuntimeCleaner): void {
	cleaner.preload("s1-mini");
}
