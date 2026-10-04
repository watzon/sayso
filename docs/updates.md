# Updates

Status: written as a plan on 2026-10-02 and changed after two reviews (see "Review record"). On 2026-10-03 the check, the download, the release pipeline, and the install on macOS exist. A run with two notarized builds on macOS passed (see "Release gates"). Linux and Windows show a new version and open the download page. No release has a manifest yet: the first one comes from the first release that the changed workflow builds.

This file says how Sayso finds, downloads, and installs a new version on macOS, Windows, and Linux. The method follows the updater of the Zed editor, which is also a GPUI app and ships the same three kinds of release file. The section "Differences from Zed" lists where this plan does something else, and why.

## Decisions

Chris made these decisions on 2026-10-02.

- Sayso has its own updater. It does not use Sparkle, WinSparkle, or Velopack.
- The release files stay as they are: a notarized DMG, an Inno Setup installer, and a tarball.
- By default, Sayso checks for a new version and shows that an update is available. The user starts the download and the install.
- A switch, off by default, makes Sayso download and stage updates without a question.
- The update manifest is a GitHub release asset.

## Terms

These terms go into `CONTEXT.md` with the code.

**Update manifest**
The signed file `latest.json` on a GitHub release. It names the version and, for each platform, the release file with its size and SHA-256 hash.

**Update check**
One request for the update manifest. Sayso compares the version in the manifest with its own version.

**Staged update**
A new version that Sayso downloaded, verified, and unpacked next to the installed app. It waits for the swap.

**Swap**
The step that puts the staged update in the place of the installed app. The swap runs only when Sayso stops or starts.

**Official build**
A build from the Release workflow. Only an official build updates itself.

## Security boundary

The updater protects against these attackers:

- A person on the network between the user and GitHub, also one who can break TLS.
- A person who can change or replace the files of a GitHub release but has no access to the secrets of the repository.

The updater does not protect against these:

- A program that already runs as the user. Such a program can change the installed app directly, so the staged files do not need more protection than the installed files.
- A person with write access to the repository and its secrets. A GitHub environment with a required reviewer for the signing secret narrows that. This is an option for later.

An attacker on the network can block updates. Sayso then shows that the check failed, and it installs nothing.

## What the user sees

The Paper design (page "Updates" in "Sayso — v0.1 Design") gives the layout and the text. This section fixes the behavior.

| State | Meaning | Action for the user |
|---|---|---|
| Idle | Sayso has the latest version, or no check ran yet | **Check for updates** |
| Checking | An update check is in progress | None |
| Available | The manifest has a newer version | **Update now**, and a link to the release notes |
| Downloading | Sayso downloads the release file. The state has the bytes received and the total | **Cancel** |
| Preparing | Sayso unpacks the new version and checks its signature. This takes a few seconds | None |
| Ready | A staged update waits for the swap | **Restart to update** |
| Failed | A check, download, or install failed. The state has the step and the cause | **Try again** |
| Manual | A newer version exists, but this install cannot update itself | A link to the download, and the reason |

The states show in three places:

- A group "Updates" in Settings › General, with all states.
- One line in the Popover, above the footer, for Available, Downloading, Ready, Failed, and Manual.
- A slip in the Hub sidebar, below the engine status and above the theme switch, for the same five states. A click opens Settings › General. For Ready, a click restarts Sayso.

After a swap, Sayso shows "Sayso is now X" one time in the Popover. Its button **What is new** opens the "What is new" window, or the release notes on GitHub when the version has no notes in the app.

### The "What is new" window

At the first start of a version that has an entry in `release-notes.toml`, Sayso opens a window with the notes of that version: a headline, the new features, an optional note (an announcement or a request for support), and a link to the release on GitHub. The Paper design is on the page "What is new".

- The notes are part of the build. The window needs no network, and Sayso opens no address from a server.
- The window does not depend on the updater. Sayso keeps the last version it showed in `state.json` (`whats_new`) and compares it with its own version, so an update from Homebrew, a package, or an installer shows the window too.
- A first install shows no window: the end of onboarding records the version.
- An item can name its systems. A version with nothing for this system shows no window.
- The link **See what is new** in Settings › General opens the window again.

