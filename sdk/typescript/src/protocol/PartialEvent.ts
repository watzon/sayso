import type { Source } from "./Source.js";

/**
 * The text of one source of a live session so far.
 */
export type PartialEvent = { session: number, source?: Source, 
/**
 * Text that does not change any more.
 */
committed: string, 
/**
 * The newest text, which can still change.
 */
tentative: string, };
