# Provenance

Some files under `sdk/` are ports of code in the Sayso app, which has another license. Decision 2 of `docs/adr/0002-mit-sdk-gpl-app.md` allows that only for code whose authors agree. Each row names the source, the commit that the port read, and the author check.

The author check is `git log --format='%an <%ae>' -- <source paths> | sort | uniq -c`. A row is allowed only when every author is Chris Watson, who wrote the app and agrees to release these parts under MIT.

| SDK file | Source in the app | Commit | Authors |
|---|---|---|---|
| `go/protocol/protocol.go` | `internal/bridge/protocol.go` | `a5d6e5c5` | Chris Watson (3 commits) |
| `go/protocol/paths.go`, `go/protocol/paths_*.go` | `internal/paths/socket.go`, `internal/paths/paths.go`, `internal/paths/socket_env_*.go`, `internal/paths/darwin_temp_darwin.go` | `a5d6e5c5` | Chris Watson |
| `go/audio/decode.go`, `go/audio/dsp.go`, `go/audio/wav.go` | `internal/bridge/audio.go`, `internal/bridge/dsp.go`, `internal/bridge/live_wav.go` | `a5d6e5c5` | Chris Watson |
| `go/cmd/sayso-fake-cleanup/main.go` | the request loop of `internal/cleanupd/cleanupd.go` | `a5d6e5c5` | Chris Watson |
| `go/engine/engine.go`, `go/engine/requests.go`, `go/engine/hide_windows.go` | the message shapes of `internal/engine/client.go`, `internal/engine/protocol.go`, `internal/engine/hide_windows.go` | `a5d6e5c5` | Chris Watson |
| `go/speech/*` (jobs, live sessions, sources, echo rule, silence limit, queue, WAV recording) and `go/speech/testdata/echo_pairs.tsv` | `internal/bridge/jobs.go`, `jobrunner.go`, `live*.go`, `vad.go`, `testdata/echo_pairs.tsv`, and their tests | `a5d6e5c5` | Chris Watson (10 commits on `internal/bridge`) |
| `go/capture/internal/device/*`, `go/capture/mic/*`, `go/capture/system/*`, `go/permission/*` | `internal/platform/audio_darwin.go`, `audio_linux.go`, `audio_windows.go`, `permissions_*.go` | `a5d6e5c5` | Chris Watson (20 commits on `internal/platform`) |
| `go/cleanup/models.go`, `go/cleanup/cleaner.go`, `go/cleanup/download.go`, `go/cleanup/runtime.go` | `internal/enhance/cleanup.go`, `internal/enhance/cleanupdl.go`, the protocol of `internal/cleanupd` | `a5d6e5c5` | Chris Watson (7 commits) |
| `go/cleanup/text.go` | `mobile/core/textrules` | `a5d6e5c5` | Chris Watson (1 commit) |
| `go/deepgram/*`, `go/embedded/*` | `crates/sayso-sdk/src/deepgram.rs`, `crates/sayso-sdk/src/embedded/` (the Rust SDK before it moved to `sdk/rust`) | `5a4e2d3b` | Chris Watson (10 commits on `crates/sayso-sdk/src`) |

Files that are not in this table were written for the SDK.

The wire protocols themselves (`protocol/*.md`, `docs/engine-protocol.md`) are MIT by decision 1 of ADR 0002.
