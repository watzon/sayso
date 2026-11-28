import type { ErrorReply } from "./ErrorReply.js";
import type { JsonValue } from "./serde_json/JsonValue.js";

/**
 * The reply to a request: `ok` with the fields of that request, or `error`.
 */
export type Reply = { "type": "ok" } & { [key in string]: JsonValue } | { "type": "error" } & ErrorReply;
