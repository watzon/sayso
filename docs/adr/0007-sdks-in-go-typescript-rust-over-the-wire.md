# The SDKs are in Go, TypeScript, and Rust, and they talk to Sayso and the engines over the wire

Status: accepted on 2026-11-27. It replaces [ADR 0006](0006-typescript-sdk-wraps-the-rust-sdk.md).

Sayso became a Go app (`cmd/sayso`), and the Rust crates went with the port (`28830e18`, `bcdaa523`). ADR 0006 made the TypeScript SDK a native Node addon around the Rust SDK, so without the Rust workspace there was no SDK in any language. Apps need one in the language of the app, with the three modes, jobs, live sessions, capture, and the same end rules in each. So there are three SDKs, all MIT, under `sdk/`: Go (`sdk/go`), TypeScript (`@justsayso/sdk`, pure TypeScript with no runtime dependency), and Rust (`sayso-sdk`, one crate). Each talks to Sayso over the Host API, to an engine over the engine protocol, and to the cleanup runtime over the cleanup protocol. The specs in `sdk/protocol` and the wire corpus in `sdk/protocol/testdata` are the contract. The TypeScript SDK gets capture and the FLAC, MP3, and AAC decoders from `sayso-audio`, an MIT Go program of `sdk/go`, over NDJSON on stdio. Design: [sdk.md](../sdk.md).

## Considered options

- **Bring the Rust SDK back and keep the addon of ADR 0006.** One copy of the rules, but Rust in a Go repository only for the SDK, five native addons to build and publish, and no Go SDK, which the app itself can use.
- **One SDK in Go, with bindings for the others.** A Go library in another language needs cgo and a C ABI, which the rest of the repository avoids (`CGO_ENABLED=0`), and a native addon again for Node.
- **TypeScript and Rust as clients of a local Go daemon.** Less code in each SDK, but an app would ship and supervise one more process, also for Sayso mode.

## Consequences

- Three copies of the rules of jobs and live sessions: Go (a port of `internal/bridge`), TypeScript, and Rust (from the crates of `5a4e2d3b`). The wire corpus, one list of scenarios, and `sdk-integration.yml` (the three suites against one test host and the same fakes) keep them the same.
- No native compilation for an SDK: the Go SDK builds with `CGO_ENABLED=0`, the TypeScript SDK has no addon, and the Rust SDK is a normal crate. All Rust code is in `sdk/rust`.
- The Sayso app may import `sdk/go` (MIT into GPL), and does for the system audio. `sdk/go` never imports the app. `scripts/sdk-boundary` checks this, the licenses, and the versions.
- The SDKs have the version of the app.
- The engines and the cleanup runtime stay GPL programs. An app ships their release files, or installs `@justsayso/engine` and `@justsayso/cleanup` (GPL-3.0-only, in `packaging/npm`), which an MIT package names only as optional peers.
- The Go SDK and the Go host have no system audio on macOS yet: the Core Audio process tap needs Objective-C blocks. The Rust SDK has the tap. TypeScript has what the Go code has.
