//! Scan root suggestions for P01. Only the user's own folders and present volume roots are
//! listed; nothing is scanned or sized here (no capacity numbers are invented).

use std::path::PathBuf;

pub struct Suggestion {
    pub label: String,
    pub kind: &'static str,
    pub path: PathBuf,
}

pub fn suggestions() -> Vec<Suggestion> {
    let mut out = Vec::new();
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
    if let Some(home) = &home {
        for (name, label) in [("Downloads", "下载"), ("Documents", "文档"), ("Desktop", "桌面")] {
            let p = home.join(name);
            if p.is_dir() {
                out.push(Suggestion { label: label.into(), kind: "user_folder", path: p });
            }
        }
        out.push(Suggestion { label: "用户目录".into(), kind: "user_home", path: home.clone() });
    }
    #[cfg(windows)]
    for letter in b'C'..=b'Z' {
        let p = PathBuf::from(format!("{}:\\", letter as char));
        if p.is_dir() {
            out.push(Suggestion { label: format!("{}: 卷", letter as char), kind: "volume", path: p });
        }
    }
    #[cfg(not(windows))]
    out.push(Suggestion { label: "系统卷".into(), kind: "volume", path: PathBuf::from("/") });
    out
}
