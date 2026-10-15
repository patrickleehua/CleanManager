//! Read-only Git index reader for the purge version-control check (PUR-03). No process is run
//! and nothing is written: the caller supplies the bytes of `<gitdir>/index`. Anything this
//! reader cannot fully account for (split index, truncation, unknown versions) is an error,
//! which the caller treats as "tracking unknown", never as "untracked".

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GitIndexError {
    #[error("index is truncated")]
    Truncated,
    #[error("not a Git index (bad signature)")]
    BadSignature,
    #[error("unsupported index version {0}")]
    UnsupportedVersion(u32),
    #[error("index extension {0} is not supported (entries may live elsewhere)")]
    UnsupportedExtension(String),
    #[error("malformed path compression")]
    BadPath,
}

/// Object-id length: SHA-1 repositories use 20 bytes, `objectFormat = sha256` uses 32.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashKind {
    Sha1,
    Sha256,
}

impl HashKind {
    fn len(self) -> usize {
        match self {
            HashKind::Sha1 => 20,
            HashKind::Sha256 => 32,
        }
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], GitIndexError> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.b.len()).ok_or(GitIndexError::Truncated)?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u16(&mut self) -> Result<u16, GitIndexError> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Result<u32, GitIndexError> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    /// Git's offset varint (index v4 path prefix lengths).
    fn varint(&mut self) -> Result<usize, GitIndexError> {
        let mut c = self.take(1)?[0];
        let mut v = usize::from(c & 0x7f);
        while c & 0x80 != 0 {
            c = self.take(1)?[0];
            v = v.checked_add(1).and_then(|v| v.checked_mul(128)).ok_or(GitIndexError::BadPath)? + usize::from(c & 0x7f);
        }
        Ok(v)
    }
    fn until_nul(&mut self) -> Result<&'a [u8], GitIndexError> {
        let rest = &self.b[self.at..];
        let n = rest.iter().position(|&c| c == 0).ok_or(GitIndexError::Truncated)?;
        self.at += n + 1;
        Ok(&rest[..n])
    }
}

fn under(path: &[u8], prefix: &[u8], ignore_case: bool) -> bool {
    if prefix.is_empty() {
        return true;
    }
    if path.len() < prefix.len() {
        return false;
    }
    let head = &path[..prefix.len()];
    let same = if ignore_case { head.eq_ignore_ascii_case(prefix) } else { head == prefix };
    same && (path.len() == prefix.len() || path[prefix.len()] == b'/')
}