The config has a new table:

```toml
[updates]
check = true       # Check at start and one time each day.
automatic = false  # Download and stage a new version without a question.
```

- `check = false` stops the automatic checks. **Check for updates** in Settings still works.
- `automatic = true` makes Sayso go from Available to Ready by itself. The swap then runs at the next start of Sayso, or when the user selects **Restart to update**.
- Sayso does not restart by itself. A staged update stays Ready, and the Popover shows the line until the user restarts.
- A change of the config during a download does not stop the download. A change does not remove a staged update.

An install is Manual in these cases:

- macOS: the app runs from a read-only volume or an App Translocation path, the user cannot write to the folder of the app (a standard user and `/Applications`), or the volume has no atomic exchange.
- Windows: the install is for all users.
- Linux: the install is a `--system` install, has no install marker, or its file system has no atomic exchange.
- All systems: the manifest has a `schema` that this version does not know. The text says that this version cannot read the update information, with a link to the download page.

## Update manifest

One file, `latest.json`, holds the manifest and its signatures. A single file means that a client never reads the manifest of one release with the signature of another.

```json
{
  "payload": "{\"schema\":1,\"version\":\"0.3.0\",...}",
  "signatures": [
    { "key": "2026-10", "signature": "<base64 Ed25519 signature>" }
  ]
}
```

The payload is a JSON string. Sayso takes the UTF-8 bytes of the decoded string, verifies a signature over those bytes, and only then parses them:

```json
{
  "schema": 1,
  "version": "0.3.0",
  "published": "2026-10-10T12:00:00Z",
  "notes_url": "https://github.com/watzon/sayso/releases/tag/v0.3.0",
  "assets": {
    "macos-aarch64":  { "url": "https://github.com/watzon/sayso/releases/download/v0.3.0/Sayso-0.3.0-macos-arm64.dmg", "sha256": "…", "size": 41234567 },
    "windows-x86_64": { "url": "…/Sayso-0.3.0-windows-x64-setup.exe", "sha256": "…", "size": 0 },
    "linux-x86_64":   { "url": "…/sayso-0.3.0-linux-x86_64.tar.gz", "sha256": "…", "size": 0 },
    "linux-aarch64":  { "url": "…/sayso-0.3.0-linux-aarch64.tar.gz", "sha256": "…", "size": 0 }
  }
}
```

Rules for the client:

- The URL of the manifest is `https://github.com/watzon/sayso/releases/latest/download/latest.json`. GitHub redirects it to the newest release that is not a draft and not a prerelease.
- Sayso follows redirects only to `https` URLs. The manifest is 64 KB at most. A request has a timeout.
- The app has a list of public keys, compiled in. The manifest is good when one entry of `signatures` names a known key and verifies. More than one entry lets one release carry the signature of an old key and of a new key. A manifest with no known key is the Manual state, with a link to the download page. A manifest with a known key and a bad signature is an error.
- Parsing is strict: an unknown `schema` is the Manual state, and a missing or wrong field is an error.
- Sayso compares versions with semver precedence (`Version::cmp_precedence`). It rejects a manifest whose version has a prerelease part.
- Sayso accepts a manifest only when its version is higher than the running version and not lower than the highest version it accepted before. It stores that highest version in `update-state.json`, which recovery never deletes. A replayed old manifest then cannot move a user to an older release than one that Sayso already saw.
- Each asset URL and the `notes_url` must be `https://github.com/watzon/sayso/releases/` plus a plain path: letters, digits, `.`, `-`, `_`, and `/`, with no `..` part. A URL with `/../`, a query, or an encoded character is refused. Sayso opens no other URL from a manifest.
- When the manifest has no asset for this platform, the state stays Idle.

## Update check

- The request is a `GET` with the header `User-Agent: Sayso/<version> (<os>; <arch>)`. It has no identifier of the user or the computer. GitHub sees the IP address.
- Sayso checks 30 seconds after start, then one time each 24 hours, measured with a monotonic clock. Zed checks each hour. A day is enough for a dictation app, and it makes fewer requests.
- An automatic check that fails is silent: it writes one log line, and the state stays as it was.
- A check that the user started and that fails shows Failed. A `404` counts as a failed check, not as "up to date".
- Settings shows the time of the last check that succeeded.

