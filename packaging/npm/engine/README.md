# @justsayso/engine

The speech engine of [Sayso](https://github.com/watzon/sayso) for the Local mode
of the Sayso SDKs. This package has no code: it installs the package of your
platform (`@justsayso/engine-<platform>`), which holds the engine release file
of the same version. `Engine.fromPackage()` of `@justsayso/sdk` finds it and
starts the engine as a program, over the engine protocol.

In a packaged Electron app, unpack the platform package from the asar file
(`asarUnpack: ["**/node_modules/@justsayso/engine-*/**"]`), because a program in
an asar file cannot start.

License: GPL-3.0-only. The SDK is MIT and only runs this program; it does not
link it.
