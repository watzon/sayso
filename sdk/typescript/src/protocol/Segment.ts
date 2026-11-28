import type { Word } from "./Word.js";

/**
 * A part of a transcript with its times. `words` is there when the model
 * has the timestamp level `word`.
 */
export type Segment = { start_ms: number, end_ms: number, text: string, words?: Array<Word>, };
