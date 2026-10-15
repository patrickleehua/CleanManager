//! Windows Credential Manager backend (generic credentials, local-machine persistence for the
//! current user) and the CredUI key prompt. Secrets are stored as UTF-8 blobs; errors carry
//! only the Win32 error code.

use windows_sys::Win32::Foundation::{ERROR_CANCELLED, ERROR_NOT_FOUND, GetLastError};
use windows_sys::Win32::Security::Credentials::{
    CRED_PACK_GENERIC_CREDENTIALS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CREDUI_INFOW, CREDUIWIN_GENERIC, CredDeleteW, CredFree,
    CredReadW, CredUIPromptForWindowsCredentialsW, CredUnPackAuthenticationBufferW, CredWriteW,
};
use windows_sys::Win32::System::Com::CoTaskMemFree;

use super::{CredentialError, CredentialStore, MAX_SECRET_BYTES, Secret, check_target};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn zero(buf: &mut [u16]) {
    for u in buf.iter_mut() {
        // SAFETY: `u` is a valid, exclusive reference into `buf`.
        unsafe { std::ptr::write_volatile(u, 0) };
    }
}

pub struct WindowsCredentialStore;

impl CredentialStore for WindowsCredentialStore {
    fn backend(&self) -> &'static str {
        "windows_credential_manager"
    }

    fn read(&self, target: &str) -> Result<Option<Secret>, CredentialError> {
        check_target(target)?;
        let name = wide(target);
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `name` is NUL-terminated; `cred` receives a buffer owned by the API.
        let ok = unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) };
        if ok == 0 {
            // SAFETY: plain thread-local error read.
            let code = unsafe { GetLastError() };
            return if code == ERROR_NOT_FOUND { Ok(None) } else { Err(CredentialError::Store(code)) };
        }
        // SAFETY: CredReadW succeeded, so `cred` points at a valid CREDENTIALW whose blob holds
        // `CredentialBlobSize` bytes; the copy is taken before CredFree.
        let result = unsafe {
            let c = &*cred;
            let len = c.CredentialBlobSize as usize;
            let bytes = if c.CredentialBlob.is_null() || len == 0 || len > MAX_SECRET_BYTES {
                None
            } else {
                Some(std::slice::from_raw_parts(c.CredentialBlob, len).to_vec())
            };
            CredFree(cred as *const _);
            bytes
        };
        match result {
            None => Err(CredentialError::InvalidSecret),
            Some(b) => match String::from_utf8(b) {
                Ok(s) => Secret::new(s).map(Some),
                Err(e) => {
                    drop(Secret::new(String::from_utf8_lossy(e.as_bytes()).into_owned()));
                    Err(CredentialError::InvalidSecret)
                }
            },
        }
    }

    fn write(&self, target: &str, secret: &Secret) -> Result<(), CredentialError> {
        check_target(target)?;
        let mut name = wide(target);
        let mut user = wide("cleanmanager");
        let mut comment = wide("CleanManager optional AI diagnosis provider key");
        let mut blob = secret.expose().as_bytes().to_vec();
        let cred = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: name.as_mut_ptr(),
            Comment: comment.as_mut_ptr(),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: user.as_mut_ptr(),
            ..Default::default()
        };
        // SAFETY: every pointer in `cred` refers to a live buffer for the duration of the call.
        let ok = unsafe { CredWriteW(&cred, 0) };
        for b in blob.iter_mut() {
            // SAFETY: valid exclusive reference into `blob`.
            unsafe { std::ptr::write_volatile(b, 0) };
        }
        if ok == 0 {
            // SAFETY: plain thread-local error read.
            return Err(CredentialError::Store(unsafe { GetLastError() }));
        }
        Ok(())
    }

    fn delete(&self, target: &str) -> Result<bool, CredentialError> {
        check_target(target)?;
        let name = wide(target);
        // SAFETY: `name` is NUL-terminated.
        let ok = unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) };
        if ok == 0 {
            // SAFETY: plain thread-local error read.
            let code = unsafe { GetLastError() };
            return if code == ERROR_NOT_FOUND { Ok(false) } else { Err(CredentialError::Store(code)) };
        }
        Ok(true)
    }
}

