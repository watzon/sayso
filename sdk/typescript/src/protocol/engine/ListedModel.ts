import type { JsonValue } from "../serde_json/JsonValue.js";

/**
 * A model of a `list_models` request: its id, and the `engine` object when
 * the client has one.
 */
export type ListedModel = { id: string, engine?: { [key in string]: JsonValue }, };
