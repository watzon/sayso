# Extensions are separate processes, and Sayso is their host

Sayso stays a dictation app, so meeting notes, media transcription, and the work of other developers are extensions. An extension is a separate app that talks to Sayso over a local socket (the Host API). Sayso owns the engine, the audio capture, the permissions of the system, the models, the API keys, and the scheduling. The extension owns its UI and its data. The engine keeps one owner and does not become a shared service. An app that does not want Sayso starts its own engine. Design: [extensions.md](../extensions.md).

## Considered options

- **Plug-ins in the Sayso process (dylibs).** Rust has no stable ABI, GPUI views cannot cross a dylib boundary, and a crash in a plug-in stops dictation.
- **WASM plug-ins.** Safe, but they cannot draw GPUI windows, and the planned extensions are mostly UI.
- **UI that Sayso draws from a description (like Raycast).** Too limited for a transcript editor with playback, and it puts the UI back in Sayso.
- **The engine as a shared service for all clients.** Capture, permissions, and many clients would be built in each engine (Swift on macOS, `sayso-engine` on Windows and Linux). No single process could put dictation first.
- **Each extension starts its own engine.** Models load twice, each extension asks for the microphone, and dictation competes with extensions for the same hardware.

## Consequences

- The engine protocol becomes a public contract, with features in `hello`, public model ids, and conformance tests for each engine.
- On macOS, Sayso starts extensions through LaunchServices, so they cannot use the microphone permission of Sayso.
