//! Windows disk insights (parity 7.1, DIN-01/02): known large system and virtualization objects.
//!
//! Detection is by fixed location and platform metadata only: `hiberfil.sys`, `pagefile.sys` and `swapfile.sys`
//! at volume roots; the WinSxS component store; System Restore / shadow copy storage from WMI
//! (`Win32_ShadowStorage`, `Win32_ShadowCopy`); WSL distributions from the per-user `Lxss` registry records;
//! Docker Desktop's documented data-disk locations; Hyper-V's default virtual-disk folder; `Windows.old`; the
//! Delivery Optimization cache.
//!
//! Safety boundary: sizes come from directory entries (`FindFirstFileExW`, never an open for data), a bounded
//! read-only walk that does not follow links, or the WMI query. Registry keys are read with `KEY_READ`. No system
//! tool is run: the guidance in each insight is text. Nothing here deletes, moves, shrinks or reconfigures.

use std::os::windows::ffi::OsStrExt;

use cm_core::insights::{Insight, Measured, SizeBasis, WindowsObject};
use cm_core::path::RawPath;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::{FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, WIN32_FIND_DATAW};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY};

use super::startup::{RegKey, expand_env, on_com_thread};

/// Upper bound for the Delivery Optimization walk; past it the size is reported as not measured.
pub const WALK_LIMIT: usize = 50_000;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn raw(path: &str) -> Option<RawPath> {
    RawPath::windows(std::ffi::OsStr::new(path).encode_wide().collect()).ok()
}

/// Size of one file from its directory entry, without opening it (works for in-use system files).
/// `Ok(None)` when it does not exist; `Err(code)` when the directory cannot be read.
pub(crate) fn entry_size(path: &str) -> Result<Option<u64>, u32> {
    let w = wide(path);
    let mut data: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    // SAFETY: NUL-terminated path without wildcards; basic info only; the handle is closed below.
    let h = unsafe { FindFirstFileExW(w.as_ptr(), FindExInfoBasic, (&mut data as *mut WIN32_FIND_DATAW).cast(), FindExSearchNameMatch, std::ptr::null(), 0) };
    if h == INVALID_HANDLE_VALUE {
        // SAFETY: reads the calling thread's last error.
        let e = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        return if matches!(e, 2 | 3) { Ok(None) } else { Err(e) };
    }
    // SAFETY: handle from a successful FindFirstFileExW.
    unsafe { FindClose(h) };
    if data.dwFileAttributes & 0x10 != 0 {
        return Ok(None); // a directory, not the file we look for
    }
    Ok(Some((u64::from(data.nFileSizeHigh) << 32) | u64::from(data.nFileSizeLow)))
}

/// Read-only walk summing file sizes; does not follow junctions or symlinks. `Err` with the reason when the root is
/// unreadable or the walk exceeds `limit` entries.
pub(crate) fn walk_size(root: &str, limit: usize) -> Result<(u64, u64), String> {
    let mut total = 0u64;
    let mut files = 0u64;
    let mut seen = 0usize;
    let mut denied = 0usize;
    let mut stack = vec![std::path::PathBuf::from(root)];
    let mut first = true;
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if first && e.kind() == std::io::ErrorKind::PermissionDenied => return Err("Requires administrator: the folder is readable only by the system".into()),
            Err(e) if first => return Err(format!("The folder could not be read ({e})")),
            Err(_) => {
                denied += 1;
                continue;
            }
        };
        first = false;
        for entry in rd.flatten() {
            seen += 1;
            if seen > limit {
                return Err(format!("More than {limit} entries; not measured, scan the folder in disk analysis"));
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue; // junctions and symlinks are boundaries
            }
            if ft.is_dir() {
                stack.push(entry.path());
            } else if let Ok(m) = entry.metadata() {
                total = total.saturating_add(m.len());
                files += 1;
            }
        }
    }
    if denied > 0 {
        return Err(format!("{denied} sub-folders could not be read; size not reported as partial"));
    }
    Ok((total, files))
}

fn system_drive() -> String {
    std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).trim_end_matches('\\').to_ascii_uppercase()
}

fn windows_dir() -> String {
    std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())
}

