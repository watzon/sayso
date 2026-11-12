import type { Capability, ErrorCode, ErrorReply } from "./protocol/index.js";

/** An `ErrorReply` of the protocol, as the addon gives it. */
export interface ErrorReplyLike {
	code?: ErrorCode | null;
	message: string;
	capability?: Capability | null;
	reason?: string | null;
}

/**
 * An error of the SDK: the same in both modes. `code` says what to do (`host_not_running`,
 * `not_approved`, `connection_lost`, ...). An error of the addon itself has no code.
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

	/** The `ErrorReply` of this error, as on the wire. */
	toReply(): ErrorReply {
		const reply: ErrorReply = { message: this.message };
		if (this.code !== undefined) reply.code = this.code;
		if (this.capability !== undefined) reply.capability = this.capability;
		if (this.reason !== undefined) reply.reason = this.reason;
		return reply;
	}

	/** The `ErrorReply` of any thrown value, for an answer to Rust. */
	static replyOf(error: unknown): ErrorReply {
		if (error instanceof SaysoError) return error.toReply();
		if (error instanceof Error) return { message: error.message };
		return { message: String(error) };
	}
}
