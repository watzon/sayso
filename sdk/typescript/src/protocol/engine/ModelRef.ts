import type { JsonValue } from "../serde_json/JsonValue.js";

/**
 * A model for `delete` and `load`. Without `engine`, the engine takes the
 * `engine` object of the id from its catalog.
 */
export type ModelRef = { model: string, engine?: { [key in string]: JsonValue }, };
