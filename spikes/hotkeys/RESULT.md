# Hotkeys spike: result

Machine: macOS 27.0 (26A428), Apple Silicon, Rust 1.94.1. Date: 2026-10-01.
Code: `src/` (about 670 lines). `cargo build` and `cargo clippy --all-targets` pass with no warnings.

Terms: **PTT** = push-to-talk hotkey. **Toggle** = toggle hotkey (chord).
All keyboard tests used synthetic events (`CGEventPost`, mode `synth`). No physical key was pressed by the agent.
Real-hardware items are marked `needs-human` and have commands in the last section.

## 1. Approach and recommendation

| Need | Chosen approach | Why |
|---|---|---|
| Toggle chord (Option+Space) | Carbon `RegisterEventHotKey`. Use the `global-hotkey` 0.8 crate in the product. Raw FFI is in `src/chord.rs` for evidence only. | No permission needed. Gives press and release. `global-hotkey` is the same Carbon call with a safe API and a Windows/Linux seam. |
| PTT (modifier-only and chord) | Own listen-only `CGEventTap` (`src/tap.rs`, about 150 lines). | Listen-only tap never swallows events, so it cannot break the system double-Fn dictation. Needs Input Monitoring only (see section 4). |
| Cancel (double Esc) | State machine in the same tap, gated on "recording". Alternative: register Esc with Carbon only while recording. | Both work (section 2). |
| Conflicts | `CopySymbolicHotKeys` for system hotkeys. Plist is a fallback. Carbon cannot see other apps. | See section 3. |
| Secure Event Input | `IsSecureEventInputEnabled()` plus the `kCGSSessionSecureInputPID` key from `CGSessionCopyCurrentDictionary()` to name the app that holds it. | Both verified here. |

### `global-hotkey` 0.8 vs raw FFI
- Both gave PRESS and RELEASE on `Option+Space` (about 170 to 250 ms apart for a 150 ms synthetic hold).
- Both need a real main-thread event loop. A bare `CFRunLoopRun` or `CFRunLoopRunInMode` loop received **zero events** in all four test combinations. `RunApplicationEventLoop()` worked. GPUI runs `NSApplication`, so the product is fine. A CLI test harness is not.
- `global-hotkey` hides the OSStatus. Failures become `FailedToRegister(String)`. `AlreadyRegistered` only covers the same process (section 3). Its event ids are hashes (for example `65598`), so keep a map from id to action.
- Recommendation: use `global-hotkey` for the toggle chord. Use raw FFI only if the OSStatus is needed.

### `handy-keys` 0.3.4 vs own tap
- Works. Right Option alone (`OptRight`), `Fn` alone and `Ctrl+Shift+D` all gave PRESS and RELEASE. An Arrow key with the Fn flag did not fire Fn.
- Details from reading `src/platform/macos/listener.rs`:
  - It always creates the tap with `CGEventTapOptions::Default` (an **active** tap), even with `HotkeyManager::new()`.
  - Its guard is `AXIsProcessTrusted()` (Accessibility), not Input Monitoring.
  - It also subscribes to all mouse-button events, which Sayso does not need.
  - It needs a spare thread.
  - It has no Esc-on-demand and no listen-only mode.
- Recommendation: **own listen-only tap on macOS.** It needs a weaker permission and has no swallow risk. `handy-keys` stays a candidate for the Windows and Linux seam, where it is the only crate here with modifier-only support. Re-check its macOS tap options before using it there.
- One unexplained observation: in one early combined run, the `OptRight` release was reported late (together with the `Fn` release). Two later runs, one alone and one combined, were correct. Not reproduced. Re-check on hardware.

## 2. Evidence per requirement

