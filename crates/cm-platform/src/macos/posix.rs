//! `mac.posix`: scoped, read-only enumeration with `openat` + `readdir` +
//! `fstatat(AT_SYMLINK_NOFOLLOW)` (SCN-01/02/03/05/06/07). The baseline every other macOS
//! backend is reconciled against; the walk itself lives in `walk.rs`.

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
