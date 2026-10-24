# The registry is a GitHub repository with people who review each listing

Extensions are listed in a GitHub repository with one file for each extension and a signed `index.json`. Each extension lives in the public repository of its author, and the listing points to its release files with a size and a SHA-256 for each platform that it supports. People review each pull request. CI checks that the repository is public, and it checks the hashes, the manifests, and the macOS signatures. It does not build the extensions. A macOS build must be signed with a Developer ID and notarized to be listed. Windows and Linux builds need no signature, because Sayso itself is not signed on Windows. An app outside the registry can use the engine alone or be sideloaded in developer mode, with no review and no notarization.

## Considered options

- **CI builds each extension from tagged source (as Flathub does).** It proves that the binary came from the source, but Chris would maintain build systems for other developers and sign their code with his own certificate.
- **No review, only automatic checks.** A registry that Sayso shows to every user needs a person in the loop.
- **Closed-source listings.** A reviewer can say little about a binary with no source, so the repository must be public.

## Consequences

- The index has its own signing key, separate from the key of the updater. Sayso has both public keys in its binary.
- `revoked.toml` in the registry turns off a version on every computer at the next fetch of the index.
- Review cannot prove that a binary matches its source. On macOS, notarization adds a scan by Apple and a second way to revoke a build. On Windows and Linux, the hash in the signed index and revocation are the only checks.