fn fixed_volume_roots() -> Vec<String> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    // SAFETY: no arguments.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u32)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| format!("{}:", char::from(b'A' + i as u8)))
        // SAFETY: NUL-terminated root path.
        .filter(|d| unsafe { GetDriveTypeW(wide(&format!("{d}\\")).as_ptr()) } == 3)
        .collect()
}

struct Out {
    items: Vec<Insight>,
    now: String,
}

impl Out {
    fn file(&mut self, object: WindowsObject, label: Option<&str>, path: &str) {
        match entry_size(path) {
            Ok(Some(bytes)) => self.items.push(Insight::windows(object, label, path, raw(path), Measured::Bytes { bytes, basis: SizeBasis::FileMetadata }, None, &self.now)),
            Ok(None) => {}
            Err(e) => self.items.push(Insight::windows(object, label, path, raw(path), Measured::Unknown(format!("The folder entry could not be read (error {e})")), None, &self.now)),
        }
    }
}

fn system_files(out: &mut Out) {
    let sys = system_drive();
    out.file(WindowsObject::HibernationFile, None, &format!(r"{sys}\hiberfil.sys"));
    for root in fixed_volume_roots() {
        out.file(WindowsObject::PageFile, Some(&root), &format!(r"{root}\pagefile.sys"));
    }
    out.file(WindowsObject::SwapFile, None, &format!(r"{sys}\swapfile.sys"));
}

fn component_store(out: &mut Out) {
    let path = format!(r"{}\WinSxS", windows_dir());
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
        let why = "Not measured: most WinSxS files are hard links shared with System32, so adding up the folder overstates it; DISM /AnalyzeComponentStore reports the actual size";
        out.items.push(Insight::windows(WindowsObject::ComponentStore, None, &path, raw(&path), Measured::Unknown(why.into()), None, &out.now));
    }
}

/// `\\?\Volume{...}\` from a WMI object path such as `Win32_Volume.DeviceID="\\\\?\\Volume{...}\\"`.
pub(crate) fn volume_from_object_path(path: &str) -> Option<String> {
    let start = path.find('"')? + 1;
    let end = path.rfind('"')?;
    (end > start).then(|| path[start..end].replace(r"\\", r"\"))
}

/// First drive letter mounted at a volume GUID path, e.g. `C:`.
fn drive_of_volume(volume: &str) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::GetVolumePathNamesForVolumeNameW;
    let w = wide(volume);
    let mut buf = vec![0u16; 1024];
    let mut len = 0u32;
    // SAFETY: NUL-terminated volume path; buffer of `buf.len()` units.
    if unsafe { GetVolumePathNamesForVolumeNameW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32, &mut len) } == 0 {
        return None;
    }
    let first_end = buf.iter().position(|&u| u == 0)?;
    let first = String::from_utf16_lossy(&buf[..first_end]);
    (first.len() >= 2 && first.as_bytes()[1] == b':').then(|| first[..2].to_ascii_uppercase())
}

