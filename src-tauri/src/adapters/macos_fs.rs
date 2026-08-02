use std::fs;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use crate::seams::filesystem::{
    ActivationEntrySnapshot, DirectoryFingerprint, FileSystem, FileSystemError,
};

pub struct MacOsFileSystem {
    home_directory: PathBuf,
}

impl MacOsFileSystem {
    pub fn new(home_directory: PathBuf) -> Self {
        Self { home_directory }
    }

    fn expand_home(&self, path: &Path) -> PathBuf {
        let Some(value) = path.to_str() else {
            return path.to_path_buf();
        };
        if value == "~" {
            return self.home_directory.clone();
        }
        value
            .strip_prefix("~/")
            .map_or_else(|| path.to_path_buf(), |rest| self.home_directory.join(rest))
    }
}

impl FileSystem for MacOsFileSystem {
    fn canonical_directory(&self, path: &Path) -> Result<PathBuf, FileSystemError> {
        let expanded = self.expand_home(path);
        let canonical = expanded
            .canonicalize()
            .map_err(|source| FileSystemError::Io {
                operation: "canonicalize directory",
                path: expanded.clone(),
                source,
            })?;
        if !canonical.is_dir() {
            return Err(FileSystemError::NotDirectory { path: canonical });
        }
        Ok(canonical)
    }

    fn normalize_configured_path(&self, path: &Path) -> Result<PathBuf, FileSystemError> {
        let expanded = self.expand_home(path);
        if !expanded.is_absolute() {
            return Err(FileSystemError::InvalidConfiguredPath { path: expanded });
        }

        let mut existing_ancestor = expanded.clone();
        let mut missing_components = Vec::new();
        while !existing_ancestor.exists() {
            let Some(component) = existing_ancestor.file_name() else {
                return Err(FileSystemError::InvalidConfiguredPath { path: expanded });
            };
            missing_components.push(component.to_os_string());
            if !existing_ancestor.pop() {
                return Err(FileSystemError::InvalidConfiguredPath { path: expanded });
            }
        }
        let mut normalized =
            existing_ancestor
                .canonicalize()
                .map_err(|source| FileSystemError::Io {
                    operation: "canonicalize configured path",
                    path: existing_ancestor,
                    source,
                })?;
        for component in missing_components.into_iter().rev() {
            normalized.push(component);
        }
        Ok(normalized)
    }

    fn directory_fingerprint(&self, path: &Path) -> Result<DirectoryFingerprint, FileSystemError> {
        let canonical_path = self.canonical_directory(path)?;
        let metadata = fs::metadata(&canonical_path).map_err(|source| FileSystemError::Io {
            operation: "fingerprint directory",
            path: canonical_path.clone(),
            source,
        })?;
        Ok(DirectoryFingerprint {
            canonical_path,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn activation_snapshot(
        &self,
        entry_path: &Path,
    ) -> Result<ActivationEntrySnapshot, FileSystemError> {
        match fs::symlink_metadata(entry_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => fs::read_link(entry_path)
                .map(|target| ActivationEntrySnapshot::Symlink { target })
                .map_err(|source| FileSystemError::Io {
                    operation: "read Activation target",
                    path: entry_path.to_path_buf(),
                    source,
                }),
            Ok(_) => Ok(ActivationEntrySnapshot::Other),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                Ok(ActivationEntrySnapshot::Missing)
            }
            Err(source) => Err(FileSystemError::Io {
                operation: "inspect Activation",
                path: entry_path.to_path_buf(),
                source,
            }),
        }
    }

    fn skill_directory_is_readable(&self, path: &Path) -> Result<bool, FileSystemError> {
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(source)
                if matches!(
                    source.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                return Ok(false);
            }
            Err(source) => {
                return Err(FileSystemError::Io {
                    operation: "inspect Skill entity",
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        if !metadata.is_dir() {
            return Ok(false);
        }

        let skill_document = path.join("SKILL.md");
        match fs::symlink_metadata(&skill_document) {
            Ok(metadata) if metadata.file_type().is_file() => {}
            Ok(_) => return Ok(false),
            Err(source)
                if matches!(
                    source.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                return Ok(false);
            }
            Err(source) => {
                return Err(FileSystemError::Io {
                    operation: "inspect SKILL.md",
                    path: skill_document,
                    source,
                });
            }
        }

        match fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
            .open(&skill_document)
        {
            Ok(file) => file
                .metadata()
                .map(|metadata| metadata.is_file())
                .map_err(|source| FileSystemError::Io {
                    operation: "inspect SKILL.md",
                    path: skill_document,
                    source,
                }),
            Err(source)
                if matches!(
                    source.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                Ok(false)
            }
            Err(source) => Err(FileSystemError::Io {
                operation: "read SKILL.md",
                path: skill_document,
                source,
            }),
        }
    }

    fn create_activation(
        &self,
        target_path: &Path,
        entry_path: &Path,
    ) -> Result<(), FileSystemError> {
        std::os::unix::fs::symlink(target_path, entry_path).map_err(|source| FileSystemError::Io {
            operation: "create Activation",
            path: entry_path.to_path_buf(),
            source,
        })
    }

    fn remove_activation(&self, entry_path: &Path) -> Result<(), FileSystemError> {
        fs::remove_file(entry_path).map_err(|source| FileSystemError::Io {
            operation: "remove Activation",
            path: entry_path.to_path_buf(),
            source,
        })
    }
}
