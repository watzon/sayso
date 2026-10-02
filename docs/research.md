# Sayso research notes

Research date: 2026-10-01. These notes are input for the v0.1 plan. Claims marked "inference" are our reasoning, not a verified fact. Claims marked "unverified" need a check before we depend on them.

## 1. UI stack: GPUI and GPUI Kit

### GPUI

- The `gpui` crate on crates.io is stale. The last release is 0.2.2 (2025-10-22). Source: [crates.io](https://crates.io/api/v1/crates/gpui).
- Zed replaced the Blade renderer with wgpu on Linux in PR #46758 (2026-02-13). That change is not in a crates.io `gpui` release. Source: [PR #46758](https://github.com/zed-industries/zed/pull/46758).
- `gpui-pre` is a weekly crates.io snapshot of Zed's in-tree GPUI. The current version is 0.3.7 (2026-09-28). Each snapshot can break the API. Source: [crates.io](https://crates.io/api/v1/crates/gpui-pre).
- Each platform uses a different renderer. Source: [GPUI README](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/README.md), [gpui_windows](https://github.com/zed-industries/zed/tree/main/crates/gpui_windows/src).
  - macOS: Metal.
  - Windows: DirectX.
  - Linux: wgpu (Vulkan) on X11 and Wayland.

### GPUI Kit

- "GPUI Kit" is [longbridge/gpui-kit](https://github.com/longbridge/gpui-kit). It is the renamed `gpui-component`.
  - License: Apache-2.0.
  - Activity: 15.5k stars. v0.7.0 shipped 2026-09-28, and releases come about once a week.
  - Components: it has 75+. These include button, input, select, switch, slider, kbd, `setting`, `form`, dialog, sheet, popover, tooltip, notification, menu, list, virtual_list, table, tab, sidebar, `title_bar`, theme, icon, progress, spinner, skeleton, and chart.
  - GPUI pin: it pins `gpui-pre =0.3.7` exactly, because a caret range once broke apps. Source: [Cargo.toml](https://raw.githubusercontent.com/longbridge/gpui-kit/main/Cargo.toml).
- Migration cost between minor versions is real. One app had 54 compile errors moving from 0.5 to 0.6. Source: [dbflux #601](https://github.com/0xErwin1/dbflux/issues/601).
- Do not confuse it with [iamnbutler/gpuikit](https://github.com/iamnbutler/gpuikit). That is a different, smaller project.

### What a dictation app needs from the UI layer

| Need | Status | Source |
|---|---|---|
| Non-activating floating overlay | `WindowKind::PopUp` creates an NSPanel with `NonactivatingPanel`, pop-up level, and all-Spaces. Use `focus: false` and a transparent background. The panel class returns YES for `canBecomeKeyWindow`, so we must prove focus behavior in a prototype. | [gpui_macos window.rs](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui_macos/src/window.rs) |
| Window position, hide, level | Not in `PlatformWindow`. Use objc2 on the raw handle. | [hi5 panel.rs](https://raw.githubusercontent.com/jondot/hi5/main/crates/hi5-gpui/src/platform/panel.rs) |
| Menu-bar-only app (no Dock icon) | Not supported. GPUI hardcodes `ActivationPolicyRegular`. Open PR #64866 adds it. Until it merges, call `setActivationPolicy` through objc2 after launch. | [platform.rs](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui_macos/src/platform.rs), [PR #64866](https://github.com/zed-industries/zed/pull/64866) |
| Tray icon | Not in GPUI. Use `tray-icon` 0.26 (macOS, Windows, Linux). | [tray-icon](https://raw.githubusercontent.com/tauri-apps/tray-icon/dev/README.md) |
| Linux overlay | `WindowKind::LayerShell` exists for Wayland. | [platform.rs](https://raw.githubusercontent.com/zed-industries/zed/main/crates/gpui/src/platform.rs) |
| Waveform animation | `canvas` and `animation` elements exist. Inference: possible, but needs a prototype. | GPUI source |

The closest existing GPUI app to Sayso is [hi5](https://github.com/jondot/hi5): a macOS menu-bar app with a tray, a global hotkey, and a non-activating panel.

## 2. Global hotkeys

- `global-hotkey` 0.8 uses Carbon `RegisterEventHotKey` on macOS. Inference: it cannot detect Fn alone or a modifier-only key. Source: [source](https://raw.githubusercontent.com/tauri-apps/global-hotkey/dev/src/platform_impl/macos/mod.rs).
- `handy-keys` 0.3.4 (MIT) gives modifier-only hotkeys, including Fn, with key-down and key-up events. It uses a CGEventTap on macOS, low-level hooks on Windows, and evdev on Linux. Unverified on real hardware. Source: [handy-keys](https://github.com/handy-computer/handy-keys).
- On macOS, Fn arrives as a `flagsChanged` event with keycode 63 and the SecondaryFn flag. Arrow and function keys also set that flag, so filter by keycode. Source: [handy-keys keycode.rs](https://raw.githubusercontent.com/handy-computer/handy-keys/main/src/platform/macos/keycode.rs).
- A tap that swallows Fn events can break the system double-Fn dictation shortcut. Use a listen-only tap. Source: [kitty #9661](https://github.com/kovidgoyal/kitty/issues/9661).
- You can stop the emoji picker from also firing on Fn. Write `AppleFnUsageType=0` to `com.apple.HIToolbox` and call the private `TISUpdateFnUsageType`. Restore the value on exit and after a crash. This is a private API. Source: [Inputalk PR #10](https://github.com/sebi75/inputalk/pull/10).
- Fn detection works only on Apple keyboards. Source: [Handy](https://github.com/cjpais/Handy).
- Secure Event Input blocks hotkeys. Password fields, Terminal secure entry, and 1Password all turn it on. Source: [Wispr docs](https://docs.wisprflow.ai/articles/8841649969-fix-flow-shortcuts-blocked-by-macos-secure-keyboard-entry-secure-event-input).

## 3. Speech recognition

### WhisperKit

- WhisperKit now lives in [argmaxinc/argmax-oss-swift](https://github.com/argmaxinc/argmax-oss-swift). It sits with SpeakerKit and TTSKit.
  - License: MIT.
  - Version: v1.1.0 (2026-08-06).
  - Hardware: Apple Silicon only.
  - OS: the README says macOS 14+, and `Package.swift` says 13+. Use 14 as the minimum.
- Models come from Hugging Face [argmaxinc/whisperkit-coreml](https://huggingface.co/argmaxinc/whisperkit-coreml/tree/main). The recommended model is `large-v3-v20240930_626MB`. Other models range from tiny to large-v3, plus turbo and distil variants. Quantized large models are about 550 to 950 MB.
- Open-source features: word and segment timestamps, language detection, VAD chunking, a prompt text option, temperature, and output streaming.
- `AudioStreamTranscriber` re-transcribes the buffer about once a second. Argmax lists "real-time transcription" and custom vocabulary (3,000 keywords) as Pro-only. Source: [OSS vs Pro](https://app.argmaxinc.com/docs/wiki/open-source-vs-pro-sdk).
- The first model load compiles the model for the Neural Engine. This takes seconds for turbo models and up to minutes for large ones (anecdotal). The OS caches the result. Source: [low-talker PR #77](https://github.com/brandon-fryslie/low-talker/pull/77).
- The Pro SDK costs $1 to $1.33 per device per month, with a minimum of 1,000 licenses. That does not fit a free app. Source: [pricing](https://www.argmaxinc.com/pricing).

### Bridge from Rust to WhisperKit

| Option | Pros | Cons |
|---|---|---|
| Swift sidecar helper, NDJSON over stdio | Swift and Cargo builds stay separate. Core ML crashes and memory spikes stay out of the UI process. The seam maps cleanly onto a backend trait. Parrot uses this approach. | One extra process. We must bundle and sign the helper. |
| Swift static library with `@_cdecl` C ABI, linked in `build.rs` | In-process, lowest latency. | Mixed SwiftPM and Cargo build. A Swift crash takes down the app. Async-to-callback lifetime management. |
| `argmax-cli serve` (OpenAI-compatible HTTP) | No code to write. | HTTP multipart latency on short clips. Less control. |
| `coremlit` (pure Rust port) | No Swift. | Pre-release. Parity tested only on tiny. |

Recommendation: a Swift sidecar with an NDJSON protocol, behind a Rust `SttBackend` trait. If we later move to an in-process build, only the transport changes. Sources: [Parrot native-core](https://raw.githubusercontent.com/basic-intelligence/parrot/main/native-core/README.md), [fluidaudio-rs](https://github.com/FluidInference/fluidaudio-rs).

Unverified: how notarization handles a bundled Swift helper. Test this early.

### Linux and Windows backends

- [transcribe-rs](https://github.com/cjpais/transcribe-rs) (MIT) puts whisper.cpp, Parakeet, Moonshine, and SenseVoice behind one API. It supports CUDA, ROCm, DirectML, Vulkan, and Core ML. Handy uses it. Inference: this is the best base for Linux and Windows.
- Parakeet TDT 0.6B v3: 6.34% average WER, 25 European languages, CC-BY-4.0. Source: [HF](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3).
- On macOS, FluidAudio gives free Parakeet through Core ML. It is a possible second engine.
- `whisper-rs` development moved to Codeberg, and the GitHub repo is archived.

### Post-processing

- Whisper and Parakeet already output punctuation and capitals.
- Remove filler words, apply dictionary replacements, and handle spoken commands ("new line") with plain Rust rules.
- For local LLM cleanup (later), use `llama-cpp-2` (cross-platform) as the default. Apple Foundation Models (macOS 26+, via the `foundation-models` crate) is a fast path on macOS.

## 4. macOS integration

### Permissions

| Permission | Why | API | Settings deep link |
|---|---|---|---|
| Microphone | Record audio | `AVCaptureDevice.authorizationStatus(for: .audio)` / `requestAccess`. Needs `NSMicrophoneUsageDescription` and, with the hardened runtime, the `com.apple.security.device.audio-input` entitlement. Without the entitlement, macOS denies silently. | `x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone` |
| Accessibility | Post Cmd+V, read AX context, filtering event tap | `AXIsProcessTrustedWithOptions` | `...?Privacy_Accessibility` |
| Input Monitoring | Listen-only event tap | `CGPreflightListenEventAccess` / `CGRequestListenEventAccess` | `...?Privacy_ListenEvent` |

- Do not trust a cached `AXIsProcessTrusted()`. Try to create the event tap live, and retry on app activation or with a poll. Sources: [mach-voice #8](https://github.com/augustomklee/mach-voice/issues/8), [Espanso #2813](https://github.com/espanso/espanso/issues/2813).
- macOS 27 renames the Accessibility pane to "Device Control and Data Access". The old deep link still works. Source: [Sorla #74](https://github.com/markstrom/sorla/issues/74).
- Open settings URLs with `NSWorkspace` or `open`.
- Ad-hoc signing resets permission grants after each update. Use a stable Developer ID signature, even for development builds. Source: [parley #75](https://github.com/pathorsAI/parley/issues/75).

### Text insertion

| Method | Pros | Cons |
|---|---|---|
| Clipboard + Cmd+V (`CGEventPost`, global), then restore | Works in native, Electron, browser, and terminal apps | Overwrites the clipboard for about 0.5 s. A fixed restore delay fails under load. |
| AX set selected text or value | No clipboard | Many Chromium and Electron fields do not support it. |
| `CGEventKeyboardSetUnicodeString` typing | No clipboard. Can stream. | 20 UTF-16 units per event. Slow for long text. |

Inference: paste is the default, typing is a per-app fallback, and AX is for reading context. Sources: [enigo #68](https://github.com/enigo-rs/enigo/issues/68), [quoth #49](https://github.com/ryan-stoffel/quoth/issues/49).

### Distribution

- The Mac App Store is not practical. The sandbox blocks AX control of other apps, and Apple rejected a dictation app under guideline 2.4.5. Ship a notarized Developer ID build. Sources: [MITM LLC](https://www.mitmllc.com/blog/apple-rejected-my-dictation-app/), [HN](https://news.ycombinator.com/item?id=48369088).
- Launch at login: `SMAppService.mainApp.register()` (macOS 13+). Handle `.requiresApproval`.
- Menu bar only: `LSUIElement=true`, plus the activation policy fix in section 1.

## 5. Windows and Linux (for the platform seams)

- Windows:
  - `RegisterHotKey` has no key-up event, so use a `WH_KEYBOARD_LL` hook for push-to-talk.
  - `SendInput` cannot inject into higher-integrity windows, and it fails silently (UIPI).
  - Source: [Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput).
- Linux on Wayland, text injection:
  - xdotool types only into XWayland apps.
  - wtype does not work on GNOME (Mutter).
  - ydotool needs `/dev/uinput`.
  - The RemoteDesktop portal with libei is the best route.
  - Source: [portal docs](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html).
- Linux on Wayland, hotkeys: the GlobalShortcuts portal has `Activated` and `Deactivated` signals (GNOME 48+, KDE, Hyprland). Source: [portal docs](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html).

Inference: the platform seams are `HotkeySource` (press and release events), `TextInserter` (a chain: paste, then type, then AX), `ContextProvider` (app id, selection), `PermissionGuide`, and `SttBackend`.

## 6. Product landscape

### WisprFlow

- Default hotkeys on Mac: Fn for push-to-talk, Fn+Space for hands-free, Fn+Ctrl for Command Mode. Source: [docs](https://docs.wisprflow.ai/articles/2612050838-supported-unsupported-keyboard-hotkey-shortcuts).
- Flow Bar: white bars show recording. Esc cancels, with Undo and Open History. Ctrl+Cmd+V pastes the last transcript. If insertion fails, a paste prompt appears. Source: [docs](https://docs.wisprflow.ai/articles/6409258247-starting-your-first-dictation).
- Hub: History, Dictionary, Snippets, Style, Settings. Context Awareness puts apps into one of four tone categories.
- Onboarding: sign in, grant Microphone and Accessibility, mic test (the bars must move), choose a hotkey, practise the hotkey, then the data-training choice. You can quit and resume. Source: [setup guide](https://docs.wisprflow.ai/articles/3152211871-setup-guide).
- Business model: cloud-processed. Free up to 2,000 words a week, Pro $12 to $15 a month. $280M Series B in Aug 2026.
- Complaints: privacy (the screenshot incident, secondary source), RAM and CPU use, cloud latency of 1 to 2 s, setup friction, and Windows reliability.

### Competitors

| App | Copy | Avoid |
|---|---|---|
| Superwhisper | Waveform, mode indicator, Esc to cancel, compact mini window, modes switched by hotkey, app rules, or deep link | Price changes, an overlay some people find intrusive |
| VoiceInk (GPL-3.0, Swift) | Local, personal dictionary, per-app Power Mode | Does not accept PRs. Do not copy its GPL code into a non-GPL project. |
| Handy (MIT, Tauri/Rust) | Hold, toggle, or tap modes, History tab, overlay pill, multiple paste methods | A fixed clipboard-restore delay fails under load. The overlay is off by default on Linux. |
| Aqua Voice | Streaming text display, dictionary | Cloud only |
| OpenWhispr | Many engines | Electron |
| Apple Dictation | Built in, on-device | No dictionary, no tone, stops after 30 s of silence |

What we take from this (inference):

- Copy:
  - Esc to cancel, with Undo.
  - "Paste last transcript".
  - A History view.
  - A paste prompt when insertion fails.
  - A compact overlay.
- Avoid:
  - Silent failures.
  - A heavy always-on overlay.
  - A subscription for local features.

## 7. Parakeet and live streaming on macOS (round 2)

- [FluidAudio](https://github.com/FluidInference/FluidAudio) is the free way to run Parakeet on the Neural Engine.
  - Version: v0.17.5 (2026-10-01), released about weekly. Pin an exact version.
  - License: Apache-2.0.
  - Platform: macOS 14+, Swift tools 6.0, no dependencies.
- Recommended models. All are English, have punctuation and capitals, and use the NVIDIA Open Model License.

  | Use | Model | Size | Quality and speed |
  |---|---|---|---|
  | Final pass (default) | Parakeet Unified EN 0.6B, batch export | about 595 MB | 2.16% WER, 144x real time |
  | Live preview | Parakeet Unified EN 0.6B, streaming export, 320 ms or 640 ms tier | about 590 MB | 2.37% or 2.40% WER |
  | Small preview fallback | Parakeet EOU 120M | about 222 MB | 4.87% WER, no punctuation, so the preview jumps more |
  | Multilingual later | Parakeet Ultra (v3 post-trained) | 595 MB | 25 languages |

  Sources: [Models.md](https://github.com/FluidInference/FluidAudio/blob/main/Documentation/Models.md), [Unified card](https://huggingface.co/FluidInference/parakeet-unified-en-0.6b-coreml).
- The batch and streaming exports of Unified share one checkpoint. Inference: the preview and the final text should be close, so less text jumps.
- Streaming API: `StreamingUnifiedAsrManager` (`appendAudio`, partial transcript callback, `finish()`, word timings). Source: [source](https://github.com/FluidInference/FluidAudio/blob/main/Sources/FluidAudio/ASR/Parakeet/Streaming/StreamingAsrManager.swift).
- Custom vocabulary: `configureVocabularyBoosting` uses CTC keyword spotting with an extra 97.5 MB CTC 110M model. It works best on the full-audio final pass. Source: [CustomVocabulary.md](https://github.com/FluidInference/FluidAudio/blob/main/Documentation/ASR/CustomVocabulary.md).
- WhisperKit and FluidAudio can live in one Swift package. Hex ships both. Build a spike to confirm. Source: [Hex Package.resolved](https://raw.githubusercontent.com/kitlangton/Hex/main/Hex.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved).
- Risk: Handy reports that a non-Core ML Nemotron streaming path gave 0 updates on macOS 27 with an M5 Pro. Use the Core ML path. Source: [Handy #2137](https://github.com/cjpais/Handy/issues/2137).
- Pattern for preview plus final pass: paste the final batch decode. Show the preview as committed text plus a provisional tail. Source: [voicetypr PR #147](https://github.com/ideaplexa/voicetypr/pull/147).
- Unverified:
  - Cold Neural Engine compile time for the Unified models. Measure it.
  - The exact license wording. The FluidInference repo is tagged cc-by-4.0, and the NVIDIA base model uses the NVIDIA Open Model License.

## 8. Rendering: shaders, overlay, tray popover, fonts, theming (round 2)

- GPUI has no custom shader API.
  - The `Primitive` enum is closed. The custom shader PR #42905 and the blur PRs were closed without merging.
  - Dynamic textures (PR #64061) are open and not merged.
  - Sources: [PR #42905](https://github.com/zed-industries/zed/pull/42905), [PR #64061](https://github.com/zed-industries/zed/pull/64061).
- Primitives that exist:
  - `canvas`.
  - `paint_path` with `PathBuilder` (curves, strokes, dashes).
  - Quads with per-corner radii.
  - `paint_image` (BGRA).
  - SVG.
  - Box shadows with blur, and inset shadows.
  - Opacity.
  - Linear gradients.
- Missing primitives: radial gradients, filters or blur on content, non-rectangular masks, blend modes.
- The `gpui-ce` fork has backdrop blur and a custom GPU rendering API (PR #237). gpui-kit pins `gpui-pre`, so moving to it is a big step. Source: [gpui-ce](https://github.com/gpui-ce/gpui-ce).
- Workarounds for effects:
  - Rasterize on the CPU into a `RenderImage` every frame. Call `drop_image` on the old frame, or the atlas grows.
  - Add a native child view (CAMetalLayer) above the GPUI content. gpui-kit's `gpui-wry` already adds a subview this way. This is macOS-only code, and the view always draws on top.
- Animation:
  - `Animation` has easings, springs, `with_max_fps`, and `reduce_motion`.
  - `request_animation_frame` is driven by `CVDisplayLink` on macOS.
- Known issues:
  - A `PopUp` window with a transparent background shows a border ([#61508](https://github.com/zed-industries/zed/issues/61508)).
  - Blurred background may show no blur on macOS 27 ([#64284](https://github.com/zed-industries/zed/issues/64284)).
- Tray popover pattern (from hi5):
  - Use tray-icon with no menu, and open on click down.
  - Get the icon rect from the status item window. `TrayIconEvent::rect` is in physical pixels on macOS.
  - Show one hidden `PopUp` window. Position it with objc2 `setFrameOrigin:`. Hide it with `orderOut:`.
  - Dismiss it with `NSEvent` global monitors.
  - Linux has no icon rect.
- Fonts:
  - `add_fonts` loads bundled fonts. OpenType feature tags are supported.
  - Variable fonts loaded with `add_fonts` render at weight 400 on cosmic-text ([#64783](https://github.com/zed-industries/zed/issues/64783)). Bundle static weights.
- gpui-kit theming:
  - `ThemeConfig` JSON covers about 129 color keys, font, mono font, radius, motion tokens, and a shadow on/off flag.
  - Textured fills and custom shadows need our own components or overrides.

## 9. AI enhancement providers (round 2)

Latency measured on this Mac on 2026-10-01, for one short sentence:

| Route | Time | Schema enforcement |
|---|---|---|
| `claude -p --model haiku --effort low`, with `MAX_THINKING_TOKENS=0` | 1.7 to 1.9 s | `--json-schema` gives a validated `structured_output`. It works with the user's login, but not with `--bare`. |
| `codex exec --ephemeral --output-schema` | 4.7 to 5.9 s | Uses about 21k tokens of agent overhead per call. A ChatGPT login does not allow small models. |
| OpenAI-compatible HTTP | not measured. Estimate 0.3 to 0.8 s with a small model. | See below. |

- OpenRouter:
  - `response_format.json_schema` is honored per endpoint.
  - `provider.require_parameters: true` limits routing to endpoints that support it. `data_collection: "deny"` and `zdr: true` give privacy-safe routing.
  - Sources: [structured outputs](https://openrouter.ai/docs/features/structured-outputs), [routing](https://openrouter.ai/docs/features/provider-routing).
- Local servers:
  - Ollama: `format` schema on the native API.
  - LM Studio: `response_format`. Models smaller than 7B often fail.
  - llama.cpp server: `json_schema` and grammar.
- Mode ladder for OpenAI-compatible endpoints:
  1. Strict `json_schema`.
  2. `json_object` with the schema in the prompt.
  3. A forced tool call.
  4. Validate locally, retry once, and if it still fails, insert the raw transcript.
- CLI hygiene:
  - Run in a neutral temp directory, with tools disabled.
  - Claude: use `--no-session-persistence` and `--setting-sources ""`.
  - Codex: use `--ephemeral`, `--sandbox read-only`, and `--ignore-user-config`.
  - Never read the CLI credential files.
- Policy risk: Anthropic does not allow third-party developers to route requests through a user's Free, Pro, or Max login. It is unverified whether that covers starting the user's own unmodified `claude` binary. Source: [legal and compliance](https://code.claude.com/docs/en/legal-and-compliance).
- Rust:
  - Use plain `reqwest` + `serde_json` (or async-openai 0.42 with a custom base URL).
  - Store API keys with `keyring` 4.2 and the `apple-native-keyring-store` feature.
