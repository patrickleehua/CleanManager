//! Test-only Cloud Files provider. Registers a temporary sync root, creates dehydrated
//! placeholders and counts the hydration/population callbacks the OS sends to the provider, so
//! a test can attribute any content access to the code under test. Callbacks are never
//! answered: a fetch would block until the sync root is disconnected.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::Storage::CloudFilters::{
    CF_CALLBACK_INFO, CF_CALLBACK_PARAMETERS, CF_CALLBACK_REGISTRATION, CF_CALLBACK_TYPE_FETCH_DATA,
    CF_CALLBACK_TYPE_FETCH_PLACEHOLDERS, CF_CALLBACK_TYPE_NONE, CF_CONNECT_FLAG_NONE, CF_CONNECTION_KEY, CF_CREATE_FLAG_NONE,
    CF_FS_METADATA, CF_HARDLINK_POLICY_NONE, CF_HYDRATION_POLICY, CF_HYDRATION_POLICY_FULL, CF_HYDRATION_POLICY_MODIFIER_NONE,
    CF_INSYNC_POLICY_NONE, CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC, CF_PLACEHOLDER_CREATE_INFO,
    CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT, CF_POPULATION_POLICY, CF_POPULATION_POLICY_MODIFIER_NONE,
    CF_POPULATION_POLICY_PARTIAL, CF_REGISTER_FLAG_NONE, CF_SYNC_POLICIES, CF_SYNC_REGISTRATION, CfConnectSyncRoot,
    CfCreatePlaceholders, CfDisconnectSyncRoot, CfRegisterSyncRoot, CfUnregisterSyncRoot,
};
use windows_sys::Win32::Storage::FileSystem::FILE_BASIC_INFO;
use windows_sys::core::GUID;

pub static FETCH_DATA: AtomicUsize = AtomicUsize::new(0);
pub static FETCH_PLACEHOLDERS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn on_fetch_data(_: *const CF_CALLBACK_INFO, _: *const CF_CALLBACK_PARAMETERS) {
    FETCH_DATA.fetch_add(1, Ordering::SeqCst);
}

unsafe extern "system" fn on_fetch_placeholders(_: *const CF_CALLBACK_INFO, _: *const CF_CALLBACK_PARAMETERS) {
    FETCH_PLACEHOLDERS.fetch_add(1, Ordering::SeqCst);
}

fn wide(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

fn check(hr: i32, what: &str) {
    assert!(hr >= 0, "{what} failed: HRESULT {hr:#010x}");
}

pub struct SyncRoot {
    path: Vec<u16>,
    key: CF_CONNECTION_KEY,
}

impl SyncRoot {
    /// Register and connect a sync root at `dir` (an existing empty NTFS directory).
    pub fn register(dir: &Path) -> SyncRoot {
        let path = wide(dir);
        let name: Vec<u16> = "CleanManagerTestProvider\0".encode_utf16().collect();
        let version: Vec<u16> = "1.0\0".encode_utf16().collect();
        let identity = b"cm-test-root";
        let registration = CF_SYNC_REGISTRATION {
            StructSize: size_of::<CF_SYNC_REGISTRATION>() as u32,
            ProviderName: name.as_ptr(),
            ProviderVersion: version.as_ptr(),
            SyncRootIdentity: identity.as_ptr().cast(),
            SyncRootIdentityLength: identity.len() as u32,
            FileIdentity: std::ptr::null(),
            FileIdentityLength: 0,
            ProviderId: GUID { data1: 0x4f1d_2c3a, data2: 0x6b7e, data3: 0x4c21, data4: [0x9a, 0x11, 0x5e, 0x77, 0x31, 0x0c, 0xc4, 0x0d] },
        };
        let policies = CF_SYNC_POLICIES {
            StructSize: size_of::<CF_SYNC_POLICIES>() as u32,
            Hydration: CF_HYDRATION_POLICY { Primary: CF_HYDRATION_POLICY_FULL, Modifier: CF_HYDRATION_POLICY_MODIFIER_NONE },
            Population: CF_POPULATION_POLICY { Primary: CF_POPULATION_POLICY_PARTIAL, Modifier: CF_POPULATION_POLICY_MODIFIER_NONE },
            InSync: CF_INSYNC_POLICY_NONE,
            HardLink: CF_HARDLINK_POLICY_NONE,
            PlaceholderManagement: CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT,
        };
        // SAFETY: all pointers reference live, NUL-terminated buffers for the call duration.
        check(unsafe { CfRegisterSyncRoot(path.as_ptr(), &registration, &policies, CF_REGISTER_FLAG_NONE) }, "CfRegisterSyncRoot");
        let table = [
            CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_FETCH_DATA, Callback: Some(on_fetch_data) },
            CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_FETCH_PLACEHOLDERS, Callback: Some(on_fetch_placeholders) },
            CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_NONE, Callback: None },
        ];
        let mut key: CF_CONNECTION_KEY = 0;
        // SAFETY: terminated callback table with 'static functions.
        check(unsafe { CfConnectSyncRoot(path.as_ptr(), table.as_ptr(), std::ptr::null(), CF_CONNECT_FLAG_NONE, &mut key) }, "CfConnectSyncRoot");
        SyncRoot { path, key }
    }

    /// Create a dehydrated placeholder file or a not-yet-populated placeholder directory.
    pub fn placeholder(&self, name: &str, size: i64, directory: bool) {
        let rel: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let identity = name.as_bytes();
        let mut info = CF_PLACEHOLDER_CREATE_INFO {
            RelativeFileName: rel.as_ptr(),
            FsMetadata: CF_FS_METADATA {
                BasicInfo: FILE_BASIC_INFO {
                    CreationTime: 133_000_000_000_000_000,
                    LastAccessTime: 133_000_000_000_000_000,
                    LastWriteTime: 133_000_000_000_000_000,
                    ChangeTime: 133_000_000_000_000_000,
                    FileAttributes: if directory { 0x10 } else { 0x80 },
                },
                FileSize: if directory { 0 } else { size },
            },
            FileIdentity: identity.as_ptr().cast(),
            FileIdentityLength: identity.len() as u32,
            Flags: CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC,
            Result: 0,
            CreateUsn: 0,
        };
        let mut done = 0u32;
        // SAFETY: one create-info entry referencing live buffers.
        check(unsafe { CfCreatePlaceholders(self.path.as_ptr(), &mut info, 1, CF_CREATE_FLAG_NONE, &mut done) }, "CfCreatePlaceholders");
        check(info.Result, "placeholder result");
    }
}

impl Drop for SyncRoot {
    fn drop(&mut self) {
        // SAFETY: key and path come from a successful registration and connection.
        unsafe {
            CfDisconnectSyncRoot(self.key);
            CfUnregisterSyncRoot(self.path.as_ptr());
        }
    }
}
