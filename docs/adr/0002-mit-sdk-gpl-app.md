# The SDK crates, `sayso-ui`, and the protocol specs are MIT, and the app stays GPL

Sayso is GPL-3.0-only. An extension that talks to Sayso over a socket is a separate program, but an extension that links a GPL crate must be GPL too. Few developers would build on Sayso under that rule, and extensions should look like Sayso. So these are MIT: the crates that extensions and engine clients link (`sayso-protocol`, `sayso-extension`, `sayso-engine-sdk`), the theme and components (`sayso-ui`, and `sayso-ink` with the ink types from `sayso-core`), and the specs of the engine protocol and the Host API. The app, the engine, and the other crates stay GPL-3.0-only.

## Consequences

- Chris wrote all of `sayso-ui` and `sayso_core::ink`, so he can change their license. Code from other GPL crates moves into an MIT crate only when every author of that code agrees. Two contributors other than Chris have commits in `sayso-core` and in translation files of `sayso-i18n`.
- `sayso-ui` loses its dependencies on `sayso-core` and `sayso-i18n`, because an MIT crate cannot depend on a GPL crate. GPL crates can depend on the MIT crates.
- The app icon and the tray icon stay out of the MIT crates. The name Sayso and the icon stay with Watzon Ventures LLC.
- `sayso-ui` pins `gpui-kit` with `=`, so an extension that uses it is tied to the same GPUI version.
- 2026-10-30: the cleanup logic moved to the MIT crates `sayso-cleanup-models` (the catalog, `render`, the plan of parts, `check`, the protocol) and `sayso-cleanup-client` (the client and the download), so `sayso-sdk` can offer it. Every author of that code is Chris (`git log`, checked on 2026-10-30). The cleanup runtime `sayso-cleanup` and the engines stay GPL, and an app ships them as separate programs.
