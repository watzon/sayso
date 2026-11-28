import type { JsonValue } from "../serde_json/JsonValue.js";

export type Download = { model: string, engine?: { [key in string]: JsonValue }, 
/**
 * The size of the model files, for an engine that reports only a fraction.
 */
size_bytes?: number, };
