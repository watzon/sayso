# Sayso SDKs

Everything under `sdk/` is MIT (see `LICENSE`). The rest of the repository is the Sayso app, under its own license.

| Directory | What it is |
|---|---|
| `protocol/` | The wire protocols that the SDKs and Sayso share: the Host API, the socket and model paths, and the audio helper. `protocol/testdata` is the wire corpus that every SDK and the host test against. |
| `go/` | The Go SDK, module `github.com/watzon/sayso/sdk/go`. |
| `typescript/` | The TypeScript SDK, package `@justsayso/sdk`. |
| `rust/` | The Rust SDK, crate `sayso-sdk`. |

## The boundary

- The SDKs talk to Sayso and to the engines only over the wire protocols. They never link or import code from outside `sdk/`.
- The Sayso app may import `sdk/go`. `sdk/go` never imports the app.
- All Rust code in the repository lives in `sdk/rust`, as one crate.

`go run ./scripts/sdk-boundary` from the root of the repository checks these rules.

`PROVENANCE.md` lists the files that came from the app, with the check that the author agreed.
