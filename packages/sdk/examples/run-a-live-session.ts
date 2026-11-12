import { setTimeout as sleep } from "node:timers/promises";
import { microphonePermission, type SessionReply, type Speech } from "@justsayso/sdk";

export async function microphoneIsAllowed(): Promise<boolean> {
	if (microphonePermission.status() === "granted") return true;
	return microphonePermission.request();
}

export async function startCall(speech: Speech, folder: string): Promise<SessionReply> {
	const started = await speech.startSession({ sources: ["mic", "system"], record_to: folder });
	if (started.excludes.length === 0) {
		console.error("The capture records the mix of the system, with the sound of this app.");
	}
	return started;
}

function follow(speech: Speech, session: number): { stop(): void; ended: Promise<string> } {
	let onEnded: (text: string) => void = () => {};
	const ended = new Promise<string>((resolve) => {
		onEnded = resolve;
	});
	const stop = speech.on((event) => {
		if (event.kind !== "message") return;
		const message = event.message;
		switch (message.type) {
			case "partial":
				if (message.session === session) console.log(`${message.source}: ${message.committed} ${message.tentative}`);
				break;
			case "segment":
				if (message.session === session) console.log(`${message.source}: ${message.text}`);
				break;
			case "source_gap":
				if (message.session === session) console.error(`${message.source} lost ${message.ms} ms of audio`);
				break;
			case "source_ended":
				if (message.session === session) console.error(`${message.source} ended: ${message.reason}`);
				break;
			case "session_ended":
				if (message.session === session) onEnded(message.text);
				break;
		}
	});
	return { stop, ended };
}

export async function recordFor(speech: Speech, folder: string, ms: number): Promise<string> {
	const { session } = await startCall(speech, folder);
	const { stop, ended } = follow(speech, session);
	try {
		const text = await Promise.race([sleep(ms).then(() => undefined), ended]);
		return text ?? (await speech.stopSession(session));
	} finally {
		stop();
	}
}

export async function sendAudio(speech: Speech, samples: Int16Array): Promise<string> {
	const { session } = await speech.startSession({ sources: ["client"] });
	for (let at = 0; at < samples.length; at += 1_600) {
		speech.sessionAudio(session, samples.subarray(at, at + 1_600));
	}
	speech.sessionAudio(session, new Int16Array(16_000));
	return speech.stopSession(session);
}
