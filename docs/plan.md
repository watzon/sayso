# Sayso v0.1 plan

Status: agreed on 2026-10-01. Research and sources are in [research.md](research.md). The terms are defined in [CONTEXT.md](../CONTEXT.md).

## 1. Goal

Sayso v0.1 is a local dictation app for macOS that Chris uses every day and shows in teaser videos. It must feel faster, more reliable, and better designed than WisprFlow.

Success for v0.1:

- You can dictate into any normal text field with one hotkey, and the text lands in under 1 s after you stop (without AI), on the M5 Pro.
- No dictation is ever lost. If insertion or AI fails, the text is still on screen and in History.
- Sayso itself sends nothing off the computer. Only the user's opt-in AI provider receives text.
- A new user goes from first launch to a successful practice dictation without reading docs.

## 2. Scope

### In v0.1

1. Dictate and insert into the focused app.
2. Overlay with ink waveform and live preview, plus the idle pill.
3. Onboarding.
4. Model manager (Parakeet and Whisper).
5. History with audio playback and a waveform.
6. Dictionary (words and replacements).
7. AI styles (built-in and user-made), with three provider types.
8. Hub window, menu bar popover, and the tactile paper theme in light and dark.

### Not in v0.1

- Snippets.
- Command mode.
- Per-app styles (the style file format leaves room for it).
- Learning replacements from manual corrections.
- Telemetry and crash reporting.
- Licensing and purchase.
- Languages other than English (the design supports them).
- Linux and Windows (the design supports them). Linux was added on 2026-10-02, see "Linux" below.

## 3. Decisions

### Platform and stack

| Topic | Decision |
|---|---|
| OS and hardware | macOS 14+, Apple Silicon only |
| UI | GPUI from `gpui-pre` and gpui-kit (longbridge), both pinned with `=` versions. We update on purpose, never by accident. |
| macOS glue GPUI lacks | objc2: accessory activation policy, window position and level, hide. `tray-icon` for the status item. |
| Speech engine | Swift sidecar `SaysoEngine` with WhisperKit and FluidAudio. It talks NDJSON over stdio. |
| Audio capture | In Rust (cpal), behind `AudioCapture`. Rust resamples to 16 kHz mono and sends chunks to the engine. Spike 3 confirms that the overhead is low. |
| Storage | SQLite (history, dictionary) plus audio files on disk |
| Config | TOML, XDG-aware, live reload, validated. The UI is a view over the file. Secrets are in the Keychain. |
| Signing | Developer ID (Watzon Ventures LLC) for all builds, including development builds, so permission grants survive rebuilds |
| Distribution | Notarized download outside the Mac App Store |
| License (2026-10-02) | GPL-3.0-only. The name Sayso and the app icon stay with Watzon Ventures LLC. |
| Price (2026-10-02) | Pay what you want. The official signed build is free and has no license key, no trial, and no license check. The download page at justsayso.app asks for an amount before the download, and $0 is allowed. Payments go through Stripe. |
| Releases (2026-10-02) | A published GitHub release starts the Release workflow, which builds, signs, notarizes, and attaches the DMG. See [releasing.md](releasing.md). |
| Pindrop (2026-10-02) | Sayso is a separate product. It does not replace Pindrop through an update. The import from Pindrop is optional and only reads: dictations (without audio), dictionary words, replacements, and the user's prompt presets. Notes, meetings, and media stay in Pindrop. |

### Linux (2026-10-02)

