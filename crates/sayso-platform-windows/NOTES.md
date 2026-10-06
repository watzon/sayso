# sayso-platform-windows

Windows implementations of the `sayso-platform` traits. Build all of them with
`WinPlatform::new(sounds_dir)`. Call it on the main thread, the one that runs the
message loop. It does not depend on GPUI.

The crate is empty on other platforms (`#![cfg(windows)]`, and every platform
dependency sits under `[target.'cfg(windows)'.dependencies]`).

## Modules

| Module | What it does |
|---|---|
| `hotkeys` | `HotkeySource`. Chords go through `RegisterHotKey` with `global-hotkey`. Push-to-talk, Esc, and the key recorder go through the hook. |
| `hook`, `hook_logic` | A listen-only `WH_KEYBOARD_LL` hook on its own thread with its own message loop, installed only while push-to-talk is bound, a dictation records, or the key recorder is open. `hook_logic` holds the decisions (no FFI). `hook` holds the thread, the callback, and the mask key. |
| `esc` | Double-Esc and single-Esc state machine. Copied from the macOS crate. |
| `keymap` | `Hotkey` to virtual-key code, `RegisterHotKey` flags, and `global-hotkey` types. Solo modifiers are `VK_RMENU`, `VK_LMENU`, `VK_RWIN`, `VK_RCONTROL`, `VK_RSHIFT`. |
| `conflicts` | A table of Windows shortcuts (every Win+letter counts), a table of apps (PowerToys Run, Command Palette, Copilot, ChatGPT, Raycast, Flow Launcher, Wox) checked against running processes, and a real `RegisterHotKey` probe. |
| `input` | `SendInput` key lists (paste, typing, mask key, releasing held modifiers). |
| `inserter` | Paste (save every memory clipboard format, publish with delayed rendering, Ctrl+V, wait for the read receipt, restore) and Type (Unicode key events in chunks of 20 UTF-16 units). Checks the foreground window and its integrity level first. |
| `paste_receipt` | When did the target app read the text. Copied from the macOS crate. |
| `context` | Frontmost app, running apps (visible app windows, one per exe), app icon as PNG at a given size, process integrity levels. `installed_apps` (the App Paths registry keys plus the running apps), `reads_page_url` (always true), and `page_url` (the address bar of a known browser in front, read with UI Automation). |
| `permissions` | Microphone from the privacy consent store. Accessibility and Input Monitoring are always granted. |
| `audio`, `resample` | `cpal` (WASAPI) input, mono mix, streaming windowed-sinc resampler to 16 kHz, 20 ms frames, display level. Copied from the macOS crate. |
| `sounds` | `rodio` on a worker thread. Copied from the macOS crate. |
| `login_item` | The `Run` key of the current user, plus the Task Manager switch in `StartupApproved\Run`. |
| `prefs` | Dark mode from `AppsUseLightTheme`. Reduce motion from `SPI_GETCLIENTAREAANIMATION`. |
| `win32` | Wide strings, environment variables, registry values and key names, owned handles. |

The app-shell window glue (`window`) is not part of this crate yet.

`examples/smoke_windows.rs` prints the state of every service, probes a few chords,
records from each input device, and plays the sounds. Its "style rules" section prints
the installed apps, then waits 5 s and prints `page_url` with its time for the app in
front and for each running browser.

## Verified on hardware (this PC, Windows 11 Pro 26200, terminal host)

- Permission states (microphone `Granted` from the consent store), device list with
  default, frontmost app, running apps (4 visible app windows), app icon PNG at 64 px
  and 32 px.
- Conflicts: Alt+Space gives "Window menu". Win+Space, Win+V, and Ctrl+Win+V give
  their Windows names. The raw probe found Win+V, Win+Space, and Ctrl+Win+V taken
  (Explorer holds them) and Alt+Space, Alt+Shift+V, and Ctrl+Alt+Space free. A chord
  held by another thread probes as taken, and free again once it is released.
- Hotkey registration of toggle, paste-last, cycle-style, and a solo push-to-talk
  (the hook thread started). Fn push-to-talk is reported in `failed`.
- A registered chord fires `Toggle`: an ignored test sends Ctrl+Alt+Shift+F18 with
  `SendInput` to a window of its own and pumps the message loop.
- The hook sees injected keys and ignores them: an ignored test sends Right Alt to
  its own window with Right Alt bound as push-to-talk, and no event comes.
- The real clipboard (ignored tests, with the user's clipboard saved and restored,
  checked by text and format list): the delayed text gives a receipt on read, the
  old clipboard comes back, a paste with no read leaves plain text, a newer copy is
  kept, and the skip formats are on the clipboard while the text is.
