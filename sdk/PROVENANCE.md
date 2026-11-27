# Provenance

Some files under `sdk/` are ports of code in the Sayso app, which has another license. Decision 2 of `docs/adr/0002-mit-sdk-gpl-app.md` allows that only for code whose authors agree. Each row names the source, the commit that the port read, and the author check.

The author check is `git log --format='%an <%ae>' -- <source paths> | sort | uniq -c`. A row is allowed only when every author is Chris Watson, who wrote the app and agrees to release these parts under MIT.

| SDK file | Source in the app | Commit | Authors |
|---|---|---|---|
| `go/protocol/protocol.go` | `internal/bridge/protocol.go` | `a5d6e5c5` | Chris Watson (3 commits) |
| `go/protocol/paths.go`, `go/protocol/paths_*.go` | `internal/paths/socket.go`, `internal/paths/paths.go`, `internal/paths/socket_env_*.go`, `internal/paths/darwin_temp_darwin.go` | `a5d6e5c5` | Chris Watson |

Files that are not in this table were written for the SDK.

The wire protocols themselves (`protocol/*.md`, `docs/engine-protocol.md`) are MIT by decision 1 of ADR 0002.
