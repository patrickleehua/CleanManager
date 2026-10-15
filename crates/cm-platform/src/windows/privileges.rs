//! Ordinary-permission context. An elevated token may carry SeBackupPrivilege/SeRestorePrivilege
//! *enabled*; together with FILE_FLAG_BACKUP_SEMANTICS they bypass file ACLs. Scans run with
//! ordinary permissions by default (elevation is a separate, explicit, recorded choice) and the
//! executor must never inherit an ACL bypass, so both privileges are disabled for the process.

use std::io;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LUID};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, GetTokenInformation, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY, TokenPrivileges,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

const BYPASS: [&str; 2] = ["SeBackupPrivilege", "SeRestorePrivilege"];

struct Token(HANDLE);

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: handle from OpenProcessToken, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

fn token(access: u32) -> io::Result<Token> {
    let mut h: HANDLE = std::ptr::null_mut();
    // SAFETY: current-process pseudo handle; out-pointer is valid.
    if unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut h) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Token(h))
}

fn luid(name: &str) -> io::Result<LUID> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut id = LUID { LowPart: 0, HighPart: 0 };
    // SAFETY: NUL-terminated name; local system lookup.
    if unsafe { LookupPrivilegeValueW(std::ptr::null(), wide.as_ptr(), &mut id) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(id)
}

/// Names of ACL-bypass privileges currently enabled in this process token.
pub fn enabled_bypass_privileges() -> io::Result<Vec<&'static str>> {
    let t = token(TOKEN_QUERY)?;
    let mut needed = 0u32;
    // SAFETY: size query with a null buffer.
    unsafe { GetTokenInformation(t.0, TokenPrivileges, std::ptr::null_mut(), 0, &mut needed) };
    let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(1)];
    // SAFETY: buffer of at least `needed` bytes, 8-byte aligned.
    if unsafe { GetTokenInformation(t.0, TokenPrivileges, buf.as_mut_ptr().cast(), needed, &mut needed) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the API wrote a TOKEN_PRIVILEGES header followed by PrivilegeCount entries.
    let privs = unsafe {
        let header = &*(buf.as_ptr() as *const TOKEN_PRIVILEGES);
        std::slice::from_raw_parts(header.Privileges.as_ptr(), header.PrivilegeCount as usize)
    };
    let mut out = Vec::new();
    for name in BYPASS {
        let id = luid(name)?;
        if privs.iter().any(|p| p.Luid.LowPart == id.LowPart && p.Luid.HighPart == id.HighPart && p.Attributes & SE_PRIVILEGE_ENABLED != 0) {
            out.push(name);
        }
    }
    Ok(out)
}

/// Disable SeBackupPrivilege and SeRestorePrivilege for this process (they stay available to
/// an explicit, separately authorized elevated mode). Absent privileges are not an error.
pub fn disable_bypass_privileges() -> io::Result<()> {
    let t = token(TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY)?;
    for name in BYPASS {
        let tp = TOKEN_PRIVILEGES { PrivilegeCount: 1, Privileges: [LUID_AND_ATTRIBUTES { Luid: luid(name)?, Attributes: 0 }] };
        // SAFETY: one-entry TOKEN_PRIVILEGES; previous state not requested.
        if unsafe { AdjustTokenPrivileges(t.0, 0, &tp, 0, std::ptr::null_mut(), std::ptr::null_mut()) } == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypass_privileges_can_be_disabled() {
        disable_bypass_privileges().unwrap();
        assert!(enabled_bypass_privileges().unwrap().is_empty());
    }
}
