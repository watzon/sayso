//! The update manifest: one signed file, `latest.json`, on a GitHub release.
//!
//! The file is an envelope with a payload and its signatures. The payload is
//! a JSON string, and a signature covers the UTF-8 bytes of that string. So
//! Sayso verifies first and parses only bytes that it trusts.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, VerifyingKey};
pub use semver::Version;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A manifest file is never larger than this.
pub const MAX_BYTES: usize = 64 * 1024;

/// The payload version that this build reads.
pub const SCHEMA: u32 = 1;

/// Each URL in a manifest must start with this.
pub const URL_PREFIX: &str = "https://github.com/watzon/sayso/releases/";

/// The file as it is on the release.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub payload: String,
    pub signatures: Vec<EnvelopeSignature>,
}

/// One signature of the payload. More than one lets a release carry the
/// signature of an old key and of a new key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeSignature {
    /// The id of the key in [`crate::keys::RELEASE_KEYS`].
    pub key: String,
    /// The Ed25519 signature in base64.
    pub signature: String,
}

/// The payload of schema 1. A later release can add fields, so unknown
/// fields are not an error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub version: Version,
    /// RFC 3339. For display only.
    pub published: String,
    pub notes_url: String,
    /// The release file of each system, by [`crate::platform`].
    pub assets: BTreeMap<String, Asset>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Asset {
    pub url: String,
    /// Lower-case hex.
    pub sha256: String,
    pub size: u64,
}

/// A public key with its id.
#[derive(Debug, Clone)]
pub struct PublicKey {
    pub id: String,
    key: VerifyingKey,
}

impl PublicKey {
    /// `key` is 32 bytes in base64.
    pub fn parse(id: &str, key: &str) -> Result<Self, ManifestError> {
        let bytes: [u8; 32] = STANDARD.decode(key).ok().and_then(|b| b.try_into().ok()).ok_or(ManifestError::BadKey)?;
        let key = VerifyingKey::from_bytes(&bytes).map_err(|_| ManifestError::BadKey)?;
        Ok(Self { id: id.to_string(), key })
    }

    /// The keys that are compiled in. A key that does not parse is a build
    /// mistake, so it is dropped with a log line and never trusted.
    pub fn release_keys() -> Vec<Self> {
        crate::keys::RELEASE_KEYS
            .iter()
            .filter_map(|(id, key)| {
                Self::parse(id, key).inspect_err(|_| log::error!("update: the public key {id} is not valid")).ok()
            })
            .collect()
    }
}

/// What a file with a good shape contains.
#[derive(Debug, Clone, PartialEq)]
pub enum Verified {
    Manifest(Manifest),
    /// No signature names a key that this build has. A newer Sayso signs
    /// with a newer key, so the user must update by hand.
    UnknownKey,
    /// The signature is good, but the payload has a schema that this build
    /// does not read.
    UnknownSchema,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ManifestError {
    #[error("the update information is too large")]
    TooLarge,
    #[error("the update information is not readable: {0}")]
    Malformed(String),
    #[error("a public key is not valid")]
    BadKey,
    #[error("the signature of the update information is not correct")]
    BadSignature,
    #[error("the update information has a value that is not permitted: {0}")]
    NotPermitted(String),
}

/// Verify the bytes of `latest.json` against `keys`, then parse the payload.
pub fn verify(bytes: &[u8], keys: &[PublicKey]) -> Result<Verified, ManifestError> {
    verify_for(bytes, keys, URL_PREFIX)
}

/// [`verify`] with another prefix for the URLs. Only a debug build with a
/// local test server uses another prefix.
pub fn verify_for(bytes: &[u8], keys: &[PublicKey], url_prefix: &str) -> Result<Verified, ManifestError> {
    if bytes.len() > MAX_BYTES {
        return Err(ManifestError::TooLarge);
    }
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|e| ManifestError::Malformed(e.to_string()))?;
    // Every signature of a known key must be good. One bad signature is an
    // error, also when another one is good.
    let mut known = 0;
    for entry in &envelope.signatures {
        let Some(key) = keys.iter().find(|k| k.id == entry.key) else { continue };
        let signature = STANDARD
            .decode(&entry.signature)
            .ok()
            .and_then(|b| Signature::from_slice(&b).ok())
            .ok_or(ManifestError::BadSignature)?;
        key.key.verify_strict(envelope.payload.as_bytes(), &signature).map_err(|_| ManifestError::BadSignature)?;
        known += 1;
    }
    if known == 0 {
        return Ok(Verified::UnknownKey);
    }

