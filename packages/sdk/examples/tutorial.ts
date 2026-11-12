import { resolve } from "node:path";
import { Engine, SaysoError, Speech } from "@justsayso/sdk";

const file = process.argv[2];
const model = process.argv[3] ?? "whisper-tiny";
if (!file) {
	console.error("usage: node scribe.ts <audio file> [model id]");
	process.exit(2);
}

try {
	await using speech = await Speech.connect({
		mode: "auto",
		app: { name: "Scribe" },
		capabilities: ["models.download", "transcribe.file"],
		engine: Engine.fromPackage(),
	});
	console.error("mode:", speech.mode());

	speech.on((event) => {
		if (event.kind !== "message") return;
		const message = event.message;
		if (message.type === "download_progress") console.error(`${message.model}: ${Math.round(message.fraction * 100)} %`);
		if (message.type === "job_progress") console.error(`${Math.round(message.fraction * 100)} %`);
	});

	await speech.downloadModel(model, "To transcribe your files");

	const transcript = await speech.transcribe({ path: resolve(file), model });
	console.log(transcript.text);
	for (const segment of transcript.segments) {
		console.log(`${segment.start_ms} ms to ${segment.end_ms} ms: ${segment.text}`);
	}
} catch (error) {
	console.error(error instanceof SaysoError ? `${error.code ?? "error"}: ${error.message}` : error);
	process.exitCode = 1;
}
