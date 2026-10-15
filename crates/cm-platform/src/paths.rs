//! Per-user data locations. The store lives in the user's application-data directory, outside
//! any likely scan root, and is overridable with `CM_STORE_DIR` (tests, portable runs).

use std::path::PathBuf;

pub fn default_store_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CM_STORE_DIR") {
        return Some(PathBuf::from(dir));
    }
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("CleanManager").join("store"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library").join("Application Support").join("CleanManager").join("store"))
    }
}

/// Lossless conversion of an OS path argument into the protocol raw path.
pub fn raw_path(os: &std::ffi::OsStr) -> Result<cm_core::path::RawPath, cm_core::path::PathError> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        cm_core::path::RawPath::windows(os.encode_wide().collect())
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        cm_core::path::RawPath::unix(os.as_bytes().to_vec())
    }
}