    #[derive(Deserialize)]
    struct Schema {
        schema: u32,
    }
    let schema: Schema = serde_json::from_str(&envelope.payload).map_err(|e| ManifestError::Malformed(e.to_string()))?;
    if schema.schema != SCHEMA {
        return Ok(Verified::UnknownSchema);
    }
    let manifest: Manifest = serde_json::from_str(&envelope.payload).map_err(|e| ManifestError::Malformed(e.to_string()))?;
    manifest.validate(url_prefix)?;
    Ok(Verified::Manifest(manifest))
}

/// True when `url` is the prefix plus a plain path below it: letters,
/// digits, `.`, `-`, `_`, and `/`, with no `..` part. So a URL cannot leave
/// the prefix with `/../`, a query, or an encoded character.
fn below_prefix(url: &str, url_prefix: &str) -> bool {
    url.strip_prefix(url_prefix).is_some_and(|rest| {
        !rest.is_empty()
            && rest.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'/'))
            && rest.split('/').all(|part| part != ".." && part != ".")
    })
}

impl Manifest {
    fn validate(&self, url_prefix: &str) -> Result<(), ManifestError> {
        let not_permitted = |what: String| Err(ManifestError::NotPermitted(what));
        if !self.version.pre.is_empty() {
            return not_permitted(format!("version {}", self.version));
        }
        if !below_prefix(&self.notes_url, url_prefix) {
            return not_permitted(format!("notes_url {}", self.notes_url));
        }
        for (platform, asset) in &self.assets {
            if !below_prefix(&asset.url, url_prefix) {
                return not_permitted(format!("{platform} url {}", asset.url));
            }
            let hex = asset.sha256.len() == 64 && asset.sha256.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
            if !hex {
                return not_permitted(format!("{platform} sha256"));
            }
            if asset.size == 0 {
                return not_permitted(format!("{platform} size"));
            }
        }
        Ok(())
    }
}

/// What a good manifest means for this install.
#[derive(Debug, Clone, PartialEq)]
pub enum Offer {
    /// The manifest has no newer version, or no file for this system.
    UpToDate,
    Newer(Release),
    /// The manifest is older than one that Sayso saw before. A server that
    /// sends it again must not move the user to that older release.
    Replayed,
}

/// A newer version for this system.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: Version,
    pub notes_url: String,
    pub asset: Asset,
}

/// Compare a manifest with the running version and with the highest version
/// from an earlier good manifest. Build metadata does not count.
pub fn offer(manifest: &Manifest, running: &Version, highest_seen: Option<&Version>, platform: &str) -> Offer {
    use std::cmp::Ordering::*;
    if manifest.version.cmp_precedence(running) != Greater {
        return Offer::UpToDate;
    }
    if let Some(seen) = highest_seen
        && manifest.version.cmp_precedence(seen) == Less
    {
        return Offer::Replayed;
    }
    match manifest.assets.get(platform) {
        Some(asset) => Offer::Newer(Release {
            version: manifest.version.clone(),
            notes_url: manifest.notes_url.clone(),
            asset: asset.clone(),
        }),
        None => Offer::UpToDate,
    }
}