The README privacy section gets a line for this request in the same release as the check, because "Sayso sends nothing off the computer" is otherwise no longer true.

## Download and verify

1. Sayso streams the release file to `<cache dir>/updates/<file name>.part` and computes the SHA-256 hash during the download. It stops when the file gets larger than the size in the manifest.
2. If the size or the hash is not the value from the manifest, Sayso deletes the file and the state is Failed.
3. Sayso renames the file to its final name and stages it (see the platform sections).

A download does not resume. A new download starts from zero. The user can cancel a download, and the state goes back to Available.

## Staging and the swap

The installed app does not change until the swap, and the swap is one atomic exchange of two folders. A full disk, a failed download, or a crash during staging leaves the installed app as it was. This is the main difference from Zed, which copies with `rsync` over the installed app and can leave it broken (Zed issue 62573).

When the file system has no atomic exchange, the install is Manual. The plan has no path with two renames, because a crash between them would leave no app at the install path.

### Install lock

The lock is for one install, not for one data folder. On macOS it is a file in `~/Library/Caches/dev.sayso.Sayso/install-locks/`, named after the path of the installed app, so it does not depend on `XDG_DATA_HOME` and leaves no file in `/Applications`. Only an install that can update itself uses it.

- Each Sayso takes a shared lock at start, before it starts the engine, and holds it until it exits.
- Staging and the swap need the exclusive lock. Sayso tries to change its shared lock to an exclusive lock without waiting. If another Sayso runs from the same install, the try fails, the state stays Ready, and Sayso tries again at the next start.
- Inside one process, only one thread has the exclusive lock at a time. A file lock belongs to the whole process, so a mutex beside it keeps the updater thread and **Restart to update** apart. The exclusive lock stays until the record of the step is written.
- A Sayso that must wait for its shared lock at start waits, because a swap or a staging step is in progress. When it gets the lock, it compares its own version with the version of the installed app. If they differ, this process is the old version, and it exits. The Sayso that did the swap starts the new version, so one process owns the restart.

The exchange is atomic, so a process that starts after it sees a complete new install. The lock closes the remaining gap: an old app that looks for the engine beside itself during the exchange.

### The record

Sayso writes each of the two files below to a temporary file, calls `fsync` on the file and on the folder, and renames it.

`update-state.json` is in `<data dir>` and stays for the life of the install. Recovery never deletes it.

| Field | Contents |
|---|---|
| `highest_seen` | The highest version from a good manifest |
| `last_check` | The time of the last check that succeeded |
| `notice` | The version for the "Sayso is now X" notice, until the user saw it |

`update.json` is the record of one update. It is in the staging folder, beside the staged app, and not in the data folder. The record, the staging folder, and the lock then all belong to one install: a Sayso with another data folder sees the same record, and a second bundle in the same folder has its own. Sayso writes each phase before the action of that phase.

| Field | Contents |
|---|---|
| `envelope` | The complete `latest.json` that the staged update came from |
| `from_version`, `to_version` | The installed version and the staged version |
| `install_path` | The canonical path of the installed app at the time of staging |
| `phase` | `staged`, `swapping`, `swapped`, or `failed` |
| `attempts` | The number of swaps that started. Sayso adds 1 when it writes `swapping` |
| `error` | The step and the cause of the last failure |

The staging folder is not in the record. Sayso computes it from `install_path`, so a changed record cannot point cleanup at another folder.

Before a swap, Sayso does these checks again. If one fails, it deletes the staged update and shows Failed.

- The signature of `envelope`.
- `install_path` is the path of the running app.
- The staging folder is a real folder that the user owns, not a symlink.
- macOS: the `codesign` and `spctl` checks of the staged bundle. Windows: the hash of the installer against `envelope`.

Sayso deletes only the fixed staging folder of its own install and the files in `<cache dir>/updates`.

### When the swap runs

