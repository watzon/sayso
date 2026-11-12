import type { Speech } from "@justsayso/sdk";

export async function watchMode(speech: Speech, signal: AbortSignal): Promise<void> {
	for await (const event of speech.events({ signal })) {
		switch (event.kind) {
			case "mode_changed":
				if (event.mode === "sayso") {
					console.error(`Sayso runs now. Enhance: ${event.features.enhance}`);
					if (event.features.subscribe) await speech.subscribe(["dictation_done"]);
				} else {
					console.error("Sayso quit. The app uses its own engine.");
				}
				break;
			case "disconnected":
				console.error(`The connection to Sayso ended: ${event.code}`);
				break;
			case "message":
				if (event.message.type === "dictation_done") console.log(`Dictated: ${event.message.text}`);
				break;
		}
	}
}

export async function reconnectWhenAsked(speech: Speech): Promise<void> {
	await speech.reconnect();
	console.error(`Connected again. Mode: ${speech.mode()}`);
}
