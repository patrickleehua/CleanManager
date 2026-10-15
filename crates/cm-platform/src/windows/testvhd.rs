//! Disposable virtual volumes for tests. Mounting disks changes system state, so callers only
//! use them when CM_ALLOW_VHD_TESTS=1 (elevated shell required). Every VHD is detached and deleted.

use std::path::{Path, PathBuf};
use std::process::Command;
pub fn allowed() -> bool {
    std::env::var("CM_ALLOW_VHD_TESTS").as_deref() == Ok("1")
}

pub fn free_letter() -> char {
    ('M'..='Z').rev().find(|c| !Path::new(&format!("{c}:\\")).exists()).expect("a free drive letter")
}

fn ps(script: &str) {
    let out = Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", script]).output().unwrap();
    assert!(out.status.success(), "powershell failed: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
}

pub struct Vhd {
    pub file: PathBuf,
}

impl Vhd {
    /// Create, format and mount a 64 MiB VHDX at `letter`.
    pub fn create(file: PathBuf, fs: &str, letter: char) -> Vhd {
        let script = file.with_extension("dp");
        let f = file.display();
        let body = format!(
            "create vdisk file=\"{f}\" maximum=64 type=expandable\nselect vdisk file=\"{f}\"\nattach vdisk\ncreate partition primary\nformat fs={fs} quick label=CMTEST\nassign letter={letter}\n"
        );
        std::fs::write(&script, body).unwrap();
        let out = Command::new("diskpart").arg("/s").arg(&script).output().unwrap();
        assert!(out.status.success(), "diskpart: {}", String::from_utf8_lossy(&out.stdout));
        std::fs::remove_file(&script).unwrap();
        Vhd { file }
    }

    pub fn dismount(&self) {
        ps(&format!("Dismount-DiskImage -ImagePath \"{}\" | Out-Null", self.file.display()));
    }

    pub fn mount_at(&self, letter: char) {
        ps(&format!(
            "$i = Mount-DiskImage -ImagePath \"{}\" -NoDriveLetter -PassThru; $i | Get-Disk | Get-Partition | Where-Object Type -ne Reserved | Select-Object -First 1 | Set-Partition -NewDriveLetter {letter}",
            self.file.display()
        ));
    }
}

impl Drop for Vhd {
    fn drop(&mut self) {
        let cmd = format!("Dismount-DiskImage -ImagePath \"{}\" -ErrorAction SilentlyContinue | Out-Null", self.file.display());
        let _ = Command::new("powershell").args(["-NoProfile", "-Command", &cmd]).output();
        let _ = std::fs::remove_file(&self.file);
    }
}
