use std::ffi::CString;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use crate::seams::agent_configuration_fs::{
    AgentConfigurationFileSystem, AgentConfigurationFileSystemError, AgentRootEntryEvidence,
    AgentRootEntryKind, AgentRootFingerprint, AgentRootInspection, CreatedAgentTargetDirectory,
};

pub struct MacOsAgentConfigurationFileSystem {
    home_directory: PathBuf,
}

impl MacOsAgentConfigurationFileSystem {
    pub fn new(home_directory: PathBuf) -> Self {
        Self { home_directory }
    }

    fn expand_home(
        &self,
        configured_path: &Path,
    ) -> Result<PathBuf, AgentConfigurationFileSystemError> {
        let text = configured_path
            .to_str()
            .ok_or(AgentConfigurationFileSystemError::InvalidPath)?;
        if text == "~" || text.starts_with("~/") {
            return Ok(self
                .home_directory
                .join(text.strip_prefix("~/").unwrap_or("")));
        }
        if text.starts_with('~') {
            return Err(AgentConfigurationFileSystemError::InvalidPath);
        }
        Ok(configured_path.to_path_buf())
    }
}

impl AgentConfigurationFileSystem for MacOsAgentConfigurationFileSystem {
    fn inspect_root(
        &self,
        configured_path: &Path,
    ) -> Result<AgentRootInspection, AgentConfigurationFileSystemError> {
        let expanded = self.expand_home(configured_path)?;
        if !expanded.is_absolute()
            || expanded
                .components()
                .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        {
            return Err(AgentConfigurationFileSystemError::InvalidPath);
        }

        let mut ancestor = expanded.clone();
        let mut missing_components = Vec::new();
        loop {
            match fs::symlink_metadata(&ancestor) {
                Ok(_) => break,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let component = ancestor
                        .file_name()
                        .and_then(|component| component.to_str())
                        .ok_or(AgentConfigurationFileSystemError::InvalidPath)?;
                    missing_components.push(component.to_owned());
                    if !ancestor.pop() {
                        return Err(AgentConfigurationFileSystemError::InvalidPath);
                    }
                }
                Err(error) => {
                    return Err(AgentConfigurationFileSystemError::Unavailable(
                        error.to_string(),
                    ));
                }
            }
        }

        let canonical_ancestor = ancestor
            .canonicalize()
            .map_err(|error| AgentConfigurationFileSystemError::Unavailable(error.to_string()))?;
        let ancestor_metadata = fs::metadata(&canonical_ancestor)
            .map_err(|error| AgentConfigurationFileSystemError::Unavailable(error.to_string()))?;
        if !ancestor_metadata.is_dir() {
            return Err(AgentConfigurationFileSystemError::NotDirectory);
        }
        let nearest_existing_ancestor = fingerprint(&canonical_ancestor, &ancestor_metadata);

