# Apps use Sayso through the SDK, and there are no extensions

Status: proposed on 2026-10-29. It replaces ADR 0003, and the install and listing parts of ADR 0001.

The extension system had a manifest, install, update, a Marketplace, and a reviewed registry. Few ideas need to be an extension: the large ones (meeting notes, media transcription) are apps with their own windows and data. What they need from Sayso is its engine and its loaded models, so that a model is on disk once and in memory once, and dictation stays first. So Sayso installs nothing. Any app connects to a running Sayso when the user approves it, and the SDK (`sayso-sdk`, MIT) runs the same API with its own engine when Sayso does not run. Jobs, live sessions, and audio capture (also the system audio) move to MIT crates, so both modes run the same code. Design: [sdk.md](../sdk.md).

## Considered options

- **Keep extensions.** The install system, the Marketplace, and the registry with signing, revocation, and review are most of the remaining work, and they only pay off when other developers publish extensions.
- **Separate apps on the engine alone.** Simple, but each app downloads and loads its own copy of a model, opens its own microphone, and competes with dictation.
- **Optional features inside Sayso.** One product, but the UI and the code of each app go into Sayso, and other developers cannot build on it.

## Consequences

- The host knows an app by the identity of its executable and the approval of the user (`apps.toml`), not by an install folder.
- The models folder of Sayso becomes a store that more than one engine uses at the same time, so both engines need lock rules (the engine feature `shared_store`).
- An app ships its own engine release file for local mode.
- The features of the user (`enhance`, `dictation_done`, the download question) exist only when the app uses Sayso.
- In local mode the app captures audio in its own process, so the system asks the user for permission for the app.
