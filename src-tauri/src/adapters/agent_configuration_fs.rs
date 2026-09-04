use std::ffi::CString;
use std::fs::{self, File};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use crate::seams::agent_configuration_fs::{
    AgentConfigurationFileSystem, AgentConfigurationFileSystemError, AgentRootEntryEvidence,
    AgentRootEntryKind, AgentRootFingerprint, AgentRootInspection, AgentRootProbe,
    CreatedAgentTargetDirectory,
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
    fn probe_root(
        &self,
        configured_path: &Path,
    ) -> Result<AgentRootProbe, AgentConfigurationFileSystemError> {
        let expanded = self.expand_home(configured_path)?;
        validate_absolute_safe_path(&expanded)?;
        match fs::symlink_metadata(&expanded) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(AgentRootProbe::Absent)
            }
            Err(error) => Ok(AgentRootProbe::Unavailable {
                diagnostic: error.to_string(),
            }),
            Ok(_) => probe_present(&expanded),
        }
    }

    fn inspect_root(
        &self,
        configured_path: &Path,
    ) -> Result<AgentRootInspection, AgentConfigurationFileSystemError> {
        let expanded = self.expand_home(configured_path)?;
        validate_absolute_safe_path(&expanded)?;

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
        let descriptor =
            open_directory_nofollow(&path, "open Agent Target ancestor without following links")?;
        let ancestor_metadata = descriptor_metadata(&descriptor, &path)?;
        if ancestor_metadata.st_dev as u64 != planned.nearest_existing_ancestor.device
            || ancestor_metadata.st_ino != planned.nearest_existing_ancestor.inode
        {
            return Err(AgentConfigurationFileSystemError::PlanStale);
        }
        let mut parent = descriptor;
        let mut receipt = CreatedAgentTargetDirectory {
            created: Vec::with_capacity(planned.missing_components.len()),
        };
        for component in &planned.missing_components {
            path.push(component);
            let name = CString::new(component.as_bytes())
                .map_err(|_| AgentConfigurationFileSystemError::InvalidPath)?;
            let status = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), libc::S_IRWXU) };
            if status != 0 {
                let error = std::io::Error::last_os_error();
                let _ = self.rollback_created_target(&receipt);
                return if error.kind() == std::io::ErrorKind::AlreadyExists {
                    Err(AgentConfigurationFileSystemError::PlanStale)
                } else {
                    Err(AgentConfigurationFileSystemError::Create(error.to_string()))
                };
            }
            let child = match open_directory_at_nofollow(&parent, &name, &path) {
                Ok(child) => child,
                Err(error) => {
                    let _ = self.rollback_created_target(&receipt);
                    return Err(error);
                }
            };
            let metadata = match descriptor_metadata(&child, &path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    let _ = self.rollback_created_target(&receipt);
                    return Err(error);
                }
            };
            receipt.created.push(AgentRootFingerprint {
                canonical_path: path.clone(),
                device: metadata.st_dev as u64,
                inode: metadata.st_ino,
            });
            if metadata.st_dev != ancestor_metadata.st_dev {
                let _ = self.rollback_created_target(&receipt);
                return Err(AgentConfigurationFileSystemError::Create(
                    "the Target creation crossed a filesystem boundary".into(),
                ));
            }
            if let Err(error) = sync_descriptor(&parent, &path) {
                let _ = self.rollback_created_target(&receipt);
                return Err(error);
            }
            parent = child;
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

fn open_directory_nofollow(
    path: &Path,
    operation: &'static str,
) -> Result<OwnedFd, AgentConfigurationFileSystemError> {
    let encoded = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| AgentConfigurationFileSystemError::InvalidPath)?;
    let descriptor = unsafe {
        libc::open(
            encoded.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        let source = std::io::Error::last_os_error();
        if matches!(
            source.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::TooManyLinks
        ) {
            return Err(AgentConfigurationFileSystemError::PlanStale);
        }
        return Err(AgentConfigurationFileSystemError::Unavailable(format!(
            "{operation}: {}",
            source
        )));
    }
    // SAFETY: `open` returned a new owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

fn open_directory_at_nofollow(
    parent: &OwnedFd,
    name: &CString,
    path: &Path,
) -> Result<OwnedFd, AgentConfigurationFileSystemError> {
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        let source = std::io::Error::last_os_error();
        return if matches!(
            source.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::TooManyLinks
        ) {
            Err(AgentConfigurationFileSystemError::PlanStale)
        } else {
            Err(AgentConfigurationFileSystemError::Unavailable(
                source.to_string(),
            ))
        };
    }
    // SAFETY: `openat` returned a new owned descriptor.
    let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let metadata = descriptor_metadata(&descriptor, path)?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(AgentConfigurationFileSystemError::NotDirectory);
    }
    Ok(descriptor)
}

