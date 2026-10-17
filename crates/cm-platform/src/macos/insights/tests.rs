use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, SystemTime};

use cm_core::insights::{CardKind, Handling, SizeBasis};

use super::*;
use crate::macos::testfs::Fixture;

const MODULE_SOURCE: &str = include_str!("../insights.rs");
const NOW: &str = "2026-10-17T08:00:00Z";

/// DIN-02: the module only reads. No delete/move/write call, no shell or ad-hoc process, and the one tool it runs is
/// the read-only snapshot listing.
#[test]
fn module_has_no_write_or_execute_path() {
    let forbidden = [
        ["fs::rem", "ove"].concat(),
        ["fs::wr", "ite"].concat(),
        ["fs::ren", "ame"].concat(),
        ["fs::cre", "ate"].concat(),
        ["set_per", "missions"].concat(),
        ["std::process", "::Command"].concat(),
        ["/bin/", "sh"].concat(),
        ["delete", "localsnapshots\""].concat(),
        ["thin", "localsnapshots"].concat(),
        ["unl", "ink("].concat(),
    ];
    for token in &forbidden {
        assert!(!MODULE_SOURCE.contains(token.as_str()), "insights.rs must not reference {token}");
    }
    assert_eq!(MODULE_SOURCE.matches("Tool::new(").count(), 1);
    assert!(MODULE_SOURCE.contains(r#"Tool::new(TMUTIL).args(["listlocalsnapshots", "/"])"#));
}

/// `tmutil listlocalsnapshots /` output captured on macOS 14/15 (with the header) and 10.15 (without), plus a system
/// update snapshot; anything else is unrecognized rather than guessed.
#[test]
fn tmutil_output_parser() {
    let sonoma = "Snapshots for disk /:\ncom.apple.TimeMachine.2026-10-16-101530.local\ncom.apple.TimeMachine.2026-10-17-091200.local\n";
    assert_eq!(parse_tmutil(sonoma), Ok(Snapshots { total: 2, time_machine: 2 }));
    assert_eq!(parse_tmutil("Snapshots for disk /:\n"), Ok(Snapshots { total: 0, time_machine: 0 }), "zero is a known count");
    assert_eq!(parse_tmutil(""), Ok(Snapshots { total: 0, time_machine: 0 }));
    let catalina = "com.apple.TimeMachine.2019-11-02-123456\n";
    assert_eq!(parse_tmutil(catalina), Ok(Snapshots { total: 1, time_machine: 1 }));
    let update = "Snapshots for disk /:\r\ncom.apple.os.update-5C1A2B3C4D5E6F\r\ncom.apple.TimeMachine.2026-10-17-091200.local\r\n";
    assert_eq!(parse_tmutil(update), Ok(Snapshots { total: 2, time_machine: 1 }));
    assert!(parse_tmutil("Snapshots for disk /:\nUnable to list snapshots for volume\n").is_err());
}

fn output(code: i32, stdout: &str, stderr: &str) -> ToolOutput {
    ToolOutput {
        exit_code: Some(code),
        signal: None,
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
        stdout_truncated: false,
        stderr_truncated: false,
        timed_out: false,
        duration: Duration::from_millis(5),
    }
}

/// DIN-03: snapshots carry a count and never a size (not 0, not a sum); a failed listing is unknown, not 0 snapshots.
#[test]
fn snapshot_insight_counts_without_a_size() {
    let none = snapshots_insight(Ok(output(0, "Snapshots for disk /:\n", "")), NOW);
    assert_eq!((none.count, none.logical_bytes, none.size_basis), (Some(0), None, SizeBasis::NotMeasured));
    assert!(none.size_note.as_deref().unwrap().starts_with("APFS does not report"), "{:?}", none.size_note);
    let downloads = cm_core::insights::old_downloads(None, vec![], 0, 90, NOW, None);
    let cards = cm_core::insights::overview_cards(std::slice::from_ref(&none), &downloads, &[]);
    let card = cards.iter().find(|c| c.kind == CardKind::Snapshots).unwrap().size_note.clone().unwrap();
    assert_eq!(card.matches("Size unavailable").count(), 1, "the card prefixes the reason once: {card}");
    let w = none.to_wire();
    assert!(w["logical_bytes"].is_null() && w["count"] == 0 && w["kind"] == "macos.local_snapshots");
    assert_eq!(w["handling"], "open_system_settings");
    let two = snapshots_insight(Ok(output(0, "com.apple.TimeMachine.a.local\ncom.apple.os.update-X\n", "")), NOW);
    assert_eq!((two.count, two.logical_bytes), (Some(2), None));
    assert!(two.size_note.as_deref().unwrap().contains("1 Time Machine snapshot(s), 1 other"));
    let failed = snapshots_insight(Ok(output(1, "", "tmutil: listlocalsnapshots requires Full Disk Access privileges.\n")), NOW);
    assert_eq!((failed.count, failed.logical_bytes), (None, None));
    assert!(failed.size_note.as_deref().unwrap().contains("Full Disk Access"), "{:?}", failed.size_note);
    let refused = snapshots_insight(Err(ToolError::Untrusted("x".into())), NOW);
    assert_eq!(refused.count, None);
}

/// On the hosted runner (and any Mac) the listing works: zero snapshots is a known count, never "unavailable".
#[test]
fn local_snapshots_on_this_mac_are_a_known_count() {
    let i = local_snapshots(NOW);
    eprintln!("local snapshots: {}", serde_json::to_string(&i.to_wire()).unwrap());
    assert!(i.count.is_some(), "tmutil count expected: {:?}", i.size_note);
    assert_eq!((i.logical_bytes, i.size_basis), (None, SizeBasis::NotMeasured));
}

/// `struct xsw_usage` as `sysctl vm.swapusage` returns it (captured on an arm64 runner: 2 GiB total, 1.25 GiB used).
#[test]
fn swap_usage_parse() {
    let mut b = Vec::new();
    for v in [2_147_483_648u64, 805_306_368, 1_342_177_280] {
        b.extend_from_slice(&v.to_ne_bytes());
    }
    b.extend_from_slice(&16_384u32.to_ne_bytes());
    b.extend_from_slice(&1i32.to_ne_bytes());
    let s = parse_swapusage(&b).unwrap();
    assert_eq!(s, SwapUsage { total: 2_147_483_648, avail: 805_306_368, used: 1_342_177_280, pagesize: 16_384, encrypted: true });
    assert_eq!(parse_swapusage(&b[..31]), None);
    let i = swap_insight(Ok(b), NOW);
    assert_eq!((i.logical_bytes, i.size_basis, i.kind.as_str()), (Some(2_147_483_648), SizeBasis::SystemQuery, "macos.swap"));
    assert!(i.size_note.as_deref().unwrap().contains("1342177280 bytes used") && i.size_note.as_deref().unwrap().contains("encrypted"));
    let failed = swap_insight(Err(libc::ENOENT), NOW);
    assert_eq!((failed.logical_bytes, failed.size_basis), (None, SizeBasis::NotMeasured));
    // The live value has the documented layout.
    let live = sysctl_bytes(c"vm.swapusage").unwrap();
    assert_eq!(live.len(), XSW_USAGE_LEN);
    let s = parse_swapusage(&live).unwrap();
    assert!(s.used <= s.total && s.pagesize > 0, "{s:?}");
    assert!(swap(NOW).logical_bytes.is_some());
}

/// The purgeable estimate is labelled an estimate, never negative, and unknown when the volume does not answer.
#[test]
fn purgeable_space_is_an_estimate() {
    let i = purgeable_insight(Ok((1_000, 400)), NOW);
    assert_eq!((i.logical_bytes, i.size_basis), (Some(600), SizeBasis::Estimate));
    assert!(i.size_note.as_deref().unwrap().contains("capacity for important use (1000 bytes) minus available capacity (400 bytes)"));
    assert_eq!(purgeable_insight(Ok((100, 400)), NOW).logical_bytes, Some(0));
    assert_eq!(purgeable_insight(Err("no answer".into()), NOW).logical_bytes, None);
    let live = purgeable(NOW);
    eprintln!("purgeable: {}", serde_json::to_string(&live.to_wire()).unwrap());
    assert_eq!(live.size_basis, SizeBasis::Estimate, "{:?}", live.size_note);
}

/// A sparse disk image: large logical size, a few allocated blocks.
fn sparse(path: &Path, logical: u64, written: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = std::fs::File::create(path).unwrap();
    f.write_all(&vec![7u8; written]).unwrap();
    f.seek(SeekFrom::Start(logical - 1)).unwrap();
    f.write_all(&[1]).unwrap();
}

fn allocated_of(path: &Path) -> u64 {
    std::fs::symlink_metadata(path).unwrap().blocks() * 512
}

/// Fake VM/container folders in a fixture home: sizes are allocated bytes (not the sparse logical size), bundles are
/// counted, UTM's container is reported without being opened, an unreadable backup folder is `permission_required`.
#[test]
fn fake_vm_dirs_are_sized_by_allocation() {
    let fx = Fixture::new("insights-vm");
    let home = fx.path();
    let docker = join(home, DOCKER_DATA).join("Docker.raw");
    sparse(&docker, 64 << 30, 8192);
    let lima = home.join(".lima/default/diffdisk");
    sparse(&lima, 100 << 30, 16_384);
    std::fs::hard_link(&lima, home.join(".lima/default/diffdisk.link")).unwrap();
    fx.write("Parallels/Windows 11.pvm/harddisk.hdd/data.hds", &[1u8; 4096]);
    fx.write("Parallels/Ubuntu.pvm/config.pvs", b"<x/>");
    fx.write("Parallels/notes.txt", b"not a bundle");
    fx.mkdir("Library/Containers/com.utmapp.UTM/Data/Documents/Linux.utm");
    let backup = fx.mkdir("Library/Application Support/MobileSync/Backup/00008110-001");
    fx.write("Library/Application Support/MobileSync/Backup/00008110-001/Manifest.db", b"db");
    let backup_root = backup.parent().unwrap().to_path_buf();
    std::fs::set_permissions(&backup_root, std::fs::Permissions::from_mode(0o000)).unwrap();

    let items = home_objects(home, NOW);
    std::fs::set_permissions(&backup_root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let by = |kind: &str| items.iter().filter(|i| i.kind == kind).collect::<Vec<_>>();
    for i in &items {
        eprintln!("{}", serde_json::to_string(&i.to_wire()).unwrap());
        assert!(matches!(i.handling, Handling::OpenOwnerTool | Handling::OpenSystemSettings), "{i:?}");
    }

    let d = by("macos.docker_disk");
    assert_eq!(d.len(), 1, "only Docker.raw exists");
    assert_eq!((d[0].logical_bytes, d[0].size_basis), (Some(allocated_of(&docker)), SizeBasis::Allocated));
    assert!(d[0].logical_bytes.unwrap() < 1 << 30, "allocated, not the 64 GiB sparse size");
    assert!(d[0].size_note.as_deref().unwrap().contains(&format!("Logical size {}", 64u64 << 30)));

    let l = by("macos.lima_vms");
    assert_eq!(l[0].size_basis, SizeBasis::Allocated);
    let lima_dirs = allocated_of(&home.join(".lima")) + allocated_of(&home.join(".lima/default"));
    assert_eq!(l[0].logical_bytes, Some(allocated_of(&lima) + lima_dirs), "the hard link is counted once");

    let p = by("macos.parallels_vms");
    assert_eq!((p[0].count, p[0].size_basis), (Some(2), SizeBasis::Allocated));
    assert!(p[0].logical_bytes.unwrap() >= 4096);

    let u = by("macos.utm_vms");
    assert_eq!((u[0].logical_bytes, u[0].count), (None, None));
    assert!(u[0].size_note.as_deref().unwrap().starts_with(PERMISSION_REQUIRED));

    let b = by("macos.device_backups");
    assert_eq!(b[0].logical_bytes, None);
    assert!(b[0].size_note.as_deref().unwrap().starts_with(PERMISSION_REQUIRED), "{:?}", b[0].size_note);
    assert!(b[0].size_note.as_deref().unwrap().contains("Full Disk Access"));

    assert!(by("macos.vmware_vms").is_empty() && by("macos.colima_vms").is_empty() && by("macos.orbstack_disk").is_empty());

    // Readable again: the backups are counted and measured.
    let again = home_objects(home, NOW);
    let b = again.iter().find(|i| i.kind == "macos.device_backups").unwrap();
    assert_eq!((b.count, b.size_basis), (Some(1), SizeBasis::Allocated));

    // Overview: the VM card sums known allocations only when every member is known.
    let downloads = old_downloads_at(Some(&home.join("Downloads")), 90);
    let cards = cm_core::insights::overview_cards(&items, &downloads, &[]);
    let vm = cards.iter().find(|c| c.kind == CardKind::VirtualMachines).unwrap();
    assert_eq!(vm.logical_bytes, None, "UTM is unknown, so the total is unknown");
    assert!(vm.size_note.as_deref().unwrap().contains("3 of 4 measured"), "{:?}", vm.size_note);
}

#[test]
fn allocated_walk_reports_unreadable_folders_and_links() {
    let fx = Fixture::new("insights-walk");
    fx.write("a/one.bin", &[1u8; 5000]);
    let locked = fx.mkdir("a/locked");
    fx.write("a/locked/x", b"x");
    std::os::unix::fs::symlink("/usr", fx.path().join("a/usr-link")).unwrap();
    let a = fx.path().join("a");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let r = allocated_size(&a, WALK_LIMIT);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(r.as_ref().unwrap_err().starts_with(PERMISSION_REQUIRED), "{r:?}");
    let (bytes, files) = allocated_size(&a, WALK_LIMIT).unwrap();
    assert_eq!(files, 3, "one.bin, x and the link itself");
    assert!((5000..1 << 20).contains(&bytes), "the link target is not followed: {bytes}");
    assert!(allocated_size(&fx.path().join("a/usr-link"), WALK_LIMIT).unwrap_err().contains("symbolic link"));
    assert!(allocated_size(&a, 2).unwrap_err().contains("More than 2 entries"));
    assert!(allocated_size(&fx.path().join("missing"), WALK_LIMIT).is_err());
}

/// DIN-04 through the resolver's Downloads location: size-sorted, age-filtered, nothing selected; an unreadable folder
/// is `permission_required` with an unknown total, not an empty folder.
#[test]
fn old_downloads_review_of_a_fixture_folder() {
    let fx = Fixture::new("insights-dl");
    let dl = fx.mkdir("home/Downloads");
    let old = SystemTime::now() - Duration::from_secs(200 * 86_400);
    for (name, size) in [("big.dmg", 9000usize), ("small.pdf", 10), ("sub/deep.zip", 500)] {
        let p = fx.write(format!("home/Downloads/{name}"), &vec![0u8; size]);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(old).unwrap();
    }
    fx.write("home/Downloads/new.txt", b"fresh");
    let env = ResolverEnv::from_vars(|_| None).with_overrides(|k| (k == "CM_RESOLVE_HOME").then(|| fx.path().join("home").into_os_string())).0;
    assert_eq!(env.downloads.as_deref(), Some(dl.as_path()));
    let r = old_downloads_at(env.downloads.as_deref(), 90);
    assert_eq!(r.items.iter().map(|i| i.logical_bytes).collect::<Vec<_>>(), [9000, 500, 10]);
    assert!(r.items.iter().all(|i| !i.selected) && r.items[0].installer_like);
    assert_eq!((r.total_logical_bytes, r.item_count, r.handling), (Some(9510), 3, Handling::ManualTrashReview));
    assert_eq!(r.root.as_ref().map(|p| p.display()), Some(dl.to_string_lossy().into_owned()));

    std::fs::set_permissions(&dl, std::fs::Permissions::from_mode(0o000)).unwrap();
    let denied = old_downloads_at(Some(&dl), 90);
    std::fs::set_permissions(&dl, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!((denied.total_logical_bytes, denied.item_count), (None, 0));
    assert!(denied.note.as_deref().unwrap().starts_with(PERMISSION_REQUIRED), "{:?}", denied.note);

    let missing = old_downloads_at(Some(&fx.path().join("nope")), 90);
    assert_eq!((missing.root.is_none(), missing.total_logical_bytes), (true, None));
}

#[test]
fn card_dirs_below_a_fixture_home() {
    let fx = Fixture::new("insights-cards");
    fx.write(".npm/_cacache/index", &[0u8; 3000]);
    fx.write(".ollama/models/blobs/sha256-1", &[0u8; 7000]);
    let dirs = card_dirs_in(fx.path());
    assert_eq!(dirs.len(), 2);
    let npm = dirs.iter().find(|d| d.label == "npm cache").unwrap();
    assert_eq!(npm.card, CardKind::DeveloperTools);
    assert!(matches!(npm.size, Measured::Bytes { bytes, basis: SizeBasis::Allocated } if bytes >= 3000));
    assert!(dirs.iter().any(|d| d.card == CardKind::ModelLibraries));
}

#[test]
fn caps_declare_insights_unverified() {
    assert_eq!(CAPS.len(), 1);
    assert_eq!((CAPS[0].0, CAPS[0].1), ("insights.overview", CapabilityState::Unverified));
    assert!(CAPS[0].2.contains("gate G1 not passed"));
}
