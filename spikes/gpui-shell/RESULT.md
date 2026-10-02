# GPUI shell spike: result

Date: 2026-10-01. Machine: macOS 27.0, M5 Pro. Main display 5120x1440 at 120 Hz, plus a second display.

## What was built

One binary, `gpui-shell`, with two spikes.

- S1 overlay (`src/overlay.rs`): a 280x64 pt transparent `WindowKind::PopUp`, `focus: false`, bottom-center of the primary display. A `canvas` draws three layered stroked quadratic curves (`PathBuilder::stroke` + `curve_to` + `paint_path`). Each frame calls `request_animation_frame`. A probe task logs frontmost bundle id, `isKeyWindow`, fps, level and collection behavior once per second for 5 s. It also toggles `setLevel:`, `setCollectionBehavior:` and `setIgnoresMouseEvents:`.
- S2 tray popover (`src/tray.rs`): a `tray-icon` status item with no menu. Click-down shows a hidden 320x420 pt `PopUp` window, anchored under the icon. The popover has a gpui-kit `Button`, `Switch`, and a list of labels. Dismiss: `NSEvent` global monitor (outside click) and Esc.
- `src/mac.rs`: objc2 helpers (NSWindow from a GPUI `Window`, frontmost app, activation policy, window chrome fix).
- Flags: `--overlay`, `--tray`, default both. Test flags: `--no-fix`, `--fix=shadow`, `--delay=N`, `--deactivate`, `--auto-toggle=N`.

## Versions that compiled

- Toolchain: Rust 1.98.1, pinned in `rust-toolchain.toml`. Rust 1.94.1 does NOT build `gpui-pre 0.3.7`: `error[E0658]: use of unstable library feature 'cold_path'` at `gpui-pre-0.3.7/src/profiler.rs:473`. 1.95 or newer is needed (1.97.0 and 1.98.1 were already installed).
- `gpui-kit =0.7.0` (pulls `gpui-pre`, `gpui-pre-platform`, `gpui-pre-macos` all 0.3.7, `gpui-component` 0.7.0, `gpui-base` 0.7.0). I also listed `gpui-pre =0.3.7` and `gpui-pre-platform =0.3.7` directly. They are not needed: `gpui_kit::*` re-exports GPUI.
- `tray-icon 0.26.0`, `objc2 0.6.4`, `objc2-app-kit 0.3.2`, `objc2-foundation 0.3.2`, `block2 0.6.2`, `raw-window-handle 0.6.2`, `futures 0.3`.
- `cargo build` and `cargo clippy`: clean (only a future-incompat note from the `block 0.1.6` crate, a transitive dependency).

## Pass criteria

S1 numbers come from `evidence/s1-overlay.log` and `evidence/s1-external-frontmost.txt`. That run was a "valid run": an external `osascript` sampler (every 250 ms) saw TextEdit frontmost for the whole run. `verify-s1.sh` retries until that holds, because the host app (t3code, codex) often grabs focus on its own during long-running tool calls. That also happens with a plain `sleep 8` process, so it is not caused by the spike.

