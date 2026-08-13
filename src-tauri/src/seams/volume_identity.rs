//! Stable volume identity seam (§4.2): `volume_fsid + volume_uuid` prove a
//! Home path stays on the same volume. Either value being unavailable makes a
//! Home path unconfirmable; the macOS adapter reads statfs plus the APFS
//! volume UUID, a deterministic adapter drives tests.

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
