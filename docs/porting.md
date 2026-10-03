# Porting Sayso to another platform

Status: written for the Windows port on 2026-10-02, and changed on the same day for the merge with the Linux port. The terms are in [CONTEXT.md](../CONTEXT.md).

This file says where the platform code lives, which seams the shared crates have, and what a new platform adds. Keep the rule from [plan.md](plan.md) §4: only `sayso-app` depends on a platform crate.

## What each platform adds

| Part | macOS | Windows | Linux | What a new platform adds |
|---|---|---|---|---|
| Platform services (`Platform` in `sayso-platform`) | `sayso-platform-macos` | `sayso-platform-windows` | `sayso-platform-linux` | `crates/sayso-platform-<os>` with `<Os>Platform::new(sounds_dir)` |
| App shell (`sayso-app/src/shell`) | `macos.rs` | `windows.rs` | `linux.rs` | A backend file with the same items (see below) |
| Speech engine | Swift sidecar, `native/macos/SaysoEngine` | Portable Rust sidecar, `native/portable` (transcribe-rs) | Rust sidecar, `crates/sayso-engine` (sherpa-onnx) | Nothing: both Rust engines have no system-specific code |
| Model catalog | `models::catalog()` Apple list | `models/portable.rs` | `models_onnx.rs` | Nothing |
| Secrets | `keyring` with the Keychain store | `keyring` with the Credential Manager store | `keyring` with the Secret Service store | A `keyring` feature in `sayso-enhance/Cargo.toml` |
| Title bar | The native traffic lights | `caption.rs` (caption buttons) | `chrome.rs` (a setting; the system title bar by default) | Nothing, when the system draws the title bar |
| Packaging | `scripts/bundle.sh`, DMG | `scripts/bundle-windows.ps1`, Inno Setup | `scripts/bundle-linux.sh`, tarball. `scripts/package-linux.sh` makes the `.deb`, the `.rpm`, and the AppImage from it | A script and a job in `release.yml` |

Windows and Linux each have their own Rust engine and model catalog, because the two ports were made at the same time. One engine for both is possible: neither engine has system-specific code.

## Seams in the shared crates

Each seam is a small `cfg` block. A new platform adds a branch next to the others.

- **`sayso-app/src/shell`**: the only place that names a platform crate. Each backend (`macos.rs`, `windows.rs`, `linux.rs`) has the same items, and `fallback.rs` does nothing, for a system without a backend. `mod.rs` holds the UI words (`COMPUTER`, `OS_NAME`, `SECRET_STORE`, `SOUND_SETTINGS`, `OPTIMIZING`, `HAS_INPUT_PERMISSIONS`) and `os_text!`, which picks the text for macOS, Windows, or the other systems.
- **`sayso-app/Cargo.toml`**: one `[target.'cfg(...)'.dependencies]` section per platform crate.
- **`sayso-app/src/caption.rs`**: window buttons and a drag area on Windows, where GPUI hides the title bar for `appears_transparent`. **`chrome.rs`** is the custom title bar on Linux.
- **`sayso-app/src/popover.rs`**: the tray icon is `shell::Tray`. The popover opens above the icon when the icon is in the lower half of its screen.
- **`sayso-app/src/hub/pages/player.rs`**: History playback (`shell::audio_player`, or `PlaySound` on Windows).
- **`sayso-core/src/paths.rs`**: `PlatformDirs::for_env`, the folders without XDG variables. A system without its own branch gets the XDG defaults.
- **`sayso-core/src/hotkey.rs`**: `ModifierNames` and `SoloModifier::label` (keycap words), the config file names of the modifiers, and the defaults (`toggle_default`, `paste_last_default`). `command` is the Windows or Super key off macOS.
- **`sayso-core/src/models.rs`**: `catalog`, `default_model`, `preview_fallback` pick the Apple list on macOS, `portable` on Windows, and `models_onnx` elsewhere.
- **`sayso-engine-client/src/client.rs`**: `ENGINE_FILE_NAME` and `DEV_ENGINE_PATH`. On Windows the development build is `native/portable/target/release/SaysoEngine.exe`. On Linux, `cargo build -p sayso-engine` puts `sayso-engine` next to the app binary.
- **`sayso-enhance`**: `detect.rs` (`find_executable`, `common_dirs`) and `cli.rs` (`new_command`). `win_shim.rs` is Windows only.
- **`sayso-platform-macos`**: the crate is empty off macOS (`#![cfg(target_os = "macos")]`, target-specific dependencies), so `cargo test --workspace` works everywhere. A new platform crate does the same with its own `cfg`.

## The window functions

`sayso-app` calls these functions on `crate::shell`. The macOS and Windows backends pass them to the `window` module of their platform crate, where the functions take the native handle as `*mut c_void` and do nothing for a null or dead handle. `shell/mod.rs` lists the other items of a backend (startup, the tray, files).

Coordinates are Cocoa-style on every platform: origin at the bottom left of the primary display, y up, in GPUI's logical pixels. The app converts with `primary_screen_height()`.

| Function | What it does |
|---|---|
| `become_accessory`, `set_dock_icon_visible` | No Dock icon at rest. A no-op off macOS. |
| `show_app_and_activate`, `deactivate_app` | Bring Sayso to the front, give focus back. |
| `configure_overlay`, `make_never_key` | The overlay never takes focus, has no frame, and stays on top. |
| `set_ignores_mouse` | Click-through for the empty part of the overlay. |
| `set_frame_origin`, `set_frame`, `frame` | Move, size, and read a window. |
| `make_clear` | No system shadow or frame around a window that draws its own sheet. |
| `add_child_window` | The model list moves with the window that opened it. |
| `order_front_without_activating`, `order_out`, `show_and_focus` | Show and hide. |
| `screens`, `mouse_location`, `mouse_button_down`, `primary_screen_height` | Displays and the mouse. |
| `install_global_click_monitor` → `MonitorToken` | Clicks in other apps close the popover. Windows returns an inactive token, because a click elsewhere deactivates the popover. |
| `frontmost_app_is_fullscreen`, `covers_a_display` | Hide the pill over full-screen apps. |
| `status_item_screen_rect` | macOS only. Other platforms convert tray-icon's rect with `rect_from_physical`. |

The Windows crate also has `rect_from_physical`, `taskbar_is_dark`, `shell_open`, `reveal_in_explorer`, `play_wav`, `stop_wav`, `user_display_name`, and `claim_single_instance`. `shell/windows.rs` calls them.

## The portable speech engine

`native/portable` is its own Cargo workspace, so the main workspace does not build ONNX Runtime and whisper.cpp. It speaks the NDJSON protocol of `crates/sayso-engine-client/NOTES.md`, so `EngineClient` runs it like the Swift sidecar. Its notes are in `native/portable/NOTES.md`.

## Known gaps on Windows

- The build is not code signed unless the release has a certificate (`WINDOWS_CERTIFICATE_PFX`). Unsigned downloads get a SmartScreen warning.
- The tray icon color follows the taskbar theme at startup only.
- Displays with different scales: positions use the scale of the display under the point, like GPUI. A window that spans two such displays can land a few pixels off.
- The Windows crate has its own copies of `audio`, `resample`, `sounds`, `esc`, and `paste_receipt`. `sayso-platform-common` now holds these modules for macOS and Linux, and the Windows crate can use it too.
