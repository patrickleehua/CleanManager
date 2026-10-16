//! `mac.posix`: scoped, read-only enumeration with `openat` + `readdir` +
//! `fstatat(AT_SYMLINK_NOFOLLOW)` (SCN-01/02/03/05/06/07). The baseline every other macOS
//! backend is reconciled against; the walk itself lives in `walk.rs`.
//!
//! `shared_blocks` is always `None` here: `struct stat` has no clone/sharing field, and the
//! only per-file source (`getattrlistat` with `ATTR_CMNEXT_EXT_FLAGS`) is one more syscall per
//! entry, which would change what this baseline measures. Block sharing comes from `mac.bulk`,
//! where the same attributes arrive in the listing itself at no extra call.

use cm_core::path::RawPath;
use cm_core::scan::{RootProbe, ScanBackend, ScanOptions, ScanReport, ScanSink};

use super::sys::{Fd, list_posix};
use super::walk::{self, Lister, Listing};

const METHOD: &str = "readdir + fstatat(AT_SYMLINK_NOFOLLOW) directory enumeration";

pub(crate) struct PosixLister;

impl Lister for PosixLister {
    fn list(&mut self, dir: &Fd) -> Listing {
        Listing::plain(list_posix(dir))
    }
}

pub struct PosixBackend;

impl ScanBackend for PosixBackend {
    fn name(&self) -> &'static str {
        "mac.posix"
    }

    fn probe(&self, root: &RawPath) -> RootProbe {
        walk::probe(self.name(), root, METHOD)
    }

    fn scan(&self, root: &RawPath, sink: &mut dyn ScanSink, options: &ScanOptions) -> ScanReport {
        walk::scan(self.name(), METHOD, root, sink, options, &mut PosixLister)
    }
}
