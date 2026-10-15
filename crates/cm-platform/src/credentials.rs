//! Provider credentials for the optional AI diagnosis (AID-12). Keys live only in the system
//! credential store (Windows Credential Manager; macOS Keychain once verified on hardware) under
//! a fixed target name. They are never written to CleanManager's files, never returned to the
//! WebView and never formatted into logs: [`Secret`] prints as `<redacted>`.

use std::fmt;

#[cfg(target_os = "macos")]
#[path = "macos/credentials.rs"]
mod macos_store;
#[cfg(windows)]
#[path = "windows/credentials.rs"]
mod windows_store;

/// Longest secret accepted (API keys are far shorter; the store's blob limit is 2560 bytes).
pub const MAX_SECRET_BYTES: usize = 2048;
/// Prefix of every target name this tool writes.
pub const TARGET_PREFIX: &str = "CleanManager:diag:";

/// A credential in memory. Not `Clone`, not `Display`; `Debug` is redacted and the bytes are
/// overwritten on drop (best effort: copies made by the OS or an HTTP stack are outside it).
pub struct Secret(String);

impl Secret {
    pub fn new(s: String) -> Result<Self, CredentialError> {
        let t = s.trim();
        if t.is_empty() || t.len() > MAX_SECRET_BYTES || t.chars().any(|c| c.is_control()) {
            return Err(CredentialError::InvalidSecret);
        }
        let out = Secret(t.to_owned());
        drop(Secret(s));
        Ok(out)
    }

    /// The secret text, for the one header that needs it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        for b in bytes.iter_mut() {
            // SAFETY: `b` is a valid, exclusive reference into `bytes`.
            unsafe { std::ptr::write_volatile(b, 0) };
        }
        std::hint::black_box(&bytes);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialError {
    /// The platform store is not available in this build (e.g. macOS until verified).
    Unsupported(&'static str),
    /// The store refused the operation; the OS error code, never the secret.
    Store(u32),
    InvalidTarget,
    InvalidSecret,
    /// The person closed the system prompt.
    Cancelled,
}

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CredentialError::Unsupported(why) => write!(f, "credential store unsupported: {why}"),
            CredentialError::Store(code) => write!(f, "credential store error {code}"),
            CredentialError::InvalidTarget => f.write_str("invalid credential target name"),
            CredentialError::InvalidSecret => f.write_str("the key is empty, too long or contains control characters"),
            CredentialError::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for CredentialError {}

/// Store access. Implementations never log or return secrets except through [`CredentialStore::read`].
pub trait CredentialStore: Send + Sync {
    /// Stable name of the backend (`windows_credential_manager`, `macos_keychain`, `memory`).
    fn backend(&self) -> &'static str;
    fn read(&self, target: &str) -> Result<Option<Secret>, CredentialError>;
    fn write(&self, target: &str, secret: &Secret) -> Result<(), CredentialError>;
    /// `Ok(false)` when nothing was stored.
    fn delete(&self, target: &str) -> Result<bool, CredentialError>;
    /// Whether a credential exists, without materializing it longer than needed.
    fn exists(&self, target: &str) -> Result<bool, CredentialError> {
        self.read(target).map(|s| s.is_some())
    }
}

/// Target name for one provider id (`[a-z0-9-]{1,40}`).
pub fn target_for(provider_id: &str) -> Result<String, CredentialError> {
    if provider_id.is_empty() || provider_id.len() > 40 || !provider_id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
        return Err(CredentialError::InvalidTarget);
    }
    Ok(format!("{TARGET_PREFIX}{provider_id}"))
}

pub(crate) fn check_target(target: &str) -> Result<(), CredentialError> {
    match target.strip_prefix(TARGET_PREFIX) {
        Some(id) => target_for(id).map(|_| ()),
        None => Err(CredentialError::InvalidTarget),
    }
}

/// The platform credential store.
pub fn os_store() -> Box<dyn CredentialStore> {
    #[cfg(windows)]
    {
        Box::new(windows_store::WindowsCredentialStore)
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(macos_store::KeychainStore)
    }
}

/// Ask for a key in a system dialog so it never passes through the WebView (Windows: the
/// CredUI "Windows Security" prompt; the key goes in the password field). `Ok(None)` when the
/// person cancels.
pub fn prompt_for_secret(caption: &str, message: &str) -> Result<Option<Secret>, CredentialError> {
    #[cfg(windows)]
    {
        windows_store::prompt(caption, message)
    }
    #[cfg(target_os = "macos")]
    {
        let _ = (caption, message);
        Err(CredentialError::Unsupported("the macOS key prompt is not verified on hardware yet"))
    }
}

/// In-memory store for tests and the browser preview; never persisted.
#[derive(Default)]
pub struct MemoryStore(std::sync::Mutex<std::collections::BTreeMap<String, String>>);

impl CredentialStore for MemoryStore {
    fn backend(&self) -> &'static str {
        "memory"
    }
    fn read(&self, target: &str) -> Result<Option<Secret>, CredentialError> {
        check_target(target)?;
        Ok(self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(target).map(|s| Secret(s.clone())))
    }
    fn write(&self, target: &str, secret: &Secret) -> Result<(), CredentialError> {
        check_target(target)?;
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(target.into(), secret.expose().into());
        Ok(())
    }
    fn delete(&self, target: &str) -> Result<bool, CredentialError> {
        check_target(target)?;
        Ok(self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(target).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_never_format() {
        let s = Secret::new("sk-test-not-a-real-key-123".into()).unwrap();
        assert_eq!(format!("{s:?}"), "Secret(<redacted>)");
        assert!(Secret::new(" ".into()).is_err());
        assert!(Secret::new("a\nb".into()).is_err());
        assert!(Secret::new("x".repeat(MAX_SECRET_BYTES + 1)).is_err());
    }

    #[test]
    fn target_names_are_fixed_and_validated() {
        assert_eq!(target_for("anthropic-default").unwrap(), "CleanManager:diag:anthropic-default");
        for bad in ["", "A", "a/b", "a b", "..", &"a".repeat(41)] {
            assert!(target_for(bad).is_err(), "{bad}");
        }
        let m = MemoryStore::default();
        assert!(m.read("SomeOtherApp:token").is_err(), "only this tool's targets are reachable");
    }

    #[test]
    fn memory_store_round_trip_and_revocation() {
        let m = MemoryStore::default();
        let t = target_for("p1").unwrap();
        assert!(!m.exists(&t).unwrap());
        m.write(&t, &Secret::new("k-123".into()).unwrap()).unwrap();
        assert_eq!(m.read(&t).unwrap().unwrap().expose(), "k-123");
        assert!(m.delete(&t).unwrap());
        assert!(!m.delete(&t).unwrap());
        assert!(m.read(&t).unwrap().is_none());
    }
}
