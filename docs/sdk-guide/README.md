# Guide to the Sayso SDK

Use the Sayso SDK to add speech to your own app. Your app transcribes files and live audio. It uses Sayso when Sayso runs. When Sayso does not run, it uses an engine that your app ships. The app calls the same API in both cases.

This guide has a tutorial, a how-to page for each task, and a reference. [docs/sdk.md](../sdk.md) is the design: read it when you want to know why the SDK works this way.

## Get the crates

The crates are not on crates.io yet. Until they are, your app uses a path dependency or a git dependency. No release has `sayso-sdk` yet (the release `v0.13.0` has the older crate `sayso-extension`), so a git dependency names a commit:

```toml
[dependencies]
# A checkout of the repository on your disk:
sayso-sdk = { path = "/path/to/sayso/crates/sayso-sdk" }
# Or a git dependency on a commit that has the crate. Cargo finds the crate in the repository:
# sayso-sdk = { git = "https://github.com/watzon/sayso", rev = "<commit>" }
```

Ship the engine of the same commit or release (see [How to ship the engine in your app](ship-the-engine.md)). The crates are MIT. The engine is GPL-3.0-only, and your app starts it as a separate program.

## Learn

- [Tutorial: build a speech app with the Sayso SDK](tutorial.md). Build a small program that transcribes a file and runs a live session. Start here.

## Do a task

- [How to transcribe a file](transcribe-a-file.md)
- [How to run a live session](run-a-live-session.md), from the microphone, the system audio, or your own audio
- [How to identify speakers](identify-speakers.md)
- [How to choose a model and download it](choose-and-download-a-model.md)
- [How to ask for capabilities and handle `not_approved` and `busy`](ask-for-capabilities.md)
- [How to react to mode changes](react-to-mode-changes.md)
- [How to ship the engine in your app](ship-the-engine.md)
- [How to prepare a macOS app](prepare-a-macos-app.md)
- [How to run your app in a Flatpak](run-in-a-flatpak.md)
- [How to handle errors and the end of each piece of work](handle-errors-and-ends.md)

## Look something up

- [Host API reference](reference.md). The transport, each request, reply, and event with its fields, the error codes, the capabilities, and what differs between modes.
- [Engine protocol](../engine-protocol.md). The protocol between an engine and its client. You need it only to write your own client of the engine.
- The README of the crate `sayso-sdk` and the docs of the crate (`cargo doc -p sayso-sdk --open`).

## How the code in this guide is checked

The code in the tutorial is `crates/sayso-sdk/examples/scribe.rs`. The code in the how-to pages is `crates/sayso-sdk/examples/guide.rs`. Both compile with the workspace (`cargo clippy --workspace --all-targets`). The script `scripts/check-sdk-guide.py` checks that each Rust block in this guide is the code of these files.
