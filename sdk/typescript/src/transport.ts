// The transport of the Host API: a Unix socket, or a named pipe on Windows, with one JSON message on each line.

import { connect, type Socket } from "node:net";
import { StringDecoder } from "node:string_decoder";
import { SaysoError } from "./errors.js";

/** The longest line that a connection reads. A longer line ends the connection. */
export const MAX_LINE = 16 * 1024 * 1024;

/** A connection with lines in and lines out. */
export interface Transport {
	/** Write one line (without the new line). Throws when the connection is closed. */
	write(line: string): void;
	/** End the connection now. */
	destroy(): void;
	readonly closed: boolean;
}

export interface TransportHandlers {
	onLine(line: string): void;
	onClose(): void;
}

/** Cut a stream of text chunks into lines. */
export class LineSplitter {
	#buffer = "";
	readonly #max: number;

	constructor(max = MAX_LINE) {
		this.#max = max;
	}

	/** The complete lines in `chunk`, without empty ones. Throws when a line is longer than the maximum. */
	push(chunk: string): string[] {
		this.#buffer += chunk;
		const lines: string[] = [];
		let at = this.#buffer.indexOf("\n");
		while (at >= 0) {
			const line = this.#buffer.slice(0, at).trim();
			this.#buffer = this.#buffer.slice(at + 1);
			if (line.length > 0) lines.push(line);
			at = this.#buffer.indexOf("\n");
		}
		if (this.#buffer.length > this.#max) throw new Error("a line is longer than the maximum");
		return lines;
	}

	/** What is left after the last new line. */
	rest(): string {
		return this.#buffer.trim();
	}
}

/** Connect to the socket or pipe at `path`. A host that does not answer is the error `host_not_running`. */
export function dial(path: string, handlers: TransportHandlers): Promise<Transport> {
	return new Promise((resolve, reject) => {
		const socket: Socket = connect({ path });
		const decoder = new StringDecoder("utf8");
		const splitter = new LineSplitter();
		let closed = false;
		let open = false;
		const close = (): void => {
			if (closed) return;
			closed = true;
			if (open) handlers.onClose();
		};
		socket.once("connect", () => {
			open = true;
			socket.removeAllListeners("error");
			socket.on("error", () => socket.destroy());
			resolve({
				write(line: string): void {
					if (closed || socket.destroyed) throw SaysoError.of("connection_lost", "the connection to Sayso ended");
					socket.write(`${line}\n`);
				},
				destroy(): void {
					socket.destroy();
				},
				get closed(): boolean {
					return closed;
				},
			});
		});
		socket.once("error", (error) => {
			if (!open) reject(SaysoError.of("host_not_running", `Sayso does not run (${path}): ${error.message}`));
		});
		socket.on("data", (chunk: Buffer) => {
			let lines: string[];
			try {
				lines = splitter.push(decoder.write(chunk));
			} catch {
				socket.destroy();
				return;
			}
			for (const line of lines) handlers.onLine(line);
		});
		socket.on("close", close);
	});
}