fn descriptor_metadata(
    descriptor: &OwnedFd,
    _path: &Path,
) -> Result<libc::stat, AgentConfigurationFileSystemError> {
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(descriptor.as_raw_fd(), metadata.as_mut_ptr()) } != 0 {
        return Err(AgentConfigurationFileSystemError::Unavailable(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    // SAFETY: a zero `fstat` return initialized the output.
    Ok(unsafe { metadata.assume_init() })
}

fn sync_descriptor(
    descriptor: &OwnedFd,
    path: &Path,
) -> Result<(), AgentConfigurationFileSystemError> {
    if unsafe { libc::fsync(descriptor.as_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(AgentConfigurationFileSystemError::Create(format!(
            "sync {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        )))
    }
}

/// The shared path-safety gate for every configured-path probe: absolute,
/// no `.`/`..` components, no unsupported anchored shortcuts (validated by
/// `expand_home`).
fn validate_absolute_safe_path(expanded: &Path) -> Result<(), AgentConfigurationFileSystemError> {
    if expanded.is_absolute()
        && !expanded
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        Ok(())
    } else {
        Err(AgentConfigurationFileSystemError::InvalidPath)
    }
}

/// Continue a root probe for a path that exists: resolve identity, require a
/// directory and check readability. Every verification failure is
/// `Unavailable` with a raw (never app-authored) diagnostic; only a clean,
/// readable directory yields `Present`.
fn probe_present(expanded: &Path) -> Result<AgentRootProbe, AgentConfigurationFileSystemError> {
    let canonical_path = match fs::canonicalize(expanded) {
        Ok(path) => path,
        Err(error) => {
            return Ok(AgentRootProbe::Unavailable {
                diagnostic: error.to_string(),
            });
        }
    };
    let metadata = match fs::metadata(&canonical_path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return Ok(AgentRootProbe::Unavailable {
                diagnostic: error.to_string(),
            });
        }
    };
    if !metadata.is_dir() {
        return Ok(AgentRootProbe::Unavailable {
            diagnostic: "not_a_directory".to_owned(),
        });
    }
    if let Err(error) = fs::read_dir(&canonical_path) {
        return Ok(AgentRootProbe::Unavailable {
            diagnostic: error.to_string(),
        });
    }
    Ok(AgentRootProbe::Present { canonical_path })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_present_root_resolves_identity_via_home_expansion() {
        let temp = tempfile::tempdir().expect("temp dir");
        let root = temp.path().join("skills-root");
        std::fs::create_dir(&root).expect("create root");
        let filesystem = MacOsAgentConfigurationFileSystem::new(temp.path().to_path_buf());

        let probe = filesystem
            .probe_root(Path::new("~/skills-root"))
            .expect("probe");
        assert_eq!(
            probe,
            AgentRootProbe::Present {
                canonical_path: root.canonicalize().expect("canonical root")
            }
        );
    }

    #[test]
    fn probe_missing_root_is_absent_and_never_creates() {
        let temp = tempfile::tempdir().expect("temp dir");
        let filesystem = MacOsAgentConfigurationFileSystem::new(temp.path().to_path_buf());

        let probe = filesystem
            .probe_root(Path::new("~/missing/skills"))
            .expect("probe");
        assert_eq!(probe, AgentRootProbe::Absent);
        assert!(
            !temp.path().join("missing").exists(),
            "probing is read-only"
        );
    }

    #[test]
    fn probe_file_root_is_unavailable_not_absent() {
        let temp = tempfile::tempdir().expect("temp dir");
        std::fs::write(temp.path().join("blocked"), b"not a directory").expect("write file");
        let filesystem = MacOsAgentConfigurationFileSystem::new(temp.path().to_path_buf());

        match filesystem
            .probe_root(Path::new("~/blocked"))
            .expect("probe")
        {
            AgentRootProbe::Unavailable { diagnostic } => {
                assert_eq!(
                    diagnostic, "not_a_directory",
                    "diagnostic is a technical token, never app copy: {diagnostic}"
                );
            }
            other => panic!("a non-directory root must be Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn probe_symlink_root_resolves_to_canonical_directory() {
        let temp = tempfile::tempdir().expect("temp dir");
        let target = temp.path().join("real-skills");
        std::fs::create_dir(&target).expect("create target");
        let link = temp.path().join("linked-skills");
        std::os::unix::fs::symlink(&target, &link).expect("create symlink");
        let filesystem = MacOsAgentConfigurationFileSystem::new(temp.path().to_path_buf());

        let probe = filesystem
            .probe_root(Path::new("~/linked-skills"))
            .expect("probe");
        assert_eq!(
            probe,
            AgentRootProbe::Present {
                canonical_path: target.canonicalize().expect("canonical target")
            }
        );
    }

    #[test]
    fn probe_home_shortcut_outside_home_is_invalid_path() {
        let temp = tempfile::tempdir().expect("temp dir");
        let filesystem = MacOsAgentConfigurationFileSystem::new(temp.path().to_path_buf());
        assert!(matches!(
            filesystem.probe_root(Path::new("~other/skills")),
            Err(AgentConfigurationFileSystemError::InvalidPath)
        ));
    }

    #[test]
    fn create_target_rejects_a_replaced_ancestor_without_mkdir_redirect() {
        let temp = tempfile::tempdir().expect("temporary Agent Target root");
        let ancestor = temp.path().join("agent");
        let outside = tempfile::tempdir().expect("outside target");
        fs::create_dir(&ancestor).expect("create ancestor");
        let filesystem = MacOsAgentConfigurationFileSystem::new(temp.path().to_path_buf());
        let planned = filesystem
            .inspect_root(Path::new("~/agent/skills"))
            .expect("inspect missing target");

        let preserved = temp.path().join("preserved-agent");
        fs::rename(&ancestor, &preserved).expect("preserve original ancestor");
        std::os::unix::fs::symlink(outside.path(), &ancestor).expect("replace ancestor");

        let result = filesystem.create_target(&planned);

        assert!(matches!(
            result,
            Err(AgentConfigurationFileSystemError::PlanStale)
        ));
        assert!(
            !outside.path().join("skills").exists(),
            "an ancestor symlink must never redirect Target creation"
        );
        assert!(preserved.exists());
    }
}
