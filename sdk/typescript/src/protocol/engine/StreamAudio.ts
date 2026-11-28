
/**
 * Audio for a stream or a VAD session. It has no `id` and gets no reply.
 */
export type StreamAudio = { session: number, 
/**
 * Base64 of 16 kHz mono little-endian 16-bit PCM (see [`crate::pcm`]).
 */
pcm: string, };
