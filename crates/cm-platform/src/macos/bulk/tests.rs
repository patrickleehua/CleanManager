//! Parser tests on synthetic `getattrlistbulk` replies: no file system is touched.

use super::*;

/// Fields of one synthetic record; `None` means the attribute is absent from the returned set.
#[derive(Clone, Default)]
struct Rec {
    error: Option<u32>,
    name: Option<Vec<u8>>,
    dev: Option<i32>,
    objtype: Option<u32>,
    mtime: Option<i64>,
    flags: Option<u32>,
    file_id: Option<u64>,
    mount: Option<u32>,
    /// (link count, ATTR_FILE_ALLOCSIZE, ATTR_FILE_DATALENGTH)
    file: Option<(u32, i64, i64)>,
    /// Extra bits forced into the returned common set (not packed).
    extra_common: u32,
}

fn file(name: &[u8], id: u64, size: i64) -> Rec {
    Rec {
        name: Some(name.to_vec()),
        dev: Some(16_777_220),
        objtype: Some(VREG),
        mtime: Some(1_760_000_000),
        flags: Some(0),
        file_id: Some(id),
        file: Some((1, 4096, size)),
        ..Rec::default()
    }
}

fn dir(name: &[u8], id: u64, mount: u32) -> Rec {
    Rec {
        name: Some(name.to_vec()),
        dev: Some(16_777_220),
        objtype: Some(VDIR),
        mtime: Some(1_760_000_001),
        flags: Some(0),
        file_id: Some(id),
        mount: Some(mount),
        ..Rec::default()
    }
}

/// Encode as the kernel packs it: length, returned set, ERROR, fixed attributes in bit order
/// (name as an attrreference_t), then the NUL-terminated name padded to 4 bytes.
fn encode(r: &Rec) -> Vec<u8> {
    let (mut common, mut dir_bits, mut file_bits) = (libc::ATTR_CMN_RETURNED_ATTRS | r.extra_common, 0u32, 0u32);
    let mut fixed: Vec<u8> = Vec::new();
    let mut name_ref = None;
    if let Some(e) = r.error {
        common |= ATTR_CMN_ERROR;
        fixed.extend_from_slice(&e.to_ne_bytes());
    }
    if r.name.is_some() {
        common |= libc::ATTR_CMN_NAME;
        name_ref = Some(fixed.len());
        fixed.extend_from_slice(&[0u8; 8]);
    }
    if let Some(d) = r.dev {
        common |= libc::ATTR_CMN_DEVID;
        fixed.extend_from_slice(&d.to_ne_bytes());
    }
    if let Some(t) = r.objtype {
        common |= libc::ATTR_CMN_OBJTYPE;
        fixed.extend_from_slice(&t.to_ne_bytes());
    }
    if let Some(m) = r.mtime {
        common |= libc::ATTR_CMN_MODTIME;
        fixed.extend_from_slice(&m.to_ne_bytes());
        fixed.extend_from_slice(&123_456_789i64.to_ne_bytes());
    }
    if let Some(f) = r.flags {
        common |= libc::ATTR_CMN_FLAGS;
        fixed.extend_from_slice(&f.to_ne_bytes());
    }
    if let Some(id) = r.file_id {
        common |= libc::ATTR_CMN_FILEID;
        fixed.extend_from_slice(&id.to_ne_bytes());
    }
    if let Some(m) = r.mount {
        dir_bits |= libc::ATTR_DIR_MOUNTSTATUS;
        fixed.extend_from_slice(&m.to_ne_bytes());
    }
    if let Some((links, alloc, len)) = r.file {
        file_bits |= REQ_FILE;
        fixed.extend_from_slice(&links.to_ne_bytes());
        fixed.extend_from_slice(&alloc.to_ne_bytes());
        fixed.extend_from_slice(&len.to_ne_bytes());
    }
    let mut out = vec![0u8; 4];
    for v in [common, 0, dir_bits, file_bits, 0] {
        out.extend_from_slice(&v.to_ne_bytes());
    }
    let fixed_start = out.len();
    out.extend_from_slice(&fixed);
    if let (Some(pos), Some(name)) = (name_ref, &r.name) {
        let ref_at = fixed_start + pos;
        let data_at = out.len();
        out.extend_from_slice(name);
        out.push(0);
        while out.len() % 4 != 0 {
            out.push(0);
        }
        out[ref_at..ref_at + 4].copy_from_slice(&((data_at - ref_at) as i32).to_ne_bytes());
        out[ref_at + 4..ref_at + 8].copy_from_slice(&((name.len() + 1) as u32).to_ne_bytes());
    }
    let len = out.len() as u32;
    out[0..4].copy_from_slice(&len.to_ne_bytes());
    out
}

