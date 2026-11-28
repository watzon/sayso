
/**
 * The state of an optional part of a model, after `prepare`.
 */
export type PartState = { model: string, 
/**
 * `preview` or `dictionary`.
 */
part: string, 
/**
 * `preparing`, `ready`, or `failed`.
 */
state: string, };
