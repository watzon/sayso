# Apps use Sayso through the SDK, and there are no extensions

Status: accepted on 2026-10-29. It replaces ADR 0003, and the install and listing parts of ADR 0001.

The extension system had a manifest, install, update, a Marketplace, and a reviewed registry. Few ideas need to be an extension: the large ones (meeting notes, media transcription) are apps with their own windows and data. What they need from Sayso is its engine and its loaded models, so that a model is on disk once and in memory once, and dictation stays first. So Sayso installs nothing. Any app connects to a running Sayso when the user approves it, and the SDK (`sayso-sdk`, MIT) runs the same API with its own engine when Sayso does not run. Jobs, live sessions, and audio capture (also the system audio) move to MIT crates, so both modes run the same code. Design: [sdk.md](../sdk.md).

## Considered options

- **Keep extensions.** The install system, the Marketplace, and the registry with signing, revocation, and review are most of the remaining work, and they only pay off when other developers publish extensions.
- **Separate apps on the engine alone.** Simple, but each app downloads and loads its own copy of a model, opens its own microphone, and competes with dictation.
- **Optional features inside Sayso.** One product, but the UI and the code of each app go into Sayso, and other developers cannot build on it.

## Consequences

- The host knows an app by the identity of its executable, or by its Flatpak app id in a sandbox, and the approval of the user (`apps.toml`), not by an install folder.
- The engines get a new model store (`<models folder>/v1`) with immutable artifacts, manifests, a metadata lock, and descriptors with file hashes in the catalog (the engine feature `store_v1`), because the folders of today share files between model ids and change files in place. Older engines never open it. Sayso moves its models into it once with hard links and a journal, and a downgrade downloads them again.
- Each model artifact is on disk once for each user. While Sayso runs, a model is in memory once, except for an app in local mode by its own choice, an app that the user refused, local work that started before Sayso, and a sandboxed app without the permission for the socket of Sayso. A Flatpak has its own store, so an app in local mode and a Sayso on the other side of the sandbox can have a model on disk twice.
- An app ships its own engine release file for local mode.
- The features of the user (`enhance`, `dictation_done`, the download question) exist only when the app uses Sayso.
- In local mode the app captures audio in its own process, so the system asks the user for permission for the app.