        if missing_components.is_empty() {
            let canonical_path = expanded.canonicalize().map_err(|error| {
                AgentConfigurationFileSystemError::Unavailable(error.to_string())
            })?;
            let metadata = fs::metadata(&canonical_path).map_err(|error| {
                AgentConfigurationFileSystemError::Unavailable(error.to_string())
            })?;
            if !metadata.is_dir() {
                return Err(AgentConfigurationFileSystemError::NotDirectory);
            }
            let fingerprint = fingerprint(&canonical_path, &metadata);
            let mut entries = fs::read_dir(&canonical_path)
                .map_err(|error| AgentConfigurationFileSystemError::Unavailable(error.to_string()))?
                .map(|entry| {
                    let entry = entry.map_err(|error| {
                        AgentConfigurationFileSystemError::Unavailable(error.to_string())
                    })?;
                    let name = entry
                        .file_name()
                        .into_string()
                        .map_err(|_| AgentConfigurationFileSystemError::InvalidPath)?;
                    let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                        AgentConfigurationFileSystemError::Unavailable(error.to_string())
                    })?;
                    let kind = if metadata.file_type().is_symlink() {
                        AgentRootEntryKind::Symlink {
                            target: fs::read_link(entry.path()).map_err(|error| {
                                AgentConfigurationFileSystemError::Unavailable(error.to_string())
                            })?,
                        }
                    } else if metadata.is_dir() {
                        AgentRootEntryKind::Directory
                    } else {
                        AgentRootEntryKind::File {
                            length: metadata.len(),
                        }
                    };
                    Ok(AgentRootEntryEvidence { name, kind })
                })
                .collect::<Result<Vec<_>, _>>()?;
            entries.sort_by(|left, right| left.name.cmp(&right.name));
            return Ok(AgentRootInspection {
                normalized_path: canonical_path.clone(),
                fingerprint: Some(fingerprint.clone()),
                nearest_existing_ancestor: fingerprint,
                missing_components: Vec::new(),
                entries,
                writable: path_is_writable(&canonical_path)?,
            });
        }

        let mut normalized_path = canonical_ancestor.clone();
        for component in missing_components.iter().rev() {
            normalized_path.push(component);
        }
        missing_components.reverse();
        Ok(AgentRootInspection {
            normalized_path,
            fingerprint: None,
            nearest_existing_ancestor,
            missing_components,
            entries: Vec::new(),
            writable: path_is_writable(&canonical_ancestor)?,
        })
    }

    fn create_target(
        &self,
        planned: &AgentRootInspection,
    ) -> Result<CreatedAgentTargetDirectory, AgentConfigurationFileSystemError> {
        if planned.exists() {
            return Ok(CreatedAgentTargetDirectory {
                created: Vec::new(),
            });
        }
        let current = self.inspect_root(&planned.normalized_path)?;
        if &current != planned {
            return Err(AgentConfigurationFileSystemError::PlanStale);
        }
        let mut path = planned.nearest_existing_ancestor.canonical_path.clone();
        let mut receipt = CreatedAgentTargetDirectory {
            created: Vec::with_capacity(planned.missing_components.len()),
        };
        for component in &planned.missing_components {
            path.push(component);
            if let Err(error) = fs::create_dir(&path) {
                let _ = self.rollback_created_target(&receipt);
                return Err(AgentConfigurationFileSystemError::Create(error.to_string()));
            }
            let metadata = fs::metadata(&path)
                .map_err(|error| AgentConfigurationFileSystemError::Create(error.to_string()))?;
            receipt.created.push(fingerprint(&path, &metadata));
        }
        Ok(receipt)
    }

    fn rollback_created_target(
        &self,
        receipt: &CreatedAgentTargetDirectory,
    ) -> Result<(), AgentConfigurationFileSystemError> {
        for expected in receipt.created.iter().rev() {
            let metadata = fs::symlink_metadata(&expected.canonical_path)
                .map_err(|error| AgentConfigurationFileSystemError::Rollback(error.to_string()))?;
            if !metadata.is_dir()
                || metadata.file_type().is_symlink()
                || metadata.dev() != expected.device
                || metadata.ino() != expected.inode
            {
                return Err(AgentConfigurationFileSystemError::Rollback(
                    "created directory identity changed".into(),
                ));
            }
            let mut entries = fs::read_dir(&expected.canonical_path)
                .map_err(|error| AgentConfigurationFileSystemError::Rollback(error.to_string()))?;
            if entries.next().is_some() {
                return Err(AgentConfigurationFileSystemError::Rollback(
                    "created directory is no longer empty".into(),
                ));
            }
            fs::remove_dir(&expected.canonical_path)
                .map_err(|error| AgentConfigurationFileSystemError::Rollback(error.to_string()))?;
        }
        Ok(())
    }

    fn random_bytes(&self, buffer: &mut [u8]) -> Result<(), AgentConfigurationFileSystemError> {
        File::open("/dev/urandom")
            .and_then(|mut random| random.read_exact(buffer))
            .map_err(|error| AgentConfigurationFileSystemError::Unavailable(error.to_string()))
    }
}

fn fingerprint(path: &Path, metadata: &fs::Metadata) -> AgentRootFingerprint {
    AgentRootFingerprint {
        canonical_path: path.to_path_buf(),
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

fn path_is_writable(path: &Path) -> Result<bool, AgentConfigurationFileSystemError> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| AgentConfigurationFileSystemError::InvalidPath)?;
    // SAFETY: `path` is a NUL-terminated CString that stays alive for the
    // duration of this read-only access check.
    let result = unsafe { libc::access(path.as_ptr(), libc::W_OK) };
    if result == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        Ok(false)
    } else {
        Err(AgentConfigurationFileSystemError::Unavailable(
            error.to_string(),
        ))
    }
}
