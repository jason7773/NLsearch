//! Files that OneDrive and other sync apps keep only in the cloud. Reading one downloads
//! it, so they are indexed only when the user turns that on, one at a time, on a
//! connection that is not metered, and each is put back in the cloud once read.

use std::path::Path;

/// Whether a cloud file may be downloaded now: connected, and not on a metered,
/// roaming or over-the-limit connection.
pub fn can_download() -> bool {
    imp::can_download().unwrap_or(false)
}

/// Gives the space of a file downloaded for indexing back, as "Free up space" in
/// Explorer does. A file the user has meanwhile set to always keep on this device stays.
pub fn free_up_space(path: &Path) {
    if let Err(e) = imp::free_up_space(path) {
        eprintln!("could not put {} back in the cloud: {e:#}", path.display());
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use std::path::Path;

    use anyhow::{bail, Result};
    use windows::Networking::Connectivity::{NetworkConnectivityLevel, NetworkCostType, NetworkInformation};
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::CloudFilters::{
        CfDehydratePlaceholder, CfSetPinState, CF_DEHYDRATE_FLAG_NONE, CF_PIN_STATE_UNPINNED, CF_SET_PIN_FLAG_NONE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        WRITE_DAC,
    };

    /// "Always keep on this device".
    const FILE_ATTRIBUTE_PINNED: u32 = 0x80000;

    pub fn can_download() -> Result<bool> {
        // No connection at all comes back as an error.
        let profile = NetworkInformation::GetInternetConnectionProfile()?;
        if profile.GetNetworkConnectivityLevel()? != NetworkConnectivityLevel::InternetAccess {
            return Ok(false);
        }
        let cost = profile.GetConnectionCost()?;
        let unmetered = matches!(cost.NetworkCostType()?, NetworkCostType::Unrestricted | NetworkCostType::Unknown);
        Ok(unmetered && !cost.Roaming()? && !cost.OverDataLimit()?)
    }

    pub fn free_up_space(path: &Path) -> Result<()> {
        if std::fs::metadata(path)?.file_attributes() & FILE_ATTRIBUTE_PINNED != 0 {
            return Ok(());
        }
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // Opening for permissions only reads and changes nothing of the file.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                WRITE_DAC,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            bail!("{}", std::io::Error::last_os_error());
        }
        // Dehydrating leaves the file as it was before, pin state and all. It fails
        // while another program has the file open; then the sync app is asked to free
        // the space itself when it can.
        let mut hr = unsafe { CfDehydratePlaceholder(handle, 0, -1, CF_DEHYDRATE_FLAG_NONE, std::ptr::null_mut()) };
        if hr < 0 {
            hr = unsafe { CfSetPinState(handle, CF_PIN_STATE_UNPINNED, CF_SET_PIN_FLAG_NONE, std::ptr::null_mut()) };
        }
        unsafe { CloseHandle(handle) };
        if hr < 0 {
            bail!("{}", windows::core::Error::from_hresult(windows::core::HRESULT(hr)));
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;

    use anyhow::Result;

    // Elsewhere no file is ever only in the cloud.
    pub fn can_download() -> Result<bool> {
        Ok(true)
    }

    pub fn free_up_space(_path: &Path) -> Result<()> {
        Ok(())
    }
}
