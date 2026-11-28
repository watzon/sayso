
/**
 * The reply to `hello`.
 */
export type Hello = { 
/**
 * The version of the engine.
 */
version: string, protocol: number, 
/**
 * The optional parts of the protocol that this engine has. An engine
 * that sends none is older than the spec.
 */
features: Array<string>, };
