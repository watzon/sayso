# sayso-extension

**This crate moved to [`sayso-sdk`](https://crates.io/crates/sayso-sdk).** The new crate has this client in `sayso_sdk::client`, and `Speech`, which also works when Sayso does not run. Use it for new apps. This crate gets no more changes.

The client of the Sayso Host API. Use it to write an app that uses [Sayso](https://github.com/watzon/sayso), the dictation app.

A connected app is a separate app. It connects to Sayso over a local socket (a named pipe on Windows), sends requests, and gets events. With the Host API, an extension can list and download models, transcribe files, run live sessions from the microphone or from its own audio, enhance text, and get each dictation when it is done. Sayso owns the engine, the microphone, the models, and the API keys. The extension owns its UI and its data.

The calls block and the events come on a channel, so you need no async runtime.

## Example

```rust,no_run
// Starts Sayso first when it does not run, and waits up to 10 s for it.
let client = sayso_extension::Client::connect_or_start()?;
println!("Sayso {}", client.hello().host_version);
let reply = client.request("list_models", sayso_extension::Fields::new())?;
# Ok::<(), sayso_extension::Error>(())
```

An app can run from any folder. At its first connection, Sayso asks the user whether it may use Sayso, and `connect` waits for the answer. Use `Client::connect_as` to tell Sayso a name and the capabilities that the app needs. An app that the user refuses gets the error `not_approved`.

`examples/sayso-ext.rs` is a small command line app that uses each request and prints each message as a JSON line.

## Documentation

- [The Host API](https://github.com/watzon/sayso/blob/main/docs/extensions.md): the requests, the events, the capabilities, and the rules.
- [`sayso-protocol`](https://crates.io/crates/sayso-protocol): the message types that this crate sends and reads.

## License

MIT. See `LICENSE`.
