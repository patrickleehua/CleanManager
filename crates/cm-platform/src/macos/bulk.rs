//! `mac.bulk`: the same scoped walk as `mac.posix`, but each directory is listed with
//! `getattrlistbulk` (SCN-02). Records are parsed by their *returned* attribute bitmap with
//! every offset and length bounds-checked; a per-entry `ATTR_CMN_ERROR` stays an error (never a
//! zero-sized object); entries missing a required attribute are completed with
//! `fstatat(AT_SYMLINK_NOFOLLOW)`; a directory whose file system answers `ENOTSUP`, or whose
//! reply contains a malformed record, is re-listed with the POSIX path. Rows are identical in
//! shape and meaning to `mac.posix` so both can be reconciled on the same fixture.
//!
//! Block sharing (SCN-06, MVP 5.3): the request also asks for the extended common attributes
//! `ATTR_CMNEXT_PRIVATESIZE | ATTR_CMNEXT_CLONEID | ATTR_CMNEXT_EXT_FLAGS`
//! (`FSOPT_ATTR_CMN_EXTENDED`, carried in the `forkattr` slots). A file's `shared_blocks` is
//! - `Some(true)` when `EF_MAY_SHARE_BLOCKS` is set (an APFS clone, either side), or when the
//!   returned private size is below the allocated size (some blocks are also held by a clone
//!   or a snapshot and would not be freed with this file);
//! - `Some(false)` when the extended flags were returned with `EF_MAY_SHARE_BLOCKS` clear and
//!   the private size gives no contrary evidence;
//! - `None` when the flags were not returned (HFS+, other file systems, the POSIX fallback,
//!   `fstatat` supplementation) and there is no private-size evidence. Nothing is faked as 0.
//!
//! The private size is never turned into a reclaimable number: exclusive APFS block ownership
//! is not verified on real hardware, so every estimate stays null with basis
//! `shared_blocks_unknown` ([`reclaimable_estimate`], docs/platforms/macos.md §0/§3).

use cm_core::path::RawPath;
use cm_core::scan::{ObjectRow, RootProbe, ScanBackend, ScanOptions, ScanReport, ScanSink};

use super::sys::{Fd, Kind, Meta, Record, errno, list_posix};
use super::walk::{self, Lister, Listing};

const METHOD: &str = "getattrlistbulk directory enumeration";
const NOTE_UNSUPPORTED: &str = "getattrlistbulk unsupported for some directories; those were listed with readdir + fstatat";
const NOTE_MALFORMED: &str = "malformed getattrlistbulk replies; affected directories were re-listed with readdir + fstatat";
const NOTE_SUPPLEMENTED: &str = "some entries lacked bulk attributes and were completed with fstatat";
const NOTE_NO_EXTENDED: &str = "extended common attributes (clone flags, private size) were refused; block sharing is unknown";
/// Initial attribute buffer (docs/platforms/macos.md §1).
const BUFFER_BYTES: usize = 64 * 1024;
/// Malformed-record messages kept per directory; the rest are counted.
const MAX_RECORD_ISSUES: usize = 4;

/// `ATTR_CMN_ERROR` (`<sys/attr.h>`, getattrlistbulk only); not exported by the `libc` crate.
pub(crate) const ATTR_CMN_ERROR: u32 = 0x2000_0000;
/// `enum vtype` values reported by `ATTR_CMN_OBJTYPE`.
const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;

pub(crate) const REQ_COMMON: u32 = libc::ATTR_CMN_RETURNED_ATTRS
    | ATTR_CMN_ERROR
    | libc::ATTR_CMN_NAME
    | libc::ATTR_CMN_DEVID
    | libc::ATTR_CMN_OBJTYPE
    | libc::ATTR_CMN_MODTIME
    | libc::ATTR_CMN_FLAGS
    | libc::ATTR_CMN_FILEID;
