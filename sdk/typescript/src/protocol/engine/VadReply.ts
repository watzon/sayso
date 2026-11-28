import type { SpeechPart } from "../SpeechPart.js";

/**
 * `ok` to `vad`.
 */
export type VadReply = { speech: Array<SpeechPart>, };