| Requirement | Result | Evidence |
|---|---|---|
| Option+Space registers (Carbon) | PASS | `RegisterEventHotKey(Opt+space) -> OSStatus 0` on macOS 27. |
| Toggle gives press events | PASS (synthetic) | `chord PRESS id=1` at +0.89 s. Posted with `CGEventPost` at HID and session locations. |
| Toggle gives release events | PASS (synthetic) | `chord RELEASE id=1` about 170 ms later. |
| `osascript ... key code 49 using option down` | INCONCLUSIVE | Exit code 0, but that run used the bare CFRunLoop (no events can arrive). Not repeated with the app loop. `CGEventPost` is the tested path. |
| `global-hotkey` 0.8 | PASS (synthetic) | PRESS and RELEASE with app loop. Zero with bare CFRunLoop. |
| Right Option alone, PTT (keycode 61) | PASS (synthetic) | `flagsChanged code=61 flags=0x20080040` then `PTT-PRESS`. Release `flags=0x20000000`. I use the device bit `0x40` (right Option) to tell Right Option from Left Option. |
| Fn alone, PTT (keycode 63 + SecondaryFn) | PASS (synthetic) | `flagsChanged code=63 flags=0x20800000` then `PTT-PRESS Fn`. Release when the flag clears. |
| Fn filter vs arrow keys | PASS (synthetic) | An Arrow `keyDown` with `0x800000` is a `keyDown`, not `flagsChanged` keycode 63. No PTT fired. Real arrow keys still need a hardware check. |
| Chord PTT Ctrl+Shift+D | PASS (synthetic) | `keyDown code=2 flags=0x20060000` then PRESS. `keyUp` then RELEASE. The PTT also releases if the modifiers go away first. |
| Double Esc cancel | PASS (synthetic) | Two Esc `keyDown` events 180 ms apart while Right Option is held gave `CANCEL (double Esc)`. |
| Esc slower than 400 ms | PASS (synthetic) | Two Esc events 770 ms apart gave no cancel. |
| Esc ignored when not recording | PASS (synthetic) | `Esc ignored (not recording)`. |
| Single-Esc option | not run | `tap --single-esc` is implemented but not exercised. |
| Esc via Carbon only while recording | PASS (synthetic) | `RegisterEventHotKey(esc) -> 0` and both Esc presses were delivered. Needs no permission. Carbon hotkeys consume the key, so the focused app would not see Esc during recording. Not checked on hardware. |
| Auto-repeat ignored | PASS by code | Repeat flag read (field 8) and skipped for chord and Esc. |
| Tap re-enable after timeout | not testable | Code handles `0xFFFFFFFE` and `0xFFFFFFFF` by re-enabling. |
| `perms` prints values | PASS | See below. |
| Secure Event Input detection | PASS | A second process saw `IsSecureEventInputEnabled = true` while another process held it. It returned to false after release. The session dictionary named a PID, but it was the host app of my terminal (T3 Code), not my own process. Treat the PID as "the responsible app", not always the exact process. |
| Tap vs Secure Input | observed (synthetic, induced from background) | With Secure Input on, the tap still received `flagsChanged` (modifier-only PTT works). It received **no** `keyDown` or `keyUp` for Esc or D. So chord PTT and double Esc stop working. The Carbon chord (Option+Space) still fired in the same state. Real behaviour with a focused password field is `needs-human`. |
| Tap creation failure | not reproduced | All permissions are granted to the host terminal. The failure path prints: `FAIL: CGEventTapCreate returned NULL. Input Monitoring is not granted ... (CGPreflightListenEventAccess=false)`. `handy-keys` returns `AccessibilityNotGranted` in the same case. |

`perms` output on this machine:
```
CGPreflightListenEventAccess (Input Monitoring) = true
CGPreflightPostEventAccess   (post events)      = true
AXIsProcessTrusted           (Accessibility)    = true
IsSecureEventInputEnabled    (Secure Input)     = false
```
TCC grants belong to the app that launched the binary (here the terminal host), not to the binary. The shipped `.app` needs its own grants.

## 3. Conflict detection findings

