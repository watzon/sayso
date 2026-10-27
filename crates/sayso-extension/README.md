# sayso-extension

The client of the Sayso Host API. Use it to write an extension for [Sayso](https://github.com/watzon/sayso), the dictation app.

An extension is a separate app. It connects to Sayso over a local socket (a named pipe on Windows), sends requests, and gets events. With the Host API, an extension can list and download models, transcribe files, run live sessions from the microphone or from its own audio, enhance text, and get each dictation when it is done. Sayso owns the engine, the microphone, the models, and the API keys. The extension owns its UI and its data.

The calls block and the events come on a channel, so you need no async runtime.

## Example

```rust,no_run
// Starts Sayso first when it does not run, and waits up to 10 s for it.
let client = sayso_extension::Client::connect_or_start()?;
println!("connected as {}", client.hello().extension_id);
let reply = client.request("list_models", sayso_extension::Fields::new())?;
# Ok::<(), sayso_extension::Error>(())
```

Sayso accepts a connection only from an extension that the user installed. The install and sideload screens of Sayso are not released yet.

`examples/sayso-ext.rs` is a small command line extension that uses each request and prints each message as a JSON line.

## Documentation

- [The Host API](https://github.com/watzon/sayso/blob/main/docs/extensions.md): the requests, the events, the capabilities, and the rules.
- [`sayso-protocol`](https://crates.io/crates/sayso-protocol): the message types that this crate sends and reads.

## License

MIT. See `LICENSE`.
