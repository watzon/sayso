# Sayso context

Sayso is a local-first voice dictation app. You speak, and Sayso puts the text into the app you are using. This file defines the words we use in code, docs, and UI.

## Terms

**Dictation**
One recording from start to insert. A dictation has a state: idle, recording, processing, inserting, done, cancelled, or failed.

**Toggle hotkey**
A key that starts a dictation on the first press and stops it on the second press. The default is Option+Space on macOS, Ctrl+Space on Windows, and Ctrl+Alt+Space on Linux.

**Push-to-talk hotkey**
A key that records while you hold it and stops when you release it. It has no default.

Both dictation hotkeys accept a chord, a single modifier, or a combination of modifiers. Modifier-only bindings distinguish left and right keys. macOS supports Command, Control, Option, Shift, and Fn; modifier-only toggles and push-to-talk need Input Monitoring. Windows and Linux do not support Fn. Modifier-only toggles fire on the first release within 400 ms of the first modifier press, unless another key, mouse button, or scroll was used. On XInput2, pointer motion also cancels a tap so smooth scrolling cannot activate dictation. Overlapping modifier-only bindings are rejected; regular chords can share modifiers.

Modifier-only push-to-talk intentionally starts immediately on press, including when those modifiers begin an ordinary keyboard or mouse shortcut. Prefer a right-side modifier you do not normally use, or a chord, to avoid opening the microphone during shortcuts. A delay would change that press-to-record behavior; the minimum dictation duration discards short recordings but does not prevent capture or its start cue.

Downgrading after saving left-side or combined modifier bindings: older builds may reject the whole `[hotkeys]` section and restore its defaults. Export or copy your shortcuts first, and check every hotkey after a downgrade.

**Send with Enter**
A setting on macOS and Windows, off by default (`hotkeys.enter_sends`). While a dictation records, Enter stops it, and Sayso presses Enter after the insert, so a chat sends the message. Sayso takes that first Enter from the app. Apart from the hotkeys of Sayso, Enter and Esc (see Block Esc) are the only keys that Sayso takes. It does not work when the toggle or push-to-talk hotkey uses Return. On Linux the command `sayso --push-to-talk-up-submit` does the same for push-to-talk.

**Block Esc**
A setting on macOS and Windows, on by default (`hotkeys.take_escape`). While a dictation records, Sayso takes each Esc without Command, Control, or Option from the app, so the Esc that cancels does not also close a dialog or clear a field. With double Esc, Sayso takes both presses. It does not work when the toggle or push-to-talk hotkey uses Esc. On macOS it needs Accessibility; without it, Esc goes to the app as before. Linux cannot take keys, so there Esc always goes to the app.

**Transcript**
The raw text from the speech model, before any changes.

**Pipeline**
The fixed steps that turn audio into inserted text: transcribe → apply replacements → apply spoken punctuation → apply style → insert. Spoken punctuation is a setting and is off by default.

**Engine**
The process that runs local models. On macOS this is the Swift sidecar (`SaysoEngine`) with WhisperKit and FluidAudio. On Windows and Linux it is the Rust sidecar (`sayso-engine`) with sherpa-onnx, and also whisper.cpp on Windows and parakeet.cpp on Linux. Both engines speak the same protocol. Rust code talks to the engine through the `SttBackend` trait. The engine never sees a cloud model. The engine on macOS also runs the language model of a style (see Apple Intelligence). Rust code reaches it through the `LanguageModel` trait.

**Engine protocol**
The NDJSON protocol between an engine and its client (`docs/engine-protocol.md`, MIT). Sayso uses it, and an app that runs an engine alone uses it.

**Engine feature**
An optional part of the engine protocol that an engine lists in its `hello` reply, for example `unload`. A client checks for a feature before it uses it.

**Segment**
A part of a transcript with its start and end time in the audio. `transcribe` gives segments when the client asks for them.

