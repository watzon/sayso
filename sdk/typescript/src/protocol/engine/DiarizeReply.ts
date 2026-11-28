import type { SpeakerTurn } from "./SpeakerTurn.js";

/**
 * `ok` to `diarize`.
 */
export type DiarizeReply = { speakers: number, turns: Array<SpeakerTurn>, };
