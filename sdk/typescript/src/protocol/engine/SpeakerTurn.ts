
/**
 * A turn of one speaker. In the engine protocol the speaker is an integer
 * from 0, in the order in which the speakers first speak.
 */
export type SpeakerTurn = { start_ms: number, end_ms: number, speaker: number, };