pub(crate) const REQ_DIR: u32 = libc::ATTR_DIR_MOUNTSTATUS;
pub(crate) const REQ_FILE: u32 = libc::ATTR_FILE_LINKCOUNT | libc::ATTR_FILE_ALLOCSIZE | libc::ATTR_FILE_DATALENGTH;
/// Extended common attributes, requested in `attrlist.forkattr` with `FSOPT_ATTR_CMN_EXTENDED`
/// and reported in the returned set's `forkattr` (getattrlist(2)). Packed after the file
/// attributes, in bit order: `off_t` private size, `u_int64_t` clone id, `u_int64_t` flags.
pub(crate) const REQ_CMNEXT: u32 = libc::ATTR_CMNEXT_PRIVATESIZE | libc::ATTR_CMNEXT_CLONEID | libc::ATTR_CMNEXT_EXT_FLAGS;
/// `EF_MAY_SHARE_BLOCKS` (`<sys/attr.h>`, an `ATTR_CMNEXT_EXT_FLAGS` bit): the file may share
/// blocks with another file (it is, or was, a clone). Not exported by the `libc` crate.
pub(crate) const EF_MAY_SHARE_BLOCKS: u64 = 0x0000_0001;

/// One parsed `getattrlistbulk` record. Every field is optional: only what the returned
/// bitmap says was packed is read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BulkRecord {
    pub name: Vec<u8>,
    pub error: Option<u32>,
    pub dev: Option<i32>,
    pub objtype: Option<u32>,
    pub modtime: Option<i64>,
    pub flags: Option<u32>,
    pub file_id: Option<u64>,
    pub mount_status: Option<u32>,
    pub link_count: Option<u32>,
    /// `ATTR_FILE_ALLOCSIZE`; `None` when not returned or negative.
    pub alloc_size: Option<u64>,
    /// `ATTR_FILE_DATALENGTH` (data fork, same scope as `st_size`); `None` when not returned or negative.
    pub data_length: Option<u64>,
    /// `ATTR_CMNEXT_PRIVATESIZE`: bytes not shared with a clone or snapshot; `None` when not returned or negative.
    pub private_size: Option<u64>,
    /// `ATTR_CMNEXT_CLONEID`: identifies the data stream (pure clones report the same id). Raw
    /// observation for TC-MAC-005; not part of the v1 rows.
    pub clone_id: Option<u64>,
    /// `ATTR_CMNEXT_EXT_FLAGS` (`EF_*` bits).
    pub ext_flags: Option<u64>,
}

