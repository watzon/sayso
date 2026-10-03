//! The release tool for update manifests.
//!
//!   update_tool keygen <key id>
//!       Make an Ed25519 key pair. Print the entry for `src/keys.rs` on
//!       stdout and the private key on stderr.
//!
//!   update_tool sign <key id> <payload file>
//!       Read the private key (base64) from the variable
//!       SAYSO_UPDATE_SIGNING_KEY. Print the envelope, `latest.json`, on stdout.
//!       For a test against a local server, SAYSO_UPDATE_URL_PREFIX names
//!       the prefix that the URLs in the payload must have.
//!
//!   update_tool verify <manifest file>
//!       Verify a `latest.json` against the keys in `src/keys.rs`.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::SigningKey;
use sayso_update::manifest::{self, PublicKey, Verified};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    if let Err(message) = run(&args) {
        eprintln!("error: {message}");
        std::process::exit(1);
    }
}

fn run(args: &[&str]) -> Result<(), String> {
    match args {
        ["keygen", id] => {
            let mut seed = [0u8; 32];
            getrandom::fill(&mut seed).map_err(|e| format!("no random bytes: {e}"))?;
            let key = SigningKey::from_bytes(&seed);
            println!("(\"{id}\", \"{}\"),", STANDARD.encode(key.verifying_key().to_bytes()));
            eprintln!("{}", STANDARD.encode(seed));
            Ok(())
        }
        ["sign", id, payload] => {
            let secret = std::env::var("SAYSO_UPDATE_SIGNING_KEY").map_err(|_| "SAYSO_UPDATE_SIGNING_KEY is not set")?;
            let seed: [u8; 32] = STANDARD
                .decode(secret.trim())
                .ok()
                .and_then(|b| b.try_into().ok())
                .ok_or("SAYSO_UPDATE_SIGNING_KEY is not 32 bytes in base64")?;
            let key = SigningKey::from_bytes(&seed);
            let payload = std::fs::read_to_string(payload).map_err(|e| format!("cannot read {payload}: {e}"))?;
            let envelope = manifest::sign(payload.trim(), id, &key);
            let bytes = serde_json::to_vec(&envelope).map_err(|e| e.to_string())?;
            // Refuse to print a manifest that the app with this key would not accept.
            let public = PublicKey::parse(id, &STANDARD.encode(key.verifying_key().to_bytes())).map_err(|e| e.to_string())?;
            let prefix = std::env::var("SAYSO_UPDATE_URL_PREFIX").unwrap_or_else(|_| manifest::URL_PREFIX.to_string());
            match manifest::verify_for(&bytes, &[public], &prefix).map_err(|e| e.to_string())? {
                Verified::Manifest(_) => {}
                other => return Err(format!("the payload does not verify: {other:?}")),
            }
            println!("{}", String::from_utf8_lossy(&bytes));
            Ok(())
        }
        ["verify", file] => {
            let bytes = std::fs::read(file).map_err(|e| format!("cannot read {file}: {e}"))?;
            match manifest::verify(&bytes, &PublicKey::release_keys()).map_err(|e| e.to_string())? {
                Verified::Manifest(m) => {
                    println!("ok: version {} for {}", m.version, m.assets.keys().cloned().collect::<Vec<_>>().join(", "));
                    Ok(())
                }
                other => Err(format!("{other:?}")),
            }
        }
        _ => Err("usage: update_tool keygen <key id> | sign <key id> <payload file> | verify <manifest file>".into()),
    }
}
