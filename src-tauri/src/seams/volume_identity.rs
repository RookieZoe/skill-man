//! Volume identity seam (§4.2): the APFS `volume_uuid` proves a Home path
//! remains on the same persistent volume. `volume_fsid` is retained as
//! mount-scoped diagnostic data, because macOS may change it after restart.
//! Both values are collected by the macOS adapter; an unavailable UUID makes
//! a Home path unconfirmable.

use std::path::Path;

use thiserror::Error;

use crate::core::home::VolumeIdentity;

#[derive(Debug, Error)]
pub enum VolumeIdentityError {
    #[error("the volume identity could not be read for {path}: {detail}")]
    Unavailable { path: String, detail: String },
}

/// Seam: read the stable identity of the volume containing `path`.
pub trait VolumeIdentitySource: Send + Sync {
    /// `Ok(None)` when the path does not exist (no volume); `Err` when the
    /// volume exists but its identity cannot be established.
    fn volume_identity(&self, path: &Path) -> Result<Option<VolumeIdentity>, VolumeIdentityError>;
}
