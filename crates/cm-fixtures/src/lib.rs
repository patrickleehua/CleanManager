//! Reproducible fixture trees for scanner and executor tests.
//!
//! Fixture preparation and destruction are separate steps from any scan; their timings are
//! recorded in the manifest's `generation` block and never count as scan measurements.
//! Every tree lives under a marker-guarded root; out-of-scope sentinel directories detect
//! boundary violations (a link inside the fixture points at the sentinel).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const GENERATOR: &str = concat!("cm-fixtures ", env!("CARGO_PKG_VERSION"));
pub const ROOT_MARKER: &str = ".cm-fixture-root.json";
pub const SENTINEL_MARKER: &str = ".cm-sentinel.json";
/// Entry lists above this size are summarized by digest only.
pub const INLINE_ENTRY_LIMIT: u64 = 1_000;
const MAX_FILE_BYTES: u64 = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profile {
    Tiny,
    Wide,
    Deep,
    P0_10k,
    P0_100k,
    P0_1m,
}

impl Profile {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "tiny" => Profile::Tiny,
            "wide" => Profile::Wide,
            "deep" => Profile::Deep,
            "p0-10k" => Profile::P0_10k,
            "p0-100k" => Profile::P0_100k,
            "p0-1m" => Profile::P0_1m,
            _ => return None,
        })
    }

    /// (directories, files) for the bulk profiles defined in docs/verification-plan.md §4.1.
    fn shape(self) -> (u64, u64) {
        match self {
            Profile::Tiny => (0, 0),
            Profile::Wide => (1, 5_000),
            Profile::Deep => (64, 64),
            Profile::P0_10k => (100, 10_000),
            Profile::P0_100k => (1_000, 100_000),
            Profile::P0_1m => (10_000, 1_000_000),
        }
    }
}

/// SplitMix64: tiny, dependency-free and stable across platforms and releases.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Entry {
    Dir { path: String },
    File { path: String, size: u64 },
    HardLink { path: String, target: String },
    /// Link to an object inside the out-of-scope sentinel (relative name within the sentinel).
    SentinelLink { path: String, sentinel_target: String, dir: bool },
}

impl Entry {
    fn line(&self) -> String {
        match self {
            Entry::Dir { path } => format!("D\t{path}\n"),
            Entry::File { path, size } => format!("F\t{path}\t{size}\n"),
            Entry::HardLink { path, target } => format!("H\t{path}\t{target}\n"),
            Entry::SentinelLink { path, sentinel_target, dir } => format!("S\t{path}\t{sentinel_target}\t{dir}\n"),
        }
    }
}

/// Names exercising Chinese text, spaces and long components (verification-plan P0-100K).
fn name_variant(rng: &mut Rng, index: u64, mixed: bool) -> String {
    if !mixed {
        return format!("f{index:07}.dat");
    }
    match rng.next_u64() % 6 {
        0 => format!("文件 {index}.txt"),
        1 => format!("space name {index}.log"),
        2 => format!("{}-{index}.bin", "long".repeat(30)),
        _ => format!("f{index:07}.dat"),
    }
}

