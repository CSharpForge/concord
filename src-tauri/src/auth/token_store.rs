//! Where the Discord token lives. Only the OS keychain in production; the
//! token is never written to SQLite, logs, or sent to the web UI.

use std::fmt;
#[cfg(test)]
use std::sync::Mutex;

const SERVICE: &str = "app.concord.desktop";
const ACCOUNT: &str = "discord-token";

#[derive(Debug)]
pub struct StoreError(pub String);

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "secure storage error: {}", self.0)
    }
}

impl std::error::Error for StoreError {}

pub trait TokenStore: Send + Sync {
    fn save(&self, token: &str) -> Result<(), StoreError>;
    fn load(&self) -> Result<Option<String>, StoreError>;
    /// Deleting a token that does not exist is not an error.
    fn delete(&self) -> Result<(), StoreError>;
}

/// Windows Credential Manager (via the `keyring` crate). Concord supports Windows only.
pub struct KeyringStore {
    service: String,
    account: String,
}

impl KeyringStore {
    /// The entry Concord uses in production.
    pub fn new() -> Self {
        Self::with_names(SERVICE, ACCOUNT)
    }

    pub fn with_names(service: &str, account: &str) -> Self {
        Self { service: service.to_owned(), account: account.to_owned() }
    }

    fn entry(&self) -> Result<keyring::Entry, StoreError> {
        keyring::Entry::new(&self.service, &self.account).map_err(|e| StoreError(e.to_string()))
    }
}

impl TokenStore for KeyringStore {
    fn save(&self, token: &str) -> Result<(), StoreError> {
        self.entry()?.set_password(token).map_err(|e| StoreError(e.to_string()))
    }

    fn load(&self) -> Result<Option<String>, StoreError> {
        match self.entry()?.get_password() {
            Ok(t) => Ok(Some(t)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(StoreError(e.to_string())),
        }
    }

    fn delete(&self) -> Result<(), StoreError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(StoreError(e.to_string())),
        }
    }
}

/// In-memory store for tests.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore(Mutex<Option<String>>);

#[cfg(test)]
impl TokenStore for MemoryStore {
    fn save(&self, token: &str) -> Result<(), StoreError> {
        *self.0.lock().unwrap() = Some(token.to_owned());
        Ok(())
    }
    fn load(&self) -> Result<Option<String>, StoreError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn delete(&self) -> Result<(), StoreError> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_roundtrip() {
        let s = MemoryStore::default();
        assert_eq!(s.load().unwrap(), None);
        s.save("abc").unwrap();
        assert_eq!(s.load().unwrap().as_deref(), Some("abc"));
        s.delete().unwrap();
        s.delete().unwrap(); // idempotent
        assert_eq!(s.load().unwrap(), None);
    }

    /// Talks to the real OS credential store (Windows Credential Manager; on a
    /// development machine, whatever the OS provides). Uses a throwaway entry name.
    /// Run with: cargo test -- --ignored keyring_roundtrip
    #[test]
    #[ignore = "needs a real OS keychain"]
    fn keyring_roundtrip() {
        let s = KeyringStore::with_names("app.concord.desktop.test", "roundtrip-test");
        s.delete().unwrap();
        assert_eq!(s.load().unwrap(), None);
        s.save("secret-1").unwrap();
        assert_eq!(s.load().unwrap().as_deref(), Some("secret-1"));
        s.save("secret-2").unwrap(); // overwrite
        assert_eq!(s.load().unwrap().as_deref(), Some("secret-2"));
        s.delete().unwrap();
        s.delete().unwrap(); // idempotent
        assert_eq!(s.load().unwrap(), None);
    }
}
