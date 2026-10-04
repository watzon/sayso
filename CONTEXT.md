# Sayso context

Sayso is a local-first voice dictation app. You speak, and Sayso puts the text into the app you are using. This file defines the words we use in code, docs, and UI.

## Terms

**Dictation**
One recording from start to insert. A dictation has a state: idle, recording, processing, inserting, done, cancelled, or failed.

**Toggle hotkey**
A key that starts a dictation on the first press and stops it on the second press. The default is Option+Space on macOS, Ctrl+Space on Windows, and Ctrl+Alt+Space on Linux.

**Push-to-talk hotkey**
A key that records while you hold it and stops when you release it. It has no default.

**Transcript**
The raw text from the speech model, before any changes.

**Pipeline**
The fixed steps that turn audio into inserted text: transcribe → apply replacements → apply style → insert.

**Engine**
The process that runs local models. On macOS this is the Swift sidecar (`SaysoEngine`) with WhisperKit and FluidAudio. On Windows it is the portable sidecar (`native/portable`, also `SaysoEngine`) with transcribe-rs. On Linux it is the Rust sidecar (`sayso-engine`) with sherpa-onnx. The Windows and Linux engines run on the CPU. All three speak the same protocol. Rust code talks to the engine through the `SttBackend` trait. The engine never sees a cloud model. The engine on macOS also runs the language model of a style (see Apple Intelligence). Rust code reaches it through the `LanguageModel` trait.

**Model**
A speech model, for example "Parakeet Unified EN" or "Whisper large-v3". One model is the active model. A model is a local model or a cloud model.

**Local model**
A model that the user downloads and that runs on this computer in the engine. The audio stays on the computer.

**Cloud model**
A model that runs on the servers of a speech provider. Sayso sends the audio of the dictation and the dictionary words to the provider. A cloud model does the final pass only. Its id is `<provider id>:<model>`.

**Speech provider**
A service that runs cloud models: OpenAI, Groq, ElevenLabs, Deepgram, AssemblyAI, Mistral, or a custom endpoint (any server with the OpenAI transcription API, also one on this computer). The user adds a provider with their own API key. It is not the same as the provider of a style (see Enhancer).

**Final pass**
The transcription that runs when the dictation stops. Its result is the transcript.

**Live preview**
Text from a streaming model while the user speaks. It shows only in the overlay and is never inserted. A streaming local model gives the preview, also when the active model is a cloud model.

**Dictation language**
The language the user speaks: a language code, or "auto" to let the model detect it. Each model gets the nearest value that it supports.

**Dictionary**
The user's list of words and replacements.
- A **word** is a name or term that biases recognition (vocabulary boosting or prompt).
- A **replacement** is a rule that changes text after transcription ("git hub" → "GitHub").

**Style**
A named AI enhancement with a prompt and optional provider, model, and timeout overrides. "Raw" is the style with no AI. Each style is one TOML file. Built-in styles ship in the app, and a user file with the same id overrides them.

**Enhancer**
The trait that sends a transcript and a style prompt to an AI provider and returns schema-checked JSON. The implementations are the OpenAI-compatible endpoint, the Claude CLI, the Codex CLI, and the language model of the engine.

**Apple Intelligence**
The provider of a style that runs the language model of macOS (macOS 26 and later) in the engine. The transcript stays on the Mac. macOS chooses the model and owns its files, so there is no download and no model choice. It is not a model in the sense of this file, because it does not transcribe speech.

**Insertion**
The step that puts the final text into the focused app. The default is paste through the clipboard, with a safe restore. Typing is the fallback.

**Shell**
The app glue that GPUI does not cover, one backend for each system: window placement, the tray, the Dock, and startup (`crates/sayso-app/src/shell`). On Linux it also picks the display server for the UI.

**Overlay**
The floating window that shows a dictation in progress (ink waveform, timer, live preview, errors).

**Pill**
The small, calm overlay state when no dictation runs. The user can drag it, and it snaps softly to edges and centers.

**Hub**
The main window. Its sections are Home, History, Dictionary, Styles, Models, and Settings.

**Popover**
The themed panel that opens from the menu bar icon (the taskbar icon on Windows, the tray icon on Linux). It is not a native menu.

**History entry**
A stored dictation. It holds the transcript, the final text, the style, the target app, the model, the time, the duration, and (until it expires) the audio. When history is off, Sayso stores no history entry for a new dictation. Entries from before stay until the user clears them.

**Incognito**
A mode in which Sayso stores no history entry for a dictation. It stays on until the user turns it off or quits Sayso. It changes only what Sayso stores: a cloud model or the provider of a style still gets the audio or the transcript.

**Update manifest**
The signed file `latest.json` on a GitHub release. It names the version and, for each system, the release file with its size and hash.

**Update check**
One request for the update manifest. Sayso compares the version in the manifest with its own version. [docs/updates.md](docs/updates.md) has the design.

**Official build**
A build from the Release workflow. Only an official build looks for updates.

**Ink**
The user's chosen base color (Sumi, Indigo, Iron Gall, Sepia, Verdigris, Oxblood, or custom). The theme generates the accent and recording colors from the ink.

**Paper**
The visual material of the app. Surfaces are sheets of paper with depth, and controls are embossed or debossed into them.