impl BulkRecord {
    /// Whether the object's blocks are known to be shared (see the module documentation).
    pub(crate) fn shared_blocks(&self) -> Option<bool> {
        let held_elsewhere = matches!((self.private_size, self.alloc_size), (Some(p), Some(a)) if p < a);
        match self.ext_flags {
            Some(f) => Some(f & EF_MAY_SHARE_BLOCKS != 0 || held_elsewhere),
            None => held_elsewhere.then_some(true),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct BulkParse {
    pub records: Vec<BulkRecord>,
    /// One message per record that was skipped or per chain break; empty for a clean reply.
    pub issues: Vec<String>,
}

struct Cursor<'a> {
    rec: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    /// Attributes start on 4-byte boundaries.
    fn align(&mut self) -> Result<(), String> {
        self.pos = self.pos.checked_add(3).ok_or("offset overflow")? & !3;
        Ok(())
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        self.align()?;
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.rec.len())
            .ok_or_else(|| format!("attribute at offset {} needs {n} bytes but the record has {}", self.pos, self.rec.len()))?;
        let s = &self.rec[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn arr<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let s = self.take(N)?;
        s.try_into().map_err(|_| "internal length mismatch".to_owned())
    }

    fn u32(&mut self) -> Result<u32, String> {
        self.arr::<4>().map(u32::from_ne_bytes)
    }

    fn i32(&mut self) -> Result<i32, String> {
        self.arr::<4>().map(i32::from_ne_bytes)
    }

    fn i64(&mut self) -> Result<i64, String> {
        self.arr::<8>().map(i64::from_ne_bytes)
    }

    fn u64(&mut self) -> Result<u64, String> {
        self.arr::<8>().map(u64::from_ne_bytes)
    }
}

/// Parse one record (`rec` starts at its `u32` length field and ends at its last byte).
fn parse_one(rec: &[u8]) -> Result<BulkRecord, String> {
    let mut c = Cursor { rec, pos: 4 };
    let (common, vol, dir, file, fork) = (c.u32()?, c.u32()?, c.u32()?, c.u32()?, c.u32()?);
    // With FSOPT_ATTR_CMN_EXTENDED the `forkattr` slot carries the extended common attributes.
    let cmnext = fork;
    if common & !REQ_COMMON != 0 || vol != 0 || dir & !REQ_DIR != 0 || file & !REQ_FILE != 0 || cmnext & !REQ_CMNEXT != 0 {
        return Err(format!(
            "unexpected returned attributes common={common:#x} vol={vol:#x} dir={dir:#x} file={file:#x} cmnext={cmnext:#x}"
        ));
    }
    let mut r = BulkRecord::default();
    // ATTR_CMN_ERROR is packed immediately after ATTR_CMN_RETURNED_ATTRS (getattrlist(2)).
    if common & ATTR_CMN_ERROR != 0 {
        r.error = Some(c.u32()?);
    }
    let mut name = None;
    if common & libc::ATTR_CMN_NAME != 0 {
        c.align()?;
        let base = c.pos;
        let (offset, len) = (c.i32()?, c.u32()? as usize);
        // attrreference_t.attr_dataoffset is relative to the attrreference_t itself.
        let start = usize::try_from(i64::try_from(base).map_err(|_| "offset overflow")? + i64::from(offset))
            .map_err(|_| format!("name offset {offset} points before the record"))?;
        let data = start
            .checked_add(len)
            .filter(|&end| len > 0 && end <= rec.len())
            .map(|end| &rec[start..end])
            .ok_or_else(|| format!("name reference ({offset}, {len}) is outside the {}-byte record", rec.len()))?;
        let nul = data.iter().position(|&b| b == 0).ok_or("name is not NUL-terminated within its length")?;
        if nul == 0 {
            return Err("empty name".into());
        }
        name = Some(data[..nul].to_vec());
    }
    if common & libc::ATTR_CMN_DEVID != 0 {
        r.dev = Some(c.i32()?);
    }
    if common & libc::ATTR_CMN_OBJTYPE != 0 {
        r.objtype = Some(c.u32()?);
    }
    if common & libc::ATTR_CMN_MODTIME != 0 {
        // struct timespec { time_t tv_sec; long tv_nsec; }: 16 bytes on 64-bit macOS.
        let secs = c.i64()?;
        let _nanos = c.i64()?;
        r.modtime = Some(secs);
    }
    if common & libc::ATTR_CMN_FLAGS != 0 {
        r.flags = Some(c.u32()?);
    }
    if common & libc::ATTR_CMN_FILEID != 0 {
        r.file_id = Some(c.u64()?);
    }
    if dir & libc::ATTR_DIR_MOUNTSTATUS != 0 {
        r.mount_status = Some(c.u32()?);
    }
    if file & libc::ATTR_FILE_LINKCOUNT != 0 {
        r.link_count = Some(c.u32()?);
    }
    if file & libc::ATTR_FILE_ALLOCSIZE != 0 {
        r.alloc_size = u64::try_from(c.i64()?).ok();
    }
    if file & libc::ATTR_FILE_DATALENGTH != 0 {
        r.data_length = u64::try_from(c.i64()?).ok();
    }
    // Extended common attributes follow the file attributes (vfs_attr_pack_internal).
    if cmnext & libc::ATTR_CMNEXT_PRIVATESIZE != 0 {
        r.private_size = u64::try_from(c.i64()?).ok();
    }
    if cmnext & libc::ATTR_CMNEXT_CLONEID != 0 {
        r.clone_id = Some(c.u64()?);
    }
    if cmnext & libc::ATTR_CMNEXT_EXT_FLAGS != 0 {
        r.ext_flags = Some(c.u64()?);
    }
    r.name = name.ok_or("record has no name")?;
    Ok(r)
}

/// Parse the `count` records `getattrlistbulk` reported into `buf`. Never panics and never
/// reads outside `buf`: a record whose fields do not fit is skipped with an issue; a length
/// that breaks the chain (too small or past the buffer) stops parsing with an issue.
pub(crate) fn parse_bulk_records(buf: &[u8], count: usize) -> BulkParse {
    let mut out = BulkParse::default();
    let mut off = 0usize;
    for i in 0..count {
        let Some(len) = off.checked_add(4).and_then(|e| buf.get(off..e)).map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]) as usize)
        else {
            out.issues.push(format!("record {i} of {count}: header at offset {off} is outside the {}-byte buffer", buf.len()));
            break;
        };
        if len < 4 {
            out.issues.push(format!("record {i} of {count}: length {len} cannot advance; remaining records unread"));
            break;
        }
        let Some(end) = off.checked_add(len).filter(|&e| e <= buf.len()) else {
            out.issues.push(format!("record {i} of {count}: length {len} at offset {off} runs past the {}-byte buffer", buf.len()));
            break;
        };
        match parse_one(&buf[off..end]) {
            Ok(r) => out.records.push(r),
            Err(msg) => out.issues.push(format!("record {i} of {count} skipped: {msg}")),
        }
        off = end;
    }
    out
}