- The user selects **Restart to update**. The button is off during a dictation. Sayso does the swap and quits in the same step, and the new version starts when the old process is gone. Sayso does not stop the engine first: the engine exits with the app, and an explicit stop needs a new call in the engine client. Between the exchange and the quit, a few milliseconds pass.
- Sayso starts and finds a record with the phase `staged`. It does the swap before GPUI and the engine start, then starts the new version and exits.

Sayso does no swap when it quits. A swap during a logout or a shutdown can be killed in the middle.

### The swap on macOS and Linux, step by step

The code is `crates/sayso-update/src/swap.rs`.

1. Get the exclusive lock.
2. Do the checks of the record again.
3. Write the phase `swapping` and add 1 to `attempts`.
4. Exchange the two folders. Call `fsync` on the parent folder.
5. Write the phase `swapped`.
6. Start the new version. If the start fails, exchange the two folders back (Sayso still has the lock), write `failed`, and continue with the old version.
7. The new version runs. After its engine answered and 30 seconds passed, it deletes the old version from the staging folder, deletes `update.json`, and sets `notice`. This is the only place that deletes the old version.

### Recovery at start (macOS and Linux)

"Old" and "new" compare the running version with `from_version` and `to_version`.

| Record | What Sayso does |
|---|---|
| No record | Normal start. Delete a staging folder that is left over |
| `staged`, `attempts` is 0 | The swap, steps 1 to 6 |
| A record whose `install_path` is not this install | Delete the record and the staged app, normal start |
| `staged`, the staged app is missing | Delete the record, normal start |
| `swapping`, running is old | The exchange did not happen. Write `failed`, delete the staged update, show Failed |
| `swapping` or `swapped`, running is new | The exchange happened. Normal start, then step 7 |
| `swapping` or `swapped`, running is old, and the installed app is new | This process started before the exchange. Exit. The staging folder holds the old version, and it stays |
| `swapped`, running is old, and the installed app is old | Sayso exchanged back in step 6, or the user put the old version back. Write `failed`, delete the staged app |
| `failed` | Show Failed with `error`. **Try again** deletes the record and starts from the check |

Step 7 can run more than one time. It does not fail when the old version is already gone. It needs the exclusive lock, so it waits while another Sayso runs from the install.

While an update waits for step 7, or after an update failed, Sayso starts no new update by itself. The old version in the staging folder is the way back, and a failure must not repeat at each check. **Update now** and **Try again** by the user start a new update, and that removes the old version.

If the new version cannot start at all, no code of Sayso runs. The old version is then still in the staging folder (`.Sayso-update/Sayso.app` beside the app, or `.sayso-update/sayso` beside the install folder). The README says how to put it back by hand. An automatic rollback for this case is not in this plan.

### macOS

`scripts/release.sh` changes: it notarizes and staples `Sayso.app` first, then makes the DMG, then notarizes and staples the DMG. The staged bundle can then pass a notarization check without a network.

Stage:

1. `hdiutil attach -nobrowse -readonly -mountroot <temp dir> <dmg>`.
2. Copy `Sayso.app` with `ditto` to `<parent of the installed app>/.Sayso-update/Sayso.app`. The folder has the name of the bundle (`.Sayso-update` for `Sayso.app`), so two bundles in one folder do not share it. Sayso makes it itself with mode 700, and it stops if the path exists and is not a folder that it owns.
3. `hdiutil detach -force`, also when a step fails.
4. Verify the staged copy:
   - `codesign --verify --deep --strict` with this requirement: `anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = "MB5789APU7" and identifier "dev.sayso.Sayso"`. The two OID fields make sure that the signature is a Developer ID signature.
   - `spctl --assess --type execute`, which checks the notarization.

Swap: the steps of "The swap on macOS and Linux". The exchange is `renamex_np(RENAME_SWAP)` on the installed bundle and the staged bundle. The review confirmed on macOS 27 that a running app stays alive through the exchange. The start of the new version is a detached shell that waits for the process to exit and then runs `open -n` on the install path. Sayso starts this shell itself in both cases, so the swap at start and **Restart to update** use one path. The first review warned about `open -n` because it can start a second Sayso. Here the shell waits until the old process is gone, so only one Sayso runs. Without `-n`, Launch Services can decide that Sayso already runs and start nothing. The shell passes the `XDG_*` variables to `open --env`, because Launch Services does not give the app the environment of the old process.

