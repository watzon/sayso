# sayso-protocol

The messages of the [Sayso](https://github.com/watzon/sayso) Host API and of the engine protocol, as serde types.

- The requests, replies, and events of the Host API, with the error codes and the capabilities.
- The messages of the engine protocol, in the module `engine`.
- PCM helpers (`pcm`) for the audio that goes over the socket.
- The socket path rules (`socket`): where Sayso opens its socket on each platform.

Each message is one JSON object on one line, with `"v":1`, a string `type`, and an optional string `id`. `encode` writes a message and `decode` reads one. A reader ignores fields that it does not know, and a message type that it does not know comes back as `Decoded::Unknown`, so an old reader does not fail on a newer peer.

This crate does no I/O. To write an app that uses Sayso, use [`sayso-sdk`](https://crates.io/crates/sayso-sdk). To run a speech engine from your own app, use [`sayso-engine-sdk`](https://crates.io/crates/sayso-engine-sdk).

## Documentation

- [The Host API](https://github.com/watzon/sayso/blob/main/docs/extensions.md)
- [The engine protocol](https://github.com/watzon/sayso/blob/main/docs/engine-protocol.md)

## License

MIT. See `LICENSE`.
