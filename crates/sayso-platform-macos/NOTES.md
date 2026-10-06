# sayso-platform-macos

macOS implementations of the `sayso-platform` traits. Build all of them with
`MacPlatform::new(sounds_dir)`. Call it on the main thread. It does not depend on GPUI.

## Modules

| Module | What it does |
|---|---|
| `hotkeys` | `HotkeySource`. Chords go through Carbon with `global-hotkey`. Push-to-talk, Esc, and the key recorder go through the tap. |
| `tap`, `tap_logic` | A listen-only `CGEventTap` on its own thread with its own run loop. `tap_logic` holds the decisions (no FFI). `tap` holds the thread, the C callback, and re-enable after a timeout. |
| `esc` | Double-Esc and single-Esc state machine. It takes the time of each press as an argument. |
| `keymap` | `Hotkey` to key code, `CGEventFlags`, Carbon mask, and `global-hotkey` types. Solo modifiers use device bits (Right Option is `0x40`). |
| `conflicts` | System shortcuts from `CopySymbolicHotKeys` (Carbon mask), a name table, and a table of apps (ChatGPT, Raycast, Alfred) checked against running apps. |
| `secure_input` | `IsSecureEventInputEnabled` plus the PID from `CGSessionCopyCurrentDictionary`, turned into an app name. |
| `inserter` | Paste (save and restore every pasteboard item, Cmd+V) and Type (Unicode strings in chunks of 20 UTF-16 units). Checks Accessibility, Secure Input, and focus first. |
| `context` | Frontmost app, running apps (regular and accessory), app icon as PNG at a given size. Installed apps: `*.app` in `/Applications`, the system folders, and `~/Applications`, plus one level of sub-folders in the two user-visible roots, plus running apps. Page URL: `AXDocument` of the focused window. |
| `permissions` | Microphone (AVFoundation), Accessibility, Input Monitoring, and System Settings deep links. |
| `audio`, `resample` | `cpal` input, mono mix, own streaming windowed-sinc resampler to 16 kHz, 20 ms frames, display level. |
| `sounds` | `rodio` on a worker thread. It opens the output on the first sound and closes it after 30 s of silence. |
| `login_item` | `SMAppService.mainApp`. |
| `prefs` | Dark mode from the `AppleInterfaceStyle` default. Reduce motion from `NSWorkspace`. |
| `window` | objc2 helpers for the UI (overlay, dock icon, screens, click monitor, status item rect, full-screen check). |

`examples/smoke.rs` prints the state of every service, records 2 s, and plays the sounds.

## Verified on hardware (this Mac, macOS 27, terminal host)

- Permission states, device list with default, frontmost app, 108 running apps, app icon PNG.
- Conflicts: Raycast found for `opt+space`. Spotlight, input source, and Emoji rows found with the right `enabled` flag.
- Secure Input query (false here), dark mode, reduce motion, login item state (`Disabled` for an unbundled binary).
- Hotkey registration of toggle, paste-last, cycle-style, and a solo push-to-talk. The tap thread started with Input Monitoring granted.
- Capture with `cpal`: 99 frames in 2 s at 16 kHz.
- Playing the four sounds returned no error.
- The sound files: 44.1 kHz mono 16-bit, 60 to 180 ms, peak -12 dBFS.

## Only compiled and unit tested

- Real key events through the tap (PTT, double Esc, key recorder). The logic has unit tests. The spike proved the same logic with synthetic events.
- Carbon hotkey delivery. It needs the GPUI event loop.
- Paste, type, and clipboard restore. Not run, because they post keys into the focused app.
- Microphone level: the terminal has no microphone grant (state `NotDetermined`), so macOS gives silence. Peak level was 0.000. Level scaling is covered by unit tests only.
- Audible playback of the sounds. Nobody listened.
- Everything in `window`, except the pure full-screen check and the null and thread guards. These need a real window and the main thread.
- `request` for the microphone. A binary with no `NSMicrophoneUsageDescription` is killed by macOS when it asks, so it was not called.
- Login item register and unregister. It needs a bundled app.

## Known gaps and decisions

- A modifier key on its own works only for push-to-talk. A solo binding for toggle, paste-last, or cycle-style is reported in `failed`.
- Chords that include `fn` work for push-to-talk only. Arrow keys and F keys set the Fn flag on their own, so the key recorder ignores it.
- Secure Event Input hides key events from the tap. Solo-modifier push-to-talk still works. Chord push-to-talk, Esc, and the key recorder do not (spike result). The Carbon chords still work.
- `Accessibility` and `InputMonitoring` report `Denied`, never `NotDetermined`. macOS gives no way to tell them apart.
- `page_url` reads one attribute on the app and one on the window with a 0.25 s Accessibility timeout. It never walks the tree (a walk took 18 s on Mail). Chromium browsers answer. Electron apps answer an empty string, which is `None`. It needs Accessibility.
- `installed_apps` looks only one level into sub-folders of `/Applications` and `~/Applications`. An app deeper than that shows up only while it runs.
- A missing focused element is `NoFocusedField` only when Accessibility answers "no value". Other Accessibility errors (some Electron apps) are treated as "a field may be focused".
- While the key recorder is open, Carbon chord events are dropped, so the toggle does not fire while the user sets a new key.
- If a push-to-talk key is held when the system disables the tap, the logic sends `PushToTalkUp` after the tap is back.
- Dark mode reads a user default, not `NSApp.effectiveAppearance`. It follows the system, not an app override.
- The window helpers are safe functions that take a raw pointer. The caller must pass a live `NSView` or `NSWindow`. A null pointer or a call off the main thread does nothing.
- `configure_overlay` cannot override `canBecomeKeyWindow`. It sets `becomesKeyOnlyIfNeeded` and the non-activating panel style on the panel, and `order_front_without_activating` resigns key and deactivates the app when the overlay became key. Check this on hardware with the real GPUI window.
- The resampler is our own (Hann-windowed sinc, 10 zero crossings). Its tail holds back up to 20 output samples at the end of a stream.
- No `rubato` dependency. Versions in this crate: `cpal` 0.18, `rodio` 0.22, `global-hotkey` 0.8, `objc2` 0.6.
- `install_global_click_monitor` always returns a token. `MonitorToken::is_active` tells whether macOS created the monitor.
