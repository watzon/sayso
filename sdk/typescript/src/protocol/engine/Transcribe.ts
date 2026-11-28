
/**
 * The final pass on a WAV file: 16 kHz, mono, 16-bit PCM.
 */
export type Transcribe = { model: string, path: string, 
/**
 * A language code such as `en`, or `auto`.
 */
language: string, 
/**
 * Words that the model should prefer.
 */
vocabulary: Array<string>, 
/**
 * Ask for `segments` in the reply (feature `segments`).
 */
segments?: boolean, };
