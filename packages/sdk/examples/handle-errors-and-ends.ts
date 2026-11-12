import { SaysoError, Speech, type SpeechOptions } from "@justsayso/sdk";

export function explain(error: unknown): string {
	if (!(error instanceof SaysoError)) return error instanceof Error ? error.message : String(error);
	switch (error.code) {
		case "not_approved":
			return "The user did not allow this app in Sayso. Tell them where to change it: Settings > Connected apps.";
		case "capability":
			return `The user did not grant ${error.capability ?? "a capability"}.`;
		case "busy":
			return "Sayso is busy. Try again later.";
		case "host_not_running":
			return "Sayso does not run.";
		default:
			return error.message;
	}
}

export async function stopAndGetText(speech: Speech, session: number): Promise<string> {
	try {
		return await speech.stopSession(session);
	} catch (error) {
		if (error instanceof SaysoError && (error.code === "connection_lost" || error.code === "host_closing")) {
			console.error(`The session ended with the connection: ${error.message}`);
		}
		throw error;
	}
}

export function logErrorsOfNoRequest(speech: Speech): () => void {
	return speech.on((event) => {
		if (event.kind === "error") console.error(`${event.error.code ?? "error"}: ${event.error.message}`);
	});
}

export async function withSpeech(options: SpeechOptions, work: (speech: Speech) => Promise<void>): Promise<void> {
	await using speech = await Speech.connect(options);
	await work(speech);
}

export async function withSpeechAndFinally(options: SpeechOptions, work: (speech: Speech) => Promise<void>): Promise<void> {
	const speech = await Speech.connect(options);
	try {
		await work(speech);
	} finally {
		await speech.close();
	}
}
