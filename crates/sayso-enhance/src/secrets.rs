//! API key storage. Keys never go in `config.toml`: the config names an
//! account, and the secret store holds the key (plan §3, "AI enhancement").

use std::collections::HashMap;

/// The Keychain service name for all Sayso secrets.
pub const KEYCHAIN_SERVICE: &str = "dev.sayso.Sayso";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("secret store error: {0}")]
pub struct SecretError(pub String);

/// A place to keep secrets, addressed by account name.
pub trait SecretStore: Send + Sync {
    /// The secret for `account`. None when there is none, or when the store
    /// cannot be read. An empty secret counts as none.
    fn get(&self, account: &str) -> Option<String>;
    /// Save the secret, replacing an earlier one.
    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError>;
    /// Remove the secret. Removing a secret that does not exist is not an error.
    fn delete(&self, account: &str) -> Result<(), SecretError>;
}

/// Secrets in the macOS Keychain, under the service [`KEYCHAIN_SERVICE`].
#[derive(Debug, Clone, Copy, Default)]
pub struct KeychainStore;

fn entry(account: &str) -> Result<keyring::Entry, SecretError> {
    keyring::Entry::new(KEYCHAIN_SERVICE, account).map_err(|e| SecretError(e.to_string()))
}

impl SecretStore for KeychainStore {
    fn get(&self, account: &str) -> Option<String> {
        match entry(account).ok()?.get_password() {
            Ok(secret) if !secret.is_empty() => Some(secret),
            Ok(_) | Err(keyring::Error::NoEntry) => None,
            Err(e) => {
                log::warn!("could not read the Keychain item \"{account}\": {e}");
                None
            }
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError> {
        entry(account)?.set_password(secret).map_err(|e| SecretError(e.to_string()))
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        match entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SecretError(e.to_string())),
        }
    }
}

/// Secrets in memory. For tests, and for runs without a Keychain.
#[derive(Debug, Default)]
pub struct MemoryStore {
    secrets: parking_lot::Mutex<HashMap<String, String>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, account: &str) -> Option<String> {
        self.secrets.lock().get(account).filter(|s| !s.is_empty()).cloned()
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), SecretError> {
        self.secrets.lock().insert(account.to_string(), secret.to_string());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), SecretError> {
        self.secrets.lock().remove(account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_set_get_delete() {
        let store = MemoryStore::new();
        assert_eq!(store.get("a"), None);
        store.set("a", "one").unwrap();
        store.set("a", "two").unwrap();
        assert_eq!(store.get("a").as_deref(), Some("two"));
        store.delete("a").unwrap();
        store.delete("a").unwrap();
        assert_eq!(store.get("a"), None);
    }

    #[test]
    fn empty_secret_counts_as_missing() {
        let store = MemoryStore::new();
        store.set("a", "").unwrap();
        assert_eq!(store.get("a"), None);
    }
}
