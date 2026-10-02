# Sayso context

Sayso is a local-first voice dictation app. You speak, and Sayso puts the text into the app you are using. This file defines the words we use in code, docs, and UI.

## Terms

**Dictation**
One recording from start to insert. A dictation has a state: idle, recording, processing, inserting, done, cancelled, or failed.

**Toggle hotkey**
A key that starts a dictation on the first press and stops it on the second press. The default is Option+Space.

**Push-to-talk hotkey**
A key that records while you hold it and stops when you release it. It has no default.

**Transcript**
The raw text from the speech model, before any changes.

**Pipeline**
The fixed steps that turn audio into inserted text: transcribe → apply replacements → apply style → insert.

**Engine**
The process that runs speech models. On macOS this is the Swift sidecar (`SaysoEngine`) with WhisperKit and FluidAudio. Rust code talks to it through the `SttBackend` trait.

**Model**
A speech model that the user downloads, for example "Parakeet Unified EN" or "Whisper large-v3". One model is the active model.

**Final pass**
The transcription that runs when the dictation stops. Its result is the transcript.

**Live preview**
Text from the streaming model while the user speaks. It shows only in the overlay and is never inserted.

**Dictionary**
The user's list of words and replacements.
- A **word** is a name or term that biases recognition (vocabulary boosting or prompt).
- A **replacement** is a rule that changes text after transcription ("git hub" → "GitHub").

**Style**
A named AI enhancement with a prompt and optional provider, model, and timeout overrides. "Raw" is the style with no AI. Each style is one TOML file. Built-in styles ship in the app, and a user file with the same id overrides them.

**Enhancer**
The trait that sends a transcript and a style prompt to an AI provider and returns schema-checked JSON. The implementations are the OpenAI-compatible endpoint, the Claude CLI, and the Codex CLI.

**Insertion**
The step that puts the final text into the focused app. The default is paste through the clipboard, with a safe restore. Typing is the fallback.

**Overlay**
The floating window that shows a dictation in progress (ink waveform, timer, live preview, errors).

**Pill**
The small, calm overlay state when no dictation runs. The user can drag it, and it snaps softly to edges and centers.

**Hub**
The main window. Its sections are Home, History, Dictionary, Styles, Models, and Settings.

**Popover**
The themed panel that opens from the menu bar icon. It is not a native menu.

**History entry**
A stored dictation. It holds the transcript, the final text, the style, the target app, the model, the time, the duration, and (until it expires) the audio.

**Ink**
The user's chosen base color (Sumi, Indigo, Iron Gall, Sepia, Verdigris, Oxblood, or custom). The theme generates the accent and recording colors from the ink.

**Paper**
The visual material of the app. Surfaces are sheets of paper with depth, and controls are embossed or debossed into them.