/// Deterministic entry sequence for a profile and seed. Parents always precede children.
pub fn entries(profile: Profile, seed: u64, mut visit: impl FnMut(&Entry)) {
    let mut rng = Rng::new(seed);
    let size = |rng: &mut Rng| rng.next_u64() % (MAX_FILE_BYTES + 1);
    match profile {
        Profile::Tiny => {
            let fixed = [
                Entry::Dir { path: "a".into() },
                Entry::Dir { path: "a/子目录 with space".into() },
                Entry::Dir { path: "b".into() },
                Entry::File { path: "zero.bin".into(), size: 0 },
                Entry::File { path: "a/中文 文件.txt".into(), size: 1_234 },
                Entry::File { path: "a/子目录 with space/space name.log".into(), size: 4_096 },
                Entry::File { path: format!("b/{}.dat", "x".repeat(180)), size: 17 },
                Entry::File { path: "b/original.dat".into(), size: 2_048 },
                Entry::HardLink { path: "a/hardlink-to-original.dat".into(), target: "b/original.dat".into() },
                Entry::SentinelLink { path: "b/link-to-sentinel-file".into(), sentinel_target: "sentinel.txt".into(), dir: false },
                Entry::SentinelLink { path: "b/link-to-sentinel-dir".into(), sentinel_target: "sentinel-dir".into(), dir: true },
            ];
            for e in &fixed {
                visit(e);
            }
        }
        Profile::Deep => {
            let mut path = String::new();
            for depth in 0..profile.shape().0 {
                if !path.is_empty() {
                    path.push('/');
                }
                path.push_str(&format!("d{depth:02}"));
                visit(&Entry::Dir { path: path.clone() });
                visit(&Entry::File { path: format!("{path}/f.dat"), size: size(&mut rng) });
            }
        }
        _ => {
            let (dirs, files) = profile.shape();
            let mixed = matches!(profile, Profile::P0_100k);
            for d in 0..dirs {
                visit(&Entry::Dir { path: format!("d{d:05}") });
            }
            // Fixed round-robin distribution keeps per-directory counts identical across runs.
            for f in 0..files {
                let dir = f % dirs;
                let name = name_variant(&mut rng, f, mixed);
                visit(&Entry::File { path: format!("d{dir:05}/{name}"), size: size(&mut rng) });
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub directories: u64,
    pub files: u64,
    pub hardlinks: u64,
    pub sentinel_links: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub profile: Profile,
    pub seed: u64,
    pub counts: Counts,
    /// Sum of regular-file logical sizes (hard links add no bytes), as a decimal string.
    pub logical_bytes: String,
    pub entries_sha256: String,
    pub entries: Option<Vec<Entry>>,
}

pub fn plan(profile: Profile, seed: u64) -> Plan {
    let mut hasher = Sha256::new();
    let mut counts = Counts::default();
    let mut bytes: u64 = 0;
    let mut list = Vec::new();
    entries(profile, seed, |e| {
        hasher.update(e.line().as_bytes());
        match e {
            Entry::Dir { .. } => counts.directories += 1,
            Entry::File { size, .. } => {
                counts.files += 1;
                bytes = bytes.checked_add(*size).expect("fixture bytes fit u64");
            }
            Entry::HardLink { .. } => counts.hardlinks += 1,
            Entry::SentinelLink { .. } => counts.sentinel_links += 1,
        }
        list.push(e.clone());
    });
    let total = counts.directories + counts.files + counts.hardlinks + counts.sentinel_links;
    Plan {
        profile,
        seed,
        counts,
        logical_bytes: bytes.to_string(),
        entries_sha256: hex(&hasher.finalize()),
        entries: (total <= INLINE_ENTRY_LIMIT).then_some(list),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkOutcome {
    pub path: String,
    pub created: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Generation {
    pub started_at: String,
    pub finished_at: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: String,
    pub generator: String,
    #[serde(flatten)]
    pub plan: Plan,
    pub root_display: String,
    pub sentinel_display: String,
    pub sentinel_links: Vec<LinkOutcome>,
    pub generation: Generation,
}

#[derive(Debug)]
pub struct FixtureError(pub String);

impl std::fmt::Display for FixtureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FixtureError {}

fn err<T>(msg: impl Into<String>) -> Result<T, FixtureError> {
    Err(FixtureError(msg.into()))
}

fn io<T>(what: &str, r: std::io::Result<T>) -> Result<T, FixtureError> {
    r.map_err(|e| FixtureError(format!("{what}: {e}")))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// UTC RFC 3339 timestamp without external crates (civil-from-days, Howard Hinnant).
pub fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3_600, rem % 3_600 / 60, rem % 60)
}

fn rel(root: &Path, p: &str) -> PathBuf {
    p.split('/').fold(root.to_path_buf(), |acc, c| acc.join(c))
}

fn write_sized(path: &Path, size: u64, seed: u64) -> Result<(), FixtureError> {
    let mut f = io("create file", fs::File::create_new(path))?;
    let mut rng = Rng::new(seed);
    let block: Vec<u8> = (0..MAX_FILE_BYTES).map(|_| rng.next_u64() as u8).collect();
    io("write file", f.write_all(&block[..size as usize]))
}

/// Reject a root/sentinel pair where either contains the other.
fn ensure_disjoint(root: &Path, sentinel: &Path) -> Result<(), FixtureError> {
    let r = io("resolve root", fs::canonicalize(root))?;
    let s = io("resolve sentinel", fs::canonicalize(sentinel))?;
    if r.starts_with(&s) || s.starts_with(&r) {
        return err("fixture root and sentinel must not contain each other");
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SentinelRecord {
    pub schema_version: String,
    pub files: Vec<(String, String)>,
}

const SENTINEL_FILES: [(&str, &str); 2] =
    [("sentinel.txt", "out-of-scope sentinel file\n"), ("sentinel-dir/inner.txt", "out-of-scope sentinel directory\n")];

/// Create the sentinel directory (must be new or empty) and record content hashes.
pub fn prepare_sentinel(sentinel: &Path) -> Result<SentinelRecord, FixtureError> {
    require_new_or_empty(sentinel)?;
    let mut files = Vec::new();
    for (name, content) in SENTINEL_FILES {
        let p = rel(sentinel, name);
        if let Some(parent) = p.parent() {
            io("create sentinel dir", fs::create_dir_all(parent))?;
        }
        io("write sentinel", fs::write(&p, content))?;
        files.push((name.to_owned(), hex(&Sha256::digest(content.as_bytes()))));
    }
    let record = SentinelRecord { schema_version: "1.0".into(), files };
    io("write sentinel marker", fs::write(sentinel.join(SENTINEL_MARKER), to_json(&record)))?;
    Ok(record)
}

/// Re-hash sentinel files and compare with the recorded hashes. Any change is a boundary violation.
pub fn verify_sentinel(sentinel: &Path) -> Result<(), FixtureError> {
    let record: SentinelRecord = read_json(&sentinel.join(SENTINEL_MARKER))?;
    for (name, expected) in &record.files {
        let p = rel(sentinel, name);
        let meta = io("stat sentinel", fs::symlink_metadata(&p))?;
        if !meta.is_file() {
            return err(format!("sentinel {name} is no longer a regular file"));
        }
        let actual = hex(&Sha256::digest(io("read sentinel", fs::read(&p))?));
        if &actual != expected {
            return err(format!("sentinel {name} was modified"));
        }
    }
    Ok(())
}

fn require_new_or_empty(dir: &Path) -> Result<(), FixtureError> {
    match fs::read_dir(dir) {
        Ok(mut it) => {
            if it.next().is_some() {
                return err(format!("{} exists and is not empty", dir.display()));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => io("create dir", fs::create_dir_all(dir)),
        Err(e) => err(format!("cannot inspect {}: {e}", dir.display())),
    }
}

fn link_to_sentinel(link: &Path, target: &Path, dir: bool) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        if dir { std::os::windows::fs::symlink_dir(target, link) } else { std::os::windows::fs::symlink_file(target, link) }
    }
    #[cfg(unix)]
    {
        let _ = dir;
        std::os::unix::fs::symlink(target, link)
    }
}

/// Generate a fixture tree under a new or empty `root`. The sentinel must already be prepared.
pub fn create(profile: Profile, seed: u64, root: &Path, sentinel: &Path) -> Result<Manifest, FixtureError> {
    require_new_or_empty(root)?;
    ensure_disjoint(root, sentinel)?;
    verify_sentinel(sentinel)?;
    let started_at = utc_now();
    let planned = plan(profile, seed);
    // Mark the root first so an interrupted run can still be destroyed safely.
    let mut manifest = Manifest {
        schema_version: "1.0".into(),
        generator: GENERATOR.into(),
        plan: planned,
        root_display: root.display().to_string(),
        sentinel_display: sentinel.display().to_string(),
        sentinel_links: Vec::new(),
        generation: Generation {
            started_at,
            finished_at: String::new(),
            note: "Fixture preparation only; never part of a scan measurement or read-only evidence.".into(),
        },
    };
    io("write root marker", fs::write(root.join(ROOT_MARKER), to_json(&manifest)))?;
    let mut failure = None;
    let mut index: u64 = 0;
    entries(profile, seed, |e| {
        if failure.is_some() {
            return;
        }
        index += 1;
        let result = match e {
            Entry::Dir { path } => io("create dir", fs::create_dir(rel(root, path))),
            Entry::File { path, size } => write_sized(&rel(root, path), *size, seed ^ index),
            Entry::HardLink { path, target } => io("hard link", fs::hard_link(rel(root, target), rel(root, path))),
            Entry::SentinelLink { path, sentinel_target, dir } => {
                let outcome = link_to_sentinel(&rel(root, path), &rel(sentinel, sentinel_target), *dir);
                manifest.sentinel_links.push(LinkOutcome {
                    path: path.clone(),
                    created: outcome.is_ok(),
                    error: outcome.err().map(|e| e.to_string()),
                });
                Ok(())
            }
        };
        if let Err(e) = result {
            failure = Some(e);
        }
    });
    if let Some(e) = failure {
        return Err(e);
    }
    manifest.generation.finished_at = utc_now();
    io("write root marker", fs::write(root.join(ROOT_MARKER), to_json(&manifest)))?;
    Ok(manifest)
}

/// Independently re-walk the tree and compare it with the manifest's deterministic plan.
pub fn verify(root: &Path) -> Result<Manifest, FixtureError> {
    let manifest: Manifest = read_json(&root.join(ROOT_MARKER))?;
    let mut problems = Vec::new();
    enum Expect {
        Dir,
        File(u64),
        AnyFile,
        Symlink,
    }
    entries(manifest.plan.profile, manifest.plan.seed, |e| {
        let (path, expect) = match e {
            Entry::Dir { path } => (path, Expect::Dir),
            Entry::File { path, size } => (path, Expect::File(*size)),
            Entry::HardLink { path, .. } => (path, Expect::AnyFile),
            Entry::SentinelLink { path, .. } => {
                if !manifest.sentinel_links.iter().any(|l| &l.path == path && l.created) {
                    return;
                }
                (path, Expect::Symlink)
            }
        };
        let matches = |m: &fs::Metadata| match expect {
            Expect::Dir => m.is_dir(),
            Expect::File(size) => m.is_file() && m.len() == size,
            Expect::AnyFile => m.is_file(),
            Expect::Symlink => m.file_type().is_symlink(),
        };
        match fs::symlink_metadata(rel(root, path)) {
            Ok(m) if matches(&m) => {}
            Ok(_) => problems.push(format!("unexpected type or size: {path}")),
            Err(e) => problems.push(format!("missing {path}: {e}")),
        }
    });
    if problems.is_empty() {
        Ok(manifest)
    } else {
        err(format!("{} mismatches, first: {}", problems.len(), problems[0]))
    }
}

/// Remove a fixture root created by this tool. Refuses unmarked directories. Links are removed
/// as links: `remove_dir_all` does not traverse symlinks or junctions, so sentinels survive.
pub fn destroy(root: &Path) -> Result<(), FixtureError> {
    let marker = root.join(ROOT_MARKER);
    let meta = io("stat root marker", fs::symlink_metadata(&marker))
        .map_err(|_| FixtureError(format!("{} is not a cm-fixtures root", root.display())))?;
    if !meta.is_file() {
        return err("root marker is not a regular file");
    }
    let manifest: Manifest = read_json(&marker)?;
    if manifest.generator.split(' ').next() != Some("cm-fixtures") {
        return err("root marker was not written by cm-fixtures");
    }
    io("remove fixture root", fs::remove_dir_all(root))
}

fn to_json<T: Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).expect("serializable") + "\n"
}

fn read_json<T: for<'de> Deserialize<'de>>(p: &Path) -> Result<T, FixtureError> {
    let text = io("read json", fs::read_to_string(p))?;
    serde_json::from_str(&text).map_err(|e| FixtureError(format!("parse {}: {e}", p.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cm-fixtures-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn plans_are_reproducible_and_seed_sensitive() {
        assert_eq!(plan(Profile::P0_10k, 7), plan(Profile::P0_10k, 7));
        assert_ne!(plan(Profile::P0_10k, 7).entries_sha256, plan(Profile::P0_10k, 8).entries_sha256);
    }

    #[test]
    fn bulk_shapes_match_verification_plan() {
        let p = plan(Profile::P0_100k, 1);
        assert_eq!((p.counts.directories, p.counts.files), (1_000, 100_000));
        assert!(p.entries.is_none(), "large manifests carry a digest, not every entry");
        let small = plan(Profile::P0_10k, 1);
        assert_eq!((small.counts.directories, small.counts.files), (100, 10_000));
    }

    #[test]
    fn sizes_stay_within_zero_to_four_kib() {
        let mut max = 0;
        entries(Profile::P0_10k, 3, |e| {
            if let Entry::File { size, .. } = e {
                max = max.max(*size);
            }
        });
        assert!(max <= MAX_FILE_BYTES);
    }

    #[test]
    fn tiny_lifecycle_keeps_sentinel_intact() {
        let base = scratch("tiny");
        let (root, sentinel) = (base.join("root"), base.join("sentinel"));
        prepare_sentinel(&sentinel).unwrap();
        let m = create(Profile::Tiny, 42, &root, &sentinel).unwrap();
        assert!(!m.generation.finished_at.is_empty());
        verify(&root).unwrap();
        destroy(&root).unwrap();
        assert!(!root.exists());
        verify_sentinel(&sentinel).expect("destroy must not follow links into the sentinel");
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn create_refuses_non_empty_root_and_nested_sentinel() {
        let base = scratch("refuse");
        let sentinel = base.join("sentinel");
        prepare_sentinel(&sentinel).unwrap();
        let busy = base.join("busy");
        fs::create_dir_all(&busy).unwrap();
        fs::write(busy.join("user-file.txt"), "keep").unwrap();
        assert!(create(Profile::Tiny, 1, &busy, &sentinel).is_err());
        assert_eq!(fs::read_to_string(busy.join("user-file.txt")).unwrap(), "keep");
        assert!(create(Profile::Tiny, 1, &sentinel.join("nested"), &sentinel).is_err());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn destroy_refuses_unmarked_directories() {
        let base = scratch("unmarked");
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("data.txt"), "user data").unwrap();
        assert!(destroy(&base).is_err());
        assert!(base.join("data.txt").exists());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn verify_detects_tampering() {
        let base = scratch("tamper");
        let (root, sentinel) = (base.join("root"), base.join("sentinel"));
        prepare_sentinel(&sentinel).unwrap();
        create(Profile::Deep, 5, &root, &sentinel).unwrap();
        fs::write(root.join("d00").join("f.dat"), vec![0u8; 5_000]).unwrap();
        assert!(verify(&root).is_err());
        fs::write(sentinel.join("sentinel.txt"), "changed").unwrap();
        assert!(verify_sentinel(&sentinel).is_err());
        fs::remove_dir_all(&base).unwrap();
    }
}
