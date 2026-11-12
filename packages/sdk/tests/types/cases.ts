// Checks of the generated protocol types (src/protocol). `wire.ts` holds
// messages that Rust wrote. This file holds values that must not compile.
// Run: pnpm test:types
import type { Capability, ErrorCode, Event, TranscribeFile } from "../../src/protocol/index.js";
import type { Request as EngineRequest } from "../../src/protocol/engine/index.js";

// A name that this version does not know is allowed, and a known name too.
export const knownName: Capability = "models.read";
export const futureName: ErrorCode = "brand_new";

// An optional field may be left out, and may have a value.
export const optionalOmitted: TranscribeFile = { path: "/tmp/a.wav" };
export const optionalSet: TranscribeFile = { path: "/tmp/a.wav", model: "parakeet" };

// The type narrows on `type`.
export function describe(event: Event): string {
    switch (event.type) {
        case "job_done":
            return event.text;
        case "job_failed":
            return event.message;
        default:
            return event.type;
    }
}

// The types of the engine protocol are in the namespace `engine`.
export const engineRequest: EngineRequest = { type: "shutdown" };

// @ts-expect-error `path` is required
export const missingField: TranscribeFile = {};

// @ts-expect-error `nope` is not a `type` of an event
export const unknownType: Event = { type: "nope" };

// @ts-expect-error an optional field is `T` or missing, not `null`
export const nullForOptional: TranscribeFile = { path: "/tmp/a.wav", model: null };

// @ts-expect-error a `u64` is a number, not a bigint
export const bigintForU64: Event = { type: "job_done", job: 5n, text: "" };
