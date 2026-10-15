//! macOS backends (MVP task group 5): read-only scan backends `mac.posix` (readdir + fstatat)
//! and `mac.bulk` (getattrlistbulk with per-directory POSIX fallback). Both share one
//! fd-relative walker so their rows can be reconciled field by field. `hard_deny` resolves the
//! protection policy's platform hard-deny roots by identity.

pub mod bulk;
pub mod hard_deny;
pub mod posix;
mod sys;
#[cfg(test)]
mod tests;
mod walk;