The permission grants (Microphone, Accessibility, Input Monitoring) and the login item depend on the bundle id and the signing identity, which stay the same. The complete run must confirm this.

### Linux

`install.sh` puts the two binaries in `~/.local/lib/sayso` (or `/usr/local/lib/sayso` with `--system`). It gets one change: it writes the marker file `<lib>/install.toml` with `kind = "user"` or `kind = "system"`.

An install can update itself when all of these are true: the folder of the running binary has the marker with `kind = "user"`, the canonical path of the folder is `<home>/.local/lib/sayso`, and the user can write to the folder and to its parent. An install from 0.2.0 has no marker. It gets one when the user installs the first version with the updater.

Stage:

1. Make `<parent>/.sayso-update` with mode 700. Stop if the path exists and is not a folder that the user owns.
2. Read the tarball with the `tar` and `flate2` crates. Take only the two entries `sayso-<version>-linux-<arch>/bin/sayso` and `…/bin/sayso-engine`, and only when each is a regular file. Skip every other entry. Stop at a second entry with the same name, and stop when an entry is larger than 500 MB.
3. Write the two files and a copy of the marker to `<parent>/.sayso-update/sayso/`, mode 755 for the binaries. The staged folder has the layout of the installed folder.

Staging is complete only when both binaries are there.

Swap: the steps of "The swap on macOS and Linux". The exchange is `renameat2(RENAME_EXCHANGE)` on `<parent>/sayso` and `<parent>/.sayso-update/sayso`. The start of the new version is `<parent>/sayso/sayso`: with GPUI, `cx.set_restart_path` and `cx.restart()`, and at start an `exec`. The lock file and the IPC socket of `shell/linux.rs` are close-on-exec, so the new process takes both again. No other Sayso can get the lock in between and see a wrong state, because the exchange is complete at that time.

The symlink `~/.local/bin/sayso`, the `Exec` line of the desktop entry, and the autostart entry name `<parent>/sayso/sayso`, so they stay correct. The updater does not change the desktop entry, the metainfo file, or the icons. A release that changes them says so in its notes, and the user runs `install.sh` again.

### Windows

On Windows, Sayso downloads and verifies the installer, and then the installer does the work in view of the user. Sayso does not run Setup hidden. A hidden Setup needs a protocol between Sayso and Setup for locks, results, and the restart, and that protocol cannot be tested without a Windows desktop. A visible Setup shows its own progress and its own errors, and it is the same installer that the user knows from the first install.

Only an install for the current user gets this. An install for all users is Manual, because Setup must start again with administrator rights, and the installer has no signature that Windows can check after that step. This changes when the Windows builds are signed.

