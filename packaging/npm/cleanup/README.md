# @justsayso/cleanup

The runtime of the local cleanup models of [Sayso](https://github.com/watzon/sayso)
(`sayso-cleanup`, llama.cpp) for the Sayso SDKs. This package has no code: it
installs the package of your platform (`@justsayso/cleanup-<platform>`), which
holds the cleanup release file of the same version, with its llama.cpp
libraries. `@justsayso/sdk` finds it and starts the runtime as a program, over
the cleanup protocol v1. macOS on Intel has no runtime package.

In a packaged Electron app, unpack the platform package from the asar file
(`asarUnpack: ["**/node_modules/@justsayso/cleanup-*/**"]`), because a program
in an asar file cannot start.

License: GPL-3.0-only. The SDK is MIT and only runs this program; it does not
link it.
