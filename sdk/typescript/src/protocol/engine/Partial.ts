
/**
 * The text of a stream so far. It has all the text of the stream.
 */
export type Partial = { session: number, 
/**
 * Text that does not change any more.
 */
committed: string, 
/**
 * The newest text, which can still change.
 */
tentative: string, };
