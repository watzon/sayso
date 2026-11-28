import type { ErrorReply } from "../ErrorReply.js";
import type { JsonValue } from "../serde_json/JsonValue.js";
import type { Final } from "./Final.js";
import type { Hello } from "./Hello.js";

/**
 * A reply of the engine. Each request with an `id` gets exactly one.
 */
export type Reply = { "type": "hello" } & Hello | { "type": "ok" } & { [key in string]: JsonValue } | { "type": "final" } & Final | { "type": "error" } & ErrorReply;