fn reply(recs: &[Rec]) -> Vec<u8> {
    let mut buf: Vec<u8> = recs.iter().flat_map(encode).collect();
    buf.resize(buf.len() + 64, 0); // the kernel buffer is larger than what was filled
    buf
}

#[test]
fn well_formed_file_and_directory_records_parse_exactly() {
    let name = "中文 文件.txt".as_bytes();
    let buf = reply(&[file(name, 0x1234, 2048), dir(b"sub", 0x99, 0), dir(b"mnt", 0x77, libc::DIR_MNTSTATUS_MNTPOINT)]);
    let p = parse_bulk_records(&buf, 3);
    assert!(p.issues.is_empty(), "{:?}", p.issues);
    assert_eq!(p.records.len(), 3);
    let f = &p.records[0];
    assert_eq!(f.name, name, "raw name bytes, not normalized");
    assert_eq!((f.dev, f.objtype, f.modtime, f.flags), (Some(16_777_220), Some(VREG), Some(1_760_000_000), Some(0)));
    assert_eq!((f.file_id, f.link_count, f.alloc_size, f.data_length), (Some(0x1234), Some(1), Some(4096), Some(2048)));
    assert_eq!(f.mount_status, None);
    assert_eq!((p.records[1].objtype, p.records[1].mount_status), (Some(VDIR), Some(0)));
    assert_eq!(p.records[2].mount_status, Some(libc::DIR_MNTSTATUS_MNTPOINT));
}

#[test]
fn invalid_utf8_and_dataless_flags_survive() {
    let mut r = file(b"\xff\xfe-bad", 7, 10);
    r.flags = Some(crate::macos::sys::SF_DATALESS);
    let p = parse_bulk_records(&reply(&[r]), 1);
    assert!(p.issues.is_empty());
    assert_eq!(p.records[0].name, b"\xff\xfe-bad");
    assert_eq!(p.records[0].flags, Some(0x4000_0000));
}

#[test]
fn per_entry_error_is_kept_not_zeroed() {
    let r = Rec { error: Some(libc::EACCES as u32), name: Some(b"locked".to_vec()), ..Rec::default() };
    let p = parse_bulk_records(&reply(&[r, file(b"next", 2, 5)]), 2);
    assert!(p.issues.is_empty(), "{:?}", p.issues);
    assert_eq!(p.records[0].error, Some(libc::EACCES as u32));
    assert_eq!((p.records[0].data_length, p.records[0].alloc_size, p.records[0].file_id), (None, None, None));
    assert_eq!(p.records[1].name, b"next", "an error record does not desynchronize the chain");
}

#[test]
fn missing_attributes_shift_the_layout_and_stay_unknown() {
    let mut r = file(b"partial", 0, 99);
    r.file_id = None;
    r.mtime = None;
    let p = parse_bulk_records(&reply(&[r]), 1);
    assert!(p.issues.is_empty(), "{:?}", p.issues);
    let rec = &p.records[0];
    assert_eq!((rec.file_id, rec.modtime), (None, None));
    assert_eq!((rec.flags, rec.link_count, rec.data_length), (Some(0), Some(1), Some(99)), "later fields read at shifted offsets");
}

#[test]
fn negative_sizes_become_unknown() {
    let mut r = file(b"neg", 3, -1);
    r.file = Some((1, -4096, -1));
    let rec = &parse_bulk_records(&reply(&[r]), 1).records[0];
    assert_eq!((rec.alloc_size, rec.data_length), (None, None));
}

#[test]
fn record_length_past_the_buffer_stops_without_reading_it() {
    let mut buf = encode(&file(b"a", 1, 1));
    let huge = (buf.len() as u32) + 1000;
    buf[0..4].copy_from_slice(&huge.to_ne_bytes());
    let p = parse_bulk_records(&buf, 1);
    assert!(p.records.is_empty());
    assert_eq!(p.issues.len(), 1);
    assert!(p.issues[0].contains("runs past"), "{}", p.issues[0]);
}

#[test]
fn zero_or_tiny_length_breaks_the_chain_safely() {
    for len in [0u32, 1, 3] {
        let mut buf = encode(&file(b"a", 1, 1));
        buf[0..4].copy_from_slice(&len.to_ne_bytes());
        let p = parse_bulk_records(&buf, 5);
        assert!(p.records.is_empty());
        assert_eq!(p.issues.len(), 1, "stops at the first unadvanceable record");
    }
}

