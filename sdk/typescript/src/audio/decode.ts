import { readFileSync } from "node:fs";
import { SaysoError } from "../errors.js";
import { AUDIO_FORMATS, Audio, decodeWav, nameOf } from "./wav.js";
import { helperError, helperPcm, requireAudioHelper, spawnHelper, type HelperMessage } from "./helper.js";

function isMp3(head: Buffer): boolean {
	if (head.toString("ascii", 0, 3) === "ID3") return true;
	return head.length >= 2 && head[0] === 0xff && ((head[1] as number) & 0xe0) === 0xe0 && ((head[1] as number) & 0x06) !== 0;
}

/** What a file is, by its first bytes. */
export type AudioKind = "wav" | "flac" | "mp3" | "m4a" | "unknown";

export function kindOf(head: Buffer): AudioKind {
	if (head.length >= 12 && head.toString("ascii", 0, 4) === "RIFF" && head.toString("ascii", 8, 12) === "WAVE") return "wav";
	if (head.length >= 4 && head.toString("ascii", 0, 4) === "fLaC") return "flac";
	if (head.length >= 8 && head.toString("ascii", 4, 8) === "ftyp") return "m4a";
	if (head.length >= 2 && head[0] === 0xff && ((head[1] as number) & 0xf6) === 0xf0) return "m4a";
	if (head.length >= 3 && isMp3(head)) return "mp3";
	return "unknown";
}

/** Decode a file with the audio helper: it gives 16 kHz mono PCM. */
export function decodeWithHelper(path: string, helper = requireAudioHelper("decoding FLAC, MP3, AAC, and M4A")): Promise<Audio> {
	return new Promise((resolve, reject) => {
		const chunks: Int16Array[] = [];
		let length = 0;
		let failure: SaysoError | undefined;
		const process = spawnHelper(helper, ["decode", path], (message: HelperMessage) => {
			if (message.type === "audio") {
				try {
					const pcm = helperPcm(message);
					chunks.push(pcm);
					length += pcm.length;
				} catch (e) {
					failure ??= SaysoError.from(e);
				}
			} else if (message.type === "error") {
				failure ??= helperError(message);
			}
		});
		process.child.stdin.end();
		void process.exited.then((code) => {
			if (failure) return reject(failure);
			if (code !== 0) return reject(new SaysoError({ message: `cannot decode the audio in ${nameOf(path)}: the audio helper exited with ${code}${process.stderr().length > 0 ? ` (${process.stderr().at(-1)})` : ""}` }));
			const all = new Int16Array(length);
			let at = 0;
			for (const chunk of chunks) {
				all.set(chunk, at);
				at += chunk.length;
			}
			resolve(new Audio(all));
		});
	});
}

/**
 * Read an audio file as 16 kHz mono 16-bit audio. WAV is read here; FLAC, MP3, AAC, and M4A go through the audio
 * helper `sayso-audio`, so they need it.
 */
export async function readAudioFile(path: string): Promise<Audio> {
	const name = nameOf(path);
	let bytes: Buffer;
	try {
		bytes = readFileSync(path);
	} catch (e) {
		throw new SaysoError({ message: `cannot open ${path}: ${(e as Error).message}` });
	}
	switch (kindOf(bytes.subarray(0, 12))) {
		case "wav":
			return decodeWav(bytes, name);
		case "flac":
		case "mp3":
		case "m4a":
			return decodeWithHelper(path);
		default:
			throw new SaysoError({ message: `${name} is not an audio file that can be read. ${AUDIO_FORMATS}` });
	}
}
