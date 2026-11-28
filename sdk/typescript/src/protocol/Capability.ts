
/**
 * A permission that an app asks for in `hello` and the user grants in the
 * approval question (`docs/sdk.md`, "Approval").
 */
export type Capability = "models.read" | "models.download" | "transcribe.file" | "session.live" | "audio.mic" | "audio.system" | "ai.enhance" | "dictation.observe" | (string & {});
