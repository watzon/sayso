// @justsayso/sdk: the speech features of Sayso for apps in TypeScript and JavaScript. Pure TypeScript with no runtime
// dependencies: Sayso mode is a client of the Host API, and Local mode runs the engine of the app. Capture and the decoding of
// FLAC, MP3, and M4A come from the audio helper `sayso-audio`, when it is there.

export { Speech } from "./speech.js";
export type { ActiveMode, Features, Mode, ModelChoices, SpeechEvent, SpeechOptions, Timing, Transcript } from "./speech.js";
export { Engine, EngineProcess, detectCpu } from "./engine.js";
export type { CpuLevel, EngineMessage, EngineSpec, EngineStartOptions } from "./engine.js";
export { EngineError, EngineStartError, SaysoError } from "./errors.js";
export { AudioWriter, helperSource } from "./capture.js";
export type { AudioSource, HelperSourceOptions, OpenInfo, Opened, WriterEndReason } from "./capture.js";
export { EmbeddedSpeech, DeepgramFinal } from "./embedded.js";
export type { EmbeddedEvent, EmbeddedModel, EmbeddedOptions, EmbeddedSession, FinalPassFunction } from "./embedded.js";
export { Client, appId, launchers, sdkName, startSayso, startWaitMs } from "./client.js";
export type { ClientEvent, ClientOptions, HostEvent, Launcher } from "./client.js";
export { Deepgram } from "./deepgram.js";
export { Cleanup, CleanupError, RuntimeCleaner, RuntimeError, cleanup } from "./cleanup.js";
export type { CleanupFormat, CleanupHello, CleanupLayout, CleanupModel, CleanupProgress, CleanupRequest, CleanupResponse, CleanupSettings, CleanupTone, Completion } from "./cleanup.js";
export { devices, microphonePermission, protocolVersion } from "./helpers.js";
export type { Device, MicrophonePermission } from "./helpers.js";
export { findEngine } from "./engine.js";
export { modelStore, modelsDir, dataDir, clientSocketPath } from "./paths.js";
export { findAudioHelper } from "./audio/helper.js";
export { VERSION } from "./version.js";
export * from "./protocol/index.js";
export * as protocol from "./protocol/index.js";
