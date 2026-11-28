import type { Capability, ErrorCode, ErrorReply } from "./protocol/index.js";

/** An `ErrorReply` of the protocol, or anything that has its fields. */
export interface ErrorReplyLike {
	code?: ErrorCode | null;
	message: string;
	capability?: Capability | null;
	reason?: string | null;
}

/**
 * An error of the SDK, the same in every mode. `code` says what to do (`host_not_running`,
 * `not_approved`, `connection_lost`, ...); a code that this version does not know is kept as
 * it came. An error with no code has only a message.
 */
export class SaysoError extends Error {
	readonly code: ErrorCode | undefined;
	readonly capability: Capability | undefined;
	readonly reason: string | undefined;

	constructor(reply: ErrorReplyLike) {
		super(reply.message);
		this.name = "SaysoError";
		this.code = reply.code ?? undefined;
		this.capability = reply.capability ?? undefined;
		this.reason = reply.reason ?? undefined;
	}

	/** An error with a code and a message. */
	static of(code: ErrorCode, message: string): SaysoError {
		return new SaysoError({ code, message });
	}

	/** The `ErrorReply` of this error, as on the wire. */
	toReply(): ErrorReply {
		const reply: ErrorReply = { message: this.message };
		if (this.code !== undefined) reply.code = this.code;
		if (this.capability !== undefined) reply.capability = this.capability;
		if (this.reason !== undefined) reply.reason = this.reason;
		return reply;
	}

	/** The `ErrorReply` of any thrown value. */
	static replyOf(error: unknown): ErrorReply {
		if (error instanceof SaysoError) return error.toReply();
		if (error instanceof Error) return { message: error.message };
		return { message: String(error) };
	}

	/** The error as a `SaysoError`: the same object, or a message-only error. */
	static from(error: unknown): SaysoError {
		return error instanceof SaysoError ? error : new SaysoError(SaysoError.replyOf(error));
	}
}

/** An engine that did not start or did not answer `hello`. `stderr` has the last lines that it wrote. */
export class EngineStartError extends SaysoError {
	readonly program: string;
	readonly stderr: string[];

	constructor(program: string, why: string, stderr: string[]) {
		let message = `the engine ${program} did not start: ${why}`;
		if (stderr.length > 0) message += `\nits last lines on stderr:\n  ${stderr.join("\n  ")}`;
		super({ message });
		this.name = "EngineStartError";
		this.program = program;
		this.stderr = stderr;
	}
}

/** An error reply of the engine protocol. */
export class EngineError extends SaysoError {
	constructor(reply: ErrorReplyLike) {
		super(reply);
		this.name = "EngineError";
	}
}