fn kind_of(objtype: u32) -> Kind {
    match objtype {
        VREG => Kind::File,
        VDIR => Kind::Directory,
        VLNK => Kind::Symlink,
        _ => Kind::Other,
    }
}

/// Turn a parsed record into the shared row input, completing missing attributes with a
/// no-follow `fstatat` relative to the listed directory.
fn to_record(dir: &Fd, b: BulkRecord, supplemented: &mut bool) -> Option<Record> {
    if b.name == b"." || b.name == b".." {
        return None;
    }
    if let Some(e) = b.error.filter(|&e| e != 0) {
        let e = i32::try_from(e).unwrap_or(libc::EIO);
        return Some(Record { name: b.name, meta: Err(e) });
    }
    let mount = b.mount_status.is_some_and(|m| m & libc::DIR_MNTSTATUS_MNTPOINT != 0);
    let shared_blocks = b.shared_blocks();
    if shared_blocks == Some(true) {
        // Raw evidence for TC-MAC-005 under `--debug`; bounded by the number of shared files.
        cm_core::debug_trace!(
            "scan.bulk",
            "shared blocks name=\"{}\" alloc={:?} private={:?} clone_id={:?} ext_flags={:?}",
            String::from_utf8_lossy(&b.name),
            b.alloc_size,
            b.private_size,
            b.clone_id,
            b.ext_flags
        );
    }
    let complete = match (b.objtype.map(kind_of), b.dev, b.file_id, b.flags) {
        (Some(kind), Some(dev), Some(ino), Some(flags)) => {
            let enough = match kind {
                Kind::File => b.data_length.is_some() && b.alloc_size.is_some() && b.link_count.is_some(),
                // Without the mount status a covered directory could not be told apart.
                Kind::Directory => b.mount_status.is_some(),
                Kind::Symlink | Kind::Other => true,
            };
            enough.then_some(Meta {
                kind,
                dev,
                ino,
                size: b.data_length,
                alloc: b.alloc_size,
                nlink: b.link_count,
                mtime: b.modtime,
                flags,
                mount,
                // Only files carry sizes; sharing is not reported for directories or links.
                shared_blocks: if kind == Kind::File { shared_blocks } else { None },
            })
        }
        _ => None,
    };
    let meta = match complete {
        Some(m) => Ok(m),
        None => {
            *supplemented = true;
            dir.stat_child(&b.name).map(|st| Meta { mount, ..Meta::from_stat(&st) })
        }
    };
    Some(Record { name: b.name, meta })
}

enum Outcome {
    Records(Vec<BulkRecord>),
    Unsupported,
    Malformed(Vec<String>),
}

pub(crate) struct BulkLister {
    buf: Vec<u8>,
    /// Ask for the extended common attributes; cleared for the rest of the scan once the
    /// kernel refuses them (`EINVAL` on an older system or file system).
    extended: bool,
}

impl BulkLister {
    pub(crate) fn new() -> Self {
        BulkLister { buf: vec![0u8; BUFFER_BYTES], extended: true }
    }

    fn read(&mut self, dir: &Fd) -> Result<Outcome, i32> {
        match self.read_once(dir) {
            Err(libc::EINVAL) if self.extended => {
                self.extended = false;
                self.read_once(dir)
            }
            other => other,
        }
    }