| Topic | Decision |
|---|---|
| Scope | X11 and Wayland, on x86_64 and aarch64. GNOME, KDE Plasma, sway, and Hyprland are the reference desktops. [linux.md](linux.md) lists what works on each. |
| UI | GPUI's Linux backend. On a Wayland compositor with the layer shell, the UI uses Wayland, and the overlay and popover are layer surfaces. On a compositor without it (GNOME), the UI uses XWayland, because only an X11 window can float over other apps there without taking focus. |
| Speech engine | `sayso-engine`, a Rust sidecar on sherpa-onnx (CPU, prebuilt static library). It speaks the same NDJSON protocol as `SaysoEngine`, so `sayso-engine-client` is unchanged. The catalog is `sayso-core/src/models_onnx.rs`. Model ids match the macOS catalog where the model is the same. Windows can use the same engine. |
| Hotkeys | X11: key grabs for chords, XInput2 for listen-only keys. Wayland: the GlobalShortcuts portal for chords, evdev (the `input` group) for listen-only keys. `sayso --toggle` and the other commands reach the running Sayso through a Unix socket, for desktops without global shortcuts. Default toggle: Ctrl+Alt+Space, because Alt+Space opens the window menu. |
| Insertion | Paste with Shift+Insert, with our text in both the clipboard and the primary selection. Keys: XTest (X11), the virtual keyboard protocol (wlroots), the remote desktop portal (GNOME, KDE), or `/dev/uinput`. |
| Permissions | Capabilities, not grants: Accessibility is "Paste access" (a key injection method works), and Input Monitoring is "Keyboard access" (evdev is readable). |
| Secrets | The Secret Service, through the same `keyring` crate. |
| Distribution | A tarball with an install script, a desktop entry, icons, and a udev rule for `/dev/uinput`. The Release workflow builds it for x86_64 and aarch64. |
| Code layout | Portable platform code is in `sayso-platform-common`. The app reaches the system only through `sayso-app/src/shell` (one backend per system, plus a no-op fallback). |

### File locations

| Kind | Lookup order |
|---|---|
| Config (`config.toml`, `styles/*.toml`) | `$XDG_CONFIG_HOME/sayso/` → `~/.config/sayso/` if it exists → `~/Library/Application Support/Sayso/` |
| Data (database, audio, models) | `$XDG_DATA_HOME/sayso/` → `~/Library/Application Support/Sayso/` |
| Cache | `$XDG_CACHE_HOME/sayso/` → `~/Library/Caches/Sayso/` |

On Linux the last step of each lookup is the XDG default: `~/.config/sayso/`, `~/.local/share/sayso/`, and `~/.cache/sayso/`.

Settings shows the active path for each kind.

### Models

- Default: Parakeet Unified EN 0.6B.
  - The batch export does the final pass.
  - The streaming export (320 ms tier) does the live preview.
  - The CTC 110M helper does vocabulary boosting.
  - Onboarding downloads all three in one step (about 1.3 GB), with progress for each file and the total.
- Optional in Models:
  - Whisper large-v3-v20240930 (626 MB).
  - Whisper large-v3 turbo.
  - distil-large-v3.
  - small.
  - Parakeet EOU 120M, as a small preview model.
- The first load compiles each model for the Neural Engine. Show this as its own "Optimizing for your Mac" state.
- Multilingual models stay possible. The language setting exists and is fixed to English in v0.1.

#### After v0.1: more local models and cloud models (2026-10-02)

- The catalog holds every model that the pinned FluidAudio and WhisperKit versions run: the Parakeet TDT family (v2, v3, Ultra, Redux, Phonon-2, TDT-CTC 110M, Japanese), Nemotron streaming (English and multilingual), Cohere Transcribe, Canary 1B v2, SenseVoice Small, Paraformer, eight Whisper variants, and Apple Speech (SpeechAnalyzer, macOS 26 and later, files managed by macOS). The catalog is in `sayso-core/src/models.rs`.
- The language setting is live: a language code or "auto". Each model gets the nearest value that it supports.
- Cloud models are opt-in. A speech provider (OpenAI, Groq, ElevenLabs, Deepgram, AssemblyAI, Mistral, or a custom OpenAI-compatible endpoint) does the final pass with the user's own API key. The key is in the Keychain.
  - This changes one v0.1 goal. With a cloud model, the audio of each dictation and the dictionary words leave the Mac. The Models page, the add form, and Settings › History and privacy say so.
  - The live preview stays local. If the provider fails and a local model with a final pass is in memory, that model does the final pass, and the overlay says so.
- Not added, with the reason:
  - Qwen3-ASR, Voxtral, Granite Speech, Kyutai: they need MLX, and command-line SwiftPM cannot build the MLX Metal shaders.
  - Moonshine: its Swift package ships iOS slices only.

### Dictation flow

