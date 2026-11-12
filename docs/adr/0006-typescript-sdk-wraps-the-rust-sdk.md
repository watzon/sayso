# The TypeScript SDK wraps the Rust SDK, and the engines ship as npm packages

Status: accepted on 2026-11-08.

Apps in TypeScript need the same SDK as apps in Rust: the three modes, jobs, live sessions, capture, and the same end rules. Local mode needs native code in the process of the app (`sayso-speech` with VAD, diarization, the resampler, and the file decoder, and `sayso-capture` with cpal and the system audio tap), and the rules of the mode changes are already in `sayso-sdk` with their tests. So `@justsayso/sdk` (MIT) is a native Node addon (napi-rs) around `sayso-sdk`, with a thin TypeScript layer. The engines ship as their own npm packages (GPL-3.0-only), one for each platform, and the SDK does not depend on them. Design: [sdk-typescript.md](../sdk-typescript.md).

## Considered options

- **A client in TypeScript.** Sayso mode is NDJSON over a socket and is easy. Local mode would need jobs, live sessions, VAD, diarization, and capture again, in a second copy that drifts from the Rust one, and capture still needs native code.
- **The engine as WASM.** There is no WASM path for the macOS engine (Swift, WhisperKit and FluidAudio on CoreML and the Neural Engine), so macOS would lose the Neural Engine and need ONNX copies of its models. Elsewhere the engine would run on the CPU only, because Node has no WebGPU. onnxruntime, whisper.cpp, and parakeet.cpp need Emscripten, and large models reach the 4 GB limit of wasm32. The addon stays native for capture, so WASM removes only the engine binary, which is 3 to 26 MB.
- **The engine inside the SDK package.** One install, but an app that uses only Sayso mode downloads an engine that it never starts, and the MIT package carries a GPL program.

## Consequences

- One copy of the rules: a fix in `sayso-sdk` reaches Rust and TypeScript apps in the same release.
- The repository builds five native addons (macOS arm64 and x64, Linux x64 and arm64 with glibc, Windows x64) and publishes them with the engine packages, at the version of the crates.
- An app can install `@justsayso/engine` for local mode, or ship the engine release file as a Rust app does.
- Platforms without a native build (Windows ARM, Linux with musl, FreeBSD) are not supported.