    fn read_once(&mut self, dir: &Fd) -> Result<Outcome, i32> {
        // A fresh descriptor: the bulk cursor starts at the first entry.
        let fd = dir.reopen_self()?;
        let mut list = libc::attrlist {
            bitmapcount: libc::ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: REQ_COMMON,
            volattr: 0,
            dirattr: REQ_DIR,
            fileattr: REQ_FILE,
            forkattr: if self.extended { REQ_CMNEXT } else { 0 },
        };
        let options = if self.extended { u64::from(libc::FSOPT_ATTR_CMN_EXTENDED) } else { 0 };
        let mut out = Vec::new();
        loop {
            // SAFETY: valid directory fd, attrlist and writable buffer of the given size.
            // No FSOPT_PACK_INVAL_ATTRS: only returned attributes are packed.
            let n = unsafe {
                libc::getattrlistbulk(fd.raw(), (&mut list as *mut libc::attrlist).cast(), self.buf.as_mut_ptr().cast(), self.buf.len(), options)
            };
            if n < 0 {
                return match errno() {
                    libc::ENOTSUP => Ok(Outcome::Unsupported),
                    e => Err(e),
                };
            }
            if n == 0 {
                return Ok(Outcome::Records(out));
            }
            let parsed = parse_bulk_records(&self.buf, n as usize);
            if !parsed.issues.is_empty() {
                return Ok(Outcome::Malformed(parsed.issues));
            }
            out.extend(parsed.records);
        }
    }
}

impl Lister for BulkLister {
    fn list(&mut self, dir: &Fd) -> Listing {
        let mut listing = self.list_inner(dir);
        if !self.extended {
            listing.notes.push(NOTE_NO_EXTENDED);
        }
        listing
    }
}

impl BulkLister {
    fn list_inner(&mut self, dir: &Fd) -> Listing {
        match self.read(dir) {
            Err(e) => Listing::plain(Err(e)),
            Ok(Outcome::Unsupported) => Listing { records: list_posix(dir), issues: Vec::new(), notes: vec![NOTE_UNSUPPORTED] },
            Ok(Outcome::Malformed(problems)) => {
                let total = problems.len();
                let mut issues: Vec<(&'static str, String)> =
                    problems.into_iter().take(MAX_RECORD_ISSUES).map(|p| ("bulk_record_malformed", p)).collect();
                if total > MAX_RECORD_ISSUES {
                    issues.push(("bulk_record_malformed", format!("{} more malformed records", total - MAX_RECORD_ISSUES)));
                }
                Listing { records: list_posix(dir), issues, notes: vec![NOTE_MALFORMED] }
            }
            Ok(Outcome::Records(recs)) => {
                let mut supplemented = false;
                let records = recs.into_iter().filter_map(|b| to_record(dir, b, &mut supplemented)).collect();
                Listing { records: Ok(records), issues: Vec::new(), notes: if supplemented { vec![NOTE_SUPPLEMENTED] } else { Vec::new() } }
            }
        }
    }
}

/// The reclaimable-space estimate of one scanned macOS object and its protocol
/// `estimate_basis` (docs/platforms/macos.md §0/§3, SCN-06).
///
/// Always `None` in this release: an unknown logical size gives `not_computed`, anything else
/// `shared_blocks_unknown`, whatever `shared_blocks` says. A clone (`Some(true)`) cannot be
/// estimated from public metadata, and even `Some(false)` with a matching private size is not
/// exclusive-ownership evidence until TC-MAC-005 has run on a real Mac (hard links, snapshots
/// and the exact private-size definition are unverified there).
pub fn reclaimable_estimate(object: &ObjectRow) -> (Option<u64>, &'static str) {
    if object.logical.is_none() { (None, "not_computed") } else { (None, "shared_blocks_unknown") }
}

pub struct BulkBackend;

impl ScanBackend for BulkBackend {
    fn name(&self) -> &'static str {
        "mac.bulk"
    }

    fn probe(&self, root: &RawPath) -> RootProbe {
        walk::probe(self.name(), root, "getattrlistbulk enumeration with per-directory readdir + fstatat fallback")
    }

    fn scan(&self, root: &RawPath, sink: &mut dyn ScanSink, options: &ScanOptions) -> ScanReport {
        walk::scan(self.name(), METHOD, root, sink, options, &mut BulkLister::new())
    }
}

#[cfg(test)]
mod tests;