| Topic | Decision |
|---|---|
| Toggle hotkey | Option+Space by default |
| Push-to-talk hotkey | No default. You can set it in onboarding or Settings. |
| Conflict detection | For both hotkeys, in onboarding and Settings |
| Cancel | Double Esc by default. Single Esc is a setting. The overlay shows "Cancelled" with Undo. |
| Paste last transcript | Ctrl+Cmd+V |
| Insertion | Paste through the clipboard. The text is published as a promise, so macOS tells Sayso when an app reads it. After the read, restore the old clipboard only if it still holds our text. With no read in 1.5 s, the insertion failed, and the text stays on the clipboard. Sayso does not ask Accessibility which field has focus, because Electron apps give no answer (2026-10-02). Typing is the fallback, and you can force it per app. |
| Insertion failure | The overlay stays with the text and a Copy action |
| Pipeline | Transcribe → replacements → style (AI) → insert. Dictionary words also go into the AI prompt. |
| AI failure or timeout | Insert the transcript. Show "Inserted without enhancement". Keep both texts in History. |

### Overlay and pill

- The overlay shows while recording and processing, then hides.
- It shows an ink waveform, a timer, and the live preview text in a visibly provisional style.
- The idle pill is on by default. It sits small and calm at the bottom center, and it hides over full-screen apps (this is a setting).
- On hover the pill shows the hotkey hint, and a click starts a dictation. A right-click opens a menu (style, model, settings, reset position).
- Free drag with soft snap to the edges and the top and bottom centers. The position is stored for each display.
- The overlay must never take focus from the target app.

### AI enhancement

- It is opt-in, and every cloud destination is labeled in the UI.
- What leaves the device: the system prompt, the style prompt, dictionary words, and the transcript.
- The response must match a JSON schema. v0.1 schema: `{ "text": string }`.
- Providers:
  - **OpenAI-compatible endpoint** (OpenRouter, OpenAI, Ollama, LM Studio, llama.cpp server). This is the main path.
    - Mode ladder: strict `json_schema` → `json_object` with the schema in the prompt → forced tool call → local validation with one retry.
    - Sayso caches the working mode for each endpoint and model.
    - OpenRouter requests send `require_parameters: true`, and offer `zdr` and `data_collection: deny`.
  - **Claude CLI**, labeled "slower" (about 1.8 s).
    - Run as `claude -p` with `--json-schema`, `--tools ""`, `--no-session-persistence`, and `--setting-sources ""`, plus Haiku with low effort and no thinking.
    - Run it in a temp directory.
    - Sayso never reads the CLI's credentials.
    - Before a public launch, check the policy again.
  - **Codex CLI** (about 5 s). Run as `codex exec --ephemeral --sandbox read-only --output-schema`.
- Timeouts: one global timeout and a per-style override. Defaults: 4 s for HTTP, 8 s for CLIs.

### Styles

- Built-in styles:
  - Clean (default)
  - Polished
  - Message
  - Email
  - Notes
  - Raw (no AI)
- One TOML file per style in `<config>/styles/`.
  - Fields: id, name, ink tag, prompt, and optional provider, model, temperature, and timeout.
  - A user file with a built-in id overrides that style. "Reset" deletes the override.
- Later: an `apps` field binds a style to bundle ids.

### History and retention

- Store every dictation. For each one, keep:
  - the transcript and the final text;
  - the style, the provider, and the model;
  - the target app;
  - the time and duration;
  - the audio file and its waveform summary.
- There are two separate limits:
  - Text: forever by default.
  - Audio: 30 days by default.
- When audio expires, only the audio file is deleted. The entry and its waveform summary stay.
- Audio is played in the app with a waveform scrubber.

### Hub, popover, and onboarding

- The Hub has these sections:
  - Home (stats, recent, status)
  - History
  - Dictionary
  - Styles (with provider setup)
  - Models
  - Settings
- A theme toggle (System, Light, Dark) sits in the sidebar.
- Settings takes over the sidebar. It has no second column. The sidebar shows "‹ Settings" (back to the Hub), the settings sections, and an "Open config file" link with the active path.
- The Hub shows a Dock icon only while it is open. At other times Sayso is a menu bar app.
- The popover holds these items:
  - status
  - start or stop dictation
  - the last transcript, with Copy and Paste
  - the style and model switchers
  - the microphone picker
  - Open Sayso
  - Quit
- Onboarding steps (you can quit at any step and resume):
  1. Welcome
  2. Model (the download starts)
  3. AI enhancement (optional)
  4. Permissions: microphone and accessibility on one screen, with live detection of the grant and the mic test inline
  5. Hotkeys (with conflict detection)
  6. Practice (shows download and optimization status if they are not done)
  7. Done

### Design

