# Sayso on Linux

This page describes how Sayso works on Linux: which desktops it supports, the permissions it needs, and how to bind a shortcut when the desktop has no global shortcuts for apps.

## Packages

Each release has five files for x86_64 and five for aarch64. All of them hold the same two binaries, `sayso` and `sayso-engine`.

| File | Installs to | Notes |
|---|---|---|
| `.deb` | `/usr/lib/sayso`, with the link `/usr/bin/sayso` | `apt` installs the libraries that Sayso needs. |
| `.rpm` | `/usr/lib/sayso`, with the link `/usr/bin/sayso` | It names libraries, not packages, so one file works with `dnf` and `zypper`. The file is not signed: `zypper` needs `--allow-unsigned-rpm`. |
| `.flatpak` | The Flatpak folder of your user (`--user`) or of the system | It brings its libraries with the runtime `org.freedesktop.Platform` from Flathub. See "Flatpak". |
| `.AppImage` | Nothing | It uses the libraries of your system, the same ones as the tarball. It cannot install the udev rule for `/dev/uinput` (see "Paste access"). |
| `.tar.gz` | `~/.local`, or `/usr/local` with `--system` | `./install.sh --uninstall` removes a user install. |

The AUR package [`sayso-bin`](https://aur.archlinux.org/packages/sayso-bin) installs the binaries of the tarball to `/usr/lib/sayso`, for x86_64 and aarch64.

The `.deb`, the `.rpm`, and the AUR package also install the udev rule for `/dev/uinput`.

Without the Flatpak, the binaries need glibc 2.35 and the libstdc++ of GCC 12, or later versions. These distributions have them:

| Distribution | Works from |
|---|---|
| Ubuntu | 22.04 |
| Debian | 12 |
| Fedora | All supported versions |
| openSUSE | Leap 15.6, Tumbleweed |
| RHEL, AlmaLinux, Rocky Linux | 10. Version 9 has glibc 2.34, which is too old. |
| Arch Linux | Current |

The install was tested in a container of each of these, except Tumbleweed and Arch Linux on aarch64. A start of Sayso was tested on Ubuntu 22.04 and 26.04 and on Debian 12.

Without a `.deb` or `.rpm`, your system must have these libraries: ALSA (`libasound`), `libxcb`, `libxkbcommon`, `libxkbcommon-x11`, fontconfig, FreeType, and the Vulkan loader (`libvulkan`) or EGL. A Wayland session also needs `libwayland-client`.

No package updates by itself. Sayso shows a new version and opens the download page.

### Flatpak

```sh
flatpak install --user ./sayso-<version>-linux-<arch>.flatpak
flatpak run dev.sayso.Sayso
```

The install gets the runtime from Flathub, so your system must have the Flathub remote.

Sayso must send keys to other apps, read the keyboard, and start tools of your system. So its Flatpak has wide permissions, and the sandbox protects little:

| Permission | Why |
|---|---|
| X11 and Wayland | The windows, the overlay, key grabs, and the paste key on X11 |
| All devices | `/dev/uinput` for the paste key and `/dev/input` for keyboard access. The udev rule and the `input` group of "Permissions" apply as they do without Flatpak. The Flatpak cannot install the udev rule. |
| Host commands (`org.freedesktop.Flatpak`) | The `claude` and `codex` tools of your system, for AI styles, and `gsettings`, for the GNOME custom shortcuts |
| `~/.config/autostart` | **Launch at login** |
| Secret Service, the tray, audio, network | API keys, the tray icon, the microphone, model downloads, and cloud providers |

Differences from the other packages:

- The files are in `~/.var/app/dev.sayso.Sayso` (`config`, `data`, `cache`), not in `~/.config/sayso` and `~/.local/share/sayso`.
- The commands for desktop shortcuts are `flatpak run dev.sayso.Sayso --toggle`, and the same for the other flags.
- Sayso finds `claude` and `codex` with the `PATH` of your desktop session, then in the usual install folders.
- Sayso cannot read the desktop files of your system. On X11, History can show less about the focused app.
- A bundle has no update source. To update, install the newer file.

### NixOS

The repository is a Nix flake. Its package takes the binaries from the release tarball and links them to the libraries of nixpkgs. To try Sayso:

```sh
nix run github:watzon/sayso
```

To install it on NixOS, add the flake as an input and turn on the module. The module installs Sayso and the udev rule for `/dev/uinput`.

```nix
{
  inputs.sayso.url = "github:watzon/sayso";

  outputs = { nixpkgs, sayso, ... }: {
    nixosConfigurations.my-computer = nixpkgs.lib.nixosSystem {
      modules = [
        sayso.nixosModules.default
        { programs.sayso.enable = true; }
      ];
    };
  };
}
```

Without the module, use the package `sayso.packages.${system}.default`, or the overlay `sayso.overlays.default`, which adds `pkgs.sayso`.

To get a new version, run `nix flake update sayso` and build your system again.

**Launch at login** and the GNOME custom shortcuts store the path of Sayso in the Nix store. That path changes with each version, and the garbage collector can delete the old one. After an update, turn **Launch at login** off and on again.

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
| Style by app | Yes | No | No | No |
| Style by site | No | No | No | No |

On a Wayland desktop the pill cannot be dragged, because only the compositor can place a layer surface.

With more than one display, the overlay follows the display in use (Settings › Overlay › Follow the display in use):

- On X11, the pill moves to the display of the mouse or of the focused window, whichever changed last.
- On KDE, sway, and Hyprland, a layer surface cannot change its display. Sayso opens a new overlay each time a dictation starts, and the compositor puts it on the display in use. The idle pill stays on the display of the last dictation.
- On GNOME (Wayland), Sayso sees the mouse and the focus only over X11 apps, so the pill follows only those.

On GNOME 47 and earlier without keyboard access, Sayso adds its chords to **Settings › Keyboard › Custom Shortcuts**, named "Sayso: …". GNOME then runs `sayso --toggle` (or `--paste-last`, `--cycle-style`, `--incognito`) for the key. Push-to-talk cannot work this way, because GNOME does not report a key release. When the shortcuts portal or keyboard access becomes available, Sayso removes these entries.

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
| `sayso --incognito` | Turn incognito on or off |
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
