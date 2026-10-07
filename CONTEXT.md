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
The fixed steps that turn audio into inserted text: transcribe → apply replacements → apply spoken punctuation → apply style → insert. Spoken punctuation is a setting and is off by default.

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
A named AI enhancement with a prompt and optional provider, model, and timeout overrides. "Raw" is the style with no AI. Each style is one TOML file. Built-in styles ship in the app, and a user file with the same id overrides them. The prompt of a style says only what the style adds to the base prompt.

**Default style**
The style of a dictation when no style rule applies. The user chooses it in the popover, on the Styles page, or with the next-style hotkey.

**Style rule**
The apps and sites that use one style. A dictation into one of them gets that style, not the default style. An app or a site has one style only. A site is a host with an optional path (`facebook.com/messages`); it also applies to its subdomains and to the paths below it. A site rule wins over the rule of the browser. The rules are in `config.toml` (`ai.style_rules`), because an app id is different on each system. They are off by default, and the first time the user turns them on, Sayso adds a starter set. Sayso reads the page of a browser on macOS and Windows, not on Linux.

**App id**
The name of an app for the system: the bundle id on macOS, the file name of the exe on Windows, and the name of the desktop entry on Linux.

**Base prompt**
The cleanup rules and examples that all styles share: filler words, self-corrections, numbers, and acronyms. A style adds its own prompt to it. The user can change the base prompt (`styles/base.md`) and reset it to the default. A standalone style does not get it.

**Standalone style**
A style that runs without the base prompt, for a prompt that is not a cleanup (a translation, for example).

**Spoken punctuation**
The step that changes the English commands "comma", "period", "question mark", "new line", and others into marks and line breaks. It runs in code, with no AI.

**Spelled word**
A word that the speaker spells after a cue: "Dana Cats, that's K A T Z". Before the model of a style runs, code puts the spelled word in place of the cue and of the words that it corrects.

**Enhancer**
The trait that sends a transcript and a style prompt to an AI provider and returns schema-checked JSON. The implementations are the OpenAI-compatible endpoint, the Claude CLI, the Codex CLI, and the language model of the engine.

**Apple Intelligence**
The provider of a style that runs the language model of macOS (macOS 26 and later) in the engine. The transcript stays on the Mac. macOS chooses the model and owns its files, so there is no download and no model choice. It is not a model in the sense of this file, because it does not transcribe speech.

**Insertion**
The step that puts the final text into the focused app. The default is paste through the clipboard, with a safe restore. Typing is the fallback. Sayso adds a space after the text, and a setting turns this off.

**Shell**
The app glue that GPUI does not cover, one backend for each system: window placement, the tray, the Dock, and startup (`crates/sayso-app/src/shell`). On Linux it also picks the display server for the UI.

**Rail**
The collapsed Hub sidebar: a column of icons with no labels. Each label shows as a tooltip. The user collapses and expands the sidebar with a button in its footer (`appearance.sidebar_collapsed`). In a window less than 920 wide the Hub always shows the rail.

**Floor**
The smallest size at which a window draws its content. A tiling window manager can make a window smaller than its minimum size. Below the floor the content keeps its size and the window scrolls.

**Overlay**
The floating window that shows a dictation in progress (ink waveform, timer, live preview, errors).

**Pill**
The small, calm overlay state when no dictation runs. The user can drag it, and it snaps softly to edges and centers. In the bottom half of a display the overlay grows up from the pill, and the live preview shows above it. In the top half the overlay grows down, and the live preview shows below it.

**Hub**
The main window. Its sections are Home, History, Dictionary, Styles, Models, and Settings.

**Popover**
The themed panel that opens from the menu bar icon (the taskbar icon on Windows, the tray icon on Linux). It is not a native menu.

**History entry**
A stored dictation. It holds the transcript, the final text, the style, the target app (with the host of the page, when a style rule read it), the model, the time, the duration, and (until it expires) the audio. When history is off, Sayso stores no history entry for a new dictation. Entries from before stay until the user clears them.

**Incognito**
A mode in which Sayso stores no history entry for a dictation. It stays on until the user turns it off or quits Sayso. It changes only what Sayso stores: a cloud model or the provider of a style still gets the audio or the transcript.

**Update manifest**
The signed file `latest.json` on a GitHub release. It names the version and, for each system, the release file with its size and hash.

**Update check**
One request for the update manifest. Sayso compares the version in the manifest with its own version. [docs/updates.md](docs/updates.md) has the design.

**Release notes**
The entry of a version in `release-notes.toml`: a headline, the new features, and an optional note. They are part of the build. The notes on the GitHub release are a different text, which GitHub makes from the pull requests.

**What is new window**
The window that shows the release notes one time, at the first start of a new version. It also opens from Settings › General.

**Official build**
A build from the Release workflow. Only an official build looks for updates.

**Ink**
The user's chosen base color (Sumi, Indigo, Iron Gall, Sepia, Verdigris, Oxblood, or custom). The theme generates the accent and recording colors from the ink.

**Paper**
The visual material of the app. Surfaces are sheets of paper with depth, and controls are embossed or debossed into them.
