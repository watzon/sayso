
/**
 * Run the language model of the system (feature `language_model`).
 */
export type Generate = { instructions: string, prompt: string, timeout_ms: number, temperature?: number, };
