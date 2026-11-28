
/**
 * `ok` to `session_start`.
 */
export type SessionReply = { session: number, 
/**
 * What the capture of the source `system` leaves out: `sayso`, `app`, or
 * both. Empty when the system does not allow it, or without that source.
 */
excludes: Array<string>, };
