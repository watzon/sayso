# Testing on virtual machines

This page says how to test a release candidate on VMs with [VMPal](https://vmpal.com/docs/command-line), and what the last run found. A test on a VM does not replace CI, and CI does not replace a test on a VM: CI never starts the real app and answers its questions.

## The machines

| VM | System | Architecture |
|---|---|---|
| `Desktop Test macOS` | macOS 27.2 | arm64 |
| `Desktop Test Windows` | Windows 11 24H2 | arm64, which runs the x64 build through emulation |
| `Desktop Test Ubuntu` | Ubuntu 24.04, GNOME on Wayland, PipeWire | aarch64 |

Keep these three free of test data. Clone one for each run, and delete the clone at the end. Take off the shared folder of the clone (`vmpal set <clone> --unshare Downloads`).

These VMs cannot test: x86_64 Linux, a real Windows x64 computer, a real GPU on Linux, a microphone, a Mac from before 14.2 (no system file for macOS 14.0 or 14.1), or a real call.

## Get the files

```sh
gh workflow run ci.yml --ref <branch>      # test-windows-installer, test-windows-programs
gh workflow run linux.yml --ref <branch>   # release-linux-packages-<arch>
scripts/release.sh --skip-notarize         # dist/Sayso-<version>-macos-arm64.dmg
```

Get the files of the last release with `gh release download`.

## The run

1. Install the last release on a clone, and download a model through onboarding. Keep the hash of each file of the models folder.
2. Take a snapshot. Install the candidate over the last release, and start it from the desktop.
3. The log has `moved to the model store`. Each hash is in `models/v1/artifacts`. Onboarding shows the model as downloaded.
4. Run the example app `transcribe` against Sayso, with no engine in its folder. Answer the question on the screen. Check the text, `apps.toml`, **Don't allow**, and a second run that gets no question.
5. Run the ignored tests of `crates/sayso-capture/tests/system_real.rs`.
6. On Linux, start the `.flatpak` as well, and run the example app outside and in a test Flatpak with `--filesystem=xdg-run/sayso:ro`. Do both against Sayso outside as well. The table in [sdk.md](sdk.md), "Flatpak", must stay true.
7. After a release, take the macOS clone back to the snapshot and let the 0.13 app update itself, then install the notarized DMG on a clean clone.

## What is different in a VM

- `vmpal exec` ends the programs that a command starts when the command ends, and at most two commands run at once. On Linux, start the app with `setsid -f`. On Windows, start it from the Start menu with `vmpal ui`.
- `vmpal exec` has no session variables on Linux. Set `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`, and `DBUS_SESSION_BUS_ADDRESS`.
- macOS gives a permission to the program that started the process. Run the system audio tests from Terminal in the VM, so the question appears.
- The screen locks when the VM is idle. Turn that off in the clone, or use `vmpal sign-in`.
- A macOS 14 VM has no VMPal Tools, so drive it with `vmpal ui` and a read-only shared folder.
- The test programs of Windows need the DLLs of the install folder in `PATH`.
- `vmpal exec` as a user has no `sudo`. Use `vmpal exec --admin`, for example for `flatpak install --system`.
- Ubuntu shows a crash dialog of `spice-vdagent` in a clone. It is not from Sayso, but it covers the question: click **Don't send**.

## Results for 0.14.0 (2026-10-31)

| Check | macOS 27.2 | macOS 14.6.1 | Windows 11 | Ubuntu 24.04 | Fedora 44 |
|---|---|---|---|---|---|
| Package installs | DMG | DMG | Installer, over 0.13 | `.deb`, over 0.13; `.flatpak`; `.AppImage` | `.rpm` |
| The app starts | Pass | Pass | Pass | Pass | **Crash**, see below |
| A real 0.13 models folder moves | Pass | Clean install | Pass | Pass | Not run |
| Connected app, Allow | Pass | Pass, up to the job | Pass | Pass | Not run |
| Don't allow | Not run | Not run | Pass | Pass | Not run |
| System audio finds a tone | Pass | Pass | Pass | Pass | Pass |
| Permission refused | Pass | Not run | No permission | No permission | No permission |
| Permission taken back | Pass | Not run | No permission | No permission | No permission |
| Change of output device | One device | One device | One device | Pass | Not run |
| The 0.13 engine beside the new one | Not run | Not run | Not run | Pass | Not run |
| The released 0.13 app finds 0.14.0 | Pass | Not run | Pass | Not run | Not run |
| The released 0.13 app updates itself | Pass | Not run | No self-update, it offers the download | No self-update | No self-update |

Problems found, none new in 0.14.0:

- **Fedora 44 in a VM.** Sayso ends with SIGSEGV one second after the start, in `libLLVM.so.22.1`, when Mesa compiles a shader for the software renderer llvmpipe on aarch64. The released 0.13.0 does the same. The VM has only that renderer, because the virgl adapter does not pass the limits of `wgpu`. Ubuntu has llvmpipe with LLVM 20 and works. A computer with a real GPU does not use this path.
- **macOS.** At the first start the system asks for Accessibility or Events access before onboarding explains why.
- **Windows.** The log has `Invalid window handle` and `window not found` when the question closes. Nothing else goes wrong.
- **Windows.** A silent install stops with exit code 1 while Sayso runs, because the installer cannot ask to close it. An install with its window asks.

## Results for connected apps across the Flatpak sandbox (2026-10-31)

A test build of the 0.14.1 work on Ubuntu 24.04.5 on aarch64, in a clone of `Desktop Test Ubuntu`. The full table and what stays open are in [sdk.md](sdk.md), "Flatpak". The logs are in `/Volumes/SHARED/VMs/evidence/sayso-0.14.1/flatpak/`.

| Sayso | App | Allow | Don't allow | Spoofs refused |
|---|---|---|---|---|
| `.flatpak` | Outside | Text in Sayso mode | Local mode | Pass |
| `.flatpak` | Test Flatpak, own id | Text in Sayso mode | Local mode | Pass |
| Test copy of the `.flatpak`, with the example | Same id | Text in Sayso mode | Local mode | Pass |
| `.deb` | Test Flatpak | Text in Sayso mode | Local mode | Pass |
| `.deb` | Outside | Text in Sayso mode | Local mode | Pass |
| Test copy, system install | Test Flatpak, system install | Text in Sayso mode | Local mode | Not repeated, except a false `.flatpak-info` |

The clean quit, `kill -9` of Sayso, a killed bridge, a stale socket, a Sayso outside and one in a Flatpak in both orders, and an app that starts before the folder exists gave the results that [sdk.md](sdk.md) describes.
