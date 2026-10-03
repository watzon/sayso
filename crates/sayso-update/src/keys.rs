//! The public keys that sign an update manifest.
//!
//! Each entry is an id and the 32 bytes of an Ed25519 public key in base64.
//! `cargo run -p sayso-update --example update_tool -- keygen <id>` makes a
//! key pair and prints the entry. The private key is the repository secret
//! `SAYSO_UPDATE_SIGNING_KEY` (see `docs/releasing.md`). It is never in this
//! repository.
//!
//! To change the key, add the new entry and keep the old one for a period.
//! Remove an entry only when its private key was stolen.

/// With no key here, an update check has nothing to trust, and the updater is off.
pub const RELEASE_KEYS: &[(&str, &str)] = &[("2026-10", "LxKkwF10F2Oski5tz8EgwPfhNPd90oOmm7/MX66Atgk=")];
