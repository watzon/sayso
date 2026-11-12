# @justsayso/engine

The speech engine of [Sayso](https://github.com/watzon/sayso) for apps in Node, Electron, Bun, and Deno. This package installs the engine for your platform, as an optional dependency. It supports macOS (arm64 and x64), Linux (x64 and arm64, with glibc), and Windows (x64).

```sh
npm install @justsayso/sdk @justsayso/engine
```

`@justsayso/sdk` finds the engine with `Engine.fromPackage()`. To find it yourself, call `releaseFolder()`. It gives the folder of the engine package of this platform:

```js
import { releaseFolder } from "@justsayso/engine";

console.log(releaseFolder());
```

Do not install this package with `--no-optional` or `--omit=optional`. Then no engine package is installed, and `releaseFolder()` throws an error.

This package is free software under the GPL-3.0-only license. The SDK has another license (MIT).