/// WMI `SELECT props FROM class` in ROOT\CIMV2; each row holds the requested properties as text. Must run on a
/// COM-initialized thread. Read-only query; no method is invoked.
fn wmi_rows(class: &str, props: &[&str]) -> Result<Vec<Vec<Option<String>>>, String> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoSetProxyBlanket, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE};
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::System::Wmi::{IWbemClassObject, IWbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_INFINITE, WbemLocator};
    use windows::core::{BSTR, HSTRING};
    let query = format!("SELECT {} FROM {class}", props.join(", "));
    // SAFETY: COM calls on an initialized thread; VARIANTs are owned and cleared on drop.
    unsafe {
        let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER).map_err(|e| format!("WMI is unavailable ({e})"))?;
        let svc = locator
            .ConnectServer(&BSTR::from(r"ROOT\CIMV2"), &BSTR::new(), &BSTR::new(), &BSTR::new(), 0, &BSTR::new(), None)
            .map_err(|e| format!("WMI connection failed ({e})"))?;
        // RPC_C_AUTHN_WINNT (10), RPC_C_AUTHZ_NONE (0).
        CoSetProxyBlanket(&svc, 10, 0, None, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, None, EOAC_NONE)
            .map_err(|e| format!("WMI security setup failed ({e})"))?;
        let rows = svc
            .ExecQuery(&BSTR::from("WQL"), &BSTR::from(query.as_str()), WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY, None)
            .map_err(|e| format!("WMI query failed ({e})"))?;
        let mut out = Vec::new();
        loop {
            let mut objs: [Option<IWbemClassObject>; 1] = [None];
            let mut n = 0u32;
            let hr = rows.Next(WBEM_INFINITE, &mut objs, &mut n);
            if hr.is_err() {
                return Err(format!("WMI query failed ({})", windows::core::Error::from(hr)));
            }
            let Some(obj) = objs[0].take().filter(|_| n == 1) else { break };
            let mut row = Vec::with_capacity(props.len());
            for p in props {
                let mut v = VARIANT::default();
                let got = obj.Get(&HSTRING::from(*p), 0, &mut v, None, None);
                row.push(if got.is_ok() && !v.is_empty() { BSTR::try_from(&v).ok().map(|b| b.to_string()).or_else(|| u64::try_from(&v).ok().map(|x| x.to_string())) } else { None });
            }
            out.push(row);
        }
        Ok(out)
    }
}

fn shadow_storage(out: &mut Out) {
    let now = out.now.clone();
    let result = on_com_thread(|| {
        let storage = wmi_rows("Win32_ShadowStorage", &["Volume", "UsedSpace"])?;
        let copies = wmi_rows("Win32_ShadowCopy", &["VolumeName"]).unwrap_or_default();
        Ok::<_, String>((storage, copies))
    })
    .unwrap_or_else(|| Err("WMI query thread failed".into()));
    match result {
        Ok((storage, copies)) => {
            for row in storage {
                let Some(volume) = row.first().cloned().flatten().and_then(|p| volume_from_object_path(&p)) else { continue };
                let label = drive_of_volume(&volume).unwrap_or_else(|| volume.clone());
                let count = copies.iter().filter(|c| c.first().cloned().flatten().is_some_and(|v| v.eq_ignore_ascii_case(&volume))).count() as u64;
                let size = match row.get(1).cloned().flatten().and_then(|s| s.parse::<u64>().ok()) {
                    Some(bytes) => Measured::Bytes { bytes, basis: SizeBasis::SystemQuery },
                    None => Measured::Unknown("The shadow copy service did not report the used space".into()),
                };
                out.items.push(Insight::windows(WindowsObject::ShadowStorage, Some(&label), &volume, None, size, Some(count), &now));
            }
        }
        Err(why) => {
            let note = if why.contains("0x80041003") || why.contains("0x80070005") {
                "Requires administrator: shadow storage is reported only to administrators".to_owned()
            } else {
                format!("Shadow storage could not be queried: {why}")
            };
            out.items.push(Insight::windows(WindowsObject::ShadowStorage, None, "shadow-storage", None, Measured::Unknown(note), None, &now));
        }
    }
}