/// Make the envelope for `payload`. The release job and the tests use it.
pub fn sign(payload: &str, key_id: &str, key: &ed25519_dalek::SigningKey) -> Envelope {
    use ed25519_dalek::Signer as _;
    let signature = key.sign(payload.as_bytes());
    Envelope {
        payload: payload.to_string(),
        signatures: vec![EnvelopeSignature { key: key_id.to_string(), signature: STANDARD.encode(signature.to_bytes()) }],
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    pub fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    pub fn public(id: &str, key: &SigningKey) -> PublicKey {
        PublicKey::parse(id, &STANDARD.encode(key.verifying_key().to_bytes())).unwrap()
    }

    pub fn payload(version: &str) -> String {
        serde_json::json!({
            "schema": 1,
            "version": version,
            "published": "2026-10-10T12:00:00Z",
            "notes_url": format!("{URL_PREFIX}tag/v{version}"),
            "assets": {
                "macos-aarch64": {
                    "url": format!("{URL_PREFIX}download/v{version}/Sayso-{version}-macos-arm64.dmg"),
                    "sha256": "ab".repeat(32),
                    "size": 41_234_567,
                },
            },
        })
        .to_string()
    }

    pub fn file(payload: &str, key_id: &str, key: &SigningKey) -> Vec<u8> {
        serde_json::to_vec(&sign(payload, key_id, key)).unwrap()
    }

    fn manifest(bytes: &[u8], keys: &[PublicKey]) -> Manifest {
        match verify(bytes, keys).unwrap() {
            Verified::Manifest(m) => m,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_signed_manifest_verifies() {
        let k = key(1);
        let m = manifest(&file(&payload("0.3.0"), "a", &k), &[public("a", &k)]);
        assert_eq!(m.version, Version::new(0, 3, 0));
        assert_eq!(m.assets["macos-aarch64"].size, 41_234_567);
    }

    #[test]
    fn a_changed_payload_fails() {
        let k = key(1);
        let mut envelope = sign(&payload("0.3.0"), "a", &k);
        envelope.payload = envelope.payload.replace("0.3.0", "0.9.0");
        let bytes = serde_json::to_vec(&envelope).unwrap();
        assert_eq!(verify(&bytes, &[public("a", &k)]), Err(ManifestError::BadSignature));
    }

    #[test]
    fn a_signature_of_another_key_under_a_known_id_fails() {
        let bytes = file(&payload("0.3.0"), "a", &key(2));
        assert_eq!(verify(&bytes, &[public("a", &key(1))]), Err(ManifestError::BadSignature));
    }

    #[test]
    fn an_unknown_key_is_not_an_error() {
        let bytes = file(&payload("0.3.0"), "new", &key(2));
        assert_eq!(verify(&bytes, &[public("old", &key(1))]), Ok(Verified::UnknownKey));
        assert_eq!(verify(&bytes, &[]), Ok(Verified::UnknownKey));
    }

    #[test]
    fn one_known_signature_of_two_is_enough() {
        let (old, new) = (key(1), key(2));
        let p = payload("0.3.0");
        let mut envelope = sign(&p, "old", &old);
        envelope.signatures.extend(sign(&p, "new", &new).signatures);
        let bytes = serde_json::to_vec(&envelope).unwrap();
        assert_eq!(manifest(&bytes, &[public("old", &old)]).version, Version::new(0, 3, 0));
        assert_eq!(manifest(&bytes, &[public("new", &new)]).version, Version::new(0, 3, 0));
    }

    #[test]
    fn a_bad_signature_beside_a_good_one_fails() {
        let (old, new) = (key(1), key(2));
        let p = payload("0.3.0");
        let mut envelope = sign(&p, "old", &old);
        envelope.signatures.push(EnvelopeSignature { key: "new".into(), signature: STANDARD.encode([0u8; 64]) });
        let bytes = serde_json::to_vec(&envelope).unwrap();
        assert_eq!(verify(&bytes, &[public("old", &old), public("new", &new)]), Err(ManifestError::BadSignature));
    }

    #[test]
    fn an_unknown_schema_is_not_an_error() {
        let k = key(1);
        let bytes = file(r#"{"schema":2,"anything":true}"#, "a", &k);
        assert_eq!(verify(&bytes, &[public("a", &k)]), Ok(Verified::UnknownSchema));
    }

    #[test]
    fn an_unknown_schema_with_a_bad_signature_fails() {
        let bytes = file(r#"{"schema":2}"#, "a", &key(2));
        assert_eq!(verify(&bytes, &[public("a", &key(1))]), Err(ManifestError::BadSignature));
    }

    #[test]
    fn a_file_that_is_too_large_or_not_json_fails() {
        assert_eq!(verify(&vec![b' '; MAX_BYTES + 1], &[]), Err(ManifestError::TooLarge));
        assert!(matches!(verify(b"<html>", &[]), Err(ManifestError::Malformed(_))));
        assert!(matches!(verify(br#"{"payload":"{}","signatures":[],"more":1}"#, &[]), Err(ManifestError::Malformed(_))));
    }

    #[test]
    fn values_that_are_not_permitted_fail() {
        let k = key(1);
        let keys = [public("a", &k)];
        let cases = [
            payload("0.3.0").replace("https://github.com/watzon/sayso/releases/download", "https://example.com/download"),
            payload("0.3.0").replace("https://github.com/watzon/sayso/releases/tag", "http://github.com/watzon/sayso/releases/tag"),
            payload("0.3.0-beta.1"),
            // A path that leaves the releases of Sayso, a query, and an encoded character.
            payload("0.3.0").replace("releases/download/v0.3.0/", "releases/../../../other/project/"),
            payload("0.3.0").replace(".dmg", ".dmg?x=1"),
            payload("0.3.0").replace("releases/tag/", "releases/%2e%2e/"),
            payload("0.3.0").replace(&"ab".repeat(32), "AB"),
            payload("0.3.0").replace("41234567", "0"),
        ];
        for case in cases {
            assert!(matches!(verify(&file(&case, "a", &k), &keys), Err(ManifestError::NotPermitted(_))), "{case}");
        }
        // A field that schema 1 needs is missing.
        let missing = file(r#"{"schema":1,"version":"0.3.0"}"#, "a", &k);
        assert!(matches!(verify(&missing, &keys), Err(ManifestError::Malformed(_))));
    }

    #[test]
    fn the_offer_follows_the_version_rules() {
        let k = key(1);
        let m = manifest(&file(&payload("0.3.0"), "a", &k), &[public("a", &k)]);
        let v = |s: &str| Version::parse(s).unwrap();
        let mac = "macos-aarch64";
        assert!(matches!(offer(&m, &v("0.2.0"), None, mac), Offer::Newer(r) if r.version == v("0.3.0")));
        assert_eq!(offer(&m, &v("0.3.0"), None, mac), Offer::UpToDate);
        assert_eq!(offer(&m, &v("0.4.0"), None, mac), Offer::UpToDate);
        // Build metadata does not make a version newer or older.
        assert_eq!(offer(&m, &v("0.3.0+local"), None, mac), Offer::UpToDate);
        // A development build of the same version is older than the release.
        assert!(matches!(offer(&m, &v("0.3.0-dev"), None, mac), Offer::Newer(_)));
        assert_eq!(offer(&m, &v("0.2.0"), None, "linux-riscv64"), Offer::UpToDate);
        assert!(matches!(offer(&m, &v("0.2.0"), Some(&v("0.3.0")), mac), Offer::Newer(_)));
        assert_eq!(offer(&m, &v("0.2.0"), Some(&v("0.3.1")), mac), Offer::Replayed);
        // An install that is already newer has nothing to fear from an old manifest.
        assert_eq!(offer(&m, &v("0.3.1"), Some(&v("0.3.1")), mac), Offer::UpToDate);
    }
}
