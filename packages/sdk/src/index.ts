// @justsayso/sdk: the speech features of Sayso for apps in TypeScript and JavaScript
// (docs/sdk-typescript.md). The rules are in the Rust crate sayso-sdk; this package wraps it.

export { Speech } from "./speech.js";
export type { ActiveMode, Features, Mode, ModelChoices, SpeechEvent, SpeechOptions, Timing, Transcript } from "./speech.js";
export { Engine } from "./engine.js";
export { SaysoError } from "./errors.js";
export { AudioWriter } from "./capture.js";
export type { AudioSource, OpenInfo, Opened, WriterEndReason } from "./capture.js";
export { EmbeddedSpeech, DeepgramFinal } from "./embedded.js";
export type { EmbeddedEvent, EmbeddedModel, EmbeddedOptions, EmbeddedSession, FinalPassFunction } from "./embedded.js";
export { Client, appId, launchers, sdkName, startSayso, startWaitMs } from "./client.js";
export type { ClientEvent, ClientOptions, Launcher } from "./client.js";
export { Deepgram } from "./deepgram.js";
export { cleanup, RuntimeCleaner } from "./cleanup.js";
export type { CleanupFormat, CleanupLayout, CleanupModel, CleanupProgress, CleanupRequest, CleanupResponse, CleanupSettings, CleanupTone } from "./cleanup.js";
export { devices, findEngine, microphonePermission, modelStore, protocolVersion } from "./helpers.js";
export type { Device, MicrophonePermission } from "./helpers.js";
export * from "./protocol/index.js";
export * as protocol from "./protocol/index.js";
