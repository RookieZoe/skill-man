//! macOS `VolumeIdentitySource` adapter: statfs fsid plus the APFS volume
//! UUID (via `diskutil info -plist`, parsed with the `plist` crate). Both
//! values are required; a path with no volume or an unreadable identity is
//! `Ok(None)`/`Err` respectively so bootstrap can fail closed.

use std::ffi::{CStr, OsString};
use std::io::Cursor;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use crate::core::home::VolumeIdentity;
use crate::seams::volume_identity::{VolumeIdentityError, VolumeIdentitySource};

pub struct MacOsVolumeIdentitySource;

struct MountedVolume {
    fsid: String,
    mount_point: PathBuf,
}

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
        let volume = mounted_volume(path)?;
        let uuid = volume_uuid(path, &volume.mount_point)?;
        Ok(Some(VolumeIdentity {
            fsid: volume.fsid,
            uuid,
        }))
    }
}

/// `statfs(2)` device identity and mount point; stable per volume while
/// mounted and suitable for the subsequent `diskutil` lookup.
fn mounted_volume(path: &Path) -> Result<MountedVolume, VolumeIdentityError> {
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
    // SAFETY: `statfs` initialized the fixed-size `f_mntonname` field after
    // the successful call above; the slice never reads past that field.
    let mount_bytes = unsafe {
        std::slice::from_raw_parts(
            stat.f_mntonname.as_ptr().cast::<u8>(),
            stat.f_mntonname.len(),
        )
    };
    let mount_name = CStr::from_bytes_until_nul(mount_bytes).map_err(|_| {
        unavailable(
            path,
            "statfs reported a mount point without a NUL terminator".into(),
        )
    })?;
    if mount_name.to_bytes().is_empty() {
        return Err(unavailable(
            path,
            "statfs reported an empty mount point".into(),
        ));
    }
    let mount_point = PathBuf::from(OsString::from_vec(mount_name.to_bytes().to_vec()));
    Ok(MountedVolume {
        fsid: format!("{:08x}-{:08x}", fsid[0] as u32, fsid[1] as u32),
        mount_point,
    })
}

/// APFS volume UUID via `diskutil info -plist <mount-point>`.
fn volume_uuid(path: &Path, mount_point: &Path) -> Result<String, VolumeIdentityError> {
    let output = std::process::Command::new("/usr/sbin/diskutil")
        .args(["info", "-plist"])
        .arg(mount_point)
        .output()
        .map_err(|error| unavailable(path, format!("diskutil failed to start: {error}")))?;
    if !output.status.success() {
        return Err(unavailable(
            path,
            format!(
                "diskutil exited with {} for mount point {}: {}",
                output.status,
                mount_point.display(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_an_existing_child_directory_to_a_volume_identity() {
        let root = tempfile::tempdir().expect("temporary root");
        let child = root.path().join("fresh-home");
        std::fs::create_dir(&child).expect("create child");

        let identity = MacOsVolumeIdentitySource::new()
            .volume_identity(&child)
            .expect("read child volume identity")
            .expect("existing child has a volume identity");

        assert!(!identity.fsid.is_empty());
        assert!(!identity.uuid.is_empty());
    }
}
