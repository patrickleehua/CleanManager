//! Static code signing facts for a bundle or binary on disk (Security framework, `SecStaticCode.h`
//! / `SecCode.h` / `SecRequirement.h` / `SecCertificate.h`). Read-only: nothing is executed.
//!
//! - `team_id` comes from the signature's Team Identifier; an ad-hoc signature (`codesign -s -`)
//!   and Apple's own platform code carry none.
//! - `authorities` are the certificate chain's subject summaries, leaf first
//!   (e.g. `Developer ID Application: Name (TEAMID)`, ..., `Apple Root CA`).
//! - `apple` is the `anchor apple` requirement (code signed by Apple itself, not merely an
//!   Apple-issued Developer ID), checked with basic validation: the signature over the code
//!   directory and the chain are verified, page and resource hashes are not.

use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr;

use super::cf::{self, CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex, CFDictionaryGetValue, CFGetTypeID, CFTypeRef, CfRef, Plist};

type OSStatus = i32;
/// `typedef uint32_t SecCSFlags;`
type SecCSFlags = u32;

const DEFAULT_FLAGS: SecCSFlags = 0;
/// `kSecCSSigningInformation`
const SIGNING_INFORMATION: SecCSFlags = 1 << 1;
/// `kSecCSBasicValidateOnly` = `kSecCSDoNotValidateExecutable | kSecCSDoNotValidateResources`
const BASIC_VALIDATE_ONLY: SecCSFlags = (1 << 1) | (1 << 2);
/// `kSecCodeSignatureAdhoc` in the `kSecCodeInfoFlags` value.
const SIGNATURE_ADHOC: i64 = 0x0002;
/// `errSecCSUnsigned`
const ERR_UNSIGNED: OSStatus = -67062;
/// `errSecCSReqFailed`: the code is validly signed but does not satisfy the requirement.
const ERR_REQ_FAILED: OSStatus = -67050;

#[allow(non_upper_case_globals)]
#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    /// `OSStatus SecStaticCodeCreateWithPath(CFURLRef path, SecCSFlags flags, SecStaticCodeRef *staticCode);`
    fn SecStaticCodeCreateWithPath(path: CFTypeRef, flags: SecCSFlags, code: *mut CFTypeRef) -> OSStatus;
    /// `OSStatus SecCodeCopySigningInformation(SecStaticCodeRef code, SecCSFlags flags, CFDictionaryRef *information);`
    fn SecCodeCopySigningInformation(code: CFTypeRef, flags: SecCSFlags, information: *mut CFTypeRef) -> OSStatus;
    /// `OSStatus SecStaticCodeCheckValidity(SecStaticCodeRef staticCode, SecCSFlags flags, SecRequirementRef requirement);`
    fn SecStaticCodeCheckValidity(code: CFTypeRef, flags: SecCSFlags, requirement: CFTypeRef) -> OSStatus;
    /// `OSStatus SecRequirementCreateWithString(CFStringRef text, SecCSFlags flags, SecRequirementRef *requirement);`
    fn SecRequirementCreateWithString(text: CFTypeRef, flags: SecCSFlags, requirement: *mut CFTypeRef) -> OSStatus;
    /// `CFStringRef SecCertificateCopySubjectSummary(SecCertificateRef certificate);`
    fn SecCertificateCopySubjectSummary(certificate: CFTypeRef) -> CFTypeRef;
    /// `const CFStringRef kSecCodeInfoIdentifier, kSecCodeInfoTeamIdentifier, kSecCodeInfoCertificates, kSecCodeInfoFlags;`
    static kSecCodeInfoIdentifier: CFTypeRef;
    static kSecCodeInfoTeamIdentifier: CFTypeRef;
    static kSecCodeInfoCertificates: CFTypeRef;
    static kSecCodeInfoFlags: CFTypeRef;
}

/// What a static signature says about its signer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Signature {
    /// Any signature is present (ad-hoc included).
    pub signed: bool,
    /// The signing identifier (usually the bundle ID).
    pub identifier: Option<String>,
    pub team_id: Option<String>,
    /// Certificate subject summaries, leaf first; empty for ad-hoc signatures.
    pub authorities: Vec<String>,
    pub adhoc: bool,
    /// Satisfies `anchor apple` (Apple's own code).
    pub apple: bool,
}