/// Number of index entries at or below `prefix` (worktree-relative, `/`-separated, no trailing
/// slash). Sparse-directory entries count as tracked content.
pub fn tracked_under(index: &[u8], prefix: &str, hash: HashKind, ignore_case: bool) -> Result<u64, GitIndexError> {
    let mut r = Reader { b: index, at: 0 };
    if r.take(4)? != b"DIRC" {
        return Err(GitIndexError::BadSignature);
    }
    let version = r.u32()?;
    if !(2..=4).contains(&version) {
        return Err(GitIndexError::UnsupportedVersion(version));
    }
    let entries = r.u32()?;
    let prefix = prefix.trim_end_matches('/').as_bytes();
    let fixed = 40 + hash.len() + 2;
    let mut count = 0u64;
    let mut previous: Vec<u8> = Vec::new();
    for _ in 0..entries {
        let start = r.at;
        r.take(40 + hash.len())?;
        let flags = r.u16()?;
        let mut header = fixed;
        if version >= 3 && flags & 0x4000 != 0 {
            r.u16()?;
            header += 2;
        }
        let path: Vec<u8> = if version == 4 {
            let strip = r.varint()?;
            let keep = previous.len().checked_sub(strip).ok_or(GitIndexError::BadPath)?;
            let mut p = previous[..keep].to_vec();
            p.extend_from_slice(r.until_nul()?);
            p
        } else {
            let name = r.until_nul()?.to_vec();
            // Entries are NUL-padded to a multiple of eight bytes (1..=8 NULs after the name).
            let len = (header + name.len() + 8) & !7;
            r.at = start.checked_add(len).filter(|e| *e <= index.len()).ok_or(GitIndexError::Truncated)?;
            name
        };
        if under(&path, prefix, ignore_case) {
            count += 1;
        }
        previous = path;
    }
    // Extensions: a lower-case signature is required to read the index correctly ("link" = split
    // index, whose entries live in a shared file). Optional ones are skipped.
    while r.b.len().saturating_sub(r.at) > hash.len() {
        let sig = r.take(4)?;
        let size = r.u32()? as usize;
        if sig[0].is_ascii_lowercase() {
            return Err(GitIndexError::UnsupportedExtension(String::from_utf8_lossy(sig).into_owned()));
        }
        r.take(size)?;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an index with `paths` (already sorted), version 2/3 or 4.
    pub(crate) fn index(version: u32, paths: &[&str], ext: Option<&[u8; 4]>) -> Vec<u8> {
        let mut b = b"DIRC".to_vec();
        b.extend_from_slice(&version.to_be_bytes());
        b.extend_from_slice(&(paths.len() as u32).to_be_bytes());
        let mut prev = String::new();
        for p in paths {
            let start = b.len();
            b.extend_from_slice(&[0u8; 40]);
            b.extend_from_slice(&[0xab; 20]);
            let flags = (p.len().min(0xfff) as u16) | if version == 3 { 0x4000 } else { 0 };
            b.extend_from_slice(&flags.to_be_bytes());
            if version == 3 {
                b.extend_from_slice(&0u16.to_be_bytes());
            }
            if version == 4 {
                let common = prev.bytes().zip(p.bytes()).take_while(|(a, c)| a == c).count();
                let strip = prev.len() - common;
                assert!(strip < 128, "test helper encodes one-byte varints");
                b.push(strip as u8);
                b.extend_from_slice(&p.as_bytes()[common..]);
                b.push(0);
            } else {
                b.extend_from_slice(p.as_bytes());
                let len = (b.len() - start + 8) & !7;
                b.resize(start + len, 0);
            }
            prev = (*p).to_owned();
        }
        if let Some(sig) = ext {
            b.extend_from_slice(sig);
            b.extend_from_slice(&3u32.to_be_bytes());
            b.extend_from_slice(b"xyz");
        }
        b.extend_from_slice(&[0u8; 20]); // trailing checksum (not verified)
        b
    }

    #[test]
    fn counts_entries_under_a_prefix_in_every_version() {
        let paths = ["README.md", "dist/app.js", "dist/sub/x.css", "dist2/y", "src/main.rs"];
        for v in [2, 3, 4] {
            let b = index(v, &paths, None);
            assert_eq!(tracked_under(&b, "dist", HashKind::Sha1, false), Ok(2), "v{v}");
            assert_eq!(tracked_under(&b, "DIST", HashKind::Sha1, true), Ok(2), "v{v}");
            assert_eq!(tracked_under(&b, "DIST", HashKind::Sha1, false), Ok(0), "v{v}");
            assert_eq!(tracked_under(&b, "node_modules", HashKind::Sha1, true), Ok(0), "v{v}");
            assert_eq!(tracked_under(&b, "dist/sub/", HashKind::Sha1, true), Ok(1), "v{v}");
        }
    }

    #[test]
    fn unreadable_indexes_are_errors_never_untracked() {
        let b = index(2, &["dist/a"], None);
        assert_eq!(tracked_under(&b[..30], "dist", HashKind::Sha1, true), Err(GitIndexError::Truncated));
        assert_eq!(tracked_under(b"DIRX\0\0\0\x02\0\0\0\0", "dist", HashKind::Sha1, true), Err(GitIndexError::BadSignature));
        let mut v9 = b.clone();
        v9[7] = 9;
        assert_eq!(tracked_under(&v9, "dist", HashKind::Sha1, true), Err(GitIndexError::UnsupportedVersion(9)));
        let split = index(2, &["dist/a"], Some(b"link"));
        assert!(matches!(tracked_under(&split, "x", HashKind::Sha1, true), Err(GitIndexError::UnsupportedExtension(_))));
        let optional = index(2, &["dist/a"], Some(b"TREE"));
        assert_eq!(tracked_under(&optional, "dist", HashKind::Sha1, true), Ok(1));
    }
}
