// WAV files: the 16 kHz mono 16-bit file of the protocol, and a reader for the WAV files of apps.

import { closeSync, openSync, writeSync, writeFileSync, readFileSync, ftruncateSync } from "node:fs";
import { basename } from "node:path";
import { SaysoError } from "../errors.js";
import { StreamResampler } from "./resample.js";

export const SAMPLE_RATE = 16000;
export const WAV_HEADER_BYTES = 44;

export const AUDIO_FORMATS = "The formats are WAV, FLAC, MP3, AAC, and M4A.";

/** The header of a 16 kHz mono 16-bit WAV file with this many samples. A WAV file holds at most 4 GiB (37 hours of this audio). */
export function wavHeader(samples: number): Buffer {
	const max = 0xffffffff - 36;
	const data = samples * 2 <= max ? samples * 2 : max;
	const b = Buffer.alloc(WAV_HEADER_BYTES);
	b.write("RIFF", 0, "ascii");
	b.writeUInt32LE(36 + data, 4);
	b.write("WAVEfmt ", 8, "ascii");
	b.writeUInt32LE(16, 16);
	b.writeUInt16LE(1, 20);
	b.writeUInt16LE(1, 22);
	b.writeUInt32LE(SAMPLE_RATE, 24);
	b.writeUInt32LE(SAMPLE_RATE * 2, 28);
	b.writeUInt16LE(2, 32);
	b.writeUInt16LE(16, 34);
	b.write("data", 36, "ascii");
	b.writeUInt32LE(data, 40);
	return b;
}

function pcmBytes(samples: Int16Array): Buffer {
	const b = Buffer.alloc(samples.length * 2);
	for (let i = 0; i < samples.length; i++) b.writeInt16LE(samples[i] as number, i * 2);
	return b;
}

/** A whole 16 kHz mono 16-bit WAV file. */
export function encodeWav(samples: Int16Array): Buffer {
	return Buffer.concat([wavHeader(samples.length), pcmBytes(samples)]);
}

/** Write samples as a WAV file that only this user can read. */
export function writeWav(path: string, samples: Int16Array): void {
	writeFileSync(path, encodeWav(samples), { mode: 0o600 });
}

/** Floats from -1 to 1 as 16-bit PCM with the scale of the protocol (32768), clamped. */
export function floatsToI16(samples: Float32Array): Int16Array {
	const out = new Int16Array(samples.length);
	for (let i = 0; i < samples.length; i++) out[i] = Math.max(-32768, Math.min(32767, Math.round((samples[i] as number) * 32768)));
	return out;
}

export function i16ToFloats(samples: Int16Array): Float32Array {
	const out = new Float32Array(samples.length);
	for (let i = 0; i < samples.length; i++) out[i] = (samples[i] as number) / 32768;
	return out;
}

/**
 * The recording of one source of a live session: a WAV file that grows while the session runs. The sizes in the
 * header are right after `finish`. Writes are synchronous, so the order of the samples is the order of the calls.
 */
export class WavWriter {
	readonly path: string;
	#fd: number;
	#samples = 0;
	#pending: Buffer[] = [];
	#pendingBytes = 0;