- Material: tactile paper (neu-skeuomorphic).
  - Stacked sheets with real depth.
  - Embossed and debossed controls, with a letterpress look.
  - Controls that press into the page.
  - Toggles like a paper tab or a wax seal.
  - Ink that spreads where you press.
  - No glass and no blur.
- Light and dark follow the system, and the sidebar has a toggle.
- Inks: the app generates these and tests them for contrast in both modes.
  - Named inks: Sumi, Indigo, Iron Gall, Sepia, Verdigris, Oxblood.
  - Custom ink: under Advanced, with a contrast warning.
  - The accent and recording colors are derived from the ink.
- Typography: a serif display face for headings and a clean sans for UI. Bundle static weights, because variable weights fail with `add_fonts`.
- Motion: ink is the motion language. Strokes draw on, and ink spreads at the start of recording. Motion follows reduce-motion.
- Rendering limits (GPUI has no custom shaders, blur, radial gradients, or masks):
  - Ink effects: layered `paint_path` curves, linear gradients, and soft shadows.
  - Fallback: per-frame CPU images for soft edges.
  - Paper grain: one tiled texture painted once. The board (window background, sidebar) carries the visible texture. Content sheets get a very soft texture so text sits on a nearly clean page. The textures come from `assets/textures/generate.py`.
  - If we need one true shader moment, add a native Metal layer to the overlay only, later.
- Sound: generated effects for start, stop, cancel, and insert. On by default at low volume, with an on/off toggle and volume for each event.

## 4. Architecture

### Workspace layout

```
sayso/
├── crates/
│   ├── sayso-core/            # Domain logic, no platform or UI imports
│   ├── sayso-platform/        # Traits only
│   ├── sayso-platform-macos/  # macOS implementations of the traits
│   ├── sayso-engine-client/   # Rust side of the NDJSON engine protocol (SttBackend impl)
│   ├── sayso-store/           # SQLite history and dictionary, audio files, retention
│   ├── sayso-enhance/         # Enhancer trait and the three providers
│   ├── sayso-ui/              # Theme, inks, tactile paper components on gpui-kit
│   └── sayso-app/             # Binary: wires everything, windows, tray, onboarding
├── native/macos/SaysoEngine/  # Swift package: WhisperKit + FluidAudio sidecar
├── assets/                    # Fonts, textures, sounds, icons
├── spikes/                    # Throwaway prototypes (deleted after M0)
└── docs/
```

Contents of `sayso-core`:

- The dictation state machine.
- The pipeline.
- Style and dictionary logic.
- The config schema and loading.
- Inks and color generation.

### Dependency rules

- `sayso-core` depends on no other Sayso crate, and on no GPUI or platform crate.
- Only `sayso-app` depends on `sayso-platform-*` crates. All other crates use the traits in `sayso-platform`.
- `sayso-ui` depends on GPUI and gpui-kit, and does not depend on the platform crates.
- A CI check enforces these rules (`cargo metadata` or `cargo-deny` bans).

### Platform traits

| Trait | macOS implementation |
|---|---|
| `HotkeySource` | Carbon hotkey for chords. CGEventTap for push-to-talk and modifier-only keys. It emits press and release events. |
| `TextInserter` | Paste and restore via CGEvent. Fallback: typing with `CGEventKeyboardSetUnicodeString`. |
| `ContextProvider` | Frontmost bundle id via NSWorkspace. Focused field via AX, for later features. |
| `PermissionGuide` | Check, request, deep-link, and live-poll microphone, accessibility, and input monitoring |
| `AudioCapture` | cpal input, device list and change events, levels |
| `SttBackend` | The engine client (sidecar). On Linux the sidecar is `sayso-engine` (sherpa-onnx), not transcribe-rs: sherpa-onnx streams partials and needs no cmake. |
| `LoginItem` | SMAppService |
| `SoundPlayer` | rodio or a native player |

### Engine protocol (NDJSON over stdio)

Requests:

- `load_model`
- `unload_model`
- `download_model`
- `start_session` (with vocabulary)
- `audio_chunk`
- `finish_session`
- `cancel_session`
- `shutdown`

Events:

- `download_progress`
- `model_state` (downloading, optimizing, ready, error)
- `partial`
- `final`
- `error`
- `log`

Each message has a `v` (protocol version) and an `id`.

