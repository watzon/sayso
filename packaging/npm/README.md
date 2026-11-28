# GPL npm packages

The npm packages of the programs of Sayso. They are GPL-3.0-only, like the app,
and live outside `sdk/`, which is MIT. An MIT package names them only as an
optional peer, never as a dependency (`scripts/sdk-boundary`).

| Folder | Package | Files |
|---|---|---|
| `engine/` | `@justsayso/engine` | README, LICENSE, and an optional dependency on each platform package |
| `engine-<platform>/` | `@justsayso/engine-<platform>` | the engine release file `sayso-engine-<version>-<os>-<arch>` |
| `cleanup/` | `@justsayso/cleanup` | README, LICENSE, and an optional dependency on each platform package |
| `cleanup-<platform>/` | `@justsayso/cleanup-<platform>` | the cleanup release file `sayso-cleanup-<version>-<os>-<arch>` (no `darwin-x64`) |

Each folder here holds only the `package.json` (and the README of a main
package). `go run ./scripts/npm-packages -release-dir <dir> -out <dir>` makes
the folders to publish from the release files of a release: it checks each
file against its `.sha256`, unpacks it into its platform package, and adds the
LICENSE of the repository. The SDK reads the folder of the platform package and
starts the program in it; no JavaScript of these packages ever runs.

The version of every package is the version of the app
(`internal/update/version.go`). `scripts/sdk-boundary` checks it.
