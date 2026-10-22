//! Minimal safe wrappers over the Win32 handle and file-information calls used by the scanner.
//! Every open uses FILE_FLAG_OPEN_REPARSE_POINT so links are inspected, never followed, and
//! requests only attribute/list rights so no file content is ever read.

use std::io;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, GetFileInformationByHandle, FILE_ATTRIBUTE_TAG_INFO, FILE_COMPRESSION_INFO, FILE_ID_INFO, FILE_STANDARD_INFO,
    FileAttributeTagInfo, FileCompressionInfo, FileIdInfo, FileStandardInfo, GetFileInformationByHandleEx, GetFinalPathNameByHandleW, GetFullPathNameW,
    GetVolumeInformationByHandleW,
};

pub const FILE_LIST_DIRECTORY: u32 = 0x0001;
pub const FILE_READ_ATTRIBUTES: u32 = 0x0080;
pub const SYNCHRONIZE: u32 = 0x0010_0000;
const SHARE_ALL: u32 = 0x1 | 0x2 | 0x4;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const VOLUME_NAME_GUID: u32 = 0x1;

pub struct Handle(HANDLE);

impl Handle {
    /// Open a file or directory for attribute queries (and directory listing) only.
    pub fn open(path: &[u16], access: u32) -> io::Result<Handle> {
        let mut z = path.to_vec();
        z.push(0);
        // SAFETY: NUL-terminated wide path; no security attributes or template.
        let h = unsafe {
            CreateFileW(
                z.as_ptr(),
                access,
                SHARE_ALL,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE { Err(io::Error::last_os_error()) } else { Ok(Handle(h)) }
    }

    pub fn raw(&self) -> HANDLE {
        self.0
    }

    /// Attributes and reparse tag of the opened object itself (opens never follow links), rather
    /// than of a path that could be replaced. The tag is 0 for an object that is not a reparse
    /// point, and also where the file system has no FileAttributeTagInfo (the attributes still tell).
    pub fn attribute_tag(&self) -> io::Result<(u32, u32)> {
        let mut info: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
        // SAFETY: buffer is a FILE_ATTRIBUTE_TAG_INFO of the declared size; this reads metadata only.
        let ok = unsafe {
            GetFileInformationByHandleEx(self.0, FileAttributeTagInfo, (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(), size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32)
        };
        if ok != 0 {
            return Ok((info.FileAttributes, info.ReparseTag));
        }
        let mut basic: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: valid handle and out-pointer; this reads metadata only.
        if unsafe { GetFileInformationByHandle(self.0, &mut basic) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((basic.dwFileAttributes, 0))
    }

    /// Volume serial number and 128-bit file ID of the opened object. File systems without
    /// FileIdInfo (FAT, exFAT) fall back to the 32-bit serial and 64-bit file index.
    pub fn id_info(&self) -> io::Result<(u64, [u8; 16])> {
        let mut info: FILE_ID_INFO = unsafe { std::mem::zeroed() };
        // SAFETY: buffer is a FILE_ID_INFO of the declared size.
        let ok = unsafe {
            GetFileInformationByHandleEx(self.0, FileIdInfo, (&mut info as *mut FILE_ID_INFO).cast(), size_of::<FILE_ID_INFO>() as u32)
        };
        if ok != 0 {
            return Ok((info.VolumeSerialNumber, info.FileId.Identifier));
        }
        let mut basic: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: valid handle and out-pointer.
        if unsafe { GetFileInformationByHandle(self.0, &mut basic) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let index = (u64::from(basic.nFileIndexHigh) << 32) | u64::from(basic.nFileIndexLow);
        let mut id = [0u8; 16];
        id[..8].copy_from_slice(&index.to_le_bytes());
        Ok((u64::from(basic.dwVolumeSerialNumber), id))
    }

    /// Hard-link count from FILE_STANDARD_INFO.
    pub fn link_count(&self) -> io::Result<u32> {
        let mut info: FILE_STANDARD_INFO = unsafe { std::mem::zeroed() };
        // SAFETY: buffer is a FILE_STANDARD_INFO of the declared size.
        let ok = unsafe {
            GetFileInformationByHandleEx(self.0, FileStandardInfo, (&mut info as *mut FILE_STANDARD_INFO).cast(), size_of::<FILE_STANDARD_INFO>() as u32)
        };
        if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(info.NumberOfLinks) }
    }

    /// Actual on-disk size of a compressed or sparse file (same value as GetCompressedFileSize).
    pub fn compressed_size(&self) -> io::Result<u64> {
        compressed_size(self.0)
    }

    /// File-system name (NTFS, ReFS, exFAT, FAT32, ...) of the volume holding this handle.
    pub fn filesystem(&self) -> io::Result<String> {
        let mut fs = [0u16; 64];
        // SAFETY: only the file-system name buffer is requested.
        let ok = unsafe {
            GetVolumeInformationByHandleW(
                self.0,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let n = fs.iter().position(|&c| c == 0).unwrap_or(fs.len());
        Ok(String::from_utf16_lossy(&fs[..n]))
    }

    /// `\\?\Volume{GUID}` prefix of the volume, when the volume has a GUID name.
    pub fn volume_guid(&self) -> Option<String> {
        let mut buf = vec![0u16; 1024];
        // SAFETY: buffer length is passed in characters.
        let n = unsafe { GetFinalPathNameByHandleW(self.0, buf.as_mut_ptr(), buf.len() as u32, VOLUME_NAME_GUID) } as usize;
        if n == 0 || n >= buf.len() {
            return None;
        }
        let s = String::from_utf16_lossy(&buf[..n]);
        let end = s.find('}')?;
        Some(s[..=end].to_owned())
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateFileW and is closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Actual on-disk size of a compressed or sparse file behind any open handle.
pub fn compressed_size(handle: HANDLE) -> io::Result<u64> {
    let mut info: FILE_COMPRESSION_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: buffer is a FILE_COMPRESSION_INFO of the declared size; caller owns the handle.
    let ok = unsafe {
        GetFileInformationByHandleEx(handle, FileCompressionInfo, (&mut info as *mut FILE_COMPRESSION_INFO).cast(), size_of::<FILE_COMPRESSION_INFO>() as u32)
    };
    if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(info.CompressedFileSize as u64) }
}

/// Absolute form of a user path (no link resolution), plus the `\\?\` form used for opens.
pub fn full_path(path: &[u16]) -> io::Result<(Vec<u16>, Vec<u16>)> {
    let mut z = path.to_vec();
    z.push(0);
    let mut buf = vec![0u16; 32_768];
    // SAFETY: NUL-terminated input; output length bounded by the buffer.
    let n = unsafe { GetFullPathNameW(z.as_ptr(), buf.len() as u32, buf.as_mut_ptr(), std::ptr::null_mut()) } as usize;
    if n == 0 || n >= buf.len() {
        return Err(io::Error::last_os_error());
    }
    buf.truncate(n);
    let display = buf.clone();
    let prefix: Vec<u16> = "\\\\?\\".encode_utf16().collect();
    let unc: Vec<u16> = "\\\\".encode_utf16().collect();
    let verbatim = if buf.starts_with(&prefix) {
        buf
    } else if buf.starts_with(&unc) {
        let mut v: Vec<u16> = "\\\\?\\UNC\\".encode_utf16().collect();
        v.extend_from_slice(&buf[2..]);
        v
    } else {
        let mut v = prefix;
        v.extend_from_slice(&buf);
        v
    };
    Ok((display, verbatim))
}