- **Crashes:** the app restarts the engine if it crashes. A dictation that is running during a crash keeps its audio, so it can be transcribed again.
- **Downloads:** inference: the engine downloads models through the WhisperKit and FluidAudio download paths, into Sayso's data directory. Spike 3 confirms that both libraries accept a custom directory.

## 5. Milestones

Each milestone ends with its checks passing. Chris uses the app at the end of M3 and every milestone after.

### M0: Spikes (alongside the Paper design)

Throwaway code in `spikes/`. Each spike answers one question and writes a short result note.

| # | Question | Pass criteria |
|---|---|---|
| S1 | Can a GPUI `PopUp` overlay float without taking focus, and without the transparent-border bug? | Type in TextEdit, show the overlay, and keep typing. TextEdit stays key, the overlay draws an animated `canvas` at 60+ fps, and no border shows. |
| S2 | Can we show a themed GPUI popover under a `tray-icon` status item, with accessory activation (no Dock icon)? | Click the icon: the popover opens under it. An outside click closes it. No Dock icon appears. |
| S3 | Do WhisperKit and FluidAudio build together in one Swift executable, and what are the load times? | One binary runs Parakeet Unified (batch and streaming) and one Whisper model on a WAV file and on streamed chunks. It reports cold load, warm load, and transcription times on the M5 Pro. |
| S4 | Does a Developer ID signed app with a bundled sidecar keep its permission grants after a rebuild? | Grant the microphone once, rebuild and re-sign, and the grant is still there. `codesign --verify --deep --strict` passes. Notarization is not submitted without approval. |
| S5 | Do the Option+Space toggle and a push-to-talk key work, including key-up, and can we detect conflicts? | Option+Space fires press events. A chosen modifier-only key gives press and release events. The spike reports whether a hotkey is already taken. |

### M1: Skeleton

- Workspace, the dependency rules in CI, the config loader with XDG lookup and live reload, inks and theme tokens.
- An accessory app with a tray icon and an empty popover, and an empty Hub window with a sidebar and a theme toggle.
- Check: `cargo test` and `cargo clippy -D warnings` pass. The dependency-rule check fails if `sayso-core` imports GPUI. The config tests cover the lookup order and invalid files.

### M2: Engine

- `SaysoEngine` sidecar, the NDJSON protocol, the engine client with restart, model catalog, downloads with progress, the optimizing state.
- Check: protocol round-trip tests with a fake engine. One integration test transcribes a fixture WAV with Parakeet and matches the expected text.

### M3: Dictation loop

- Hotkeys, audio capture, the overlay with the ink waveform and live preview, the pipeline (without AI), insertion with safe clipboard restore and fallback, cancel and Undo, paste last transcript, the idle pill with drag and snap.
- Check:
  - State-machine unit tests cover every transition, including cancel during processing and engine crash during recording.
  - Clipboard-restore tests cover "user copied something during the dictation".
  - A manual run list: TextEdit, Safari, Slack, VS Code, Terminal, a password field.

### M4: History and dictionary

- SQLite store, audio files, retention jobs (two limits), History view with search, playback, and waveform, Dictionary view, replacements in the pipeline, vocabulary boosting.
- Check:
  - Retention tests show that audio expiry keeps the entry.
  - Replacement tests cover case, word boundaries, and overlapping rules.
  - A migration test creates the database from scratch.

### M5: AI styles

- Enhancer trait, the three providers, the mode ladder, schema validation, style files with built-in overrides, the Styles view, Keychain secrets.
- Check:
  - Provider tests run against a local mock HTTP server: strict mode, fallback modes, bad JSON, timeout → raw text.
  - CLI providers are tested with fake `claude` and `codex` scripts on `PATH`.

### M6: Onboarding

- All seven steps, live permission detection, the inline mic test, hotkey conflict detection, practice with real dictation, quit and resume.
- Check: a fresh-user run on a clean macOS user account (or with permissions reset by `tccutil reset`) reaches Done with no restart.

### M7: Polish

- Generated sounds, the full ink set with a contrast test, motion pass, reduce-motion, performance pass, launch at login, notarized build.
- Check:
  - Idle RAM and CPU measured and written down.
  - The contrast test passes for every ink in both modes.
  - A notarized build passes `spctl --assess`.

## 6. Risks

