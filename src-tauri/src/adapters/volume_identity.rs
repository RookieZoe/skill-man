//! macOS `VolumeIdentitySource` adapter: statfs fsid plus the APFS volume
//! UUID (via `diskutil info -plist`, parsed with the `plist` crate). Both
//! values are required; a path with no volume or an unreadable identity is
//! `Ok(None)`/`Err` respectively so bootstrap can fail closed.

use std::io::Cursor;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::core::home::VolumeIdentity;
use crate::seams::volume_identity::{VolumeIdentityError, VolumeIdentitySource};

pub struct MacOsVolumeIdentitySource;

impl MacOsVolumeIdentitySource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MacOsVolumeIdentitySource {
    fn default() -> Self {
        Self::new()
    }
}

impl VolumeIdentitySource for MacOsVolumeIdentitySource {
    fn volume_identity(&self, path: &Path) -> Result<Option<VolumeIdentity>, VolumeIdentityError> {
        if !path.exists() {
            return Ok(None);
        }
        let fsid = statfs_fsid(path)?;
        let uuid = volume_uuid(path)?;
        Ok(Some(VolumeIdentity { fsid, uuid }))
    }
}

/// `statfs(2)` device identity; stable per volume while mounted.
fn statfs_fsid(path: &Path) -> Result<String, VolumeIdentityError> {
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| unavailable(path, "path contains a NUL byte".into()))?;
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::statfs(c_path.as_ptr(), &mut stat) };
    if result != 0 {
        return Err(unavailable(
            path,
            format!("statfs failed: {}", std::io::Error::last_os_error()),
        ));
    }
    // `fsid_t` is a repr(C) pair of i32 (libc keeps the field private);
    // transmute is layout-exact and stable on Darwin.
    let fsid: [i32; 2] = unsafe { std::mem::transmute(stat.f_fsid) };
    Ok(format!("{:08x}-{:08x}", fsid[0] as u32, fsid[1] as u32))
}

/// APFS volume UUID via the stable `diskutil info -plist <path>` contract.
fn volume_uuid(path: &Path) -> Result<String, VolumeIdentityError> {
    let output = std::process::Command::new("/usr/sbin/diskutil")
        .args(["info", "-plist"])
        .arg(path)
        .output()
        .map_err(|error| unavailable(path, format!("diskutil failed to start: {error}")))?;
    if !output.status.success() {
        return Err(unavailable(
            path,
            format!(
                "diskutil exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    let plist = plist::Value::from_reader_xml(Cursor::new(output.stdout))
        .map_err(|error| unavailable(path, format!("diskutil plist unreadable: {error}")))?;
    let dictionary = plist
        .as_dictionary()
        .ok_or_else(|| unavailable(path, "diskutil plist is not a dictionary".into()))?;
    dictionary
        .get("VolumeUUID")
        .and_then(|value| value.as_string())
        .map(str::to_string)
        .ok_or_else(|| unavailable(path, "diskutil reported no VolumeUUID".into()))
}

fn unavailable(path: &Path, detail: String) -> VolumeIdentityError {
    VolumeIdentityError::Unavailable {
        path: path.display().to_string(),
        detail,
    }
}
