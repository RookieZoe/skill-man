use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

use crate::seams::filesystem::{
    ActivationEntrySnapshot, DirectoryFingerprint, FileSystem, FileSystemError,
    LinkSourceEntryKind, LinkSourceHop, LinkSourceSnapshot, SkillFingerprint,
};

const MAX_SKILL_DOCUMENT_BYTES: u64 = 512 * 1024;
const MAX_LINK_SOURCE_DEPTH: usize = 16;

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
    fn inspect_link_source(&self, path: &Path) -> Result<LinkSourceSnapshot, FileSystemError> {
        let entry_path = normalize_absolute_path(&self.expand_home(path))?;
        let directory_name = entry_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| FileSystemError::InvalidConfiguredPath {
                path: entry_path.clone(),
            })?
            .to_owned();
        let (final_entity_path, symlink_chain) = resolve_link_source_directory(&entry_path)?;
        let entry_metadata =
            fs::symlink_metadata(&entry_path).map_err(|source| FileSystemError::Io {
                operation: "inspect Link source",
                path: entry_path.clone(),
                source,
            })?;
        let entry_kind = if entry_metadata.file_type().is_symlink() {
            LinkSourceEntryKind::Symlink {
                target: fs::read_link(&entry_path).map_err(|source| FileSystemError::Io {
                    operation: "read Link source target",
                    path: entry_path.clone(),
                    source,
                })?,
            }
        } else if entry_metadata.is_dir() {
            LinkSourceEntryKind::Directory
        } else {
            return Err(FileSystemError::NotDirectory { path: entry_path });
        };

        Ok(LinkSourceSnapshot {
            entry_path,
            directory_name,
            entry_device: entry_metadata.dev(),
            entry_inode: entry_metadata.ino(),
            entry_kind,
            symlink_chain,
            final_entity_path,
        })
    }

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

    fn skill_fingerprint(&self, path: &Path) -> Result<SkillFingerprint, FileSystemError> {
        let directory = self.directory_fingerprint(path)?;
        let skill_document = directory.canonical_path.join("SKILL.md");
        let metadata =
            fs::symlink_metadata(&skill_document).map_err(|source| FileSystemError::Io {
                operation: "fingerprint SKILL.md",
                path: skill_document.clone(),
                source,
            })?;
        if !metadata.file_type().is_file() {
            return Err(FileSystemError::Io {
                operation: "fingerprint SKILL.md",
                path: skill_document,
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "SKILL.md is not a regular file",
                ),
            });
        }
        Ok(SkillFingerprint {
            directory,
            document_device: metadata.dev(),
            document_inode: metadata.ino(),
            document_length: metadata.len(),
            document_modified_seconds: metadata.mtime(),
            document_modified_nanoseconds: metadata.mtime_nsec(),
        })
    }

    fn read_skill_document(&self, path: &Path) -> Result<String, FileSystemError> {
        read_skill_document_at(path)
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

#[derive(Clone)]
enum OwnedPathComponent {
    Root,
    Parent,
    Normal(OsString),
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf, FileSystemError> {
    if !path.is_absolute() {
        return Err(FileSystemError::InvalidConfiguredPath {
            path: path.to_path_buf(),
        });
    }
    let mut normalized = PathBuf::from("/");
    for component in path.components() {
        match component {
            Component::RootDir => normalized = PathBuf::from("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(value) => normalized.push(value),
            Component::Prefix(_) => unreachable!("macOS paths do not contain Windows prefixes"),
        }
    }
    Ok(normalized)
}

fn owned_components(path: &Path) -> Vec<OwnedPathComponent> {
    path.components()
        .filter_map(|component| match component {
            Component::RootDir => Some(OwnedPathComponent::Root),
            Component::CurDir => None,
            Component::ParentDir => Some(OwnedPathComponent::Parent),
            Component::Normal(value) => Some(OwnedPathComponent::Normal(value.to_os_string())),
            Component::Prefix(_) => None,
        })
        .collect()
}

fn resolve_link_source_directory(
    source_path: &Path,
) -> Result<(PathBuf, Vec<LinkSourceHop>), FileSystemError> {
    let mut pending: VecDeque<_> = owned_components(source_path).into();
    let mut resolved = PathBuf::from("/");
    let mut symlink_chain = Vec::new();

    while let Some(component) = pending.pop_front() {
        match component {
            OwnedPathComponent::Root => resolved = PathBuf::from("/"),
            OwnedPathComponent::Parent => {
                resolved.pop();
            }
            OwnedPathComponent::Normal(value) => {
                let candidate = resolved.join(value);
                let metadata =
                    fs::symlink_metadata(&candidate).map_err(|source| FileSystemError::Io {
                        operation: "inspect Link source target",
                        path: candidate.clone(),
                        source,
                    })?;
                if metadata.file_type().is_symlink() {
                    if symlink_chain.len() == MAX_LINK_SOURCE_DEPTH {
                        return Err(FileSystemError::Io {
                            operation: "resolve Link source",
                            path: source_path.to_path_buf(),
                            source: std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "Link source contains a symlink loop or exceeds 16 hops",
                            ),
                        });
                    }
                    let target =
                        fs::read_link(&candidate).map_err(|source| FileSystemError::Io {
                            operation: "read Link source target",
                            path: candidate.clone(),
                            source,
                        })?;
                    symlink_chain.push(LinkSourceHop {
                        path: candidate,
                        target: target.clone(),
                        device: metadata.dev(),
                        inode: metadata.ino(),
                    });
                    let target_components = owned_components(&target);
                    for target_component in target_components.into_iter().rev() {
                        pending.push_front(target_component);
                    }
                } else {
                    if !metadata.is_dir() {
                        return Err(FileSystemError::NotDirectory { path: candidate });
                    }
                    resolved = candidate;
                }
            }
        }
    }
    Ok((resolved, symlink_chain))
}

pub(crate) fn read_skill_document_at(path: &Path) -> Result<String, FileSystemError> {
    let skill_document = path.join("SKILL.md");
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(&skill_document)
        .map_err(|source| FileSystemError::Io {
            operation: "read SKILL.md",
            path: skill_document.clone(),
            source,
        })?;
    let metadata = file.metadata().map_err(|source| FileSystemError::Io {
        operation: "inspect SKILL.md",
        path: skill_document.clone(),
        source,
    })?;
    if !metadata.is_file() || metadata.len() > MAX_SKILL_DOCUMENT_BYTES {
        return Err(FileSystemError::Io {
            operation: "read SKILL.md",
            path: skill_document,
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "SKILL.md must be a regular UTF-8 file no larger than 512 KB",
            ),
        });
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_SKILL_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| FileSystemError::Io {
            operation: "read SKILL.md",
            path: skill_document.clone(),
            source,
        })?;
    if bytes.len() as u64 > MAX_SKILL_DOCUMENT_BYTES {
        return Err(FileSystemError::Io {
            operation: "read SKILL.md",
            path: skill_document,
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "SKILL.md exceeds the 512 KB limit",
            ),
        });
    }
    String::from_utf8(bytes).map_err(|source| FileSystemError::Io {
        operation: "decode SKILL.md",
        path: skill_document,
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
    })
}