| Criterion | Result | Evidence |
|---|---|---|
| Frontmost stays TextEdit | PASS | In-process `frontmost=com.apple.TextEdit` at t=1..5 s; external sampler agrees. |
| Overlay never key | PASS in 5 of 5 validated runs | `isKey=false isMain=false appActive=false` at every probe. See the caveat below. |
| fps >= 55 | PASS with a fix (see gotchas) | 107 to 115 fps (display runs at 120 Hz). |
| Transparent border bug (#61508) | APPEARS, FIXED | See below. |
| `setLevel:` | PASS (read-back) | Default PopUp level 101. Set 25 (status), read back 25. |
| `setCollectionBehavior:` | PASS (read-back) | Default `0x101` (canJoinAllSpaces + fullScreenAuxiliary). Set `+Stationary+IgnoresCycle`, read back `0x151`. Whether it appears on all Spaces / over a full-screen app needs a human. |
| `setIgnoresMouseEvents:` | read-back PASS, click-through NEEDS-HUMAN | `ignoresMouse=true` at t=4 s, `false` at t=5 s. Real click-through was not tested. |
| Smooth waveform | PASS (visual) | `evidence/overlay-fixed.png`. |

Caveat on "never key": in three early runs (older build, TextEdit possibly not settled yet) the log showed `appActive=true isKey=true` for the first seconds, with TextEdit still frontmost. I could not reproduce it in the five validated runs. The likely cause is that the process was active at launch. If your app is active when the overlay appears (for example right after the user used the tray popover), expect the overlay to become key, because GPUI's panel class returns YES for `canBecomeKeyWindow`. Another observation: after the tray popover is shown with `makeKeyAndOrderFront:`, `NSApp.isActive` becomes true even though `NSWorkspace.frontmostApplication` stays TextEdit. Untested mitigations: `NSApp.deactivate()` (the `--deactivate` flag exists but was only tried in the early, confounded runs), or a `canBecomeKeyWindow` override on the overlay panel.

### Transparent border (zed #61508)

- Bug present. With `WindowBackgroundAppearance::Transparent` and a rounded translucent pill, macOS draws the window shadow around the opaque shape. It looks like a thin outline plus a soft drop shadow (`evidence/overlay-border-nofix.png`, over a dark TextEdit window).
- Fix that worked: `NSWindow.setHasShadow(false)` alone. Verified with `--fix=shadow` and with the full fix. The full fix also calls `setOpaque(false)` and `setBackgroundColor(clearColor)`, which was not needed. See `mac::clear_window_chrome` in `src/mac.rs`. The fix held for the 5 s of the run. Comparison image: `evidence/overlay-fixed.png`.
- I did not test `invalidateShadow`, other style mask changes, or resizing after the fix.

### S2 results

Evidence: `evidence/popover.png`, `evidence/s2-tray.log`. A real click on the status item was sent with cua-driver (desktop-scope click at the icon). Tests below used real clicks and a real Esc key press.

| Item | Result |
|---|---|
| gpui-kit renders (Button, Switch, labels, theme) | PASS. Clicking the Button incremented its counter. The Switch toggled. In-popover clicks do not dismiss. |
| Anchored under the status item | PASS. Icon frame (Cocoa coords): x=4163, y=1409, 38x33. Popover frame: x=4022, y=989, 320x420. Popover top edge = icon bottom edge (989+420 = 1409). Popover center x 4182 = icon center x 4182. |
| Click on icon opens, second click closes | PASS after one fix (see gotchas). |
| Outside click dismiss | PASS. Click in TextEdit hid the popover (`[tray] outside click -> hide`). |
| Esc dismiss | PASS. A real Esc key (HID, desktop scope) reached the key panel and hid it. TextEdit stayed frontmost. |
| No Dock icon, `setActivationPolicy(.accessory)` | PASS. Call returns true. Read-back 1 right after and still 1 at 9 s, with windows open. It sticks because GPUI sets Regular in `applicationDidFinishLaunching`, which runs before the `Application::run` closure. A Dock icon is not visible for this unbundled binary, but I did not look at the Dock itself. NEEDS-HUMAN for a bundled app. |
| No NSMenu | PASS. `TrayIconBuilder` without `with_menu`. |

## Workarounds (code locations)

1. Activation policy: `mac::become_accessory`, called at the top of the `run` closure in `src/main.rs`.
2. Shadow/border: `mac::clear_window_chrome` (`src/mac.rs`), called in `overlay::open`.
3. Frame throttling: `inactive_frame_interval: None` in the overlay `WindowOptions` (`src/overlay.rs`).
4. Popover position: `tray::anchored_origin` and `NSWindow.setFrameOrigin` (Cocoa coordinates, no flip needed). Shown with `makeKeyAndOrderFront:`, hidden with `orderOut:`.
5. Icon click while open: the global monitor sees the click on our own icon and hides the popover, then the toggle reopens it. Fix: ignore monitor clicks inside the cached icon rect (`icon_rect` in `src/tray.rs`).

## API gotchas

- Frame throttle: the default `WindowOptions::inactive_frame_interval` is a throttle. A `PopUp` is never "active", so the overlay ran at about 26 fps until it was set to `None`. This is the main risk for the "fps >= 55" criterion.
- `use gpui_kit::*` brings the inherent `Window::window_handle() -> AnyWindowHandle`. It shadows `HasWindowHandle::window_handle`. Call `HasWindowHandle::window_handle(window)` explicitly.
- `FluentBuilder` (`.when`) is not in the glob. Import `gpui_kit::prelude::FluentBuilder as _`.
- `canvas` paint closures are `FnOnce`. Capture time in `render` and move it in. `window.request_animation_frame()` in `render` re-notifies the view, so render doubles as the frame counter.
- `curve_to(to, ctrl)` takes the end point first, then the control point.
- Window position: `WindowBounds::Windowed` origin is in display coordinates (top-left). Use `cx.primary_display().bounds()`.
- gpui-kit: `gpui_kit::open_window` wraps the view in `Root`. Call `gpui_kit::init(cx)` first. `Switch::on_click` is an alias of `on_change`. It is a controlled value, so write the state and call `cx.notify()`. Kit imports: `gpui_kit::component::{button::*, switch::Switch, ActiveTheme}`.
- `WindowKind::PopUp` is an NSPanel, not the new `WindowKind::AnchoredPopup(PopupOptions)`. The macOS backend rejects `AnchoredPopup` (see comment in `gpui-pre-macos` `window.rs`), so it was not used.
- `tray-icon 0.26`: `with_icon_as_template` is deprecated. Use `with_icon_templated`. Right after `build()` the status item frame is `(0,0,38,0)`. The real position is only available after the first run loop turn, so read it at click time.
- With `titlebar: None` the opaque popover still gets rounded corners and a shadow from AppKit.
- tray-icon's event handler must be `Send + Sync`. Forward events through a `futures` unbounded channel and await it in `cx.spawn`. Keep the `TrayIcon` and the `NSEvent` monitor alive by moving them into that task.

## Verify the rest by hand

Build once: `cd /Users/watzon/Projects/personal/sayso/spikes/gpui-shell && cargo build`.

1. S1 focus and fps (automated, retries when the host steals focus): `./verify-s1.sh myrun --delay=1`, then read `/tmp/myrun.ext` and `/tmp/myrun.log`. Needs Screen Recording for the screenshot. Compare with `--no-fix` and `--fix=shadow`.
2. Click-through (needs a human): run `./target/debug/gpui-shell --overlay`. At t=4 s the overlay ignores mouse events for 1 s. Move your cursor over the pill and click the window behind it during that second. The click should reach the window behind.
3. All Spaces and full-screen (needs a human): run `./target/debug/gpui-shell --overlay`, then switch Spaces or enter a full-screen app within 5 s. The overlay should stay visible.
4. Dock icon (needs a human): run `./target/debug/gpui-shell --tray` and check that no Dock icon appears and that Cmd+Tab does not list the app.
5. Tray, full flow: run `./target/debug/gpui-shell --tray`. Click the white circle in the menu bar: the popover opens under it. Click again, click elsewhere, or press Esc: it closes. Click the Button and the Switch. Try with the main display set to a different screen, and with a notched display (not tested here).
6. Both spikes together: `./target/debug/gpui-shell`.
7. Run the overlay while another app is key and typing, and confirm your typing is not interrupted (only the frontmost-bundle check was automated).

## Not tested

- Release build runtime (it compiles).
- Key behavior of the overlay when our app is already active (see caveat).
- Blurred background (#64284), fonts, a bundled `.app` with `LSUIElement`.
- Multiple displays and notched displays for the popover position clamp. Only horizontal clamping to the visible frame exists in `anchored_origin`.

## Files

- `src/main.rs`, `src/overlay.rs`, `src/tray.rs`, `src/mac.rs`, `Cargo.toml`, `rust-toolchain.toml`, `verify-s1.sh`
- `evidence/`: `overlay-border-nofix.png`, `overlay-fixed.png`, `popover.png`, `s1-overlay.log`, `s1-external-frontmost.txt`, `s2-tray.log`
