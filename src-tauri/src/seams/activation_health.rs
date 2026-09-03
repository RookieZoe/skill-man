//! Activation Health entry facts (spec §4.10; ADR-0020): the narrow
//! read-only surface the Target-scoped health observation needs. The
//! blanket implementation means every `FileSystem` adapter already
//! provides it — the Observation module depends on the fact interface, not
//! on the full 40-method seam.

use std::path::Path;

use crate::seams::filesystem::{ActivationEntrySnapshot, FileSystem, FileSystemError};

pub trait ActivationEntryFileSystem: Send + Sync {
    /// Read-only snapshot of one managed activation entry (missing /
    /// symlink target / other occupant).
    fn activation_snapshot(
        &self,
        entry_path: &Path,
    ) -> Result<ActivationEntrySnapshot, FileSystemError>;

    /// Read-only readability probe of the final entity directory.
    fn skill_directory_is_readable(&self, path: &Path) -> Result<bool, FileSystemError>;
}

impl<T: FileSystem + ?Sized> ActivationEntryFileSystem for T {
    fn activation_snapshot(
        &self,
        entry_path: &Path,
    ) -> Result<ActivationEntrySnapshot, FileSystemError> {
        FileSystem::activation_snapshot(self, entry_path)
    }

    fn skill_directory_is_readable(&self, path: &Path) -> Result<bool, FileSystemError> {
        FileSystem::skill_directory_is_readable(self, path)
    }
}