fn wsl_disks(out: &mut Out) {
    let Ok(lxss) = RegKey::open(HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\Lxss", 0) else { return };
    for id in lxss.subkeys().unwrap_or_default() {
        let Ok(k) = lxss.child(&id, 0) else { continue };
        let Some(base) = k.string("BasePath").and_then(|b| expand_env(&b)) else { continue };
        let base = base.strip_prefix(r"\\?\").unwrap_or(&base).trim_end_matches('\\').to_owned();
        let name = k.string("DistributionName").unwrap_or_else(|| id.clone());
        if k.dword("Version") == Some(1) {
            let why = "WSL 1 distribution stored as ordinary files in its folder; scan the folder in disk analysis for its size";
            out.items.push(Insight::windows(WindowsObject::WslDisk, Some(&name), &id, raw(&base), Measured::Unknown(why.into()), None, &out.now));
            continue;
        }
        let vhd = format!(r"{base}\{}", k.string("VhdFileName").unwrap_or_else(|| "ext4.vhdx".into()));
        out.file(WindowsObject::WslDisk, Some(&name), &vhd);
    }
}

fn docker_disks(out: &mut Out) {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    let program_data = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into());
    let candidates = [
        format!(r"{local}\Docker\wsl\disk\docker_data.vhdx"),
        format!(r"{local}\Docker\wsl\data\ext4.vhdx"),
        format!(r"{local}\Docker\wsl\main\ext4.vhdx"),
        format!(r"{local}\Docker\wsl\distro\ext4.vhdx"),
        format!(r"{program_data}\DockerDesktop\vm-data\DockerDesktop.vhdx"),
    ];
    for c in candidates.iter().filter(|c| !c.starts_with('\\')) {
        let label = std::path::Path::new(c).file_name().map(|f| f.to_string_lossy().into_owned());
        out.file(WindowsObject::DockerDisk, label.as_deref(), c);
    }
}

fn hyperv_disks(out: &mut Out) {
    let configured = RegKey::open(HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Virtualization", KEY_WOW64_64KEY)
        .ok()
        .and_then(|k| k.string("DefaultVirtualHardDiskPath"))
        .and_then(|p| expand_env(&p));
    let program_data = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into());
    let dir = configured.unwrap_or_else(|| format!(r"{program_data}\Microsoft\Windows\Virtual Hard Disks"));
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    let (mut bytes, mut count) = (0u64, 0u64);
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().to_ascii_lowercase();
        if !(n.ends_with(".vhdx") || n.ends_with(".vhd") || n.ends_with(".avhdx")) {
            continue;
        }
        if let Ok(Some(s)) = entry_size(&e.path().to_string_lossy()) {
            bytes = bytes.saturating_add(s);
            count += 1;
        }
    }
    if count > 0 {
        let dir = dir.trim_end_matches('\\').to_owned();
        out.items.push(Insight::windows(WindowsObject::HyperVDisks, None, &dir, raw(&dir), Measured::Bytes { bytes, basis: SizeBasis::FileMetadata }, Some(count), &out.now));
    }
}

fn previous_installation(out: &mut Out) {
    let path = format!(r"{}\Windows.old", system_drive());
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
        let why = "Not measured: the folder is large and partly readable only by the system; scan it in disk analysis for its size";
        out.items.push(Insight::windows(WindowsObject::PreviousInstallation, None, &path, raw(&path), Measured::Unknown(why.into()), None, &out.now));
    }
}

fn delivery_optimization(out: &mut Out) {
    let path = format!(r"{}\ServiceProfiles\NetworkService\AppData\Local\Microsoft\Windows\DeliveryOptimization\Cache", windows_dir());
    let moved = RegKey::open(HKEY_LOCAL_MACHINE, r"SOFTWARE\Policies\Microsoft\Windows\DeliveryOptimization", KEY_WOW64_64KEY).ok().and_then(|k| k.string("DOModifyCacheDrive"));
    if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) && moved.is_none() {
        return;
    }
    let size = match walk_size(&path, WALK_LIMIT) {
        Ok((bytes, files)) => (Measured::Bytes { bytes, basis: SizeBasis::DirectoryWalk }, Some(files)),
        Err(why) => (Measured::Unknown(why), None),
    };
    let mut insight = Insight::windows(WindowsObject::DeliveryOptimization, None, &path, raw(&path), size.0, size.1, &out.now);
    if let Some(drive) = moved {
        let note = format!("Policy DOModifyCacheDrive moves the cache to {drive}; only the default location was measured");
        insight.size_note = Some(match insight.size_note {
            Some(n) => format!("{n}. {note}"),
            None => note,
        });
    }
    out.items.push(insight);
}

/// Read-only list of known large objects on this machine, largest known size first.
pub fn list() -> Vec<Insight> {
    let mut out = Out { items: Vec::new(), now: cm_core::time::now_utc() };
    system_files(&mut out);
    component_store(&mut out);
    shadow_storage(&mut out);
    wsl_disks(&mut out);
    docker_disks(&mut out);
    hyperv_disks(&mut out);
    previous_installation(&mut out);
    delivery_optimization(&mut out);
    cm_core::insights::sort(&mut out.items);
    out.items
}

#[cfg(test)]
mod tests;