**Timestamp level**
How much timing a model gives with its text: `word` (segments with the time of each word), `segment` (the model's own segments), or `none`. The model catalog says the level of each model.

**Engine catalog**
The models that an engine knows by id, built into the engine from the model catalog of Sayso. A client can ask for the catalog and can leave out the `engine` object of a model that is in it. An id that leaves the catalog stays as a retired id.

**Engine release file**
The file with an engine that the Release workflow attaches to a GitHub release: `sayso-engine-<version>-<os>-<arch>` (`.tar.gz`, and `.zip` on Windows). It holds the engine from the app build, the libraries it needs, its licenses, and a README. Sayso never uses it: the app has its own engine, and Sayso never downloads an engine.

**Model**
A speech model, for example "Parakeet Unified EN" or "Whisper large-v3". A dictation uses a final model and, when the live preview is on, a preview model. A model is a local model or a cloud model. VAD models and diarization models are local models too, but they give no text (see Model task).

**Local model**
A model that the user downloads and that runs on this computer in the engine. The audio stays on the computer.

**Cloud model**
A model that runs on the servers of a speech provider. Sayso sends the audio of the dictation and the dictionary words to the provider. A cloud model does the final pass only. Its id is `<provider id>:<model>`.

**Speech provider**
A service that runs cloud models: OpenAI, Groq, ElevenLabs, Deepgram, AssemblyAI, Mistral, or a custom endpoint (any server with the OpenAI transcription API, also one on this computer). The user adds a provider with their own API key. It is not the same as the provider of a style (see Enhancer).

**Final pass**
The transcription that runs when the dictation stops. Its result is the transcript.

**Live preview**
Text from a streaming model while the user speaks. It shows only in the overlay and is never inserted. A streaming local model gives the preview, also when the final model is a cloud model.

**Final model**
The model that does the final pass (`dictation.model`). Any local model with a final pass, or a cloud model.

**Preview model**
The streaming local model that gives the live preview. The user chooses it on the Models page, or leaves it on "Automatic" (`dictation.preview_model = "auto"`): the final model when it streams, else a streaming model on disk that understands the dictation language. A chosen model that does not understand the dictation language gives no preview, and the Models page says so. The preview model can be the final model, and then it loads once.

**Streaming model**
A local model that can give the live preview. Every streaming model can also do the final pass.

**Model task**
What a local model does: speech (the final pass or the live preview), VAD, or diarization. The engines check it: a request for one task needs a model of that task. The list of speech models (`models::catalog`) has only speech models, so no surface of dictation can offer a VAD or diarization model. The engine catalog has all of them (`models::local_models`).

**VAD model**
A local model that finds the parts of audio that contain speech. VAD means voice activity detection. The jobs and live sessions of extensions use it to cut long audio at pauses. Dictation does not use VAD. The user chooses it on the Models page (`models.vad`): "auto" (the first VAD model on disk, Silero VAD first, else Basic VAD), "basic", or a model id. A chosen model that is not on disk gives Basic VAD.

**Basic VAD**
The VAD of the host (`sayso_core::basic_vad`). It finds speech by its sound level, a margin above the noise floor of the last 5 seconds, so a constant noise is not speech. It follows the same rules as the engines: speech under 250 ms is left out, a pause under 500 ms does not end speech, and a part is at most 30 s. It needs no download, so a new install has it. Its id is `basic`.

**Diarization model**
A local model that finds who speaks when in a recording. It can have more than one file (on Linux and Windows, a segmentation model and a speaker embedding model), but it is one model with one id. The user chooses it on the Models page (`models.diarization`): "auto" (the diarization model on disk, else none) or a model id. There is no built-in fallback.

**Extension**
A separate app that uses the speech features of Sayso through the Host API (`docs/extensions.md`). Sayso installs it in `<data dir>/extensions/<id>/<version>/`. While at least one is installed, the Models page also shows the VAD and diarization models.

**Host**
Sayso, when it serves extensions (`sayso-host`). It knows an extension by its executable and the record of the extension in `<data dir>/extensions/state.toml`.

**Host API**
The protocol between the host and an extension, over a local socket (a named pipe on Windows). It uses the message shapes of the engine protocol. The types are in `sayso-protocol`, and the client is `sayso-extension` (both MIT).

**Capability**
A permission that an extension declares in its manifest and the user grants at install, for example `audio.mic`. The host refuses a request whose capability the user did not grant. `models.read` is always granted.

**Dictation language**
The language the user speaks: a language code, or "auto" to let the model detect it. Each model gets the nearest value that it supports.

**Interface language**
The language of the text that Sayso shows. It is not the dictation language. The setting `general.language` is "system" or a locale. "system" takes the first language of the system that has a catalog, else English.

**Catalog**
The file with the interface text of one locale, keyed by name: `crates/sayso-i18n/locales/<locale>.yml`. English is the source catalog.

**Locale**
A code such as `en`, `pt-BR`, or `zh-CN` that names one catalog.

**Dictionary**
The user's list of words and replacements.
- A **word** is a name or term that biases recognition (vocabulary boosting or prompt).
- A **replacement** is a rule that changes text after transcription ("git hub" → "GitHub").

**Style**
A named AI enhancement with a prompt, optional cleanup settings, and optional provider, model, and timeout overrides. "Raw" is the style with no AI: no prompt and no cleanup settings. Each style is one TOML file. Built-in styles ship in the app, and a user file with the same id overrides them. The prompt of a style says only what the style adds to the base prompt.

**Default style**
The style of a dictation when no style rule applies. The user chooses it in the popover, on the Styles page, or with the next-style hotkey.

**Style rule**
The apps and sites that use one style. A dictation into one of them gets that style, not the default style. An app or a site has one style only. A site is a host with an optional path (`facebook.com/messages`); it also applies to its subdomains and to the paths below it. A site rule wins over the rule of the browser. The rules are in `config.toml` (`ai.style_rules`), because an app id is different on each system. They are off by default, and the first time the user turns them on, Sayso adds a starter set. Sayso reads the page of a browser on macOS and Windows, not on Linux.

**Site icon**
The icon of the site of a history entry. It shows on the corner of the app icon. Sayso gets it from the site itself, or from the icon service of DuckDuckGo when the site gives none, and keeps it in the cache for 7 days. A setting turns it off (`history.site_icons`).

**App id**
The name of an app for the system: the bundle id on macOS, the file name of the exe on Windows, and the name of the desktop entry on Linux.

**Base prompt**
The cleanup rules and examples that all styles share: filler words, self-corrections, numbers, acronyms, and the words of the speaker, which stay. A style adds its own prompt to it. The user can change the base prompt (`styles/base.md`) and reset it to the default. A standalone style does not get it.

**Standalone style**
A style that runs without the base prompt, for a prompt that is not a cleanup (a translation, for example).

**Spoken punctuation**
The step that changes the English commands "comma", "period", "question mark", "new line", and others into marks and line breaks. It runs in code, with no AI.

**Spelled word**
A word that the speaker spells after a cue: "Dana Cats, that's K A T Z". Before the model of a style runs, code puts the spelled word in place of the cue and of the words that it corrects.

**Enhancer**
The trait that sends a transcript and a style prompt to an AI provider and returns schema-checked JSON. The implementations are the OpenAI-compatible endpoint, the Claude CLI, the Codex CLI, and the language model of the engine. A cleanup model is not an enhancer: it runs through `Cleaner`.

**Cleanup model**
A local model that cleans up text with fixed settings and reads no prompt: S1-mini by Superwhisper (English) and Cleanup 1 by Spokenly (seven languages). It is a provider of a style, like Apple Intelligence, with the badge "No prompts". It runs a style only when the style has cleanup settings that the model supports; else it does a cleanup only, and the overlay says so. It runs only when Sayso knows the language of the dictation and the model supports it. The text stays on the computer. [docs/cleanup-models.md](docs/cleanup-models.md) has the design.

**Cleanup settings**
The part of a style that a cleanup model uses (`[cleanup]` in the style file): tone, layout (paragraphs or list), and format (plain or email). A style without them needs a provider that reads prompts. An override of a built-in style without the key keeps the settings of the built-in style; `cleanup = false` turns them off.

**Tone**
How a cleanup model writes capital letters and the final period: very casual, casual, or formal. It does not change words. Only Cleanup 1 has tones.

**Cleanup runtime**
The process `sayso-cleanup` (`native/cleanup`) that runs a cleanup model with llama.cpp. It knows no model: it loads a GGUF file and completes text. Sayso starts it at the first use and stops it after 10 minutes with no use.

**Thinking level**
How much the model of a provider thinks before it answers, for example low, medium, or high. A higher level is slower. The levels are those that the provider reports for the model, with its model list: the Claude tool, the Codex tool, and OpenRouter do. The user sets the level for each provider in Settings. With no choice, a CLI provider gets low, and an OpenAI-compatible provider gets no level, so the model uses its own default. When the user chooses another model, a level that the model does not have becomes its lowest level. The level goes out as `--effort` (Claude), `model_reasoning_effort` (Codex), `reasoning.effort` (OpenRouter), or `reasoning_effort` (other servers).

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
The floating window that shows a dictation in progress (ink waveform, timer, live preview, errors). Its overlay style is the pill or the notch.

**Overlay style**
How the overlay shows a dictation (`overlay.style`): the pill or the notch. The user picks it in Settings › Overlay. A change of style opens the overlay window again.

**Notch**
The overlay style that opens from the top center of the display only while the user dictates, and is gone when no dictation runs. It is always black, the size of the MacBook camera notch. Its two wings show the state to the left and the right of the camera, and a slip of paper under it shows the live preview and the notices. On a Mac display with a camera notch it takes the size of that notch. On a display without one, Sayso draws a notch of the same size, or uses the pill when `overlay.notch_on_other_displays` is off (macOS only). It covers the menu bar while it shows. The user cannot drag it.

**Pill**
The overlay style that the user can drag, and its small, calm state when no dictation runs. The user can drag it, and it snaps softly to edges and centers. In the bottom half of a display the overlay grows up from the pill, and the live preview shows above it. In the top half the overlay grows down, and the live preview shows below it.

**Hub**
The main window. Its sections are Home, History, Dictionary, Styles, Models, and Settings.

**Popover**
The themed panel that opens from the menu bar icon (the taskbar icon on Windows, the tray icon on Linux). It is not a native menu.

**History entry**
A stored dictation. It holds the transcript, the final text, the style, the target app (with the host of the page, when the app showed a web page), the model, the time, the duration, the diagnostics, and (until it expires) the audio. When history is off, Sayso stores no history entry for a new dictation. Entries from before stay until the user clears them.

**Diagnostics**
How a dictation ran, for a bug report: the version of Sayso, the system, the time to open the microphone, and the time of each step from the stop of the recording to the end of the insertion. They hold no dictated text. "Copy metadata" in the menu of a history entry copies them as JSON, with the other data of the entry and without the text and the site.

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
