
/**
 * Start a job. `ok {job}` comes at once.
 */
export type TranscribeFile = { path: string, model?: string, vad_model?: string, language?: string, diarize?: boolean, diarization_model?: string, 
/**
 * The number of speakers, when the app knows it.
 */
speakers?: number, };