### (a) `com.apple.symbolichotkeys` plist
- `defaults export com.apple.symbolichotkeys -` parsed with the `plist` crate. Each entry has `enabled` and `value.parameters = [ascii, keycode, modifiers]`. The modifier mask uses **CGEventFlags** bits: Shift `0x20000`, Ctrl `0x40000`, Opt `0x80000`, Cmd `0x100000`.
- The plist holds only entries that were written to it. Entries that were never touched are missing, even if they are enabled by the OS default. So the plist alone cannot prove "no conflict".
- `conflicts cmd+space` reports `id 64 Spotlight (Cmd+Space)`, **enabled=false on this machine**. `conflicts ctrl+space` reports `id 60 previous input source`, enabled=false. Ctrl+Opt+Space (id 61) and Cmd+Opt+Space (id 65) are also disabled here. I did not change settings, so I could not show an enabled Spotlight row.

### (c) `CopySymbolicHotKeys` (Carbon, public API in HIToolbox)
- Works, with no permission. It returned 234 entries on this machine, each with `kHISymbolicHotKeyCode`, `kHISymbolicHotKeyModifiers` and `kHISymbolicHotKeyEnabled`.
- Modifiers here use the **Carbon mask**, not CGEventFlags: Cmd `0x100`, Shift `0x200`, Opt `0x800`, Ctrl `0x1000`. Convert before comparing.
- It includes OS defaults that never appear in the plist. Example: `Ctrl+Cmd+Space` (Emoji and Symbols) is `enabled=true` in the live list. The plist has no row for it.
- There is no id or name in the live list. Use the plist or a static id table to label a hit.
- Recommendation: use `CopySymbolicHotKeys` as the source of truth. Use the plist only to get names.

### (b) `RegisterEventHotKey` as a conflict probe
- Same process, same chord twice: second call returns `-9878 eventHotKeyExistsErr`. PASS.
- **Two processes, same chord:** both calls return `0`. Both processes then receive the press and release (test: two instances of this binary, one synthetic `Option+Space`, both printed PRESS and RELEASE). So `eventHotKeyExistsErr` does **not** detect another app. A second app with the same chord does not block Sayso. It fires at the same time as Sayso.
- Registering a chord that is an enabled system hotkey (`Ctrl+Cmd+Space`) also returns `0`. Which one wins on a real key press is `needs-human`.
- ChatGPT is running on this machine. `Option+Space` registered with `0` here, so ChatGPT does not hold that chord through Carbon, or it is not set up. This does not prove the chord is free.
- Consequence for Sayso: it cannot detect a clash with another app (ChatGPT, Raycast, Alfred) through Carbon. Options:
  1. Check system hotkeys with `CopySymbolicHotKeys`.
  2. Add a "test your hotkey" step in setup. If the user presses it and another app also reacts, the user sees it.
  3. Show a static warning for known clashes (Option+Space is the default of ChatGPT; Cmd+Space is Spotlight).
- Private APIs that list other apps' registered hotkeys may exist (inference, not tested, not recommended).

## 4. Permission requirements per mode

| Mode | TCC permission | Notes |
|---|---|---|
| `perms` | none | `CGPreflight*` and `AXIsProcessTrusted` only read state. They do not prompt. |
| `chord` / `chord-gh` | none | Carbon hotkeys need no TCC grant. Needs an event loop. Blocked by Secure Input in some cases (see section 2). |
| `tap` | Input Monitoring (`kTCCServiceListenEvent`) | Listen-only tap. Based on Apple's rule for listen-only taps. Verified only with the permission granted. Not verified with it revoked. |
| `handy` | Accessibility (`AXIsProcessTrusted`) | The crate checks it before it creates an active tap. The tap type is read from source. |
| `conflicts` | none | `defaults export` and `CopySymbolicHotKeys` and Carbon register need no grant. |
| `synth` (test helper only) | Post-event access (`CGPreflightPostEventAccess`), true here | Test tool, not part of the product. |

Product note: a listen-only tap plus Carbon chord means Sayso asks only for Input Monitoring (plus Microphone, and whatever text insertion needs). `handy-keys` would add Accessibility for the hotkeys alone.

