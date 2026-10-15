//! macOS backends (MVP task group 5): read-only scan backends `mac.posix` (readdir + fstatat)
//! and `mac.bulk` (getattrlistbulk with per-directory POSIX fallback). Both share one
//! fd-relative walker so their rows can be reconciled field by field.

pub mod bulk;
pub mod posix;
mod sys;
#[cfg(test)]
mod tests;
mod walk;
