//! macOS Keychain backend: compile-checked stub. A Security.framework implementation
//! (generic password items, `SecItemAdd`/`SecItemCopyMatching`/`SecItemDelete`) needs evidence
//! on a real Mac before it is enabled (task 15.9); until then every call reports `unsupported`
//! and the diagnosis UI shows the API mode as unavailable instead of storing keys elsewhere.

use super::{CredentialError, CredentialStore, Secret, check_target};

const WHY: &str = "the macOS Keychain adapter is not yet verified on a real Mac";

pub struct KeychainStore;

impl CredentialStore for KeychainStore {
    fn backend(&self) -> &'static str {
        "macos_keychain"
    }
    fn read(&self, target: &str) -> Result<Option<Secret>, CredentialError> {
        check_target(target)?;
        Err(CredentialError::Unsupported(WHY))
    }
    fn write(&self, target: &str, _secret: &Secret) -> Result<(), CredentialError> {
        check_target(target)?;
        Err(CredentialError::Unsupported(WHY))
    }
    fn delete(&self, target: &str) -> Result<bool, CredentialError> {
        check_target(target)?;
        Err(CredentialError::Unsupported(WHY))
    }
}