## 5. Commands for a human

Build once: `cd /Users/watzon/Projects/personal/sayso/spikes/hotkeys && cargo build`. Then `B=./target/debug/hotkeys-spike`.
Run each listener in a terminal that has Input Monitoring. Press Ctrl-C to stop.

1. Chord, real keyboard: `$B chord opt+space 30`. Press and release Option+Space. Expect PRESS then RELEASE.
2. Same through the crate: `$B chord-gh 30`.
3. Tap, real hardware: `$B tap 60`. Do each of these and check the `>>>` lines:
   - Hold and release Right Option. Expect `PTT-PRESS` and `PTT-RELEASE Right Option`.
   - Hold and release Fn (Apple keyboard). Expect `Fn` lines. Press the Arrow keys and F-keys: expect no Fn line.
   - Press Ctrl+Shift+D.
   - Hold Right Option, press Esc twice quickly: expect `CANCEL (double Esc)`. Press Esc twice with no PTT held: expect "ignored".
   - Run `$B tap 60 --recording` to test double Esc without holding a PTT. Run `$B tap 60 --single-esc` for single Esc.
4. Check what the system does with Fn while the tap runs. Does the emoji picker or system dictation still open? (Listen-only should not change it.)
5. Handy comparison: `$B handy 60` and repeat the PTT checks.
6. Permission failure: in System Settings, Privacy and Security, Input Monitoring, turn off the terminal. Run `$B tap 5`. Expect the `FAIL: CGEventTapCreate returned NULL` line. Then turn it back on. Also run `$B handy 5` with Accessibility off.
7. Secure Input with a real password field: run `$B tap 60` in one terminal. In another app, focus a password field (Safari login form, or Terminal with Secure Keyboard Entry). While it is focused, press the PTT chord, Esc, and Option+Space (use `chord` in a second terminal). Run `$B perms` from a third terminal and read the line `Secure Input is held by pid ...`. Does Option+Space still fire?
8. System hotkey conflicts, enabled case: in System Settings, Keyboard, Keyboard Shortcuts, turn on Spotlight (Cmd+Space) or "Select previous input source" (Ctrl+Space). Run `$B conflicts cmd+space` and `$B conflicts ctrl+space`. The `(a)` and `(c)` lines should show `enabled=true`. Then run `$B chord cmd+space 20` and press the keys. Does Spotlight or Sayso win?
9. Other-app conflict: start ChatGPT with its Option+Space shortcut on. Run `$B chord opt+space 30` and press the keys. Do both react? Same test for Raycast or Alfred.
10. Duplicate within one process: `$B conflicts opt+space --twice` should show `-9878` on the second line. Across processes: `$B conflicts opt+space --hold 20 &` then `$B conflicts opt+space` shows `0` for both.
11. Optional: `$B conflicts opt+space --dump` lists the plist rows and the live rows.

Test helper (synthetic events): `SYNTH_TAP=0 $B synth <opt-space|right-option|fn|ctrl-shift-d|esc-esc|arrow-fn|ptt-esc-esc|ptt-esc-slow>`. `SYNTH_TAP=0` posts at HID level and `1` (default) at session level.
Useful env: `CHORD_LOOP=cf` uses the bare CFRunLoop to show that it gets no Carbon events.

## 6. Open items

- Secure Input is the main risk for chord PTT and double Esc (tap gets no key events). Modifier-only PTT survives it in this test. Sayso should call `IsSecureEventInputEnabled()` when a hotkey seems dead and show the holder app from the session dictionary.
- Carbon hotkeys with Option as the only modifier were restricted in some macOS 15 releases (from memory, not verified). `Option+Space` works on macOS 27.0 here. Test on the oldest macOS Sayso supports.
- No hardware Fn test. Per research.md, Fn works only on Apple keyboards. Handling of `AppleFnUsageType` is out of scope for this spike.
