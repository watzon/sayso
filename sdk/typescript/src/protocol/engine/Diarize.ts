
/**
 * Find who speaks when in a WAV file (feature `diarization`).
 */
export type Diarize = { model: string, path: string, 
/**
 * The number of speakers, when the client knows it.
 */
speakers?: number, };