- Paste and Type end to end into an EDIT control of a test window: Ctrl+V with the
  receipt and the restore, then typed text with an emoji and a line break.
- A clipboard monitor (Parsec here) reads every clipboard change, ignores the skip
  formats, and would have faked a receipt. Readers that are not the app in front are
  now refused (see below). An ignored test shows that a refused read leaves the text
  delayed, so the next reader still gets it.
- Login item: state `Disabled`; an ignored test turns it on and off in the registry
  and puts back what was there.
- Dark mode, reduce motion.
- Capture: 48 frames in 1 s at 16 kHz from the Steam Streaming Microphone. The default
  device (Parsec Virtual Audio) gave no frames: a virtual device that sends no buffers
  while nothing streams to it.
- Playing the four sounds returned no error.

## Only compiled and unit tested

- Real (not injected) key events through the hook: push-to-talk, double Esc, the key
  recorder, AltGr, and the mask key. They need a person at the keyboard. The logic has
  unit tests with the event sequences Windows sends.
- Insertion into real apps (Chrome, Electron, Office, terminals, Store apps). Not run,
  because it would send keys to the user's apps.
- The "runs as administrator" check against a real elevated app.
- `request` and `open_settings` for the microphone (they open Settings).
- Store app frames (`ApplicationFrameHost.exe`) in `frontmost_app`: no Store app was in front.
- Microphone level: the terminal mic gave silence. Level scaling has unit tests.
- Audible playback. Nobody listened.

## Not verified on hardware

The code for style rules (`installed_apps`, `page_url`, and the `win32` helpers
`subkey_names` and `expand_env`) was written on a Mac. It compiles for
`x86_64-pc-windows-msvc` and passes clippy there. It never ran, and its unit tests
never ran. Check these on a Windows PC, with `cargo test -p sayso-platform-windows`
and the "style rules" section of the smoke example:

- `page_url` returns the address for Chrome, Edge, and Firefox when the browser
  window is in front. Then try Brave, Opera, Vivaldi, Arc, and Zen if they are there.
- `page_url` takes less than about 300 ms, also for the first call after the browser
  starts, and returns `None` at once for an app that is not a browser.
- The read does not turn on the accessibility mode of Chromium for web pages. Open
  `chrome://accessibility` (or `edge://accessibility`) before and after a read. "Native
  accessibility API support" is expected to turn on. "Web accessibility" and the modes
  after it must stay off.
- The address bar is what the walk finds first. Check with the find bar open, with a
  side panel open, with vertical tabs in Edge, and with the search box added to the
  Firefox toolbar.
- The value of the address bar. Browsers can hide `https://` and `www.` there. Write
  down what the value is for a plain page and while the user types.
- `installed_apps` lists the apps a user expects (Chrome, Office, VS Code), names
  them well, and returns in a time that is fine for a picker. The first call reads
  the version resource of every exe.
- `app_icon_png` gives an icon for an installed app that does not run.

## Known gaps and decisions

- **Ids and names.** `AppInfo.bundle_id` is the lowercase exe file name (`code.exe`).
  The name is the exe's FileDescription, else the file name without `.exe`.
- **Installed apps.** Windows has no list of apps with their exe. `installed_apps`
  reads `App Paths` under HKCU and HKLM, where most installers register an exe, and
  adds the running apps. An app with no entry that does not run is missing (many
  Store apps, portable apps). Some entries are tools and not apps. The id is the file
  name of the exe the entry points to, not the key name, because the key name can be
  an alias and a running app is known by its real exe.
- **Page address.** Only for the browsers in `browser_kind`, and only when the browser
  owns the foreground window. A tree walker goes through the controls of the window
  depth first, in window order, and stops at the first Edit control (Chromium) or the
  Edit with the id `urlbar-input` (Firefox). It does not enter a Document (a web page)
  or the tab strip, looks at 300 controls at most, starts no call after 250 ms, and
  sets the UI Automation timeouts to 250 ms. A `FindFirst` over all descendants was
  rejected: it makes the browser search the web page too. Vivaldi draws its whole
  window as a web page, so it is expected to give `None`. A value with white space
  (a search the user types) gives `None`.
- **Fn** never reaches Windows. A hotkey with Fn is reported in `failed`.
- **Mask key.** Releasing Alt alone opens the menu bar of the focused app, and
  releasing Win alone opens Start. When Right Alt, Left Alt, or Right Win goes down as
  a solo push-to-talk key (or in the key recorder), the hook thread sends an
  unassigned key (`0xE8`) down and up, the trick AutoHotkey uses. The hook still
  swallows nothing.