#[test]
fn short_record_is_skipped_and_the_next_one_still_parses() {
    let mut first = encode(&file(b"short", 1, 1));
    // Claim only the header + returned set: the fixed fields no longer fit in the record.
    first.truncate(24);
    first[0..4].copy_from_slice(&24u32.to_ne_bytes());
    let mut buf = first;
    buf.extend(encode(&file(b"ok", 2, 2)));
    let p = parse_bulk_records(&buf, 2);
    assert_eq!(p.issues.len(), 1, "{:?}", p.issues);
    assert!(p.issues[0].contains("record 0"));
    assert_eq!(p.records.len(), 1);
    assert_eq!(p.records[0].name, b"ok");
}

#[test]
fn name_reference_outside_the_record_is_rejected() {
    for bad_offset in [i32::MIN, -100, 10_000, i32::MAX] {
        let mut buf = encode(&file(b"name", 1, 1));
        // attrreference_t sits right after the 24-byte header (no ERROR attribute).
        buf[24..28].copy_from_slice(&bad_offset.to_ne_bytes());
        let p = parse_bulk_records(&buf, 1);
        assert!(p.records.is_empty());
        assert!(p.issues[0].contains("name"), "{}", p.issues[0]);
    }
    let mut buf = encode(&file(b"name", 1, 1));
    buf[28..32].copy_from_slice(&u32::MAX.to_ne_bytes());
    assert!(parse_bulk_records(&buf, 1).records.is_empty(), "oversized name length");
    let mut buf = encode(&file(b"name", 1, 1));
    buf[28..32].copy_from_slice(&0u32.to_ne_bytes());
    assert!(parse_bulk_records(&buf, 1).records.is_empty(), "zero name length");
}

#[test]
fn name_without_terminator_or_empty_is_rejected() {
    let mut buf = encode(&file(b"abcd", 1, 1));
    buf[28..32].copy_from_slice(&4u32.to_ne_bytes()); // excludes the NUL
    assert!(parse_bulk_records(&buf, 1).issues[0].contains("NUL"));
    let p = parse_bulk_records(&encode(&file(b"", 1, 1)), 1);
    assert!(p.records.is_empty() && p.issues[0].contains("empty"));
}

#[test]
fn missing_name_or_unexpected_attributes_skip_the_record() {
    let mut r = file(b"x", 1, 1);
    r.name = None;
    assert!(parse_bulk_records(&encode(&r), 1).issues[0].contains("no name"));
    let mut r = file(b"x", 1, 1);
    r.extra_common = libc::ATTR_CMN_CRTIME; // size unknown to the parser: cannot be skipped over
    let p = parse_bulk_records(&encode(&r), 1);
    assert!(p.records.is_empty() && p.issues[0].contains("unexpected"));
}

#[test]
fn count_larger_than_the_records_present_stops_at_the_padding() {
    let buf = reply(&[file(b"only", 1, 1)]);
    let p = parse_bulk_records(&buf, 3);
    assert_eq!(p.records.len(), 1);
    assert_eq!(p.issues.len(), 1);
    let p = parse_bulk_records(&buf[..buf.len() - 64], 2);
    assert_eq!((p.records.len(), p.issues.len()), (1, 1), "header beyond the buffer");
}

/// Deterministic SplitMix64 for the robustness sweep.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[test]
fn truncations_corruptions_and_noise_never_panic() {
    let good = reply(&[
        file("中文".as_bytes(), 1, 10),
        Rec { error: Some(1), name: Some(b"e".to_vec()), ..Rec::default() },
        dir(b"d", 2, 0),
    ]);
    for cut in 0..=good.len() {
        let _ = parse_bulk_records(&good[..cut], 3);
    }
    let mut state = 42u64;
    for _ in 0..2_000 {
        let mut b = good.clone();
        for _ in 0..1 + next(&mut state) % 4 {
            let i = (next(&mut state) as usize) % b.len();
            b[i] = next(&mut state) as u8;
        }
        let p = parse_bulk_records(&b, 3);
        assert!(p.records.len() <= 3);
    }
    for _ in 0..500 {
        let len = (next(&mut state) % 512) as usize;
        let noise: Vec<u8> = (0..len).map(|_| next(&mut state) as u8).collect();
        let _ = parse_bulk_records(&noise, (next(&mut state) % 64) as usize);
    }
}
