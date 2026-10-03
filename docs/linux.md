# Sayso on Linux

This page describes how Sayso works on Linux: which desktops it supports, the permissions it needs, and how to bind a shortcut when the desktop has no global shortcuts for apps.

## Desktop support

Sayso runs on X11 and on Wayland. The parts that touch other apps (hotkeys, insertion, and the overlay) use a different method on each desktop.

| Part | X11 (any desktop) | GNOME (Wayland) | KDE Plasma (Wayland) | sway, Hyprland (Wayland) |
|---|---|---|---|---|
| UI windows | X11 | XWayland | Wayland | Wayland |
| Overlay and pill | Over all windows, can be dragged | Over all windows, can be dragged | Layer surface at the bottom center | Layer surface at the bottom center |
| Toggle and other chords | Key grab | Shortcuts portal (GNOME 48 or later), else keyboard access, else GNOME custom shortcuts | Shortcuts portal | Shortcuts portal (Hyprland), else keyboard access, else `sayso --toggle` |
| Push-to-talk with a modifier alone, double Esc | Always | Keyboard access | Keyboard access | Keyboard access |
| Paste key | XTest | Remote desktop portal, else `/dev/uinput` | Remote desktop portal, else `/dev/uinput` | Virtual keyboard protocol |
| Clipboard | X11 selections | X11 selections, through XWayland | Data-control protocol | Data-control protocol |
| Tray icon | Most panels | With the AppIndicator extension | Yes | With a tray in the bar (for example Waybar) |
| Focused app in History | Yes | No | No | No |

On a Wayland desktop the pill cannot be dragged, because only the compositor can place a layer surface.

On GNOME 47 and earlier without keyboard access, Sayso adds its chords to **Settings › Keyboard › Custom Shortcuts**, named "Sayso: …". GNOME then runs `sayso --toggle` (or `--paste-last`, `--cycle-style`) for the key. Push-to-talk cannot work this way, because GNOME does not report a key release. When the shortcuts portal or keyboard access becomes available, Sayso removes these entries.

## Title bar

By default, the Hub and onboarding use the title bar of your desktop. To let Sayso draw its own title bar and window buttons, turn off **Settings › Appearance › Use the system title bar** (`appearance.system_title_bar = false` in `config.toml`). The change applies at once. On an X11 desktop without a compositor, the system title bar stays.

With the system title bar, your desktop draws the shape of the window. GNOME rounds only the top corners of such a window. With Sayso's own title bar, all four corners are square.

## How Sayso pastes

Sayso puts the text in the clipboard and in the primary selection, then sends Shift+Insert. Shift+Insert pastes in GTK, Qt, Chromium, Electron, Firefox, and LibreOffice apps, and in terminals. Ctrl+V is not the default, because a terminal sends Ctrl+V to the program that runs in it. When an app reads the text, Sayso puts your old clipboard and selection back. When no app reads the text, the text stays on the clipboard, and the overlay offers to copy it.

Set `SAYSO_PASTE_KEY` to `ctrl+v` or `ctrl+shift+v` to send another key.

## Permissions

Linux has no permission prompts for these features, so Sayso shows each one as a capability.

### Microphone

Sayso records through ALSA, PipeWire, or PulseAudio. No setup is necessary.

### Paste access

Sayso needs paste access to send the paste key to the app you use.

- On X11, paste access is always on.
- On GNOME and KDE (Wayland), your desktop shows a dialog the first time. Select **Allow**. Sayso keeps the permission in `~/.local/state/sayso/remote-desktop.token` until you remove it in the system settings.
- On sway and Hyprland, the compositor gives access through its virtual keyboard protocol. No setup is necessary.
- If none of these work, let the user at the seat use `/dev/uinput`. Copy the rule from the release and load it:

  ```sh
  sudo cp lib/udev/rules.d/70-sayso-uinput.rules /etc/udev/rules.d/
  sudo udevadm control --reload && sudo udevadm trigger
  ```

  `sudo ./install.sh --system` does the same.

### Keyboard access

Keyboard access lets Sayso watch the keyboard without taking keys away from other apps. Sayso needs it on Wayland for three things:

- A push-to-talk key that is a modifier alone (for example Right Ctrl).
- Esc to cancel a dictation.
- The key recorder outside Sayso's own windows.

To turn it on, add your user to the `input` group, then log out and log in again:

```sh
sudo usermod -aG input "$USER"
```

Every app that runs as your user can then read the keyboard. If you do not want that, use a toggle key or a chord for push-to-talk.

## Commands for desktop shortcuts

If your desktop gives no global shortcuts to apps, bind these commands to keys in the keyboard settings of your desktop. Each command goes to the running Sayso.

| Command | Action |
|---|---|
| `sayso --toggle` | Start or stop a dictation |
| `sayso --cancel` | Cancel the dictation |
| `sayso --paste-last` | Paste the last text again |
| `sayso --cycle-style` | Switch to the next style |
| `sayso --push-to-talk-down`, `sayso --push-to-talk-up` | Push-to-talk, for tools that run one command on press and one on release |

On sway, for example, add this to the config:

```
bindsym Ctrl+Alt+space exec sayso --toggle
```

A second `sayso` start without a command opens the Hub of the Sayso that runs.

## Local models

The Linux engine, `sayso-engine`, runs the models on the CPU with sherpa-onnx. The default model is Parakeet TDT v2 (English). It comes with Kroko, a small streaming model for the live preview. On an arm64 virtual machine, Parakeet TDT v2 took 170 ms for 8 s of speech and 590 ms for 29 s. The live preview changes about every 1.4 s.

## Files

| Kind | Location |
|---|---|
| Config | `~/.config/sayso`, or `$XDG_CONFIG_HOME/sayso` |
| History, audio, models | `~/.local/share/sayso`, or `$XDG_DATA_HOME/sayso` |
| Logs | `~/.cache/sayso/logs/sayso.log`, or `$XDG_CACHE_HOME/sayso/logs` |
| Start at login | `~/.config/autostart/dev.sayso.Sayso.desktop` |
| API keys | The Secret Service keyring (GNOME Keyring, KWallet), service `dev.sayso.Sayso` |

## Environment variables

| Variable | Effect |
|---|---|
| `SAYSO_UI_BACKEND` | `x11` or `wayland` forces the display server of the UI. |
| `SAYSO_ENGINE_PATH` | Selects another engine binary. |
| `SAYSO_PASTE_KEY` | `shift+insert` (default), `ctrl+v`, or `ctrl+shift+v`. |
| `SAYSO_CLIPBOARD` | `x11`, `ext`, or `wlr` forces the clipboard method, for tests. |

## Known limits

- Linux has no common way to detect a password field, so Sayso cannot warn you about one.
- On GNOME, typing (the fallback method) works only for characters of the active keyboard layout. Pasting is not affected.
- Through `/dev/uinput`, typing works only for ASCII characters with a US layout.
- When Sayso quits, a clipboard that it put back becomes empty, because Sayso serves the old content itself.