| Risk | Plan |
|---|---|
| gpui-pre and gpui-kit break APIs every week | Pin exact versions. Update in a separate change with a test run. Keep our own components thin over gpui-kit. |
| The overlay takes focus or shows a border | S1 proves it first. Fallback: set the NSPanel style directly through objc2. |
| Cold Neural Engine compile is slow | S3 measures it. Onboarding shows "Optimizing for your Mac" and warms the models while the user does the permission steps. |
| Streaming on macOS 27 with an M5 is broken in some non-Core ML paths | Use FluidAudio's Core ML path. S3 tests streaming on this exact machine. |
| Fn and secure input block hotkeys | Default to Option+Space. Detect Secure Event Input and tell the user why the hotkey does not work. |
| Claude CLI policy | Opt-in, labeled, and we never touch the credentials. Check again before a public launch. |
| GPUI cannot draw an effect | Path, gradient, and shadow first, then per-frame CPU images, then a native Metal layer for the overlay only. |

## 7. Spike results (2026-10-01)

Details, evidence, and the remaining human checks are in `spikes/*/RESULT.md`.

| Spike | Result | What it changes |
|---|---|---|
| S1 overlay | Pass. In 5 clean runs the target app stayed frontmost, the overlay was never key, and it drew at 107 to 115 fps. `setHasShadow(false)` removes the transparent-border bug. | Set `inactive_frame_interval: None`, or a PopUp drops to about 26 fps. In 3 early runs the overlay became key while our app was active. Guard this case in M3. |
| S2 tray popover | Pass. The popover opens under the icon and closes on an outside click, a second icon click, and Esc. The accessory policy sticks. | Ignore global clicks inside the icon rect. `gpui-pre 0.3.7` needs Rust 1.98.1, so pin it in `rust-toolchain.toml`. |
| S3 engine | Pass. WhisperKit 1.1.0 and FluidAudio 0.17.5 build into one 21 MB binary. Both accept a custom model directory. | Parakeet batch: 10.9 s cold load, 0.13 s warm, 58 ms for 8 s of speech. Whisper large-v3: 62 s cold. Throttle FluidAudio download progress. Skip the download call when the files exist. |
| S4 signing | Pass for signing and hardened runtime. Not notarized, because that needs approval. | The grant-survives-rebuild test needs a human. Record audio in the Rust app and send it to the engine, so the mic grant stays on the app. |
| S5 hotkeys | Pass with synthetic events. Use `global-hotkey` (Carbon) for the toggle chord, and our own listen-only CGEventTap for push-to-talk and double Esc. | Carbon cannot detect another app that uses the same chord. Use `CopySymbolicHotKeys` for system shortcuts, plus a list of known apps (for example ChatGPT and Option+Space) that we check against running apps. Push-to-talk needs Input Monitoring. |

## 8. Build status (2026-10-01)

M1 to M6 are built, and M7 is partly done. Run `scripts/bundle.sh`, then `open build/Sayso.app`.

Verified:
- `cargo test --workspace`: about 240 tests pass. `cargo clippy --workspace --all-targets -D warnings` and `scripts/check-deps.sh` pass.
- End to end, with `SAYSO_FAKE_MIC` and a synthetic Option+Space (the real Carbon hotkey):
  - Capture, the live preview in the overlay, the final pass, and insertion into TextEdit with the clipboard restored all work.
  - The final pass takes 72 ms for 8 s of speech, warm. The 30 s clip is transcribed correctly.
- Every Hub page, every Settings page, all onboarding steps, the popover, and the overlay states were compared with the Paper boards in light and dark.
- Idle use, release build: about 1.3% CPU and a 33 MB footprint for the app, plus about 45 MB for the engine.

Fixed during integration:
- The overlay panel took key focus and pastes went nowhere. It now has a runtime subclass that cannot become key.
- A pending key recorder blocked hotkeys. It now expires after 15 s.
- Audio device ids and names were mixed up. Both are now accepted.

Not verified yet (needs a person):
- The microphone and Accessibility prompts for the bundle.
- Real key presses for Fn and Right Option push-to-talk.
- Secure Input with a real password field.
- Notarization (`notarize.sh` in spikes/engine). It needs approval, because it uploads the app to Apple.

Known gaps:
- No letter spacing (GPUI has none).
- "Hold to speak" in the Dictionary "Try a phrase" tray is not built.
- The app icon is a first draft.
