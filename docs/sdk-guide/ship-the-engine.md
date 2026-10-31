# How to ship the engine in your app

An app that uses `Mode::Local` or `Mode::Auto` needs its own copy of the engine. The SDK does not use the engine that is inside an installed Sayso. A Sayso update would then change the engine of your app.

## Get the release file

Each Sayso release has the engine as separate files:

| System | File |
|---|---|
| macOS, Apple silicon | `sayso-engine-<version>-macos-arm64.tar.gz` |
| macOS, Intel | `sayso-engine-<version>-macos-x86_64.tar.gz` |
| Linux, x86-64 | `sayso-engine-<version>-linux-x86_64.tar.gz` |
| Linux, ARM | `sayso-engine-<version>-linux-arm64.tar.gz` |
| Windows | `sayso-engine-<version>-windows-x64.zip` |

Each file holds one folder, `sayso-engine-<version>`. In it are the program (`sayso-engine`, and `sayso-engine.exe` on Windows), `README.md`, `LICENSE`, and the folder `licenses`. On macOS, the Swift engine has the file name `sayso-engine` in the release file too.

Use the engine of the same version as your copy of the SDK. No release has the SDK yet, so for now build the engine from the same checkout as the SDK (the [tutorial](tutorial.md) shows how). The SDK checks the protocol version when it starts the engine. It checks the features that each request needs when you make the request. An old engine without the feature `catalog` fails with `unsupported`.

The Intel macOS engine is a separate build of the same Swift engine. The models run without the Neural Engine there, and they are slower.

To make a release file from a checkout, build the engine and run `scripts/package-engine.sh`. The [tutorial](tutorial.md) does this.

## Put the engine in your app

Unpack the file into a folder of your app, and ship the whole folder. Keep `LICENSE` and `licenses/` with the program. The engine is GPL-3.0-only, and you start it as a separate program. Check the license terms for what you ship.

| System | A good place |
|---|---|
| macOS app bundle | `YourApp.app/Contents/Resources/engine/` |
| Windows | A folder next to your `.exe`, for example `engine\` |
| Linux | A folder next to your program, for example `/usr/lib/yourapp/engine/` |

Rules for the folder:

- The program must be able to run. Keep its permissions (`chmod +x`).
- Do not rename `sayso-engine`. A rename keeps a macOS signature valid, but `find_engine` looks for this name.
- On macOS, the engine in the release file has a Developer ID signature of Sayso and a notarization ticket (the Intel build has no stapled ticket). Read the rules of Apple for nested code in a notarized app. Your tool may need to sign the engine again with your identity. We did not test a notarized app.

## Find the engine in your code

`Engine::from_release` takes the folder that you unpacked. It calls `find_engine`, which accepts two layouts:

- The folder `sayso-engine-<version>` itself, which holds the program.
- A folder that holds one or more `sayso-engine-<version>` folders. With more than one, it takes the one that sorts last by name.

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
fn engine_in_resources(resources: &Path) -> Engine {
    Engine::from_release(resources.join("engine"))
}
```

If the folder has no engine, `Speech::connect` with `Mode::Local` returns the error code `unsupported`, with a message that names the folder. `Mode::Auto` does not check the engine when it connects. The first request that needs the engine returns `unsupported`. If you set no `Options::engine`, both modes return `bad_request`.

To get the path of the program yourself, call `find_engine`:

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
fn engine_program(resources: &Path) -> std::io::Result<PathBuf> {
    sayso_sdk::find_engine(resources.join("engine"))
}
```

To point at a program that is not in a release layout, use `Engine::from_program`. The SDK gives the engine its folders through `Engine::store` and `Engine::models_dir`. Use them only when you must move the model files:

<!-- check: verbatim crates/sayso-sdk/examples/guide.rs -->
```rust
fn engine_with_own_folders(program: PathBuf, data: &Path) -> Engine {
    Engine::from_program(program).store(data.join("models").join("v1")).models_dir(data.join("models-old"))
}
```

## Check that the engine starts

Send `hello` to the engine on its standard input. The engine writes its version and features. It needs `SAYSO_MODELS_DIR`:

```sh
printf '{"v":1,"type":"hello","id":"1","protocol":1}\n{"v":1,"type":"shutdown","id":"2"}\n' | SAYSO_MODELS_DIR=/tmp/engine-check ./engine/sayso-engine-0.13.0/sayso-engine
```

The output has a `hello` line with `"protocol":1` and the list of features. For the SDK, the list must have `catalog`.

## Know where the models go

The engine does not ship models. The user downloads them (see [How to choose a model and download it](choose-and-download-a-model.md)). They go to the model store of the user. On macOS that is `~/Library/Application Support/Sayso/models/v1`, on Windows `%LOCALAPPDATA%\Sayso\models\v1`, and on Linux `$XDG_DATA_HOME/sayso/models/v1` (`~/.local/share/sayso/models/v1` when the variable is not set). `XDG_DATA_HOME` counts on every system. `sayso_sdk::model_store()` gives the path.

Sayso uses the same store, so a model that the user downloaded for Sayso is on disk for your app, and the other way round. The SDK gives the store only to an engine that lists `store_v1`. An older engine uses a folder named after your app in the data folder of the system.
