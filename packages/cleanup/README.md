# @justsayso/cleanup

The runtime of the local cleanup models of [Sayso](https://github.com/watzon/sayso), for apps in Node, Electron, Bun, and Deno. This package installs the runtime `sayso-cleanup` for your platform, as an optional dependency. It supports macOS (arm64 and x64), Linux (x64 and arm64, with glibc), and Windows (x64).

```sh
npm install @justsayso/sdk @justsayso/cleanup
```

`@justsayso/sdk` finds the runtime with `cleanup.runtimeFromPackage()`. Give the path to `RuntimeCleaner`:

```js
import { cleanup, RuntimeCleaner } from "@justsayso/sdk";

const cleaner = new RuntimeCleaner({
	runtime: cleanup.runtimeFromPackage(),
	modelsDir: "/path/to/models",
});
```

To find the folder yourself, call `runtimeFolder()`. It gives the folder of the runtime package of this platform. On x64 for Linux and Windows the folder has three builds, for three levels of the processor. `cleanup.bestRuntime(folder)` of the SDK gives the build that the processor runs.

```js
import { runtimeFolder } from "@justsayso/cleanup";

console.log(runtimeFolder());
```

Do not install this package with `--no-optional` or `--omit=optional`. Then no runtime package is installed, and `runtimeFolder()` throws an error.

The models are not in this package. `cleanup.download()` of the SDK downloads them.

This package is free software under the GPL-3.0-only license. The SDK has another license (MIT).