	constructor(path: string) {
		this.path = path;
		this.#fd = openSync(path, "w", 0o600);
		writeSync(this.#fd, wavHeader(0));
	}

	/** The number of samples written so far. */
	get samples(): number {
		return this.#samples;
	}

	write(samples: Float32Array): void {
		const b = pcmBytes(floatsToI16(samples));
		this.#pending.push(b);
		this.#pendingBytes += b.length;
		this.#samples += samples.length;
		if (this.#pendingBytes >= 64 * 1024) this.#flush();
	}

	#flush(): void {
		if (this.#pending.length === 0) return;
		writeSync(this.#fd, Buffer.concat(this.#pending));
		this.#pending = [];
		this.#pendingBytes = 0;
	}

	/** Write the sizes in the header and close the file. Gives the number of samples. */
	finish(): number {
		try {
			this.#flush();
			writeSync(this.#fd, wavHeader(this.#samples), 0, WAV_HEADER_BYTES, 0);
		} finally {
			closeSync(this.#fd);
		}
		return this.#samples;
	}

	/** Close the file after an error. */
	abandon(): void {
		try {
			closeSync(this.#fd);
		} catch {
			// Closed already.
		}
	}

	/** Make the file empty again (for a start that failed). */
	truncate(): void {
		try {
			ftruncateSync(this.#fd, 0);
		} catch {
			// The file is gone or closed.
		}
	}
}

/** The samples of a recording of a live session, from -1 to 1. It reads only the format that `WavWriter` writes. */
export function readLiveWav(path: string): Float32Array {
	const b = readFileSync(path);
	const bad = (what: string): Error => new Error(`${path} is not a recording of a live session: ${what}`);
	if (b.length < WAV_HEADER_BYTES || b.toString("ascii", 0, 4) !== "RIFF" || b.toString("ascii", 8, 16) !== "WAVEfmt ") throw bad("no RIFF header");
	if (b.toString("ascii", 36, 40) !== "data") throw bad("no data chunk after the format");
	const size = b.readUInt32LE(40);
	const end = Math.min(WAV_HEADER_BYTES + size, b.length);
	const out = new Float32Array(Math.floor((end - WAV_HEADER_BYTES) / 2));
	for (let i = 0; i < out.length; i++) out[i] = b.readInt16LE(WAV_HEADER_BYTES + i * 2) / 32768;
	return out;
}

/** Audio as 16 kHz mono 16-bit samples. */
export class Audio {
	readonly samples: Int16Array;

	constructor(samples: Int16Array) {
		this.samples = samples;
	}

	/** The length in whole milliseconds. */
	get lenMs(): number {
		return Math.floor(this.samples.length / (SAMPLE_RATE / 1000));
	}

	/** The samples from `startMs` to `endMs` as floats from -1 to 1. */
	floats(startMs: number, endMs: number): Float32Array {
		const perMs = SAMPLE_RATE / 1000;
		const index = (ms: number): number => Math.min(Math.floor(ms * perMs), this.samples.length);
		const s = index(startMs);
		const e = Math.max(index(endMs), s);
		return i16ToFloats(this.samples.subarray(s, e));
	}

	allFloats(): Float32Array {
		return i16ToFloats(this.samples);
	}
}

/** Mixes channels and resamples to 16 kHz. */
export class MonoMixer {
	#out: Int16Array[] = [];
	#length = 0;
	#rate = 0;
	#resampler: StreamResampler | undefined;
	#heard = 0;
	#start = 0;

	push(interleaved: Float32Array, channels: number, rate: number): void {
		channels = Math.max(channels, 1);
		const frames = Math.floor(interleaved.length / channels);
		const mixed = new Float32Array(frames);
		for (let i = 0; i < frames; i++) {
			let sum = 0;
			for (let c = 0; c < channels; c++) sum += interleaved[i * channels + c] as number;
			mixed[i] = sum / channels;
		}
		if (!this.#resampler || this.#rate !== rate) {
			this.#flush();
			this.#rate = rate;
			this.#resampler = new StreamResampler(rate, SAMPLE_RATE);
			this.#heard = 0;
			this.#start = this.#length;
		}
		this.#heard += mixed.length;
		this.#add(this.#resampler.process(mixed));
	}

	#add(floats: Float32Array): void {
		if (floats.length === 0) return;
		const out = new Int16Array(floats.length);
		for (let i = 0; i < floats.length; i++) out[i] = Math.round(Math.max(-1, Math.min(1, floats[i] as number)) * 32767);
		this.#out.push(out);
		this.#length += out.length;
	}

	#flush(): void {
		if (!this.#resampler) return;
		this.#add(this.#resampler.process(new Float32Array(Math.floor(this.#rate / 10))));
		const expected = Math.floor((this.#heard * SAMPLE_RATE) / this.#rate);
		const keep = this.#start + expected;
		if (keep < this.#length) {
			const all = this.#join();
			this.#out = [all.slice(0, keep)];
			this.#length = keep;
		}
		this.#resampler = undefined;
	}

	#join(): Int16Array {
		const all = new Int16Array(this.#length);
		let at = 0;
		for (const chunk of this.#out) {
			all.set(chunk, at);
			at += chunk.length;
		}
		return all;
	}

	finish(): Int16Array {
		this.#flush();
		return this.#join();
	}
}

/** Read WAV bytes as 16 kHz mono audio: PCM of 8, 16, 24, or 32 bits, or float of 32 or 64 bits, with any number of channels. */
export function decodeWav(bytes: Buffer, name: string): Audio {
	const damaged = (why: string): SaysoError => new SaysoError({ message: `${name} is damaged: ${why}` });
	if (bytes.length < 12) throw damaged("it is too short");
	let at = 12;
	let format = 0;
	let channels = 0;
	let rate = 0;
	let bits = 0;
	let haveFormat = false;
	const mixer = new MonoMixer();
	while (at + 8 <= bytes.length) {
		const id = bytes.toString("ascii", at, at + 4);
		let size = bytes.readUInt32LE(at + 4);
		at += 8;
		if (id === "fmt ") {
			if (size < 16 || at + size > bytes.length) throw damaged("bad fmt chunk");
			format = bytes.readUInt16LE(at);
			channels = bytes.readUInt16LE(at + 2);
			rate = bytes.readUInt32LE(at + 4);
			bits = bytes.readUInt16LE(at + 14);
			if (format === 0xfffe && size >= 26) format = bytes.readUInt16LE(at + 24);
			haveFormat = true;
		} else if (id === "data") {
			if (!haveFormat || channels === 0 || rate === 0) throw damaged("no fmt chunk before the data");
			const pcm = format === 1 && [8, 16, 24, 32].includes(bits);
			const float = format === 3 && [32, 64].includes(bits);
			if (!pcm && !float) throw new SaysoError({ message: `cannot decode the audio in ${name} (WAV format ${format} with ${bits} bits). ${AUDIO_FORMATS}` });
			if (size === 0 || size === 0xffffffff || at + size > bytes.length) size = bytes.length - at;
			const bps = bits / 8;
			const frame = bps * channels;
			const end = at + Math.floor(size / frame) * frame;
			const step = 4096 * frame;
			for (let from = at; from < end; from += step) {
				const to = Math.min(from + step, end);
				const floats = new Float32Array((to - from) / bps);
				for (let i = 0, p = from; p < to; i++, p += bps) floats[i] = sampleAt(bytes, p, format, bps);
				mixer.push(floats, channels, rate);
			}
			return new Audio(mixer.finish());
		}
		at += size + (size % 2);
	}
	throw new SaysoError({ message: `${name} has no audio.` });
}

function sampleAt(b: Buffer, p: number, format: number, bps: number): number {
	if (format === 3) return bps === 4 ? b.readFloatLE(p) : b.readDoubleLE(p);
	switch (bps) {
		case 1:
			return ((b[p] as number) - 128) / 128;
		case 2:
			return b.readInt16LE(p) / 32768;
		case 3:
			return b.readIntLE(p, 3) / 8388608;
		default:
			return b.readInt32LE(p) / 2147483648;
	}
}

/** The name of a path for messages. */
export const nameOf = (path: string): string => basename(path);