Stage: keep the verified installer in `%LOCALAPPDATA%\Sayso\updates\`. The state is Ready.

Swap, when the user selects **Restart to update** or when Sayso starts with a staged update:

1. Check the hash of the installer against `envelope`.
2. Write the phase `swapping`.
3. Start the installer, detached: `Sayso-<version>-windows-x64-setup.exe /SILENT /NORESTART /CURRENTUSER /NOCLOSEAPPLICATIONS /NORESTARTAPPLICATIONS /UPDATE=1 /DIR="<install path>" /LOG="<log dir>\update-setup.log"`. `/SILENT` shows the progress window and the error dialogs, and no wizard pages.
4. Stop the engine, wait for its process to exit, and quit Sayso.

Changes to `packaging/windows/Sayso.iss`:

- A function `IsUpdating` is true for `/UPDATE=1`.
- `PrepareToInstall` waits during an update until the mutex `Local\dev.sayso.Sayso.instance` is gone, for 15 seconds at most. `AppMutex` stays as it is: if Sayso still runs after that, Setup shows its "Sayso is running" dialog, and the user closes Sayso.
- A second `[Run]` entry starts `Sayso.exe` with `Check: IsUpdating` and `Flags: nowait`. The entry that exists has `skipifsilent`, so it does not run during an update.

Recovery at start on Windows:

| Record | What Sayso does |
|---|---|
| `staged` | The swap, steps 1 to 4 |
| `swapping`, running is new | Setup completed. Delete the installer and the record, set `notice` |
| `swapping`, running is old | Setup did not complete, or it still runs. Delete the record, keep the installer, and show Failed with the text "The installer did not finish" and the action **Try again**, which runs the installer again |

Setup replaces files one at a time, so it is not atomic as the exchange on macOS and Linux is. If Setup stops in the middle, it shows the error, and the user can run the installer again from the download page.

## Official builds

The updater is on only when the build has `SAYSO_OFFICIAL_BUILD=1` at compile time (`option_env!`). The Release workflow sets it. A build from source, a build by a distribution, and `cargo run` have no updater: Settings shows the version, and no request goes out. A changed version of Sayso under another name (the license asks for that) then never replaces itself with the official app.

## Release pipeline

Changes to `.github/workflows/release.yml` and `docs/releasing.md`:

1. Create each release as a prerelease: `gh release create v0.3.0 --prerelease …`. The `published` event still starts the workflow, and `releases/latest` still names the release before it. The review confirmed both in the GitHub documentation. The prerelease is public on the Releases page. Only the updater waits.
2. The three platform jobs set `SAYSO_OFFICIAL_BUILD=1`. Each job uploads its files to the release and also as a workflow artifact.
3. Each job that uploads to the release, the `manifest` job also, first reads the live state of the release with `gh release view "$TAG"` and stops when the release is not a prerelease. This covers a new run through `workflow_dispatch` and a rerun of one job. A release that users already get is then never changed. To build again for such a release, make a new version.
4. A new job `manifest` needs all three platform jobs. It takes the files from the workflow artifacts of this run, not from the release, so a person who can only change release files cannot get a signature for a changed file. It requires all four files, writes the payload, and signs it. Then it attaches those same four files and `latest.json` to the release. The release then has the files that the manifest names, also when another run for the tag attached other files in the meantime. While the release is a prerelease, the job can replace a `latest.json` that is already there. If the upload succeeds and the promotion in step 5 fails, run the `manifest` job again.
5. The same job then compares the version with the release that `releases/latest` names. It runs `gh release edit "$TAG" --prerelease=false --latest` only when the new version is higher. If it cannot read the latest release, it stops. Only "no release exists" counts as no latest release. All `manifest` jobs share one concurrency group, so two releases cannot be promoted at the same time.

Users see a new version only after step 5. If one platform job fails, the release stays a prerelease, and no user gets a release with a missing platform.

`docs/releasing.md` says how to make the release notes with `--notes-start-tag`, because the notes of a prerelease otherwise start from the wrong tag.

Signing:

- The signature is Ed25519. The private key is the repository secret `SAYSO_UPDATE_SIGNING_KEY`. The public keys are in `crates/sayso-update/keys/`, each with an id.
- A small program in the `sayso-update` crate makes a key pair and signs a manifest. The app and the release job then use the same code for the signature, and a test covers both.
- Chris makes the key pair on his Mac and stores the secret with `scripts/set-release-secrets.sh`. The private key also goes into his password manager. It never goes into the repository.
- To change the key: add the new public key to the app, and sign each release with the old key and the new key for a period. An app that has only the old key still updates during that period. After the period, an app that never updated in it is Manual forever, and the download page is its route.
- If the key is lost: make a new key, ship a release signed with the new key, and tell users on the website to download it. Apps with only the old key show Manual and must be updated by hand.
- If the key is stolen: do the same, and also remove the old public key from the app in that release. An app that still has the old key accepts a manifest that the thief signs, but only for files under `github.com/watzon/sayso/releases/`, so the thief also needs write access to the releases. Tell users on the website and in the release notes to install the new version by hand at once.
- `docs/releasing.md` gets both procedures.

## Code layout

A new crate, `crates/sayso-update`, holds everything that is not UI.

| Module | Contents |
|---|---|
| `manifest` | The envelope, the payload, the signature check, the platform key, and the version rules |
| `check` | The request for the manifest |
| `download` | The streamed download with the hash, the limits, the progress, and cancel |
| `record` | The record `update.json`, the install lock, the staging folder, and the recovery table |
| `install::{macos, linux, windows}` | Stage and swap for each system, behind `cfg` |
| `updater` | The state machine. It runs on its own thread and sends each state change through a channel |
| `bin/sayso-update-tool` | `keygen` and `sign`, for the release job |

- New dependencies: `ed25519-dalek`, `sha2`, `semver`, `flate2`. `ureq`, `tar`, `base64`, and `libc` are in the workspace already.
- `sayso-update` depends on `sayso-core` for `Paths` and the config. It does not depend on GPUI or on a platform crate. `scripts/check-deps.sh` gets that rule.
- `sayso-app` owns the UI and the shutdown: it refuses new dictations, stops the engine, and calls `cx.restart()`. `AppModel` holds the update state and starts the updater. `app::run` runs the recovery table at start, before GPUI. The Popover and Settings › General render the state.
- `Config` gets the `updates` table.

## Tests

- Unit tests: signature (good, bad, unknown key, two signatures, changed payload), version rules (prerelease, build metadata, lower than `highest_seen`), unknown schema, missing platform, URL prefix, size limits.
- The record: each row of the recovery table, a record with a path outside the staging folder, a staging folder that is a symlink, and a second process that cannot get the lock.
- `download` against `tiny_http`, as `sayso-enhance` does: a good file, a wrong hash, a file larger than the manifest says, a dropped connection, cancel.
- The exchange on macOS and Linux against temporary folders. The `codesign` and `spctl` checks are functions that a test replaces.
- The Linux unpack: a symlink entry, a device entry, an entry with `..`, an absolute path, a duplicate entry, and an entry that is too large.
- The Windows command line as a pure function.
- A debug build reads `SAYSO_UPDATE_URL` and a test public key, so a complete run works against a local server. A release build ignores both.

## Release gates

The check and the Manual state can ship first. Self-install stays off on a platform until its gate passes. A compile-time list of platforms with self-install controls this.

- **macOS, done on 2026-10-03, before and after the changes from the code review:** two notarized debug builds (0.2.0 and 0.2.1) in a test folder, against a local server. These passed: automatic staging from the real DMG with the `codesign` and `spctl` checks, the swap at start, **Update now** and **Restart to update** by a click, the delete of the old version after 30 seconds, the notice, and the refusal of a staged app with a changed binary. A second Sayso (the developer's own, from another folder) ran through the test without effect.
- **macOS, not done:** two notarized builds with different versions, on a clean user account. The swap from **Restart to update** and at start, a kill during staging, the three permission grants, the login item, an app in `~/Applications`, an app on the DMG, and an app with App Translocation.
- **Linux:** the OrbStack VM `sayso-linux` and the GNOME VM. A user install, a `--system` install (Manual), a start of a second `sayso` during the swap, the autostart entry, and the GNOME custom shortcuts after an update.
- **Windows:** a step in the Windows release job installs the build before it in silent mode, runs the new installer with `/UPDATE=1`, and checks the installed version. This proves the install. The progress window, the start of the new version, a Sayso that still runs, and a locked file need a run by hand on a Windows desktop, which is not available now. The installer hand-off on Windows stays off until that run is done, and **Update now** opens the download page until then.
- **GitHub:** a test release in a scratch repository confirms the prerelease and promotion steps before the first real release.

## Order of work

1. The Paper design (done as a first pass) and Chris's approval of it.
2. `sayso-update`: manifest, check, download, the record, the state machine, and the signing tool, with tests.
3. The release pipeline: the key, the artifacts, the `manifest` job, the prerelease and promotion steps, `SAYSO_OFFICIAL_BUILD`, and the two notarization steps on macOS.
4. The config, the UI, the privacy text in the README, and the state Manual on every platform. A user then sees each new version, and the action opens the download page. This can ship as one release.
5. Stage and swap: Linux, then macOS, then Windows. Each one turns on after its release gate.
6. Docs: README (install), `docs/releasing.md`, `docs/linux.md`, `CONTEXT.md`, `SECURITY.md`.

Users on 0.2.0 and older have no updater. They must install one more version by hand.

## Differences from Zed

| Topic | Zed | Sayso | Reason |
|---|---|---|---|
| Server | Its own service, `cloud.zed.dev` | A static file on GitHub | No server to run |
| Integrity | TLS only, no hash and no signature | A signed manifest, a hash, and `codesign` and `spctl` checks on macOS | The updater runs code on the computers of all users |
| Install on macOS and Linux | `rsync` over the installed app, while the app runs | Stage, then one atomic exchange when Sayso stops or starts | A failed copy cannot break the installed app. The app and the engine never run with different versions |
| Install on Windows | Installer to a side folder, then a helper program moves files | The installer replaces the files after Sayso quits | No helper program. Sayso has no open documents to keep |
| When the user must act | Never. The update installs, and the user restarts | **Update now** by default, automatic with a switch | Chris's decision |
| Interval | 1 hour | 24 hours | Fewer requests |
| Package builds | An environment variable turns the updater off | The updater is on only in an official build | A build from source never updates to the official app |

## Review record

GPT-6-Astra (Codex, high reasoning) reviewed this plan two times on 2026-10-02.

The first review gave "proceed after the blocking items are fixed". These changes came from it:

- The record holds the signed envelope, and Sayso verifies it again before a swap. Cleanup uses fixed paths only. The security boundary is stated.
- The path with two renames is gone. No atomic exchange means Manual. The record has phases.
- The app stops and reaps the engine itself. `open -n` is gone.
- An install for all users on Windows is Manual. The review showed that `runasoriginaluser` does not always remove administrator rights.
- The release pipeline signs hashes of workflow artifacts, refuses to change a promoted release, and promotes only a higher version.
- Linux: an allowlist of two tarball entries, an install marker, and no change to the desktop entry.
- macOS: the requirement has the Developer ID fields, `spctl` checks the notarization, and the app is stapled.
- The manifest has size limits, strict parsing, a list of signatures for key rotation, and a `highest_seen` rule against replay. A `404` is a failed check.

The second review checked those changes. It found the first and sixth findings resolved and gave "fix these first" for the others. These changes came from it:

- The old version is deleted only after the new version ran for 30 seconds with a working engine. The swap has numbered steps, and the recovery table names the versions it compares. A failed start of the new version exchanges the folders back.
- The lock is for the install, not for the data folder. Each Sayso holds a shared lock, and the swap needs the exclusive lock.
- Windows no longer runs Setup hidden. The second review showed that `SetupMutex` cancels a silent Setup and that the new Sayso could see the mutex and exit. The plan now hands off to a visible installer, which needs no protocol between Sayso and Setup.
- Each job that uploads checks the live state of the release, so a rerun of one job cannot change a promoted release.
- `highest_seen` is in its own file, which recovery never deletes.
- The procedures for a lost key and a stolen key are separate. An unknown key is the Manual state.

The reviews also answered the three open questions: no automatic restart, no `pkexec` on Linux, and yes to the notarization check.

The same reviewer reviewed the code on 2026-10-03, with scratch tests, and gave "fix these first". These changes came from it:

- A mutex beside the file lock keeps two threads of one process apart, and the exclusive lock stays until the record is written. Before that, **Restart to update** and an automatic download could run at the same time and install a version that the record did not name.
- The record moved from the data folder into the staging folder, and the staging folder has the name of the bundle. Before that, a Sayso with another data folder, or a second bundle in the same folder, could delete the old version of an update that was not confirmed.
- Sayso starts no new update by itself while the last one waits for its confirmation or after it failed.
- An old process that starts after the exchange exits and keeps the old version. A process that waited for the lock no longer starts the app a second time.
- The manifest job attaches the files that it signed to the release.
- The URL rule refuses `..`, a query, and an encoded character. A cancel during a failed read is a cancel. The lookup of the latest release stops on an error.

Not checked by a review or by a test, and so part of the release gates: the permission grants, Launch Services, and the login item after an exchange, `spctl` without a network, and whether a DMG that Sayso downloads gets a quarantine attribute.
