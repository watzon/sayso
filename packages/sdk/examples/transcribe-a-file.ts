import { resolve } from "node:path";
import { SaysoError, Speech, type Transcript } from "@justsayso/sdk";

export function transcribeFile(speech: Speech, file: string): Promise<Transcript> {
	return speech.transcribe({ path: resolve(file), language: "en" });
}

export async function transcribeWithProgress(speech: Speech, file: string): Promise<string> {
	const events = speech.events();
	const job = await speech.transcribeFile({ path: resolve(file) });
	for await (const event of events) {
		if (event.kind !== "message") continue;
		const message = event.message;
		if (message.type === "job_progress" && message.job === job) console.error(`${Math.round(message.fraction * 100)} %`);
		if (message.type === "job_done" && message.job === job) return message.text;
		if (message.type === "job_failed" && message.job === job) {
			throw new SaysoError({ code: message.code, message: message.message });
		}
	}
	throw new SaysoError({ code: "connection_lost", message: "the Speech closed before the end of the job" });
}

export function transcribeWithTimeout(speech: Speech, file: string, ms: number): Promise<Transcript> {
	return speech.transcribe({ path: resolve(file) }, { signal: AbortSignal.timeout(ms) });
}

export function cancel(speech: Speech, job: number): Promise<void> {
	return speech.cancelJob(job);
}

export function showWords(transcript: Transcript): void {
	for (const segment of transcript.segments) {
		for (const word of segment.words ?? []) console.log(word.start_ms, word.end_ms, word.text);
	}
}