/// CredUI "Windows Security" prompt; the key is taken from the password field.
pub fn prompt(caption: &str, message: &str) -> Result<Option<Secret>, CredentialError> {
    let caption = wide(caption);
    let message = wide(message);
    let info = CREDUI_INFOW {
        cbSize: std::mem::size_of::<CREDUI_INFOW>() as u32,
        hwndParent: std::ptr::null_mut(),
        pszMessageText: message.as_ptr(),
        pszCaptionText: caption.as_ptr(),
        hbmBanner: std::ptr::null_mut(),
    };
    let mut package = 0u32;
    let mut out: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut out_len = 0u32;
    let mut save = 0;
    // SAFETY: `info` and its strings outlive the call; the output buffer is freed below.
    let rc = unsafe {
        CredUIPromptForWindowsCredentialsW(&info, 0, &mut package, std::ptr::null(), 0, &mut out, &mut out_len, &mut save, CREDUIWIN_GENERIC)
    };
    if rc == ERROR_CANCELLED {
        return Ok(None);
    }
    if rc != 0 || out.is_null() {
        return Err(CredentialError::Store(rc));
    }
    let mut user = vec![0u16; 514];
    let mut domain = vec![0u16; 338];
    let mut pass = vec![0u16; MAX_SECRET_BYTES + 1];
    let (mut ul, mut dl, mut pl) = (user.len() as u32, domain.len() as u32, pass.len() as u32);
    // SAFETY: `out` holds `out_len` bytes from CredUI; each output buffer has the stated capacity.
    let ok = unsafe {
        CredUnPackAuthenticationBufferW(
            CRED_PACK_GENERIC_CREDENTIALS, out, out_len, user.as_mut_ptr(), &mut ul, domain.as_mut_ptr(), &mut dl, pass.as_mut_ptr(), &mut pl,
        )
    };
    // SAFETY: `out` is a CoTaskMem buffer of `out_len` bytes owned by us; zero it, then free it.
    unsafe {
        let bytes = std::slice::from_raw_parts_mut(out as *mut u8, out_len as usize);
        for b in bytes.iter_mut() {
            std::ptr::write_volatile(b, 0);
        }
        CoTaskMemFree(out);
    }
    zero(&mut user);
    zero(&mut domain);
    if ok == 0 {
        zero(&mut pass);
        // SAFETY: plain thread-local error read.
        return Err(CredentialError::Store(unsafe { GetLastError() }));
    }
    let n = pass.iter().position(|&u| u == 0).unwrap_or(pass.len());
    let text = String::from_utf16(&pass[..n]).map_err(|_| CredentialError::InvalidSecret);
    zero(&mut pass);
    Secret::new(text?).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::target_for;

    /// Writes, reads and revokes a throw-away test value under a test-only target name; no real
    /// credential is read or printed.
    #[test]
    fn credential_manager_round_trip_with_a_test_target() {
        let store = WindowsCredentialStore;
        let target = target_for(&format!("test-{}", std::process::id())).unwrap();
        let _ = store.delete(&target);
        assert!(store.read(&target).unwrap().is_none(), "absent before the test");
        let value = Secret::new(format!("not-a-real-key-{}", std::process::id())).unwrap();
        store.write(&target, &value).unwrap();
        let back = store.read(&target).unwrap().expect("stored");
        assert_eq!(back.expose(), value.expose());
        assert!(store.exists(&target).unwrap());
        assert!(store.delete(&target).unwrap(), "revocation removes it");
        assert!(store.read(&target).unwrap().is_none());
        assert!(!store.delete(&target).unwrap());
        assert!(store.read("Some:other:app").is_err(), "targets outside this tool's prefix are refused");
    }
}