/// Value of `key` in a signing-information dictionary (Get rule: borrowed from `dict`).
///
/// # Safety
/// `dict` must be a live CFDictionary and `key` a live CFString.
unsafe fn field(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef {
    // SAFETY: forwarded caller contract.
    unsafe { CFDictionaryGetValue(dict, key) }
}

/// Signing facts of the bundle or binary at `path` (symbolic links are resolved by the Security
/// framework). `Err` when the path cannot be evaluated at all; unsigned code is `Ok` with
/// `signed == false`.
pub(crate) fn signature(path: &Path) -> Result<Signature, String> {
    let is_dir = path.is_dir();
    let url = cf::file_url(path.as_os_str().as_bytes(), is_dir).ok_or("path cannot be converted to a URL")?;
    let mut code: CFTypeRef = ptr::null();
    // SAFETY: live CFURL and out-pointer; on success `code` is +1.
    let st = unsafe { SecStaticCodeCreateWithPath(url.as_ptr(), DEFAULT_FLAGS, &mut code) };
    // SAFETY: owned (+1) reference or NULL.
    let code = match unsafe { CfRef::from_create(code) } {
        Some(c) if st == 0 => c,
        _ => return Err(format!("static code unavailable (OSStatus {st})")),
    };
    let mut info: CFTypeRef = ptr::null();
    // SAFETY: live static code and out-pointer; on success `info` is +1.
    let st = unsafe { SecCodeCopySigningInformation(code.as_ptr(), SIGNING_INFORMATION, &mut info) };
    // SAFETY: owned (+1) reference or NULL.
    let info = match unsafe { CfRef::from_create(info) } {
        Some(i) if st == 0 => i,
        _ => return Err(format!("signing information unavailable (OSStatus {st})")),
    };
    let d = info.as_ptr();
    let mut sig = Signature::default();
    // SAFETY: `d` is the live CFDictionary above; the keys are the framework's constant CFStrings;
    // every value read is borrowed from `d` and converted before `info` drops.
    unsafe {
        sig.identifier = cf::string_value(field(d, kSecCodeInfoIdentifier));
        sig.signed = sig.identifier.is_some();
        sig.team_id = cf::string_value(field(d, kSecCodeInfoTeamIdentifier));
        let flags = field(d, kSecCodeInfoFlags);
        if !flags.is_null()
            && let Ok(Plist::Integer(f)) = cf::to_plist(flags)
        {
            sig.adhoc = f & SIGNATURE_ADHOC != 0;
        }
        let certs = field(d, kSecCodeInfoCertificates);
        if !certs.is_null() && CFGetTypeID(certs) == CFArrayGetTypeID() {
            for i in 0..CFArrayGetCount(certs) {
                let cert = CFArrayGetValueAtIndex(certs, i);
                let summary = CfRef::from_create(SecCertificateCopySubjectSummary(cert));
                if let Some(s) = summary.and_then(|s| cf::string_value(s.as_ptr())) {
                    sig.authorities.push(s);
                }
            }
        }
    }
    if sig.signed {
        sig.apple = satisfies(&code, "anchor apple")?;
    }
    Ok(sig)
}

/// Whether the static code satisfies a requirement string (basic validation only).
fn satisfies(code: &CfRef, requirement: &str) -> Result<bool, String> {
    let text = cf::string(requirement).ok_or("bad requirement text")?;
    let mut req: CFTypeRef = ptr::null();
    // SAFETY: live CFString and out-pointer; on success `req` is +1.
    let st = unsafe { SecRequirementCreateWithString(text.as_ptr(), DEFAULT_FLAGS, &mut req) };
    // SAFETY: owned (+1) reference or NULL.
    let req = match unsafe { CfRef::from_create(req) } {
        Some(r) if st == 0 => r,
        _ => return Err(format!("requirement not compiled (OSStatus {st})")),
    };
    // SAFETY: live static code and requirement.
    match unsafe { SecStaticCodeCheckValidity(code.as_ptr(), BASIC_VALIDATE_ONLY, req.as_ptr()) } {
        0 => Ok(true),
        ERR_REQ_FAILED | ERR_UNSIGNED => Ok(false),
        st => Err(format!("signature could not be validated (OSStatus {st})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macos::cmd::Tool;
    use crate::macos::testfs::Fixture;

    #[test]
    fn calculator_is_signed_by_apple() {
        let sig = signature(Path::new("/System/Applications/Calculator.app")).unwrap();
        assert!(sig.signed && sig.apple && !sig.adhoc, "{sig:?}");
        assert_eq!(sig.identifier.as_deref(), Some("com.apple.calculator"));
        assert!(sig.authorities.iter().any(|a| a.contains("Apple")), "{sig:?}");
    }

    #[test]
    fn ad_hoc_signature_has_no_team_id() {
        let fx = Fixture::new("sec-adhoc");
        let bin = fx.path().join("true-copy");
        std::fs::copy("/usr/bin/true", &bin).unwrap();
        let out = Tool::new("/usr/bin/codesign").args(["--force", "--sign", "-"]).arg(&bin).run().unwrap();
        assert_eq!(out.exit_code, Some(0), "{}", String::from_utf8_lossy(&out.stderr));
        let sig = signature(&bin).unwrap();
        assert!(sig.signed && sig.adhoc, "{sig:?}");
        assert_eq!(sig.team_id, None);
        assert!(sig.authorities.is_empty(), "{sig:?}");
        assert!(!sig.apple);
    }

    #[test]
    fn unsigned_files_and_missing_paths() {
        let fx = Fixture::new("sec-unsigned");
        let script = fx.write("script.sh", b"#!/bin/sh\nexit 0\n");
        match signature(&script) {
            Ok(sig) => assert!(!sig.signed && !sig.apple && sig.team_id.is_none(), "{sig:?}"),
            Err(e) => assert!(e.contains("OSStatus"), "{e}"),
        }
        assert!(signature(&fx.path().join("missing")).is_err());
    }
}