- **AltGr.** Layouts with AltGr send a fake Left Ctrl (scan code bit `0x200`) with
  Right Alt. The logic drops it, so Right Alt works as push-to-talk and AltGr+Q is
  recorded as Alt+Q, not Ctrl+Alt+Q. `RegisterHotKey` sees AltGr as Ctrl+Alt, so a
  chord recorded with AltGr only fires with Left Alt.
- **Modifier state** comes from `GetAsyncKeyState` for every event, so a key-up lost
  to the lock screen or Ctrl+Alt+Del does not stick. Auto-repeat is found by the
  logic (a key-down for a key that is already down).
- **Injected keys** (Sayso's paste and typing, and any other program's) never
  trigger a hotkey.
- **Slow hooks.** Windows removes a low-level hook without notice after about one
  second in the callback. The callback only reads key state, runs the logic, and
  sends on channels. There is no way to detect a removal.
- **Elevated apps.** While an app that runs as administrator is in front, Windows keeps
  its keys from the hook and drops Sayso's synthetic keys. The inserter says so:
  "<App> runs as administrator, so Windows does not let Sayso type into it." A
  process whose integrity level Windows will not show counts as elevated.
- **No foreground window** (lock screen, UAC prompt) is `NoFocusedField`. Windows has
  no reliable focused-element query that works across apps, so that is the only check.
- **Readers of the delayed text.** Windows asks the owner to render only once, and
  clipboard monitors read every change. So only the app in front (or a reader that
  opened the clipboard without a window, which cannot be told apart) gets the text and
  counts as the receipt. Other readers get no data, and the text stays delayed for the
  target. A monitor that opens the clipboard without a window can still take the
  receipt; then the paste reports `NotTaken` and the text stays on the clipboard.
- **Skip formats.** The delayed text always carries `ExcludeClipboardContentFromMonitorProcessing`,
  `CanIncludeInClipboardHistory` = 0, `CanUploadToCloudClipboard` = 0, and
  `Clipboard Viewer Ignore`, also when the clipboard is not restored. Then the plain
  text that replaces it goes into clipboard history as usual. The restored clipboard
  carries them too, so clipboard history does not get the old entry twice.
- **Saved formats.** Only formats that are plain memory (`HGLOBAL`) are saved.
  Bitmaps, metafiles, palettes, owner-display, and private formats are skipped.
  Windows makes `CF_BITMAP` again from the saved `CF_DIB`.
- **Clipboard thread.** A message-only window on the thread `sayso-clipboard` owns
  Sayso's clipboard content and runs every clipboard call. `OpenClipboard` is tried
  10 times, 15 ms apart.
- **Held modifiers.** Before Ctrl+V, Alt, Shift, and Win keys that are still down are
  released, after Ctrl goes down. Before typing, every held modifier is released after
  the mask key. They stay logically up until the user presses them again.
- **Chord thread.** `global-hotkey` makes its window on the thread of the first
  `register` call. A later call from another thread is reported in `failed`.
- **Conflicts.** Windows has no list of its own shortcuts, so the table is static and
  every entry is `enabled: true`. Running apps for conflicts come from all processes,
  not from `running_apps`: launchers have no visible window. A taken chord with one
  known app running confirms that app; with no Windows shortcut and no known app, it
  is "Another app". The probe skips chords Sayso holds. Win+period (Emoji panel) is not
  in the table, because Sayso has no period key.
- **Microphone.** One "Deny" in the three consent switches (machine, user, desktop
  apps) is `Denied`. Windows has no prompt for desktop apps, so `request` opens
  Settings unless the state is `Granted`.
- **Login item.** Turning it on writes the quoted path of the running exe and clears
  the Task Manager switch. `RequiresApproval` is never returned.
- **Icons.** `PrivateExtractIconsW` at the exact size, then `SHDefExtractIconW`, then
  the shell icon from `SHGetFileInfoW`, scaled with Lanczos3. Store apps whose exe has
  no icon get the generic exe icon.
- **Secure input.** Windows has no Secure Event Input. `secure_input_holder` is `None`,
  and password fields take synthetic keys.
- **Copied modules.** `audio`, `resample`, `sounds`, `esc`, and `paste_receipt` are
  copies of the macOS files with `Mac` renamed to `Win`. Keep them in step until they
  move to a shared crate.
- Versions: `windows` 0.62.2 (the one gpui-pre uses), `global-hotkey` 0.8, `cpal`
  0.18, `rodio` 0.22, `image` 0.25 with only `png`.
