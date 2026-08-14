use std::collections::VecDeque;
use std::ffi::{CStr, CString, OsString};
use std::fs;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::seams::filesystem::{
    ActivationEntrySnapshot, ActivationRecoveryBaseline, ActivationReplaceJournal,
    ActivationReplacePhase, AdoptActivationStep, AdoptAppearanceKind, AdoptAppearanceStep,
    AdoptItemPhase, AdoptJournal, AdoptJournalItem, AdoptJournalKind, AdoptJournalPhase,
    DirectoryEntry, DirectoryFingerprint, FileImportJournal, FileImportJournalPhase,
    FileImportRecoveryBaseline, FileReplacement, FileSystem, FileSystemError, LinkSourceEntryKind,
    LinkSourceHop, LinkSourceSnapshot, OccupantKind, OccupantSnapshot, RelocateInitialEntry,
    RelocateJournal, RelocateJournalPhase, RelocateRecoveryBaseline, RemoveInitialEntry,
    RemoveJournal, RemoveRecoveryBaseline, RemoveSourceKind, ScannedSkillEntry, SkillFingerprint,
    StagedEntryKind, StagedTreeEntry, StagedTreeSnapshot,
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

    fn recover_planned_file_import_item(
        &self,
        library_root: &Path,
        operation_id: &str,
        item: &crate::seams::filesystem::FileImportJournalItem,
    ) -> Result<(), FileSystemError> {
        let skills_root = library_root.join("skills");
        let final_entity_path = self.normalize_configured_path(&item.final_entity_path)?;
        if final_entity_path.parent() != Some(skills_root.as_path()) {
            return Err(FileSystemError::InvalidConfiguredPath {
                path: item.final_entity_path.clone(),
            });
        }
        let directory_name = final_entity_path.file_name().ok_or_else(|| {
            FileSystemError::InvalidConfiguredPath {
                path: item.final_entity_path.clone(),
            }
        })?;
        let temporary_path = skills_root.join(format!(
            ".{}.new-{operation_id}",
            directory_name.to_string_lossy()
        ));
        let backup_path = library_root
            .join("operations")
            .join(operation_id)
            .join("backup")
            .join(directory_name);

        if real_directory_exists(&temporary_path)? {
            ensure_directory_identity(&temporary_path, &item.staged_root_fingerprint)?;
            if tree_hash_at(&temporary_path)? != item.expected_content_hash {
                return Err(stale_tree_entry(&temporary_path));
            }
            let fingerprint = self.directory_fingerprint(&temporary_path)?;
            self.discard_installed_skill(&temporary_path, library_root, &fingerprint)?;
        }

        if item.replacement_planned {
            let expected_original = item.replacement_original_tree.as_ref().ok_or_else(|| {
                FileSystemError::RecoveryRequired {
                    operation: "recover interrupted file reinstall",
                    path: final_entity_path.clone(),
                    message: "journal does not identify the original stable entity".into(),
                }
            })?;
            if real_directory_exists(&backup_path)? {
                let backup_tree = staged_tree_snapshot_at(&backup_path)?;
                ensure_moved_tree_matches(&backup_tree, expected_original, &backup_path)?;
                let backup_fingerprint = self.directory_fingerprint(&backup_path)?;
                if real_directory_exists(&final_entity_path)? {
                    ensure_directory_identity(&final_entity_path, &item.staged_root_fingerprint)?;
                    if tree_hash_at(&final_entity_path)? != item.expected_content_hash {
                        return Err(stale_tree_entry(&final_entity_path));
                    }
                    let installed_fingerprint = self.directory_fingerprint(&final_entity_path)?;
                    self.rollback_replaced_skill(
                        &FileReplacement {
                            final_entity_path: final_entity_path.clone(),
                            installed_fingerprint,
                            backup_path,
                            backup_fingerprint,
                            original_tree_snapshot: expected_original.clone(),
                        },
                        library_root,
                    )?;
                } else {
                    fs::rename(&backup_path, &final_entity_path).map_err(|source| {
                        FileSystemError::Io {
                            operation: "restore interrupted file reinstall backup",
                            path: final_entity_path.clone(),
                            source,
                        }
                    })?;
                    if staged_tree_snapshot_at(&final_entity_path)? != *expected_original {
                        return Err(stale_tree_entry(&final_entity_path));
                    }
                    remove_empty_backup_directory(&backup_path, &library_root.join("operations"))?;
                }
            } else if !real_directory_exists(&final_entity_path)?
                || staged_tree_snapshot_at(&final_entity_path)? != *expected_original
            {
                return Err(FileSystemError::RecoveryRequired {
                    operation: "recover interrupted file reinstall",
                    path: final_entity_path,
                    message: "the original stable entity changed before its backup was durable"
                        .into(),
                });
            }
        } else if real_directory_exists(&final_entity_path)? {
            ensure_directory_identity(&final_entity_path, &item.staged_root_fingerprint)?;
            if tree_hash_at(&final_entity_path)? != item.expected_content_hash {
                return Err(stale_tree_entry(&final_entity_path));
            }
            let fingerprint = self.directory_fingerprint(&final_entity_path)?;
            self.discard_installed_skill(&final_entity_path, library_root, &fingerprint)?;
        }
        Ok(())
    }

    /// Reverse-compensate an Adopt item whose catalog row is absent: restore a
    /// migrated entity and every original symlink appearance. Link items only
    /// need their appearances restored because their entity never moved.
    fn restore_uncommitted_adopt_item(
        &self,
        library_root: &Path,
        operation_id: &str,
        item: &AdoptJournalItem,
    ) -> Result<(), FileSystemError> {
        if !matches!(item.kind, AdoptJournalKind::Migrate) {
            return self.restore_uncommitted_adopt_appearances(item);
        }
        let original_path = item
            .appearances
            .iter()
            .find(|appearance| matches!(appearance.kind, AdoptAppearanceKind::RealDirectory))
            .map(|appearance| &appearance.entry_path)
            .unwrap_or(&item.original_path);
        let source_fingerprint = item.source_fingerprint.as_ref().or_else(|| {
            (item.phase == AdoptItemPhase::Planned).then_some(&item.staged_fingerprint)
        });
        if item.phase == AdoptItemPhase::Planned
            && source_fingerprint
                .is_some_and(|fingerprint| fingerprint.device != 0 && fingerprint.inode != 0)
        {
            let source_fingerprint = source_fingerprint.expect("checked above");
            let (isolated_name, _) =
                adopt_isolated_source_name(operation_id, source_fingerprint.inode, original_path)?;
            let isolated_path = original_path
                .parent()
                .ok_or_else(|| FileSystemError::InvalidConfiguredPath {
                    path: original_path.clone(),
                })?
                .join(&isolated_name);
            match fs::symlink_metadata(&isolated_path) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    match fs::symlink_metadata(original_path) {
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Ok(_) => {
                            return Err(FileSystemError::RecoveryRequired {
                                operation: "restore isolated Adopt source",
                                path: original_path.clone(),
                                message: "both the isolated source and original location exist"
                                    .into(),
                            });
                        }
                        Err(error) => {
                            return Err(FileSystemError::Io {
                                operation: "inspect isolated Adopt restore destination",
                                path: original_path.clone(),
                                source: error,
                            });
                        }
                    }
                    restore_isolated_adopt_source_at(
                        original_path,
                        operation_id,
                        source_fingerprint,
                    )?;
                    return self.restore_uncommitted_adopt_appearances(item);
                }
                Ok(_) => {
                    return Err(FileSystemError::RecoveryRequired {
                        operation: "restore isolated Adopt source",
                        path: isolated_path,
                        message: "the isolated source is no longer a real directory".into(),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(FileSystemError::Io {
                        operation: "inspect isolated Adopt source",
                        path: isolated_path,
                        source: error,
                    });
                }
            }
        }
        match fs::symlink_metadata(original_path) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                let snapshot = staged_tree_snapshot_at(original_path)?;
                if (item.phase == AdoptItemPhase::Planned
                    && source_fingerprint.is_some_and(|fingerprint| {
                        snapshot.root.device == fingerprint.device
                            && snapshot.root.inode == fingerprint.inode
                    }))
                    || snapshot.content_hash == item.recorded_content_hash
                {
                    // Apply had not yet moved the source (or a cross-volume
                    // copy was interrupted before its verified source deletion).
                    if item.phase != AdoptItemPhase::Planned {
                        if let Some(source_fingerprint) = source_fingerprint {
                            self.discard_isolated_adopt_source(
                                original_path,
                                operation_id,
                                source_fingerprint,
                            )?;
                        }
                    }
                    return self.restore_uncommitted_adopt_appearances(item);
                }
                return Err(FileSystemError::RecoveryRequired {
                    operation: "restore interrupted Adopt entity",
                    path: original_path.clone(),
                    message: "the original location contains a changed or partial tree; staged content is retained"
                        .into(),
                });
            }
            Ok(_) => {
                return Err(FileSystemError::RecoveryRequired {
                    operation: "restore interrupted Adopt entity",
                    path: original_path.clone(),
                    message: "the original location is occupied".into(),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(FileSystemError::Io {
                    operation: "inspect interrupted Adopt original location",
                    path: original_path.clone(),
                    source: error,
                });
            }
        }

        let directory_name = item.final_entity_path.file_name().ok_or_else(|| {
            FileSystemError::InvalidConfiguredPath {
                path: item.final_entity_path.clone(),
            }
        })?;
        let temporary_path = library_root.join("skills").join(format!(
            ".{}.new-{operation_id}",
            directory_name.to_string_lossy()
        ));
        let staged_exists = real_directory_exists(&item.staged_root)?;
        let final_exists = real_directory_exists(&item.final_entity_path)?;
        let temporary_exists = real_directory_exists(&temporary_path)?;
        let (source, expected_identity) = if item.installed_fingerprint.is_some() && final_exists {
            (&item.final_entity_path, item.installed_fingerprint.as_ref())
        } else if staged_exists {
            (
                &item.staged_root,
                (item.phase == AdoptItemPhase::Staged).then_some(&item.staged_fingerprint),
            )
        } else if final_exists {
            (
                &item.final_entity_path,
                (item.phase == AdoptItemPhase::Staged).then_some(&item.staged_fingerprint),
            )
        } else if temporary_exists {
            (
                &temporary_path,
                (item.phase == AdoptItemPhase::Staged).then_some(&item.staged_fingerprint),
            )
        } else {
            return Err(FileSystemError::RecoveryRequired {
                operation: "restore interrupted Adopt entity",
                path: original_path.clone(),
                message: "neither the original, staged, temporary, nor Library entity exists"
                    .into(),
            });
        };
        if let Some(expected) = expected_identity {
            ensure_directory_identity(source, expected)?;
        }
        let snapshot = staged_tree_snapshot_at(source)?;
        if snapshot.content_hash != item.recorded_content_hash {
            return Err(stale_tree_entry(source));
        }
        self.restore_external_directory(source, original_path, &snapshot.root)?;
        if item.phase != AdoptItemPhase::Planned {
            if let Some(source_fingerprint) = source_fingerprint {
                self.discard_isolated_adopt_source(
                    original_path,
                    operation_id,
                    source_fingerprint,
                )?;
            }
        }
        self.restore_uncommitted_adopt_appearances(item)
    }

    fn restore_uncommitted_adopt_appearances(
        &self,
        item: &AdoptJournalItem,
    ) -> Result<(), FileSystemError> {
        for appearance in item.appearances.iter().rev() {
            let AdoptAppearanceKind::Symlink { original_target } = &appearance.kind else {
                continue;
            };
            let create_original = match self.activation_snapshot(&appearance.entry_path)? {
                ActivationEntrySnapshot::Missing => true,
                ActivationEntrySnapshot::Symlink { target } if target == *original_target => false,
                ActivationEntrySnapshot::Symlink { target }
                    if item.activations.iter().any(|activation| {
                        activation.entry_path == appearance.entry_path
                            && activation.target_path == target
                    }) =>
                {
                    fs::remove_file(&appearance.entry_path).map_err(|source| {
                        FileSystemError::Io {
                            operation: "remove interrupted Adopt Activation",
                            path: appearance.entry_path.clone(),
                            source,
                        }
                    })?;
                    true
                }
                ActivationEntrySnapshot::Symlink { .. } | ActivationEntrySnapshot::Other => {
                    return Err(FileSystemError::RecoveryRequired {
                        operation: "restore interrupted Adopt appearance",
                        path: appearance.entry_path.clone(),
                        message: "the original appearance location is occupied".into(),
                    });
                }
            };
            if create_original {
                std::os::unix::fs::symlink(original_target, &appearance.entry_path).map_err(
                    |source| FileSystemError::Io {
                        operation: "restore interrupted Adopt appearance",
                        path: appearance.entry_path.clone(),
                        source,
                    },
                )?;
            }
        }
        Ok(())
    }

    fn verify_committed_adopt_entity(
        &self,
        item: &AdoptJournalItem,
    ) -> Result<(), FileSystemError> {
        if !real_directory_exists(&item.final_entity_path)? {
            return Err(FileSystemError::RecoveryRequired {
                operation: "recover committed Adopt item",
                path: item.final_entity_path.clone(),
                message: "the final entity is missing; staged or temporary content is retained"
                    .into(),
            });
        }
        let expected_identity = item.installed_fingerprint.as_ref().or_else(|| {
            (item.staged_fingerprint.device != 0 || item.staged_fingerprint.inode != 0)
                .then_some(&item.staged_fingerprint)
        });
        if let Some(expected) = expected_identity {
            ensure_directory_identity(&item.final_entity_path, expected)?;
        }
        if !item.recorded_content_hash.is_empty()
            && staged_tree_snapshot_at(&item.final_entity_path)?.content_hash
                != item.recorded_content_hash
        {
            return Err(stale_tree_entry(&item.final_entity_path));
        }
        Ok(())
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

        match read_skill_document_at(path) {
            Ok(_) => Ok(true),
            Err(FileSystemError::Io { source, .. })
                if matches!(
                    source.kind(),
                    std::io::ErrorKind::NotFound
                        | std::io::ErrorKind::PermissionDenied
                        | std::io::ErrorKind::InvalidData
                        | std::io::ErrorKind::TooManyLinks
                ) =>
            {
                Ok(false)
            }
            Err(FileSystemError::NotDirectory { .. } | FileSystemError::PlanStale { .. }) => {
                Ok(false)
            }
            Err(error) => Err(error),
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

    fn read_utf8_file(&self, path: &Path) -> Result<Option<String>, FileSystemError> {
        match std::fs::read_to_string(path) {
            Ok(content) => Ok(Some(content)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(FileSystemError::Io {
                operation: "read UTF-8 file",
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    fn write_utf8_file(&self, path: &Path, content: &str) -> Result<(), FileSystemError> {
        std::fs::write(path, content).map_err(|source| FileSystemError::Io {
            operation: "write UTF-8 file",
            path: path.to_path_buf(),
            source,
        })
    }

    fn path_is_directory(&self, path: &Path) -> Result<bool, FileSystemError> {
        match std::fs::metadata(path) {
            Ok(metadata) => Ok(metadata.is_dir()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(FileSystemError::Io {
                operation: "inspect directory existence",
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    fn create_directory_all(&self, path: &Path) -> Result<(), FileSystemError> {
        if fs::symlink_metadata(path).is_ok() {
            return Err(FileSystemError::Io {
                operation: "create directory tree",
                path: path.to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "the recovery path already exists",
                ),
            });
        }
        fs::create_dir_all(path).map_err(|source| FileSystemError::Io {
            operation: "create directory tree",
            path: path.to_path_buf(),
            source,
        })
    }

    fn rename_directory(&self, from: &Path, to: &Path) -> Result<(), FileSystemError> {
        if fs::symlink_metadata(to).is_ok() {
            return Err(FileSystemError::Io {
                operation: "rename directory",
                path: to.to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "the rename destination already exists",
                ),
            });
        }
        if !fs::symlink_metadata(from)
            .map_err(|source| FileSystemError::Io {
                operation: "rename directory",
                path: from.to_path_buf(),
                source,
            })?
            .is_dir()
        {
            return Err(FileSystemError::Io {
                operation: "rename directory",
                path: from.to_path_buf(),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "the rename source is not a directory",
                ),
            });
        }
        fs::rename(from, to).map_err(|source| FileSystemError::Io {
            operation: "rename directory",
            path: from.to_path_buf(),
            source,
        })
    }

    fn fsync_directory(&self, path: &Path) -> Result<(), FileSystemError> {
        let dir = fs::File::open(path).map_err(|source| FileSystemError::Io {
            operation: "fsync directory",
            path: path.to_path_buf(),
            source,
        })?;
        dir.sync_all().map_err(|source| FileSystemError::Io {
            operation: "fsync directory",
            path: path.to_path_buf(),
            source,
        })
    }

    fn remove_recovery_artifact(
        &self,
        artifact: &Path,
        home_root: &Path,
    ) -> Result<(), FileSystemError> {
        let Some(home_name) = home_root.file_name() else {
            return Err(FileSystemError::InvalidConfiguredPath {
                path: home_root.to_path_buf(),
            });
        };
        let Some(artifact_name) = artifact.file_name() else {
            return Err(FileSystemError::InvalidConfiguredPath {
                path: artifact.to_path_buf(),
            });
        };
        let name = artifact_name.to_string_lossy();
        let prefix = format!("{}.snapshot-", home_name.to_string_lossy());
        let prepared_prefix = format!("{}.prepared-", home_name.to_string_lossy());
        let op_id = name
            .strip_prefix(&prefix)
            .or_else(|| name.strip_prefix(&prepared_prefix));
        let Some(op_id) = op_id else {
            return Err(FileSystemError::InvalidConfiguredPath {
                path: artifact.to_path_buf(),
            });
        };
        if op_id.is_empty()
            || op_id.contains('/')
            || op_id.contains(std::path::MAIN_SEPARATOR)
            || artifact.parent() != home_root.parent()
        {
            return Err(FileSystemError::InvalidConfiguredPath {
                path: artifact.to_path_buf(),
            });
        }
        fs::remove_dir_all(artifact).map_err(|source| FileSystemError::Io {
            operation: "remove recovery artifact",
            path: artifact.to_path_buf(),
            source,
        })
    }

    /// SQLite's WAL-index protocol uses POSIX record locks (`fcntl`), so the
    /// probe must use `F_SETLK` — `flock` would never see a SQLite writer.
    /// An exclusive lock over the whole shm file conflicts with any lock any
    /// other process holds on the index (readers included); failing to
    /// acquire one is therefore proof enough to stop quiesce fail-closed.
    fn try_lock_wal_index_exclusive(&self, shm_path: &Path) -> Result<bool, FileSystemError> {
        if !fs::symlink_metadata(shm_path).is_ok() {
            // No WAL index: no SQLite connection can be active in WAL mode.
            return Ok(true);
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(shm_path)
            .map_err(|source| FileSystemError::Io {
                operation: "open WAL-index for lock probe",
                path: shm_path.to_path_buf(),
                source,
            })?;
        let mut lock = libc::flock {
            l_type: libc::F_WRLCK,
            l_whence: libc::SEEK_SET as i16,
            l_start: 0,
            l_len: 0,
            l_pid: 0,
        };
        let result = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &mut lock) };
        if result == 0 {
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EACCES) | Some(libc::EAGAIN) => Ok(false),
            _ => Err(FileSystemError::Io {
                operation: "probe WAL-index lock",
                path: shm_path.to_path_buf(),
                source: error,
            }),
        }
    }

    fn list_directory(&self, path: &Path) -> Result<Vec<DirectoryEntry>, FileSystemError> {
        let mut entries: Vec<DirectoryEntry> = fs::read_dir(path)
            .map_err(|source| FileSystemError::Io {
                operation: "list directory",
                path: path.to_path_buf(),
                source,
            })?
            .map(|entry| {
                let entry = entry.map_err(|source| FileSystemError::Io {
                    operation: "list directory",
                    path: path.to_path_buf(),
                    source,
                })?;
                let metadata = entry.metadata().map_err(|source| FileSystemError::Io {
                    operation: "list directory",
                    path: entry.path(),
                    source,
                })?;
                Ok(DirectoryEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    is_directory: metadata.is_dir(),
                    len: metadata.len(),
                })
            })
            .collect::<Result<Vec<_>, FileSystemError>>()?;
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(entries)
    }

    fn tree_hash_excluding(
        &self,
        path: &Path,
        excluded: &[String],
    ) -> Result<String, FileSystemError> {
        tree_hash_at_excluding(path, excluded)
    }

    fn ensure_directory(&self, path: &Path) -> Result<(), FileSystemError> {
        match fs::symlink_metadata(path) {
            Ok(metadata) => {
                if !metadata.is_dir() {
                    return Err(FileSystemError::NotDirectory {
                        path: path.to_path_buf(),
                    });
                }
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(path).map_err(|source| FileSystemError::Io {
                    operation: "ensure directory",
                    path: path.to_path_buf(),
                    source,
                })
            }
            Err(source) => Err(FileSystemError::Io {
                operation: "ensure directory",
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    fn path_has_no_symlink_component(&self, path: &Path) -> Result<bool, FileSystemError> {
        // Check the raw path (home-expanded but not canonicalized):
        // normalization resolves existing ancestors and would erase the
        // symlink facts this probe exists to find.
        let expanded = self.expand_home(path);
        let mut current = PathBuf::new();
        let mut depth = 0_u32;
        for component in expanded.components() {
            match component {
                std::path::Component::RootDir => {
                    current.push("/");
                    depth = 0;
                }
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    // A normalized absolute candidate never contains `..`;
                    // treat it as an unresolvable path, never a valid one.
                    return Ok(false);
                }
                std::path::Component::Prefix(_) => return Ok(false),
                std::path::Component::Normal(name) => {
                    current.push(name);
                    depth += 1;
                    match fs::symlink_metadata(&current) {
                        Ok(metadata) => {
                            // Direct children of the root (`/var`, `/tmp`,
                            // `/etc`, …) are stable system-level symlinks on
                            // macOS; user-controlled components start at
                            // depth 2 and any symlink there is refused.
                            if depth > 1 && metadata.file_type().is_symlink() {
                                return Ok(false);
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(source) => {
                            return Err(FileSystemError::Io {
                                operation: "inspect path components for symlinks",
                                path: current,
                                source,
                            });
                        }
                    }
                }
            }
        }
        Ok(true)
    }

    fn path_is_writable(&self, path: &Path) -> Result<bool, FileSystemError> {
        let encoded = CString::new(path.as_os_str().as_bytes()).map_err(|source| {
            FileSystemError::Io {
                operation: "encode path for writability probe",
                path: path.to_path_buf(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidInput, source),
            }
        })?;
        // SAFETY: `encoded` is NUL-terminated; access() never retains it.
        let result = unsafe { libc::access(encoded.as_ptr(), libc::W_OK) };
        Ok(result == 0)
    }

    fn copy_tree_verified(
        &self,
        source: &Path,
        destination: &Path,
    ) -> Result<(), FileSystemError> {
        match fs::symlink_metadata(destination) {
            Ok(metadata) => {
                if !metadata.is_dir() {
                    return Err(FileSystemError::NotDirectory {
                        path: destination.to_path_buf(),
                    });
                }
                let mut entries = fs::read_dir(destination).map_err(|source_error| {
                    FileSystemError::Io {
                        operation: "inspect copy destination",
                        path: destination.to_path_buf(),
                        source: source_error,
                    }
                })?;
                if entries.next().is_some() {
                    return Err(FileSystemError::Io {
                        operation: "copy tree",
                        path: destination.to_path_buf(),
                        source: std::io::Error::new(
                            std::io::ErrorKind::AlreadyExists,
                            "the copy destination is not empty",
                        ),
                    });
                }
                fs::remove_dir(destination).map_err(|source_error| FileSystemError::Io {
                    operation: "clear empty copy destination",
                    path: destination.to_path_buf(),
                    source: source_error,
                })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(FileSystemError::Io {
                    operation: "inspect copy destination",
                    path: destination.to_path_buf(),
                    source,
                });
            }
        }
        copy_directory_verified(source, destination)
    }

    fn tree_size(&self, path: &Path) -> Result<u64, FileSystemError> {
        let mut total = 0_u64;
        let mut stack = vec![path.to_path_buf()];
        while let Some(current) = stack.pop() {
            let entries = fs::read_dir(&current).map_err(|source| FileSystemError::Io {
                operation: "measure tree size",
                path: current.clone(),
                source,
            })?;
            for entry in entries {
                let entry = entry.map_err(|source| FileSystemError::Io {
                    operation: "measure tree size",
                    path: current.clone(),
                    source,
                })?;
                let metadata =
                    fs::symlink_metadata(entry.path()).map_err(|source| FileSystemError::Io {
                        operation: "measure tree size",
                        path: entry.path(),
                        source,
                    })?;
                if metadata.file_type().is_symlink() {
                    total = total.saturating_add(metadata.len());
                } else if metadata.is_dir() {
                    stack.push(entry.path());
                } else if metadata.is_file() {
                    total = total.saturating_add(metadata.len());
                }
            }
        }
        Ok(total)
    }

    fn remove_directory_verified(&self, path: &Path) -> Result<(), FileSystemError> {
        let metadata = fs::symlink_metadata(path).map_err(|source| FileSystemError::Io {
            operation: "remove candidate directory",
            path: path.to_path_buf(),
            source,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(FileSystemError::NotDirectory {
                path: path.to_path_buf(),
            });
        }
        fs::remove_dir_all(path).map_err(|source| FileSystemError::Io {
            operation: "remove candidate directory",
            path: path.to_path_buf(),
            source,
        })
    }

    fn tree_hash(&self, path: &Path) -> Result<String, FileSystemError> {
        tree_hash_at(path)
    }

    fn staged_tree_snapshot(&self, path: &Path) -> Result<StagedTreeSnapshot, FileSystemError> {
        staged_tree_snapshot_at(path)
    }

    fn available_space(&self, path: &Path) -> Result<u64, FileSystemError> {
        let path = self.normalize_configured_path(path)?;
        let existing = path
            .ancestors()
            .find(|candidate| candidate.exists())
            .ok_or_else(|| FileSystemError::Io {
                operation: "locate filesystem for free-space preflight",
                path: path.clone(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "no ancestor exists"),
            })?;
        let encoded = CString::new(existing.as_os_str().as_bytes()).map_err(|source| {
            FileSystemError::Io {
                operation: "encode filesystem path for free-space preflight",
                path: existing.to_path_buf(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidInput, source),
            }
        })?;
        let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: `encoded` is NUL-terminated and `stats` points to writable storage.
        let status = unsafe { libc::statvfs(encoded.as_ptr(), stats.as_mut_ptr()) };
        if status != 0 {
            return Err(FileSystemError::Io {
                operation: "read free space for file Import",
                path: existing.to_path_buf(),
                source: std::io::Error::last_os_error(),
            });
        }
        // SAFETY: a zero return from statvfs initializes the output structure.
        let stats = unsafe { stats.assume_init() };
        Ok(u64::from(stats.f_bavail).saturating_mul(stats.f_frsize))
    }

    fn staged_child_directories(&self, path: &Path) -> Result<Vec<PathBuf>, FileSystemError> {
        let mut directories = Vec::new();
        for entry in fs::read_dir(path).map_err(|source| FileSystemError::Io {
            operation: "enumerate staged candidate directories",
            path: path.to_path_buf(),
            source,
        })? {
            let entry = entry.map_err(|source| FileSystemError::Io {
                operation: "enumerate staged candidate directories",
                path: path.to_path_buf(),
                source,
            })?;
            let entry_path = entry.path();
            let metadata =
                fs::symlink_metadata(&entry_path).map_err(|source| FileSystemError::Io {
                    operation: "inspect staged candidate directory",
                    path: entry_path.clone(),
                    source,
                })?;
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                directories.push(entry_path);
            }
        }
        directories.sort_by(|left, right| {
            left.as_os_str()
                .as_bytes()
                .cmp(right.as_os_str().as_bytes())
        });
        Ok(directories)
    }

    fn staged_has_skill_document(
        &self,
        directory: &Path,
        filename: &str,
    ) -> Result<bool, FileSystemError> {
        let path = directory.join(filename);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => Ok(metadata.is_file() || metadata.file_type().is_symlink()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(FileSystemError::Io {
                operation: "inspect staged candidate file",
                path,
                source,
            }),
        }
    }

    fn canonicalize_staged_path(&self, path: &Path) -> Result<PathBuf, FileSystemError> {
        path.canonicalize().map_err(|source| FileSystemError::Io {
            operation: "resolve staged path",
            path: path.to_path_buf(),
            source,
        })
    }

    fn install_staged_skill(
        &self,
        staged_skill_path: &Path,
        final_entity_path: &Path,
        library_root: &Path,
        operation_id: &str,
        expected_staged_tree: &StagedTreeSnapshot,
    ) -> Result<DirectoryFingerprint, FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        let staging_root = library_root.join("staging");
        let skills_root = library_root.join("skills");
        let staged_skill_path = self.normalize_configured_path(staged_skill_path)?;
        let final_entity_path = self.normalize_configured_path(final_entity_path)?;
        if !staged_skill_path.starts_with(&staging_root)
            || final_entity_path.parent() != Some(skills_root.as_path())
        {
            return Err(FileSystemError::InvalidConfiguredPath {
                path: final_entity_path,
            });
        }
        if staged_tree_snapshot_at(&staged_skill_path)? != *expected_staged_tree {
            return Err(FileSystemError::Io {
                operation: "verify staged Skill before install",
                path: staged_skill_path,
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "the staged Skill changed after preview",
                ),
            });
        }
        if fs::symlink_metadata(&final_entity_path).is_ok() {
            return Err(FileSystemError::Io {
                operation: "preflight stable file Install path",
                path: final_entity_path,
                source: std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "the stable Library path is occupied",
                ),
            });
        }
        fs::create_dir_all(&skills_root).map_err(|source| FileSystemError::Io {
            operation: "create Library skills directory",
            path: skills_root.clone(),
            source,
        })?;
        let directory_name = final_entity_path.file_name().ok_or_else(|| {
            FileSystemError::InvalidConfiguredPath {
                path: final_entity_path.clone(),
            }
        })?;
        let temporary_path = skills_root.join(format!(
            ".{}.new-{operation_id}",
            directory_name.to_string_lossy()
        ));
        if fs::symlink_metadata(&temporary_path).is_ok() {
            return Err(FileSystemError::Io {
                operation: "preflight temporary file Install path",
                path: temporary_path,
                source: std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "the temporary Library path is occupied",
                ),
            });
        }
        fs::rename(&staged_skill_path, &temporary_path).map_err(|source| FileSystemError::Io {
            operation: "move staged Skill to temporary Library path",
            path: staged_skill_path.clone(),
            source,
        })?;
        if let Err(source) = fs::rename(&temporary_path, &final_entity_path) {
            if let Err(compensation) = fs::rename(&temporary_path, &staged_skill_path) {
                return Err(FileSystemError::Io {
                    operation: "restore staged Skill after install failure",
                    path: temporary_path,
                    source: compensation,
                });
            }
            return Err(FileSystemError::Io {
                operation: "move staged Skill to stable Library path",
                path: final_entity_path,
                source,
            });
        }
        self.directory_fingerprint(&final_entity_path)
            .map_err(|error| FileSystemError::RecoveryRequired {
                operation: "fingerprint installed stable Skill",
                path: final_entity_path,
                message: error.to_string(),
            })
    }

    fn discard_staging(
        &self,
        staging_operation_root: &Path,
        library_root: &Path,
        expected: Option<&DirectoryFingerprint>,
    ) -> Result<(), FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        let configured_path = self.normalize_configured_path(staging_operation_root)?;
        if configured_path.parent() != Some(library_root.join("staging").as_path()) {
            return Err(FileSystemError::InvalidConfiguredPath {
                path: configured_path,
            });
        }
        let operation_id =
            configured_path
                .file_name()
                .ok_or_else(|| FileSystemError::InvalidConfiguredPath {
                    path: configured_path.clone(),
                })?;
        validate_path_component_bytes(operation_id.as_bytes(), "discard staging operation")?;
        let operation_name =
            CString::new(operation_id.as_bytes()).map_err(|source| FileSystemError::Io {
                operation: "encode staging operation identifier",
                path: configured_path.clone(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidInput, source),
            })?;

        let staging_path = library_root.join("staging");
        let operation_path = staging_path.join(operation_id);
        let (library, _) =
            open_directory_nofollow(&library_root, "open Library root for staging cleanup")?;
        let staging_name = CString::new("staging").expect("static path component");
        let (staging, staging_metadata) = match open_directory_at_nofollow(
            &library,
            &staging_name,
            &staging_path,
            "open staging root for cleanup without following links",
        ) {
            Ok(opened) => opened,
            Err(error) if file_system_error_is_not_found(&error) => return Ok(()),
            Err(error) => return Err(error),
        };
        let operation_metadata = match metadata_at_nofollow(
            &staging,
            &operation_name,
            &operation_path,
            "inspect staging operation for cleanup",
        ) {
            Ok(metadata) => metadata,
            Err(error) if file_system_error_is_not_found(&error) => return Ok(()),
            Err(error) => return Err(error),
        };
        if operation_metadata.st_mode & libc::S_IFMT != libc::S_IFDIR
            || operation_metadata.st_dev != staging_metadata.st_dev
        {
            return Err(FileSystemError::RecoveryRequired {
                operation: "discard staging operation",
                path: operation_path,
                message:
                    "the owned staging entry is not a real directory on the staging filesystem"
                        .into(),
            });
        }
        if let Some(expected) = expected {
            if expected.canonical_path != operation_path
                || expected.device != operation_metadata.st_dev as u64
                || expected.inode != operation_metadata.st_ino
            {
                return Err(FileSystemError::PlanStale {
                    path: operation_path,
                });
            }
        }
        remove_child_directory_at(
            &staging,
            staging_metadata.st_dev,
            operation_metadata.st_ino,
            &operation_name,
            &operation_path,
            "discard staging operation",
        )?;
        sync_descriptor(&staging, &staging_path, "sync cleaned staging root")
    }

    fn discard_installed_skill(
        &self,
        final_entity_path: &Path,
        library_root: &Path,
        expected: &DirectoryFingerprint,
    ) -> Result<(), FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        let skills_root = library_root.join("skills");
        let path = self.normalize_configured_path(final_entity_path)?;
        if path.parent() != Some(skills_root.as_path()) {
            return Err(FileSystemError::InvalidConfiguredPath { path });
        }
        remove_owned_directory_if_present(&path, Some(expected), "roll back file Install")
    }

    fn replace_staged_skill(
        &self,
        staged_skill_path: &Path,
        final_entity_path: &Path,
        library_root: &Path,
        operation_id: &str,
        expected_staged_tree: &StagedTreeSnapshot,
        expected_existing_tree: &StagedTreeSnapshot,
    ) -> Result<FileReplacement, FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        let staging_root = library_root.join("staging");
        let skills_root = library_root.join("skills");
        let staged_skill_path = self.normalize_configured_path(staged_skill_path)?;
        let final_entity_path = self.normalize_configured_path(final_entity_path)?;
        if !staged_skill_path.starts_with(&staging_root)
            || final_entity_path.parent() != Some(skills_root.as_path())
        {
            return Err(FileSystemError::InvalidConfiguredPath {
                path: final_entity_path,
            });
        }
        if staged_tree_snapshot_at(&staged_skill_path)? != *expected_staged_tree {
            return Err(stale_tree_entry(&staged_skill_path));
        }
        if staged_tree_snapshot_at(&final_entity_path)? != *expected_existing_tree {
            return Err(stale_tree_entry(&final_entity_path));
        }
        let directory_name = final_entity_path.file_name().ok_or_else(|| {
            FileSystemError::InvalidConfiguredPath {
                path: final_entity_path.clone(),
            }
        })?;
        let temporary_path = skills_root.join(format!(
            ".{}.new-{operation_id}",
            directory_name.to_string_lossy()
        ));
        let operation_root = library_root.join("operations").join(operation_id);
        let backup_root = operation_root.join("backup");
        let backup_path = backup_root.join(directory_name);
        for path in [&temporary_path, &backup_path] {
            if fs::symlink_metadata(path).is_ok() {
                return Err(FileSystemError::Io {
                    operation: "preflight file reinstall path",
                    path: path.to_path_buf(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "a replacement work path is occupied",
                    ),
                });
            }
        }
        fs::create_dir_all(&backup_root).map_err(|source| FileSystemError::Io {
            operation: "create file reinstall backup directory",
            path: backup_root,
            source,
        })?;
        fs::rename(&staged_skill_path, &temporary_path).map_err(|source| FileSystemError::Io {
            operation: "move staged reinstall to temporary Library path",
            path: staged_skill_path.clone(),
            source,
        })?;
        if staged_tree_snapshot_at(&final_entity_path)? != *expected_existing_tree {
            return Err(stale_tree_entry(&final_entity_path));
        }
        if let Err(source) = fs::rename(&final_entity_path, &backup_path) {
            if let Err(compensation) = fs::rename(&temporary_path, &staged_skill_path) {
                return Err(FileSystemError::Io {
                    operation: "restore staged reinstall after backup failure",
                    path: temporary_path,
                    source: compensation,
                });
            }
            return Err(FileSystemError::Io {
                operation: "back up existing stable Skill",
                path: final_entity_path,
                source,
            });
        }
        let backup_tree = staged_tree_snapshot_at(&backup_path).map_err(|error| {
            FileSystemError::RecoveryRequired {
                operation: "verify file reinstall backup",
                path: backup_path.clone(),
                message: error.to_string(),
            }
        })?;
        ensure_moved_tree_matches(&backup_tree, expected_existing_tree, &backup_path).map_err(
            |error| FileSystemError::RecoveryRequired {
                operation: "verify file reinstall backup",
                path: backup_path.clone(),
                message: error.to_string(),
            },
        )?;
        if let Err(source) = fs::rename(&temporary_path, &final_entity_path) {
            let restore = fs::rename(&backup_path, &final_entity_path);
            let unstage = fs::rename(&temporary_path, &staged_skill_path);
            if let Err(compensation) = restore.and(unstage) {
                return Err(FileSystemError::Io {
                    operation: "restore stable Skill after replacement failure",
                    path: final_entity_path,
                    source: compensation,
                });
            }
            return Err(FileSystemError::Io {
                operation: "move replacement to stable Library path",
                path: final_entity_path,
                source,
            });
        }
        let installed_fingerprint =
            self.directory_fingerprint(&final_entity_path)
                .map_err(|error| FileSystemError::RecoveryRequired {
                    operation: "fingerprint replaced stable Skill",
                    path: final_entity_path.clone(),
                    message: error.to_string(),
                })?;
        let backup_fingerprint = self.directory_fingerprint(&backup_path).map_err(|error| {
            FileSystemError::RecoveryRequired {
                operation: "fingerprint file reinstall backup",
                path: backup_path.clone(),
                message: error.to_string(),
            }
        })?;
        Ok(FileReplacement {
            final_entity_path,
            installed_fingerprint,
            backup_path,
            backup_fingerprint,
            original_tree_snapshot: expected_existing_tree.clone(),
        })
    }

    fn commit_replaced_skill(
        &self,
        replacement: &FileReplacement,
        library_root: &Path,
    ) -> Result<(), FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        let operations_root = library_root.join("operations");
        if !replacement.backup_path.starts_with(&operations_root) {
            return Err(FileSystemError::InvalidConfiguredPath {
                path: replacement.backup_path.clone(),
            });
        }
        remove_owned_directory_if_present(
            &replacement.backup_path,
            Some(&replacement.backup_fingerprint),
            "discard committed file reinstall backup",
        )?;
        remove_empty_backup_directory(&replacement.backup_path, &operations_root)
    }

    fn rollback_replaced_skill(
        &self,
        replacement: &FileReplacement,
        library_root: &Path,
    ) -> Result<(), FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        let skills_root = library_root.join("skills");
        let operations_root = library_root.join("operations");
        if replacement.final_entity_path.parent() != Some(skills_root.as_path())
            || !replacement.backup_path.starts_with(&operations_root)
        {
            return Err(FileSystemError::InvalidConfiguredPath {
                path: replacement.final_entity_path.clone(),
            });
        }
        if !real_directory_exists(&replacement.backup_path)? {
            if real_directory_exists(&replacement.final_entity_path)?
                && staged_tree_snapshot_at(&replacement.final_entity_path)?
                    == replacement.original_tree_snapshot
            {
                return remove_empty_backup_directory(&replacement.backup_path, &operations_root);
            }
            return Err(FileSystemError::RecoveryRequired {
                operation: "roll back file reinstall",
                path: replacement.backup_path.clone(),
                message: "the owned backup is missing and the original entity is not restored"
                    .into(),
            });
        }
        let backup_tree = staged_tree_snapshot_at(&replacement.backup_path)?;
        ensure_moved_tree_matches(
            &backup_tree,
            &replacement.original_tree_snapshot,
            &replacement.backup_path,
        )?;
        remove_owned_directory_if_present(
            &replacement.final_entity_path,
            Some(&replacement.installed_fingerprint),
            "remove failed file reinstall replacement",
        )?;
        ensure_directory_fingerprint(&replacement.backup_path, &replacement.backup_fingerprint)?;
        fs::rename(&replacement.backup_path, &replacement.final_entity_path).map_err(|source| {
            FileSystemError::Io {
                operation: "restore file reinstall backup",
                path: replacement.final_entity_path.clone(),
                source,
            }
        })?;
        if staged_tree_snapshot_at(&replacement.final_entity_path)?
            != replacement.original_tree_snapshot
        {
            return Err(stale_tree_entry(&replacement.final_entity_path));
        }
        remove_empty_backup_directory(&replacement.backup_path, &operations_root)
    }

    fn write_file_import_journal(
        &self,
        library_root: &Path,
        journal: &FileImportJournal,
    ) -> Result<(), FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        validate_operation_id(&journal.operation_id)?;
        let operations_root = library_root.join("operations");
        let operation_root = operations_root.join(&journal.operation_id);
        fs::create_dir_all(&operation_root).map_err(|source| FileSystemError::Io {
            operation: "create file Import operation directory",
            path: operation_root.clone(),
            source,
        })?;
        sync_directory(&operations_root, "sync file Import operations directory")?;
        let journal_path = operation_root.join("journal.json");
        let temporary_path = operation_root.join("journal.tmp");
        let bytes = serde_json::to_vec_pretty(journal).map_err(|source| FileSystemError::Io {
            operation: "serialize file Import journal",
            path: journal_path.clone(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
        })?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary_path)
            .map_err(|source| FileSystemError::Io {
                operation: "open temporary file Import journal",
                path: temporary_path.clone(),
                source,
            })?;
        file.write_all(&bytes)
            .map_err(|source| FileSystemError::Io {
                operation: "write temporary file Import journal",
                path: temporary_path.clone(),
                source,
            })?;
        file.sync_all().map_err(|source| FileSystemError::Io {
            operation: "sync temporary file Import journal",
            path: temporary_path.clone(),
            source,
        })?;
        fs::rename(&temporary_path, &journal_path).map_err(|source| FileSystemError::Io {
            operation: "publish file Import journal",
            path: journal_path.clone(),
            source,
        })?;
        sync_directory(&operation_root, "sync file Import operation directory")
    }

    fn finish_file_import_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        validate_operation_id(operation_id)?;
        let operation_root = library_root.join("operations").join(operation_id);
        let journal_path = operation_root.join("journal.json");
        if journal_path.is_file() {
            let journal_bytes = fs::read(&journal_path).map_err(|source| FileSystemError::Io {
                operation: "read completed file Import journal",
                path: journal_path.clone(),
                source,
            })?;
            let history_root = library_root.join("operation-history");
            fs::create_dir_all(&history_root).map_err(|source| FileSystemError::Io {
                operation: "create file Import operation history",
                path: history_root.clone(),
                source,
            })?;
            let archive_path = history_root.join(format!("{operation_id}.json"));
            let archive_temporary = history_root.join(format!(".{operation_id}.tmp"));
            let mut archive = fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&archive_temporary)
                .map_err(|source| FileSystemError::Io {
                    operation: "open temporary file Import history",
                    path: archive_temporary.clone(),
                    source,
                })?;
            archive
                .write_all(&journal_bytes)
                .map_err(|source| FileSystemError::Io {
                    operation: "write file Import history",
                    path: archive_temporary.clone(),
                    source,
                })?;
            archive.sync_all().map_err(|source| FileSystemError::Io {
                operation: "sync file Import history",
                path: archive_temporary.clone(),
                source,
            })?;
            fs::rename(&archive_temporary, &archive_path).map_err(|source| {
                FileSystemError::Io {
                    operation: "publish file Import history",
                    path: archive_path,
                    source,
                }
            })?;
            sync_directory(&history_root, "sync file Import operation history")?;
        }
        match fs::remove_file(&journal_path) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(FileSystemError::Io {
                    operation: "remove completed file Import journal",
                    path: journal_path,
                    source,
                });
            }
        }
        match fs::remove_dir(&operation_root) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(FileSystemError::Io {
                operation: "remove completed file Import operation directory",
                path: operation_root,
                source,
            }),
        }
    }

    fn recover_file_import_journals(
        &self,
        library_root: &Path,
        baselines: &[FileImportRecoveryBaseline],
    ) -> Result<u32, FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        let operations_root = library_root.join("operations");
        let entries = match fs::read_dir(&operations_root) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                discard_orphaned_staging(&library_root, &["file-import-", "git-import-"])?;
                return Ok(0);
            }
            Err(source) => {
                return Err(FileSystemError::Io {
                    operation: "enumerate file Import recovery journals",
                    path: operations_root,
                    source,
                });
            }
        };
        let mut recovered = 0_u32;
        for entry in entries {
            let entry = entry.map_err(|source| FileSystemError::Io {
                operation: "enumerate file Import recovery journals",
                path: operations_root.clone(),
                source,
            })?;
            let operation_root = entry.path();
            let journal_path = operation_root.join("journal.json");
            if !journal_path.is_file() {
                continue;
            }
            let bytes = fs::read(&journal_path).map_err(|source| FileSystemError::Io {
                operation: "read file Import recovery journal",
                path: journal_path.clone(),
                source,
            })?;
            let journal: FileImportJournal =
                serde_json::from_slice(&bytes).map_err(|source| FileSystemError::Io {
                    operation: "parse file Import recovery journal",
                    path: journal_path.clone(),
                    source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
                })?;
            validate_operation_id(&journal.operation_id)?;
            if operation_root.file_name().and_then(|name| name.to_str())
                != Some(journal.operation_id.as_str())
            {
                return Err(FileSystemError::InvalidConfiguredPath {
                    path: operation_root,
                });
            }
            let catalog_committed = journal.items.iter().all(|item| {
                baselines.iter().any(|baseline| {
                    baseline.skill_id == item.skill_id
                        && baseline.final_entity_path == item.final_entity_path
                        && baseline.recorded_content_hash == item.expected_content_hash
                })
            });
            if matches!(journal.phase, FileImportJournalPhase::Planned) {
                for item in journal.items.iter().rev() {
                    self.recover_planned_file_import_item(
                        &library_root,
                        &journal.operation_id,
                        item,
                    )?;
                }
                self.discard_staging(
                    &journal.staging_operation_root,
                    &library_root,
                    Some(&journal.staging_fingerprint),
                )?;
            } else if catalog_committed {
                for item in &journal.items {
                    if let Some(replacement) = &item.replacement {
                        self.commit_replaced_skill(replacement, &library_root)?;
                    }
                }
                self.discard_staging(
                    &journal.staging_operation_root,
                    &library_root,
                    Some(&journal.staging_fingerprint),
                )?;
            } else {
                for item in journal.items.iter().rev() {
                    if let Some(replacement) = &item.replacement {
                        self.rollback_replaced_skill(replacement, &library_root)?;
                    } else if let Some(fingerprint) = &item.installed_fingerprint {
                        self.discard_installed_skill(
                            &item.final_entity_path,
                            &library_root,
                            fingerprint,
                        )?;
                    } else {
                        self.recover_planned_file_import_item(
                            &library_root,
                            &journal.operation_id,
                            item,
                        )?;
                    }
                }
                self.discard_staging(
                    &journal.staging_operation_root,
                    &library_root,
                    Some(&journal.staging_fingerprint),
                )?;
            }
            self.finish_file_import_journal(&library_root, &journal.operation_id)?;
            recovered = recovered.saturating_add(1);
        }
        discard_orphaned_staging(&library_root, &["file-import-", "git-import-"])?;
        Ok(recovered)
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

    fn create_directory(&self, path: &Path) -> Result<(), FileSystemError> {
        let path = self.normalize_configured_path(path)?;
        if fs::symlink_metadata(&path).is_ok() {
            return Err(FileSystemError::Io {
                operation: "create Agent skills directory",
                path: path.clone(),
                source: std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "the Agent skills directory already exists",
                ),
            });
        }
        fs::create_dir_all(&path).map_err(|source| FileSystemError::Io {
            operation: "create Agent skills directory",
            path,
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

    fn occupant_snapshot(&self, path: &Path) -> Result<OccupantSnapshot, FileSystemError> {
        let path = self.normalize_configured_path(path)?;
        occupant_snapshot_at(&path)
    }

    fn move_occupant_to_backup(
        &self,
        entry_path: &Path,
        backup_path: &Path,
        library_root: &Path,
        expected: &OccupantSnapshot,
    ) -> Result<(), FileSystemError> {
        let entry_path = self.normalize_configured_path(entry_path)?;
        let backup_path = self.normalize_configured_path(backup_path)?;
        let library_root = self.normalize_configured_path(library_root)?;
        let operations_root = library_root.join("operations");
        if !backup_path.starts_with(&operations_root) {
            return Err(FileSystemError::InvalidConfiguredPath { path: backup_path });
        }
        if fs::symlink_metadata(&backup_path).is_ok() {
            return Err(FileSystemError::PlanStale { path: backup_path });
        }
        let current = self.occupant_snapshot(&entry_path)?;
        if current != *expected {
            return Err(FileSystemError::PlanStale { path: entry_path });
        }
        if let Some(parent) = backup_path.parent() {
            fs::create_dir_all(parent).map_err(|source| FileSystemError::Io {
                operation: "create Activation backup directory",
                path: parent.to_path_buf(),
                source,
            })?;
        }
        move_entry_verified(&entry_path, &backup_path, expected)
    }

    fn restore_occupant_from_backup(
        &self,
        backup_path: &Path,
        entry_path: &Path,
        library_root: &Path,
        expected: &OccupantSnapshot,
    ) -> Result<(), FileSystemError> {
        let backup_path = self.normalize_configured_path(backup_path)?;
        let entry_path = self.normalize_configured_path(entry_path)?;
        let library_root = self.normalize_configured_path(library_root)?;
        let operations_root = library_root.join("operations");
        if !backup_path.starts_with(&operations_root) {
            return Err(FileSystemError::InvalidConfiguredPath { path: backup_path });
        }
        match fs::symlink_metadata(&entry_path) {
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => return Err(FileSystemError::PlanStale { path: entry_path }),
            Err(source) => {
                return Err(FileSystemError::Io {
                    operation: "inspect Activation restore destination",
                    path: entry_path,
                    source,
                });
            }
        }
        let backup = self.occupant_snapshot(&backup_path)?;
        if !occupant_kind_matches(&expected.kind, &backup.kind) {
            return Err(FileSystemError::PlanStale {
                path: backup_path.clone(),
            });
        }
        if let Some(parent) = entry_path.parent() {
            fs::create_dir_all(parent).map_err(|source| FileSystemError::Io {
                operation: "create Activation restore parent",
                path: parent.to_path_buf(),
                source,
            })?;
        }
        move_entry_verified(&backup_path, &entry_path, expected)
    }

    fn discard_replace_backup(
        &self,
        backup_path: &Path,
        library_root: &Path,
        expected: &OccupantSnapshot,
    ) -> Result<(), FileSystemError> {
        let backup_path = self.normalize_configured_path(backup_path)?;
        let library_root = self.normalize_configured_path(library_root)?;
        let operations_root = library_root.join("operations");
        if !backup_path.starts_with(&operations_root) {
            return Err(FileSystemError::InvalidConfiguredPath { path: backup_path });
        }
        let metadata = match fs::symlink_metadata(&backup_path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(FileSystemError::Io {
                    operation: "inspect Activation backup",
                    path: backup_path.clone(),
                    source,
                });
            }
        };
        let actual = occupant_snapshot_from_metadata(&backup_path, &metadata)?;
        if !occupant_kind_matches(&expected.kind, &actual.kind) {
            return Err(FileSystemError::RecoveryRequired {
                operation: "discard Activation occupant backup",
                path: backup_path.clone(),
                message: "the backup content no longer matches the recorded occupant".into(),
            });
        }
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            fs::remove_dir_all(&backup_path).map_err(|source| FileSystemError::Io {
                operation: "discard Activation occupant backup",
                path: backup_path.clone(),
                source,
            })
        } else {
            fs::remove_file(&backup_path).map_err(|source| FileSystemError::Io {
                operation: "discard Activation occupant backup",
                path: backup_path.clone(),
                source,
            })
        }
    }

    fn write_activation_replace_journal(
        &self,
        library_root: &Path,
        journal: &ActivationReplaceJournal,
    ) -> Result<(), FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        validate_operation_id(&journal.operation_id)?;
        let operations_root = library_root.join("operations");
        let operation_root = operations_root.join(&journal.operation_id);
        fs::create_dir_all(&operation_root).map_err(|source| FileSystemError::Io {
            operation: "create Activation replace operation directory",
            path: operation_root.clone(),
            source,
        })?;
        sync_directory(
            &operations_root,
            "sync Activation replace operations directory",
        )?;
        let journal_path = operation_root.join("activation-replace-journal.json");
        let temporary_path = operation_root.join("activation-replace-journal.tmp");
        let bytes = serde_json::to_vec_pretty(journal).map_err(|source| FileSystemError::Io {
            operation: "serialize Activation replace journal",
            path: journal_path.clone(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
        })?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary_path)
            .map_err(|source| FileSystemError::Io {
                operation: "open temporary Activation replace journal",
                path: temporary_path.clone(),
                source,
            })?;
        file.write_all(&bytes)
            .map_err(|source| FileSystemError::Io {
                operation: "write Activation replace journal",
                path: temporary_path.clone(),
                source,
            })?;
        file.sync_all().map_err(|source| FileSystemError::Io {
            operation: "sync Activation replace journal",
            path: temporary_path.clone(),
            source,
        })?;
        fs::rename(&temporary_path, &journal_path).map_err(|source| FileSystemError::Io {
            operation: "publish Activation replace journal",
            path: journal_path,
            source,
        })?;
        sync_directory(
            &operation_root,
            "sync Activation replace operation directory",
        )
    }

    fn finish_activation_replace_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        validate_operation_id(operation_id)?;
        let operation_root = library_root.join("operations").join(operation_id);
        let journal_path = operation_root.join("activation-replace-journal.json");
        if journal_path.is_file() {
            let journal_bytes = fs::read(&journal_path).map_err(|source| FileSystemError::Io {
                operation: "read completed Activation replace journal",
                path: journal_path.clone(),
                source,
            })?;
            let history_root = library_root.join("operation-history");
            fs::create_dir_all(&history_root).map_err(|source| FileSystemError::Io {
                operation: "create Activation replace operation history",
                path: history_root.clone(),
                source,
            })?;
            let archive_path = history_root.join(format!("{operation_id}.activation-replace.json"));
            let archive_temporary =
                history_root.join(format!(".{operation_id}.activation-replace.tmp"));
            let mut archive = fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&archive_temporary)
                .map_err(|source| FileSystemError::Io {
                    operation: "open temporary Activation replace history",
                    path: archive_temporary.clone(),
                    source,
                })?;
            archive
                .write_all(&journal_bytes)
                .map_err(|source| FileSystemError::Io {
                    operation: "write Activation replace history",
                    path: archive_temporary.clone(),
                    source,
                })?;
            archive.sync_all().map_err(|source| FileSystemError::Io {
                operation: "sync Activation replace history",
                path: archive_temporary.clone(),
                source,
            })?;
            fs::rename(&archive_temporary, &archive_path).map_err(|source| {
                FileSystemError::Io {
                    operation: "publish Activation replace history",
                    path: archive_path,
                    source,
                }
            })?;
            sync_directory(&history_root, "sync Activation replace operation history")?;
        }
        match fs::remove_file(&journal_path) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(FileSystemError::Io {
                    operation: "remove completed Activation replace journal",
                    path: journal_path,
                    source,
                });
            }
        }
        // The backup entry is consumed by Undo/rollback or discarded before
        // finishing; only an empty backup directory may remain here.
        let backup_dir = operation_root.join("backup");
        match fs::remove_dir(&backup_dir) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) if source.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                return Err(FileSystemError::Io {
                    operation: "remove Activation replace backup directory",
                    path: backup_dir,
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "the Activation replace backup still contains content; refusing to remove it",
                    ),
                });
            }
            Err(source) => {
                return Err(FileSystemError::Io {
                    operation: "remove Activation replace backup directory",
                    path: backup_dir,
                    source,
                });
            }
        }
        match fs::remove_dir(&operation_root) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(FileSystemError::Io {
                operation: "remove completed Activation replace operation directory",
                path: operation_root,
                source,
            }),
        }
    }

    fn recover_activation_replace_journals(
        &self,
        library_root: &Path,
        baselines: &[ActivationRecoveryBaseline],
    ) -> Result<u32, FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        let operations_root = library_root.join("operations");
        let entries = match fs::read_dir(&operations_root) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(source) => {
                return Err(FileSystemError::Io {
                    operation: "enumerate Activation replace recovery journals",
                    path: operations_root,
                    source,
                });
            }
        };
        let mut recovered = 0_u32;
        for entry in entries {
            let entry = entry.map_err(|source| FileSystemError::Io {
                operation: "enumerate Activation replace recovery journals",
                path: operations_root.clone(),
                source,
            })?;
            let operation_root = entry.path();
            let journal_path = operation_root.join("activation-replace-journal.json");
            if !journal_path.is_file() {
                continue;
            }
            let bytes = fs::read(&journal_path).map_err(|source| FileSystemError::Io {
                operation: "read Activation replace recovery journal",
                path: journal_path.clone(),
                source,
            })?;
            let journal: ActivationReplaceJournal =
                serde_json::from_slice(&bytes).map_err(|source| FileSystemError::Io {
                    operation: "parse Activation replace recovery journal",
                    path: journal_path.clone(),
                    source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
                })?;
            validate_operation_id(&journal.operation_id)?;
            if operation_root.file_name().and_then(|name| name.to_str())
                != Some(journal.operation_id.as_str())
            {
                return Err(FileSystemError::InvalidConfiguredPath {
                    path: operation_root,
                });
            }
            match journal.phase {
                ActivationReplacePhase::Applying => {
                    let committed = baselines.iter().any(|baseline| {
                        baseline.skill_id == journal.skill_id
                            && baseline.agent_id == journal.agent_id
                            && baseline.expected_entry_path == journal.entry_path
                            && baseline.expected_target_path == journal.target_path
                    });
                    if committed {
                        self.discard_replace_backup(
                            &journal.backup_path,
                            &library_root,
                            &journal.occupant,
                        )?;
                    } else {
                        rollback_activation_replace(&journal)?;
                    }
                }
                ActivationReplacePhase::Committed => {
                    self.discard_replace_backup(
                        &journal.backup_path,
                        &library_root,
                        &journal.occupant,
                    )?;
                }
                ActivationReplacePhase::Undoing => {
                    complete_activation_replace_undo(&journal)?;
                }
            }
            self.finish_activation_replace_journal(&library_root, &journal.operation_id)?;
            recovered = recovered.saturating_add(1);
        }
        Ok(recovered)
    }

    fn write_relocate_journal(
        &self,
        library_root: &Path,
        journal: &RelocateJournal,
    ) -> Result<(), FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        validate_operation_id(&journal.operation_id)?;
        let operations_root = library_root.join("operations");
        let operation_root = operations_root.join(&journal.operation_id);
        fs::create_dir_all(&operation_root).map_err(|source| FileSystemError::Io {
            operation: "create Link relocation operation directory",
            path: operation_root.clone(),
            source,
        })?;
        sync_directory(
            &operations_root,
            "sync Link relocation operations directory",
        )?;
        let journal_path = operation_root.join("relocate-journal.json");
        let temporary_path = operation_root.join("relocate-journal.tmp");
        let bytes = serde_json::to_vec_pretty(journal).map_err(|source| FileSystemError::Io {
            operation: "serialize Link relocation journal",
            path: journal_path.clone(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
        })?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary_path)
            .map_err(|source| FileSystemError::Io {
                operation: "open temporary Link relocation journal",
                path: temporary_path.clone(),
                source,
            })?;
        file.write_all(&bytes)
            .map_err(|source| FileSystemError::Io {
                operation: "write Link relocation journal",
                path: temporary_path.clone(),
                source,
            })?;
        file.sync_all().map_err(|source| FileSystemError::Io {
            operation: "sync Link relocation journal",
            path: temporary_path.clone(),
            source,
        })?;
        fs::rename(&temporary_path, &journal_path).map_err(|source| FileSystemError::Io {
            operation: "publish Link relocation journal",
            path: journal_path,
            source,
        })?;
        sync_directory(&operation_root, "sync Link relocation operation directory")
    }

    fn finish_relocate_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        validate_operation_id(operation_id)?;
        let operation_root = library_root.join("operations").join(operation_id);
        let journal_path = operation_root.join("relocate-journal.json");
        if journal_path.is_file() {
            let journal_bytes = fs::read(&journal_path).map_err(|source| FileSystemError::Io {
                operation: "read completed Link relocation journal",
                path: journal_path.clone(),
                source,
            })?;
            let history_root = library_root.join("operation-history");
            fs::create_dir_all(&history_root).map_err(|source| FileSystemError::Io {
                operation: "create Link relocation operation history",
                path: history_root.clone(),
                source,
            })?;
            let archive_path = history_root.join(format!("{operation_id}.relocate.json"));
            let archive_temporary = history_root.join(format!(".{operation_id}.relocate.tmp"));
            let mut archive = fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&archive_temporary)
                .map_err(|source| FileSystemError::Io {
                    operation: "open temporary Link relocation history",
                    path: archive_temporary.clone(),
                    source,
                })?;
            archive
                .write_all(&journal_bytes)
                .map_err(|source| FileSystemError::Io {
                    operation: "write Link relocation history",
                    path: archive_temporary.clone(),
                    source,
                })?;
            archive.sync_all().map_err(|source| FileSystemError::Io {
                operation: "sync Link relocation history",
                path: archive_temporary.clone(),
                source,
            })?;
            fs::rename(&archive_temporary, &archive_path).map_err(|source| {
                FileSystemError::Io {
                    operation: "publish Link relocation history",
                    path: archive_path,
                    source,
                }
            })?;
        }
        fs::remove_dir_all(&operation_root).map_err(|source| FileSystemError::Io {
            operation: "discard Link relocation operation directory",
            path: operation_root,
            source,
        })
    }

    /// Replay interrupted Link relocations: a journal whose catalog commit
    /// went through (the Skill's recorded final entity is the new path) rolls
    /// forward — every Activation symlink is rewritten to the new entity;
    /// otherwise it rolls back to each entry's planned initial state.
    fn recover_relocate_journals(
        &self,
        library_root: &Path,
        baselines: &[RelocateRecoveryBaseline],
    ) -> Result<u32, FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        let operations_root = library_root.join("operations");
        let entries = match fs::read_dir(&operations_root) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(source) => {
                return Err(FileSystemError::Io {
                    operation: "enumerate Link relocation recovery journals",
                    path: operations_root,
                    source,
                });
            }
        };
        let mut recovered = 0_u32;
        for entry in entries {
            let entry = entry.map_err(|source| FileSystemError::Io {
                operation: "enumerate Link relocation recovery journals",
                path: operations_root.clone(),
                source,
            })?;
            let operation_root = entry.path();
            let journal_path = operation_root.join("relocate-journal.json");
            if !journal_path.is_file() {
                continue;
            }
            let bytes = fs::read(&journal_path).map_err(|source| FileSystemError::Io {
                operation: "read Link relocation recovery journal",
                path: journal_path.clone(),
                source,
            })?;
            let journal: RelocateJournal =
                serde_json::from_slice(&bytes).map_err(|source| FileSystemError::Io {
                    operation: "parse Link relocation recovery journal",
                    path: journal_path.clone(),
                    source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
                })?;
            validate_operation_id(&journal.operation_id)?;
            if operation_root.file_name().and_then(|name| name.to_str())
                != Some(journal.operation_id.as_str())
            {
                return Err(FileSystemError::InvalidConfiguredPath {
                    path: operation_root,
                });
            }
            match journal.phase {
                RelocateJournalPhase::Applying => {
                    let committed = baselines.iter().any(|baseline| {
                        baseline.skill_id == journal.skill_id
                            && baseline.final_entity_path == journal.new_final_entity_path
                    });
                    if committed {
                        rewrite_relocate_entries(&journal, true)?;
                    } else {
                        rewrite_relocate_entries(&journal, false)?;
                    }
                }
                RelocateJournalPhase::Committed => rewrite_relocate_entries(&journal, true)?,
            }
            self.finish_relocate_journal(&library_root, &journal.operation_id)?;
            recovered = recovered.saturating_add(1);
        }
        Ok(recovered)
    }

    fn write_remove_journal(
        &self,
        library_root: &Path,
        journal: &RemoveJournal,
    ) -> Result<(), FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        validate_operation_id(&journal.operation_id)?;
        let operations_root = library_root.join("operations");
        let operation_root = operations_root.join(&journal.operation_id);
        fs::create_dir_all(&operation_root).map_err(|source| FileSystemError::Io {
            operation: "create Remove operation directory",
            path: operation_root.clone(),
            source,
        })?;
        sync_directory(&operations_root, "sync Remove operations directory")?;
        let journal_path = operation_root.join("remove-journal.json");
        let temporary_path = operation_root.join("remove-journal.tmp");
        let bytes = serde_json::to_vec_pretty(journal).map_err(|source| FileSystemError::Io {
            operation: "serialize Remove journal",
            path: journal_path.clone(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
        })?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary_path)
            .map_err(|source| FileSystemError::Io {
                operation: "open temporary Remove journal",
                path: temporary_path.clone(),
                source,
            })?;
        file.write_all(&bytes)
            .map_err(|source| FileSystemError::Io {
                operation: "write Remove journal",
                path: temporary_path.clone(),
                source,
            })?;
        file.sync_all().map_err(|source| FileSystemError::Io {
            operation: "sync Remove journal",
            path: temporary_path.clone(),
            source,
        })?;
        fs::rename(&temporary_path, &journal_path).map_err(|source| FileSystemError::Io {
            operation: "publish Remove journal",
            path: journal_path,
            source,
        })?;
        sync_directory(&operation_root, "sync Remove operation directory")
    }

    fn finish_remove_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        validate_operation_id(operation_id)?;
        let operation_root = library_root.join("operations").join(operation_id);
        let journal_path = operation_root.join("remove-journal.json");
        if journal_path.is_file() {
            let journal_bytes = fs::read(&journal_path).map_err(|source| FileSystemError::Io {
                operation: "read completed Remove journal",
                path: journal_path.clone(),
                source,
            })?;
            let history_root = library_root.join("operation-history");
            fs::create_dir_all(&history_root).map_err(|source| FileSystemError::Io {
                operation: "create Remove operation history",
                path: history_root.clone(),
                source,
            })?;
            let archive_path = history_root.join(format!("{operation_id}.remove.json"));
            let archive_temporary = history_root.join(format!(".{operation_id}.remove.tmp"));
            let mut archive = fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&archive_temporary)
                .map_err(|source| FileSystemError::Io {
                    operation: "open temporary Remove history",
                    path: archive_temporary.clone(),
                    source,
                })?;
            archive
                .write_all(&journal_bytes)
                .map_err(|source| FileSystemError::Io {
                    operation: "write Remove history",
                    path: archive_temporary.clone(),
                    source,
                })?;
            archive.sync_all().map_err(|source| FileSystemError::Io {
                operation: "sync Remove history",
                path: archive_temporary.clone(),
                source,
            })?;
            fs::rename(&archive_temporary, &archive_path).map_err(|source| {
                FileSystemError::Io {
                    operation: "publish Remove history",
                    path: archive_path,
                    source,
                }
            })?;
        }
        fs::remove_dir_all(&operation_root).map_err(|source| FileSystemError::Io {
            operation: "discard Remove operation directory",
            path: operation_root,
            source,
        })
    }

    /// Replay interrupted Removes: a surviving catalog row rolls back
    /// (restore the backed-up entity, recreate removed Activation symlinks
    /// that were symlinks at plan time); a vanished row rolls forward
    /// (finish deleting the entity and its backup). The journal is archived
    /// either way, preserving the operation audit.
    fn recover_remove_journals(
        &self,
        library_root: &Path,
        baselines: &[RemoveRecoveryBaseline],
    ) -> Result<u32, FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        let operations_root = library_root.join("operations");
        let entries = match fs::read_dir(&operations_root) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(source) => {
                return Err(FileSystemError::Io {
                    operation: "enumerate Remove recovery journals",
                    path: operations_root,
                    source,
                });
            }
        };
        let mut recovered = 0_u32;
        for entry in entries {
            let entry = entry.map_err(|source| FileSystemError::Io {
                operation: "enumerate Remove recovery journals",
                path: operations_root.clone(),
                source,
            })?;
            let operation_root = entry.path();
            let journal_path = operation_root.join("remove-journal.json");
            if !journal_path.is_file() {
                continue;
            }
            let bytes = fs::read(&journal_path).map_err(|source| FileSystemError::Io {
                operation: "read Remove recovery journal",
                path: journal_path.clone(),
                source,
            })?;
            let journal: RemoveJournal =
                serde_json::from_slice(&bytes).map_err(|source| FileSystemError::Io {
                    operation: "parse Remove recovery journal",
                    path: journal_path.clone(),
                    source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
                })?;
            validate_operation_id(&journal.operation_id)?;
            if operation_root.file_name().and_then(|name| name.to_str())
                != Some(journal.operation_id.as_str())
            {
                return Err(FileSystemError::InvalidConfiguredPath {
                    path: operation_root,
                });
            }
            let committed = !baselines
                .iter()
                .any(|baseline| baseline.skill_id == journal.skill_id);
            if committed {
                complete_remove(&journal, &library_root)?;
            } else {
                rollback_remove(&journal)?;
            }
            self.finish_remove_journal(&library_root, &journal.operation_id)?;
            recovered = recovered.saturating_add(1);
        }
        Ok(recovered)
    }

    fn backup_library_entity(
        &self,
        final_entity_path: &Path,
        backup_path: &Path,
        library_root: &Path,
    ) -> Result<DirectoryFingerprint, FileSystemError> {
        let final_entity_path = self.normalize_configured_path(final_entity_path)?;
        let backup_path = self.normalize_configured_path(backup_path)?;
        let library_root = self.normalize_configured_path(library_root)?;
        let skills_root = library_root.join("skills");
        if final_entity_path.parent() != Some(skills_root.as_path()) {
            return Err(FileSystemError::InvalidConfiguredPath {
                path: final_entity_path,
            });
        }
        if !backup_path.starts_with(library_root.join("operations")) {
            return Err(FileSystemError::InvalidConfiguredPath { path: backup_path });
        }
        if fs::symlink_metadata(&backup_path).is_ok() {
            return Err(FileSystemError::PlanStale { path: backup_path });
        }
        if let Some(parent) = backup_path.parent() {
            fs::create_dir_all(parent).map_err(|source| FileSystemError::Io {
                operation: "create Library entity backup directory",
                path: parent.to_path_buf(),
                source,
            })?;
        }
        move_directory_verified(&final_entity_path, &backup_path)?;
        self.directory_fingerprint(&backup_path)
    }

    fn restore_library_entity(
        &self,
        backup_path: &Path,
        final_entity_path: &Path,
        library_root: &Path,
        expected: &DirectoryFingerprint,
    ) -> Result<(), FileSystemError> {
        let backup_path = self.normalize_configured_path(backup_path)?;
        let final_entity_path = self.normalize_configured_path(final_entity_path)?;
        let library_root = self.normalize_configured_path(library_root)?;
        if final_entity_path.parent() != Some(library_root.join("skills").as_path()) {
            return Err(FileSystemError::InvalidConfiguredPath {
                path: final_entity_path,
            });
        }
        if fs::symlink_metadata(&final_entity_path).is_ok() {
            return Err(FileSystemError::PlanStale {
                path: final_entity_path,
            });
        }
        let current = self.directory_fingerprint(&backup_path)?;
        if current != *expected {
            return Err(FileSystemError::PlanStale { path: backup_path });
        }
        move_directory_verified(&backup_path, &final_entity_path)
    }

    fn discard_library_entity_backup(
        &self,
        backup_path: &Path,
        library_root: &Path,
        expected: Option<&DirectoryFingerprint>,
    ) -> Result<(), FileSystemError> {
        let backup_path = self.normalize_configured_path(backup_path)?;
        let library_root = self.normalize_configured_path(library_root)?;
        if !backup_path.starts_with(library_root.join("operations")) {
            return Err(FileSystemError::InvalidConfiguredPath { path: backup_path });
        }
        remove_owned_directory_if_present(&backup_path, expected, "discard Library entity backup")
    }

    fn scan_skills_directory(
        &self,
        path: &Path,
    ) -> Result<Vec<ScannedSkillEntry>, FileSystemError> {
        let path = self.normalize_configured_path(path)?;
        let mut entries = Vec::new();
        let directory = match fs::read_dir(&path) {
            Ok(directory) => directory,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(entries),
            Err(source) => {
                return Err(FileSystemError::Io {
                    operation: "enumerate Adopt scan source",
                    path,
                    source,
                });
            }
        };
        for entry in directory {
            let entry = entry.map_err(|source| FileSystemError::Io {
                operation: "enumerate Adopt scan source",
                path: path.clone(),
                source,
            })?;
            let entry_path = entry.path();
            let metadata =
                fs::symlink_metadata(&entry_path).map_err(|source| FileSystemError::Io {
                    operation: "inspect Adopt scan entry",
                    path: entry_path.clone(),
                    source,
                })?;
            if !metadata.is_dir() && !metadata.file_type().is_symlink() {
                continue;
            }
            let Some(name) = entry_path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let name = name.to_owned();
            match self.inspect_link_source(&entry_path) {
                Ok(snapshot) => entries.push(ScannedSkillEntry {
                    entry_path: entry_path.clone(),
                    name: name.clone(),
                    kind: snapshot.entry_kind,
                    final_entity_path: Some(snapshot.final_entity_path),
                    dangling: false,
                }),
                Err(_) => entries.push(ScannedSkillEntry {
                    entry_path: entry_path.clone(),
                    name,
                    kind: LinkSourceEntryKind::Symlink {
                        target: fs::read_link(&entry_path).unwrap_or_default(),
                    },
                    final_entity_path: None,
                    dangling: true,
                }),
            }
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(entries)
    }

    fn stage_external_directory(
        &self,
        source: &Path,
        staging_destination: &Path,
    ) -> Result<DirectoryFingerprint, FileSystemError> {
        let source = self.normalize_configured_path(source)?;
        let source_metadata =
            fs::symlink_metadata(&source).map_err(|source_error| FileSystemError::Io {
                operation: "inspect Adopt source directory",
                path: source.clone(),
                source: source_error,
            })?;
        if !source_metadata.is_dir() || source_metadata.file_type().is_symlink() {
            return Err(FileSystemError::NotDirectory { path: source });
        }
        let canonical = source
            .canonicalize()
            .map_err(|source_error| FileSystemError::Io {
                operation: "canonicalize Adopt source directory",
                path: source.clone(),
                source: source_error,
            })?;
        if canonical != source {
            return Err(FileSystemError::PlanStale { path: source });
        }
        if let Some(parent) = staging_destination.parent() {
            fs::create_dir_all(parent).map_err(|source_error| FileSystemError::Io {
                operation: "create Adopt staging directory",
                path: parent.to_path_buf(),
                source: source_error,
            })?;
        }
        move_directory_verified(&source, staging_destination)?;
        self.directory_fingerprint(staging_destination)
    }

    fn create_adopt_staging_operation(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<DirectoryFingerprint, FileSystemError> {
        validate_adopt_operation_id(operation_id)?;
        let library_root = self.normalize_configured_path(library_root)?;
        let (library, library_metadata) = open_directory_nofollow(
            &library_root,
            "open Adopt Library root without following links",
        )?;
        let staging_name = CString::new("staging").expect("static path component");
        let staging_path = library_root.join("staging");
        let created_staging = mkdirat_if_missing(
            &library,
            &staging_name,
            &staging_path,
            "create Adopt staging root",
        )?;
        let (staging, staging_metadata) = open_directory_at_nofollow(
            &library,
            &staging_name,
            &staging_path,
            "open Adopt staging root without following links",
        )?;
        if staging_metadata.st_dev != library_metadata.st_dev {
            return Err(FileSystemError::RecoveryRequired {
                operation: "create Adopt staging operation",
                path: staging_path,
                message: "the staging root crosses the owned Library filesystem boundary".into(),
            });
        }
        if created_staging {
            sync_descriptor(&library, &library_root, "sync Adopt Library root")?;
        }

        let operation_name =
            CString::new(operation_id.as_bytes()).map_err(|source| FileSystemError::Io {
                operation: "encode Adopt operation identifier",
                path: PathBuf::from(operation_id),
                source: std::io::Error::new(std::io::ErrorKind::InvalidInput, source),
            })?;
        let operation_path = staging_path.join(operation_id);
        let status =
            unsafe { libc::mkdirat(staging.as_raw_fd(), operation_name.as_ptr(), libc::S_IRWXU) };
        if status != 0 {
            return Err(FileSystemError::Io {
                operation: "create Adopt operation staging directory",
                path: operation_path,
                source: std::io::Error::last_os_error(),
            });
        }
        let (operation, operation_metadata) = open_directory_at_nofollow(
            &staging,
            &operation_name,
            &operation_path,
            "open Adopt operation staging without following links",
        )?;
        if operation_metadata.st_dev != staging_metadata.st_dev {
            return Err(FileSystemError::RecoveryRequired {
                operation: "create Adopt staging operation",
                path: operation_path,
                message: "the operation staging directory crosses a filesystem boundary".into(),
            });
        }
        sync_descriptor(&operation, &operation_path, "sync Adopt operation staging")?;
        sync_descriptor(&staging, &staging_path, "sync Adopt staging root")?;
        Ok(DirectoryFingerprint {
            canonical_path: operation_path,
            device: operation_metadata.st_dev as u64,
            inode: operation_metadata.st_ino,
        })
    }

    fn stage_external_directory_in_adopt_operation(
        &self,
        source: &Path,
        library_root: &Path,
        operation_id: &str,
        directory_name: &str,
        expected_operation_root: &DirectoryFingerprint,
        expected_source: &DirectoryFingerprint,
    ) -> Result<DirectoryFingerprint, FileSystemError> {
        validate_adopt_operation_id(operation_id)?;
        validate_path_component(directory_name, "stage Adopt Skill")?;
        let library_root = self.normalize_configured_path(library_root)?;
        let operation_path = library_root.join("staging").join(operation_id);
        if expected_operation_root.canonical_path != operation_path {
            return Err(FileSystemError::PlanStale {
                path: operation_path,
            });
        }
        let (_staging, operation, _operation_metadata) =
            open_adopt_operation_nofollow(&library_root, operation_id, expected_operation_root)?;

        let source = self.normalize_configured_path(source)?;
        if expected_source.canonical_path != source {
            return Err(FileSystemError::PlanStale { path: source });
        }
        let source_parent =
            source
                .parent()
                .ok_or_else(|| FileSystemError::InvalidConfiguredPath {
                    path: source.clone(),
                })?;
        let source_name =
            source
                .file_name()
                .ok_or_else(|| FileSystemError::InvalidConfiguredPath {
                    path: source.clone(),
                })?;
        let source_name =
            CString::new(source_name.as_bytes()).map_err(|source_error| FileSystemError::Io {
                operation: "encode Adopt source name",
                path: source.clone(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidInput, source_error),
            })?;
        let (source_parent, _) = open_directory_nofollow(
            source_parent,
            "open Adopt source parent without following links",
        )?;
        let source_metadata = metadata_at_nofollow(
            &source_parent,
            &source_name,
            &source,
            "reidentify Adopt source",
        )?;
        if source_metadata.st_mode & libc::S_IFMT != libc::S_IFDIR
            || source_metadata.st_dev as u64 != expected_source.device
            || source_metadata.st_ino != expected_source.inode
        {
            return Err(FileSystemError::PlanStale { path: source });
        }

        let destination_name = CString::new(directory_name.as_bytes()).map_err(|source_error| {
            FileSystemError::Io {
                operation: "encode Adopt staging destination",
                path: PathBuf::from(directory_name),
                source: std::io::Error::new(std::io::ErrorKind::InvalidInput, source_error),
            }
        })?;
        ensure_entry_missing_at(
            &operation,
            &destination_name,
            &operation_path.join(directory_name),
            "preflight Adopt staging destination",
        )?;
        let status = unsafe {
            libc::renameatx_np(
                source_parent.as_raw_fd(),
                source_name.as_ptr(),
                operation.as_raw_fd(),
                destination_name.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        if status == 0 {
            let destination_path = operation_path.join(directory_name);
            let fingerprint = match fingerprint_child_directory(
                &operation,
                &destination_name,
                destination_path.clone(),
                "fingerprint staged Adopt Skill",
            ) {
                Ok(fingerprint) => fingerprint,
                Err(error) => {
                    let restore_status = unsafe {
                        libc::renameatx_np(
                            operation.as_raw_fd(),
                            destination_name.as_ptr(),
                            source_parent.as_raw_fd(),
                            source_name.as_ptr(),
                            libc::RENAME_EXCL,
                        )
                    };
                    if restore_status != 0 {
                        return Err(FileSystemError::RecoveryRequired {
                            operation: "restore Adopt source after staging failure",
                            path: source,
                            message: format!(
                                "staging failed with {error}; compensation also failed: {}",
                                std::io::Error::last_os_error()
                            ),
                        });
                    }
                    return Err(error);
                }
            };
            if fingerprint.device != expected_source.device
                || fingerprint.inode != expected_source.inode
            {
                let restore_status = unsafe {
                    libc::renameatx_np(
                        operation.as_raw_fd(),
                        destination_name.as_ptr(),
                        source_parent.as_raw_fd(),
                        source_name.as_ptr(),
                        libc::RENAME_EXCL,
                    )
                };
                if restore_status != 0 {
                    return Err(FileSystemError::RecoveryRequired {
                        operation: "restore replaced Adopt source after staging",
                        path: source,
                        message: format!(
                            "the staged source identity changed and compensation failed: {}",
                            std::io::Error::last_os_error()
                        ),
                    });
                }
                return Err(FileSystemError::PlanStale { path: source });
            }
            sync_descriptor(
                &source_parent,
                source.parent().expect("source has a parent"),
                "sync Adopt source parent",
            )?;
            sync_descriptor(&operation, &operation_path, "sync Adopt operation staging")?;
            if let Err(error) = ensure_adopt_operation_path_matches(
                &library_root,
                operation_id,
                expected_operation_root,
            ) {
                let restore_status = unsafe {
                    libc::renameatx_np(
                        operation.as_raw_fd(),
                        destination_name.as_ptr(),
                        source_parent.as_raw_fd(),
                        source_name.as_ptr(),
                        libc::RENAME_EXCL,
                    )
                };
                if restore_status != 0 {
                    return Err(FileSystemError::RecoveryRequired {
                        operation: "restore Adopt source after staging path changed",
                        path: source,
                        message: format!(
                            "the staging path changed and source compensation failed: {}",
                            std::io::Error::last_os_error()
                        ),
                    });
                }
                sync_descriptor(
                    &source_parent,
                    source.parent().expect("source has a parent"),
                    "sync restored Adopt source parent",
                )?;
                sync_descriptor(
                    &operation,
                    &operation_path,
                    "sync restored Adopt operation staging",
                )?;
                return Err(error);
            }
            return Ok(fingerprint);
        }
        let rename_error = std::io::Error::last_os_error();
        if rename_error.raw_os_error() != Some(libc::EXDEV) {
            return Err(FileSystemError::Io {
                operation: "move Adopt source into owned staging",
                path: source,
                source: rename_error,
            });
        }

        let destination_path = operation_path.join(directory_name);
        let copied_metadata = copy_directory_verified_to_at(
            &source,
            &operation,
            &destination_name,
            &destination_path,
        )?;
        let cleanup_copy = || {
            remove_child_directory_at(
                &operation,
                copied_metadata.st_dev,
                copied_metadata.st_ino,
                &destination_name,
                &destination_path,
                "discard unsafe Adopt cross-volume copy",
            )
        };
        if let Err(error) = ensure_adopt_operation_path_matches(
            &library_root,
            operation_id,
            expected_operation_root,
        ) {
            let _ = cleanup_copy();
            return Err(error);
        }
        let current_source = metadata_at_nofollow(
            &source_parent,
            &source_name,
            &source,
            "reidentify copied Adopt source",
        )?;
        if current_source.st_mode & libc::S_IFMT != libc::S_IFDIR
            || current_source.st_dev as u64 != expected_source.device
            || current_source.st_ino != expected_source.inode
        {
            let _ = cleanup_copy();
            return Err(FileSystemError::PlanStale { path: source });
        }
        let source_tree = staged_tree_snapshot_at(&source)?;
        let copied_tree = staged_tree_snapshot_at(&destination_path)?;
        if source_tree.content_hash != copied_tree.content_hash
            || source_tree.total_file_bytes != copied_tree.total_file_bytes
        {
            let _ = cleanup_copy();
            return Err(stale_tree_entry(&source));
        }
        if let Err(error) = ensure_adopt_operation_path_matches(
            &library_root,
            operation_id,
            expected_operation_root,
        ) {
            let _ = cleanup_copy();
            return Err(error);
        }
        isolate_copied_adopt_source_at(
            &source_parent,
            &source_name,
            &source,
            operation_id,
            expected_source,
            &copied_tree,
        )?;
        sync_descriptor(&operation, &operation_path, "sync Adopt operation staging")?;
        ensure_adopt_operation_path_matches(&library_root, operation_id, expected_operation_root)?;
        let fingerprint = fingerprint_child_directory(
            &operation,
            &destination_name,
            destination_path,
            "fingerprint staged Adopt Skill",
        )?;
        Ok(fingerprint)
    }

    fn discard_isolated_adopt_source(
        &self,
        source: &Path,
        operation_id: &str,
        expected_source: &DirectoryFingerprint,
    ) -> Result<(), FileSystemError> {
        let expanded_source = self.expand_home(source);
        if !expanded_source.is_absolute() {
            return Err(FileSystemError::InvalidConfiguredPath {
                path: expanded_source,
            });
        }
        let source_name =
            expanded_source
                .file_name()
                .ok_or_else(|| FileSystemError::InvalidConfiguredPath {
                    path: expanded_source.clone(),
                })?;
        validate_path_component_bytes(source_name.as_bytes(), "discard isolated Adopt source")?;
        let parent_path =
            self.normalize_configured_path(expanded_source.parent().ok_or_else(|| {
                FileSystemError::InvalidConfiguredPath {
                    path: expanded_source.clone(),
                }
            })?)?;
        let source = parent_path.join(source_name);
        if expected_source.canonical_path != source {
            return Err(FileSystemError::PlanStale { path: source });
        }
        let (isolated_name, encoded_isolated_name) =
            adopt_isolated_source_name(operation_id, expected_source.inode, &source)?;
        let isolated_path = parent_path.join(isolated_name);
        let (parent, _) = open_directory_nofollow(
            &parent_path,
            "open isolated Adopt cleanup parent without following links",
        )?;
        match metadata_at_nofollow(
            &parent,
            &encoded_isolated_name,
            &isolated_path,
            "inspect isolated Adopt source for cleanup",
        ) {
            Ok(_) => {}
            Err(error) if file_system_error_is_not_found(&error) => return Ok(()),
            Err(error) => return Err(error),
        }
        remove_child_directory_at(
            &parent,
            expected_source.device as libc::dev_t,
            expected_source.inode,
            &encoded_isolated_name,
            &isolated_path,
            "discard isolated Adopt source after durable staging",
        )?;
        sync_descriptor(
            &parent,
            &parent_path,
            "sync discarded isolated Adopt source parent",
        )
    }

    fn restore_external_directory(
        &self,
        source: &Path,
        destination: &Path,
        expected: &DirectoryFingerprint,
    ) -> Result<(), FileSystemError> {
        let source = self.normalize_configured_path(source)?;
        let destination = self.normalize_configured_path(destination)?;
        let current = self.directory_fingerprint(&source)?;
        if current != *expected {
            return Err(FileSystemError::PlanStale { path: source });
        }
        match fs::symlink_metadata(&destination) {
            Err(source_error) if source_error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => return Err(FileSystemError::PlanStale { path: destination }),
            Err(source_error) => {
                return Err(FileSystemError::Io {
                    operation: "inspect Adopt restore destination",
                    path: destination,
                    source: source_error,
                });
            }
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|source_error| FileSystemError::Io {
                operation: "create Adopt restore parent",
                path: parent.to_path_buf(),
                source: source_error,
            })?;
        }
        move_directory_verified(&source, &destination)
    }

    fn write_adopt_journal(
        &self,
        library_root: &Path,
        journal: &AdoptJournal,
    ) -> Result<(), FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        validate_adopt_operation_id(&journal.operation_id)?;
        let operations_root = library_root.join("operations");
        let operation_root = operations_root.join(&journal.operation_id);
        fs::create_dir_all(&operation_root).map_err(|source| FileSystemError::Io {
            operation: "create Adopt operation directory",
            path: operation_root.clone(),
            source,
        })?;
        sync_directory(&operations_root, "sync Adopt operations directory")?;
        let journal_path = operation_root.join("adopt-journal.json");
        let temporary_path = operation_root.join("adopt-journal.tmp");
        let bytes = serde_json::to_vec_pretty(journal).map_err(|source| FileSystemError::Io {
            operation: "serialize Adopt journal",
            path: journal_path.clone(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
        })?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary_path)
            .map_err(|source| FileSystemError::Io {
                operation: "open temporary Adopt journal",
                path: temporary_path.clone(),
                source,
            })?;
        file.write_all(&bytes)
            .map_err(|source| FileSystemError::Io {
                operation: "write Adopt journal",
                path: temporary_path.clone(),
                source,
            })?;
        file.sync_all().map_err(|source| FileSystemError::Io {
            operation: "sync Adopt journal",
            path: temporary_path.clone(),
            source,
        })?;
        fs::rename(&temporary_path, &journal_path).map_err(|source| FileSystemError::Io {
            operation: "publish Adopt journal",
            path: journal_path,
            source,
        })?;
        sync_directory(&operation_root, "sync Adopt operation directory")
    }

    fn finish_adopt_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        validate_adopt_operation_id(operation_id)?;
        let operation_root = library_root.join("operations").join(operation_id);
        let journal_path = operation_root.join("adopt-journal.json");
        if journal_path.is_file() {
            let journal_bytes = fs::read(&journal_path).map_err(|source| FileSystemError::Io {
                operation: "read completed Adopt journal",
                path: journal_path.clone(),
                source,
            })?;
            let history_root = library_root.join("operation-history");
            fs::create_dir_all(&history_root).map_err(|source| FileSystemError::Io {
                operation: "create Adopt operation history",
                path: history_root.clone(),
                source,
            })?;
            let archive_path = history_root.join(format!("{operation_id}.adopt.json"));
            let archive_temporary = history_root.join(format!(".{operation_id}.adopt.tmp"));
            let mut archive = fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&archive_temporary)
                .map_err(|source| FileSystemError::Io {
                    operation: "open temporary Adopt history",
                    path: archive_temporary.clone(),
                    source,
                })?;
            archive
                .write_all(&journal_bytes)
                .map_err(|source| FileSystemError::Io {
                    operation: "write Adopt history",
                    path: archive_temporary.clone(),
                    source,
                })?;
            archive.sync_all().map_err(|source| FileSystemError::Io {
                operation: "sync Adopt history",
                path: archive_temporary.clone(),
                source,
            })?;
            fs::rename(&archive_temporary, &archive_path).map_err(|source| {
                FileSystemError::Io {
                    operation: "publish Adopt history",
                    path: archive_path,
                    source,
                }
            })?;
            sync_directory(&history_root, "sync Adopt operation history")?;
        }
        match fs::remove_file(&journal_path) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(FileSystemError::Io {
                    operation: "remove completed Adopt journal",
                    path: journal_path,
                    source,
                });
            }
        }
        match fs::remove_dir(&operation_root) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(FileSystemError::Io {
                operation: "remove completed Adopt operation directory",
                path: operation_root,
                source,
            }),
        }
    }

    fn recover_adopt_journals(
        &self,
        library_root: &Path,
        baselines: &[FileImportRecoveryBaseline],
        adopted_entities: &[FileImportRecoveryBaseline],
    ) -> Result<u32, FileSystemError> {
        let library_root = self.normalize_configured_path(library_root)?;
        let operations_root = library_root.join("operations");
        let entries = match fs::read_dir(&operations_root) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                reject_orphaned_adopt_staging(&library_root)?;
                return Ok(0);
            }
            Err(source) => {
                return Err(FileSystemError::Io {
                    operation: "enumerate Adopt recovery journals",
                    path: operations_root,
                    source,
                });
            }
        };
        let mut recovered = 0_u32;
        for entry in entries {
            let entry = entry.map_err(|source| FileSystemError::Io {
                operation: "enumerate Adopt recovery journals",
                path: operations_root.clone(),
                source,
            })?;
            let operation_root = entry.path();
            let journal_path = operation_root.join("adopt-journal.json");
            if !journal_path.is_file() {
                continue;
            }
            let bytes = fs::read(&journal_path).map_err(|source| FileSystemError::Io {
                operation: "read Adopt recovery journal",
                path: journal_path.clone(),
                source,
            })?;
            let mut journal: AdoptJournal =
                serde_json::from_slice(&bytes).map_err(|source| FileSystemError::Io {
                    operation: "parse Adopt recovery journal",
                    path: journal_path.clone(),
                    source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
                })?;
            validate_adopt_operation_id(&journal.operation_id)?;
            if operation_root.file_name().and_then(|name| name.to_str())
                != Some(journal.operation_id.as_str())
            {
                return Err(FileSystemError::InvalidConfiguredPath {
                    path: operation_root,
                });
            }
            if !matches!(journal.version, 1 | 2) {
                return Err(FileSystemError::RecoveryRequired {
                    operation: "recover Adopt journal",
                    path: journal_path,
                    message: format!(
                        "unsupported Adopt journal version {}; staging is retained",
                        journal.version
                    ),
                });
            }
            match journal.phase {
                AdoptJournalPhase::Planned => {
                    // Version 1 wrote Planned journals after Preview had
                    // already moved the user's entity. Version 2 never
                    // persists this phase, but the same fail-closed recovery
                    // is safe if such a journal is encountered.
                    for item in &journal.items {
                        self.restore_uncommitted_adopt_item(
                            &library_root,
                            &journal.operation_id,
                            item,
                        )?;
                    }
                }
                AdoptJournalPhase::Applying | AdoptJournalPhase::Committed => {
                    for item in &mut journal.items {
                        let catalog_committed = adopted_entities.iter().any(|entity| {
                            entity.skill_id == item.skill_id
                                && entity.final_entity_path == item.final_entity_path
                                && (item.recorded_content_hash.is_empty()
                                    || baselines.iter().any(|baseline| {
                                        baseline.skill_id == item.skill_id
                                            && baseline.recorded_content_hash
                                                == item.recorded_content_hash
                                    }))
                        });
                        if !catalog_committed {
                            self.restore_uncommitted_adopt_item(
                                &library_root,
                                &journal.operation_id,
                                item,
                            )?;
                            item.installed_fingerprint = None;
                            item.phase = AdoptItemPhase::Planned;
                        } else {
                            self.verify_committed_adopt_entity(item)?;
                            if let Some(source_fingerprint) = &item.source_fingerprint {
                                self.discard_isolated_adopt_source(
                                    &item.original_path,
                                    &journal.operation_id,
                                    source_fingerprint,
                                )?;
                            }
                            self.apply_adopt_appearances(&item.appearances, &item.activations)?;
                            item.phase = AdoptItemPhase::Done;
                        }
                    }
                    self.write_adopt_journal(&library_root, &journal)?;
                }
            }
            let expected_staging = (journal.staging_fingerprint.device != 0
                || journal.staging_fingerprint.inode != 0)
                .then_some(&journal.staging_fingerprint);
            self.discard_staging(
                &journal.staging_operation_root,
                &library_root,
                expected_staging,
            )?;
            self.finish_adopt_journal(&library_root, &journal.operation_id)?;
            recovered = recovered.saturating_add(1);
        }
        reject_orphaned_adopt_staging(&library_root)?;
        Ok(recovered)
    }

    fn apply_adopt_appearances(
        &self,
        appearances: &[AdoptAppearanceStep],
        activations: &[AdoptActivationStep],
    ) -> Result<(), FileSystemError> {
        for appearance in appearances {
            if let AdoptAppearanceKind::Symlink { original_target } = &appearance.kind {
                match self.activation_snapshot(&appearance.entry_path)? {
                    ActivationEntrySnapshot::Missing => {}
                    ActivationEntrySnapshot::Symlink { target } if target == *original_target => {
                        fs::remove_file(&appearance.entry_path).map_err(|source| {
                            FileSystemError::Io {
                                operation: "remove Adopt appearance entry",
                                path: appearance.entry_path.clone(),
                                source,
                            }
                        })?;
                    }
                    ActivationEntrySnapshot::Symlink { target }
                        if activations.iter().any(|activation| {
                            activation.entry_path == appearance.entry_path
                                && activation.target_path == target
                        }) => {}
                    ActivationEntrySnapshot::Symlink { .. } | ActivationEntrySnapshot::Other => {
                        return Err(FileSystemError::RecoveryRequired {
                            operation: "replace Adopt appearance entry",
                            path: appearance.entry_path.clone(),
                            message: "the appearance entry changed while Adopt was interrupted"
                                .into(),
                        });
                    }
                }
            }
        }
        for activation in activations {
            match self.activation_snapshot(&activation.entry_path)? {
                ActivationEntrySnapshot::Symlink { target } if target == activation.target_path => {
                }
                ActivationEntrySnapshot::Missing => {
                    self.create_activation(&activation.target_path, &activation.entry_path)?;
                }
                ActivationEntrySnapshot::Symlink { .. } | ActivationEntrySnapshot::Other => {
                    return Err(FileSystemError::RecoveryRequired {
                        operation: "create Adopt Activation",
                        path: activation.entry_path.clone(),
                        message: "the Activation entry is occupied".into(),
                    });
                }
            }
        }
        Ok(())
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
    let root = path.canonicalize().map_err(|source| FileSystemError::Io {
        operation: "canonicalize Skill before reading SKILL.md",
        path: path.to_path_buf(),
        source,
    })?;
    let skill_document = path.join("SKILL.md");
    let entry_metadata =
        fs::symlink_metadata(&skill_document).map_err(|source| FileSystemError::Io {
            operation: "inspect SKILL.md entry",
            path: skill_document.clone(),
            source,
        })?;
    if !entry_metadata.is_file() && !entry_metadata.file_type().is_symlink() {
        return Err(FileSystemError::Io {
            operation: "read SKILL.md",
            path: skill_document,
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "SKILL.md must be a regular file or a contained relative symlink",
            ),
        });
    }
    let resolved_document =
        skill_document
            .canonicalize()
            .map_err(|source| FileSystemError::Io {
                operation: "resolve SKILL.md",
                path: skill_document.clone(),
                source,
            })?;
    if !resolved_document.starts_with(&root) {
        return Err(FileSystemError::Io {
            operation: "read SKILL.md",
            path: skill_document,
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "SKILL.md resolves outside its Skill root",
            ),
        });
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(&resolved_document)
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
    ensure_entry_unchanged(&skill_document, &entry_metadata)?;
    String::from_utf8(bytes).map_err(|source| FileSystemError::Io {
        operation: "decode SKILL.md",
        path: skill_document,
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
    })
}

fn tree_hash_at(root: &Path) -> Result<String, FileSystemError> {
    Ok(staged_tree_snapshot_at(root)?.content_hash)
}

/// Tree hash that skips files whose name is in `excluded` (SQLite WAL/SHM
/// sidecars are derived artifacts — the recovery manifest must not depend on
/// whether a read-only probe recreated them). Directories and symlinks are
/// never excluded.
fn tree_hash_at_excluding(root: &Path, excluded: &[String]) -> Result<String, FileSystemError> {
    Ok(staged_tree_snapshot_at_filtered(root, excluded)?.content_hash)
}

fn staged_tree_snapshot_at(root: &Path) -> Result<StagedTreeSnapshot, FileSystemError> {
    staged_tree_snapshot_at_filtered(root, &[])
}

fn staged_tree_snapshot_at_filtered(
    root: &Path,
    excluded: &[String],
) -> Result<StagedTreeSnapshot, FileSystemError> {
    let original_root = root.to_path_buf();
    let root_metadata = fs::symlink_metadata(root).map_err(|source| FileSystemError::Io {
        operation: "inspect Skill root",
        path: original_root.clone(),
        source,
    })?;
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err(FileSystemError::NotDirectory {
            path: original_root,
        });
    }
    let root = root.canonicalize().map_err(|source| FileSystemError::Io {
        operation: "canonicalize Skill for tree hash",
        path: root.to_path_buf(),
        source,
    })?;
    let mut entries = Vec::new();
    collect_tree_entries(&root, &root, &mut entries)?;
    entries.sort_by(|left, right| {
        left.as_os_str()
            .as_bytes()
            .cmp(right.as_os_str().as_bytes())
    });

    let mut hasher = Sha256::new();
    let mut snapshot_entries = Vec::with_capacity(entries.len());
    let mut total_file_bytes = 0_u64;
    for relative_path in entries {
        if excluded.iter().any(|name| {
            relative_path
                .file_name()
                .is_some_and(|file_name| file_name == name.as_str())
        }) {
            continue;
        }
        let absolute_path = root.join(&relative_path);
        let metadata =
            fs::symlink_metadata(&absolute_path).map_err(|source| FileSystemError::Io {
                operation: "inspect Skill tree hash entry",
                path: absolute_path.clone(),
                source,
            })?;
        if metadata.file_type().is_symlink() {
            hash_field(&mut hasher, b"symlink");
            hash_field(&mut hasher, relative_path.as_os_str().as_bytes());
            let target = fs::read_link(&absolute_path).map_err(|source| FileSystemError::Io {
                operation: "read Skill tree hash symlink",
                path: absolute_path.clone(),
                source,
            })?;
            hash_field(&mut hasher, target.as_os_str().as_bytes());
            ensure_entry_unchanged(&absolute_path, &metadata)?;
            snapshot_entries.push(StagedTreeEntry {
                relative_path,
                kind: StagedEntryKind::Symlink { target },
                device: metadata.dev(),
                inode: metadata.ino(),
                modified_seconds: metadata.mtime(),
                modified_nanoseconds: metadata.mtime_nsec(),
            });
        } else if metadata.is_dir() {
            hash_field(&mut hasher, b"directory");
            hash_field(&mut hasher, relative_path.as_os_str().as_bytes());
            snapshot_entries.push(StagedTreeEntry {
                relative_path,
                kind: StagedEntryKind::Directory,
                device: metadata.dev(),
                inode: metadata.ino(),
                modified_seconds: metadata.mtime(),
                modified_nanoseconds: metadata.mtime_nsec(),
            });
        } else if metadata.is_file() {
            hash_field(&mut hasher, b"file");
            hash_field(&mut hasher, relative_path.as_os_str().as_bytes());
            hasher.update(metadata.len().to_be_bytes());
            total_file_bytes = total_file_bytes.saturating_add(metadata.len());
            let mut file = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
                .open(&absolute_path)
                .map_err(|source| FileSystemError::Io {
                    operation: "read Skill tree hash file",
                    path: absolute_path.clone(),
                    source,
                })?;
            let opened_metadata = file.metadata().map_err(|source| FileSystemError::Io {
                operation: "inspect opened Skill tree hash file",
                path: absolute_path.clone(),
                source,
            })?;
            if !same_file_version(&metadata, &opened_metadata) {
                return Err(stale_tree_entry(&absolute_path));
            }
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let count = file
                    .read(&mut buffer)
                    .map_err(|source| FileSystemError::Io {
                        operation: "read Skill tree hash file",
                        path: absolute_path.clone(),
                        source,
                    })?;
                if count == 0 {
                    break;
                }
                hasher
                    .write_all(&buffer[..count])
                    .expect("SHA-256 writes cannot fail");
            }
            let finished_metadata = file.metadata().map_err(|source| FileSystemError::Io {
                operation: "reinspect opened Skill tree hash file",
                path: absolute_path.clone(),
                source,
            })?;
            if !same_file_version(&metadata, &finished_metadata) {
                return Err(stale_tree_entry(&absolute_path));
            }
            ensure_entry_unchanged(&absolute_path, &metadata)?;
            snapshot_entries.push(StagedTreeEntry {
                relative_path,
                kind: StagedEntryKind::File {
                    length: metadata.len(),
                },
                device: metadata.dev(),
                inode: metadata.ino(),
                modified_seconds: metadata.mtime(),
                modified_nanoseconds: metadata.mtime_nsec(),
            });
        } else {
            return Err(FileSystemError::Io {
                operation: "hash Skill tree entry",
                path: absolute_path,
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Skill contains an unsupported filesystem entry",
                ),
            });
        }
    }
    let final_root_metadata =
        fs::symlink_metadata(&root).map_err(|source| FileSystemError::Io {
            operation: "reinspect Skill root",
            path: root.clone(),
            source,
        })?;
    if root_metadata.dev() != final_root_metadata.dev()
        || root_metadata.ino() != final_root_metadata.ino()
        || !final_root_metadata.is_dir()
        || final_root_metadata.file_type().is_symlink()
    {
        return Err(stale_tree_entry(&root));
    }
    Ok(StagedTreeSnapshot {
        root: DirectoryFingerprint {
            canonical_path: root,
            device: root_metadata.dev(),
            inode: root_metadata.ino(),
        },
        entries: snapshot_entries,
        content_hash: format!("tree-sha256-v1:{:x}", hasher.finalize()),
        total_file_bytes,
    })
}

fn same_file_version(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.file_type() == right.file_type()
}

fn ensure_entry_unchanged(path: &Path, expected: &fs::Metadata) -> Result<(), FileSystemError> {
    let current = fs::symlink_metadata(path).map_err(|source| FileSystemError::Io {
        operation: "reinspect Skill tree entry",
        path: path.to_path_buf(),
        source,
    })?;
    if same_file_version(expected, &current) {
        Ok(())
    } else {
        Err(stale_tree_entry(path))
    }
}

fn stale_tree_entry(path: &Path) -> FileSystemError {
    FileSystemError::PlanStale {
        path: path.to_path_buf(),
    }
}

fn collect_tree_entries(
    root: &Path,
    directory: &Path,
    entries: &mut Vec<PathBuf>,
) -> Result<(), FileSystemError> {
    for entry in fs::read_dir(directory).map_err(|source| FileSystemError::Io {
        operation: "enumerate Skill tree hash",
        path: directory.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| FileSystemError::Io {
            operation: "enumerate Skill tree hash",
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .expect("enumerated entries remain inside their root")
            .to_path_buf();
        let metadata = fs::symlink_metadata(&path).map_err(|source| FileSystemError::Io {
            operation: "inspect Skill tree hash entry",
            path: path.clone(),
            source,
        })?;
        entries.push(relative);
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            collect_tree_entries(root, &path, entries)?;
        }
    }
    Ok(())
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn remove_owned_directory_if_present(
    path: &Path,
    expected: Option<&DirectoryFingerprint>,
    operation: &'static str,
) -> Result<(), FileSystemError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            if let Some(expected) = expected {
                let canonical_path = path.canonicalize().map_err(|source| FileSystemError::Io {
                    operation,
                    path: path.to_path_buf(),
                    source,
                })?;
                if metadata.dev() != expected.device
                    || metadata.ino() != expected.inode
                    || canonical_path != expected.canonical_path
                {
                    return Err(FileSystemError::Io {
                        operation,
                        path: path.to_path_buf(),
                        source: std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "owned directory identity changed after it was planned",
                        ),
                    });
                }
            }
            fs::remove_dir_all(path).map_err(|source| FileSystemError::Io {
                operation,
                path: path.to_path_buf(),
                source,
            })
        }
        Ok(_) => Err(FileSystemError::Io {
            operation,
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "owned path is not a real directory",
            ),
        }),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(FileSystemError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn move_directory_verified(source: &Path, destination: &Path) -> Result<(), FileSystemError> {
    match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(source_error) if source_error.raw_os_error() == Some(libc::EXDEV) => {
            copy_directory_verified(source, destination)?;
            remove_owned_directory_if_present(source, None, "remove copied Adopt source")?;
            Ok(())
        }
        Err(source_error) => Err(FileSystemError::Io {
            operation: "move Adopt source directory",
            path: source.to_path_buf(),
            source: source_error,
        }),
    }
}

fn occupant_snapshot_at(path: &Path) -> Result<OccupantSnapshot, FileSystemError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| FileSystemError::Io {
        operation: "inspect Activation occupant",
        path: path.to_path_buf(),
        source,
    })?;
    occupant_snapshot_from_metadata(path, &metadata)
}

fn occupant_snapshot_from_metadata(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<OccupantSnapshot, FileSystemError> {
    let kind = if metadata.file_type().is_symlink() {
        OccupantKind::Symlink {
            target: fs::read_link(path).map_err(|source| FileSystemError::Io {
                operation: "read Activation occupant symlink target",
                path: path.to_path_buf(),
                source,
            })?,
        }
    } else if metadata.is_dir() {
        OccupantKind::RealDirectory
    } else if metadata.is_file() {
        OccupantKind::File {
            length: metadata.len(),
        }
    } else {
        return Err(FileSystemError::Io {
            operation: "snapshot Activation occupant",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unsupported Activation occupant entry type",
            ),
        });
    };
    Ok(OccupantSnapshot {
        kind,
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn occupant_kind_matches(expected: &OccupantKind, actual: &OccupantKind) -> bool {
    match (expected, actual) {
        (OccupantKind::RealDirectory, OccupantKind::RealDirectory) => true,
        (OccupantKind::Symlink { target: left }, OccupantKind::Symlink { target: right }) => {
            left == right
        }
        (OccupantKind::File { length: left }, OccupantKind::File { length: right }) => {
            left == right
        }
        _ => false,
    }
}

fn activation_entry_snapshot_at(path: &Path) -> Result<ActivationEntrySnapshot, FileSystemError> {
    match fs::symlink_metadata(path) {
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            Ok(ActivationEntrySnapshot::Missing)
        }
        Err(source) => Err(FileSystemError::Io {
            operation: "inspect Activation entry",
            path: path.to_path_buf(),
            source,
        }),
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(ActivationEntrySnapshot::Symlink {
            target: fs::read_link(path).map_err(|source| FileSystemError::Io {
                operation: "read Activation entry target",
                path: path.to_path_buf(),
                source,
            })?,
        }),
        Ok(_) => Ok(ActivationEntrySnapshot::Other),
    }
}

/// Move an occupant entry (real directory, symlink or file) between the
/// Agent entry and its operation backup: same-volume rename (identity is
/// preserved and re-verified), cross-volume verified copy then delete.
fn move_entry_verified(
    source: &Path,
    destination: &Path,
    expected: &OccupantSnapshot,
) -> Result<(), FileSystemError> {
    match fs::rename(source, destination) {
        Ok(()) => {
            let moved = occupant_snapshot_at(destination)?;
            if &moved != expected {
                return Err(FileSystemError::PlanStale {
                    path: destination.to_path_buf(),
                });
            }
            Ok(())
        }
        Err(source_error) if source_error.raw_os_error() == Some(libc::EXDEV) => {
            copy_occupant_verified(source, destination, expected)?;
            remove_occupant_source(source, expected)?;
            Ok(())
        }
        Err(source_error) => Err(FileSystemError::Io {
            operation: "move Activation occupant",
            path: source.to_path_buf(),
            source: source_error,
        }),
    }
}

fn copy_occupant_verified(
    source: &Path,
    destination: &Path,
    expected: &OccupantSnapshot,
) -> Result<(), FileSystemError> {
    match &expected.kind {
        OccupantKind::RealDirectory => copy_directory_verified(source, destination),
        OccupantKind::Symlink { target } => {
            let current = fs::read_link(source).map_err(|source_error| FileSystemError::Io {
                operation: "read Activation occupant symlink target",
                path: source.to_path_buf(),
                source: source_error,
            })?;
            if &current != target {
                return Err(FileSystemError::PlanStale {
                    path: source.to_path_buf(),
                });
            }
            std::os::unix::fs::symlink(target, destination).map_err(|source_error| {
                FileSystemError::Io {
                    operation: "copy Activation occupant symlink",
                    path: source.to_path_buf(),
                    source: source_error,
                }
            })
        }
        OccupantKind::File { length } => copy_file_verified(source, destination, *length),
    }
}

fn copy_file_verified(
    source: &Path,
    destination: &Path,
    expected_length: u64,
) -> Result<(), FileSystemError> {
    let metadata = fs::symlink_metadata(source).map_err(|source_error| FileSystemError::Io {
        operation: "inspect Activation occupant file",
        path: source.to_path_buf(),
        source: source_error,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != expected_length
    {
        return Err(FileSystemError::PlanStale {
            path: source.to_path_buf(),
        });
    }
    let mut input = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(source)
        .map_err(|source_error| FileSystemError::Io {
            operation: "open Activation occupant file without following links",
            path: source.to_path_buf(),
            source: source_error,
        })?;
    let opened = input
        .metadata()
        .map_err(|source_error| FileSystemError::Io {
            operation: "inspect opened Activation occupant file",
            path: source.to_path_buf(),
            source: source_error,
        })?;
    if opened.dev() != metadata.dev()
        || opened.ino() != metadata.ino()
        || opened.len() != metadata.len()
    {
        return Err(FileSystemError::PlanStale {
            path: source.to_path_buf(),
        });
    }
    let mut output = fs::File::create(destination).map_err(|source_error| FileSystemError::Io {
        operation: "create Activation occupant copy",
        path: destination.to_path_buf(),
        source: source_error,
    })?;
    std::io::copy(&mut input, &mut output).map_err(|source_error| FileSystemError::Io {
        operation: "copy Activation occupant file",
        path: source.to_path_buf(),
        source: source_error,
    })?;
    let finished = input
        .metadata()
        .map_err(|source_error| FileSystemError::Io {
            operation: "reinspect copied Activation occupant file",
            path: source.to_path_buf(),
            source: source_error,
        })?;
    if finished.dev() != metadata.dev()
        || finished.ino() != metadata.ino()
        || finished.len() != metadata.len()
    {
        return Err(FileSystemError::PlanStale {
            path: source.to_path_buf(),
        });
    }
    let copied = fs::symlink_metadata(destination).map_err(|source_error| FileSystemError::Io {
        operation: "inspect Activation occupant copy",
        path: destination.to_path_buf(),
        source: source_error,
    })?;
    if copied.len() != expected_length {
        return Err(FileSystemError::Io {
            operation: "copy Activation occupant file",
            path: destination.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "copied occupant size mismatch",
            ),
        });
    }
    Ok(())
}

fn remove_occupant_source(
    source: &Path,
    expected: &OccupantSnapshot,
) -> Result<(), FileSystemError> {
    let current = occupant_snapshot_at(source)?;
    if !occupant_kind_matches(&expected.kind, &current.kind) {
        return Err(FileSystemError::PlanStale {
            path: source.to_path_buf(),
        });
    }
    if matches!(expected.kind, OccupantKind::RealDirectory) {
        remove_owned_directory_if_present(source, None, "remove copied Activation occupant")
    } else {
        match fs::remove_file(source) {
            Ok(()) => Ok(()),
            Err(source_error) if source_error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source_error) => Err(FileSystemError::Io {
                operation: "remove copied Activation occupant",
                path: source.to_path_buf(),
                source: source_error,
            }),
        }
    }
}

/// Roll an interrupted (uncommitted) replace back: remove the Activation if
/// it exists, restore the occupant from backup. When the crash happened
/// before the occupant moved (backup absent, entry untouched) this is a
/// no-op that simply closes the journal (§6.4: nothing to compensate).
fn rollback_activation_replace(journal: &ActivationReplaceJournal) -> Result<(), FileSystemError> {
    match fs::symlink_metadata(&journal.backup_path) {
        Ok(_) => {
            match activation_entry_snapshot_at(&journal.entry_path)? {
                ActivationEntrySnapshot::Missing => {}
                ActivationEntrySnapshot::Symlink { target } if target == journal.target_path => {
                    fs::remove_file(&journal.entry_path).map_err(|source| FileSystemError::Io {
                        operation: "remove interrupted Activation replace entry",
                        path: journal.entry_path.clone(),
                        source,
                    })?;
                }
                _ => {
                    return Err(FileSystemError::RecoveryRequired {
                        operation: "roll back Activation replace",
                        path: journal.entry_path.clone(),
                        message: "the Activation entry changed while the replace was interrupted"
                            .into(),
                    });
                }
            }
            restore_occupant_verified(journal)
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            // The occupant never moved; verify the entry is still it and
            // finish the journal without touching anything.
            let current = occupant_snapshot_at(&journal.entry_path)?;
            if current == journal.occupant {
                Ok(())
            } else {
                Err(FileSystemError::RecoveryRequired {
                    operation: "roll back Activation replace",
                    path: journal.entry_path.clone(),
                    message: "the entry changed before the replace had applied anything".into(),
                })
            }
        }
        Err(source) => Err(FileSystemError::Io {
            operation: "inspect Activation replace rollback backup",
            path: journal.backup_path.clone(),
            source,
        }),
    }
}

/// Roll a Link relocation forward (`committed` = the catalog recorded the
/// new pointer) or back to each entry's planned initial state. Forward:
/// every entry ends up a symlink to the new entity. Backward: entries
/// already repointed are returned to Missing or to a symlink to the old
/// (possibly dangling) target. Entries that changed externally stop recovery
/// with `RecoveryRequired`.
fn rewrite_relocate_entries(
    journal: &RelocateJournal,
    committed: bool,
) -> Result<(), FileSystemError> {
    for step in &journal.activations {
        let entry = activation_entry_snapshot_at(&step.entry_path)?;
        if committed {
            match entry {
                ActivationEntrySnapshot::Symlink { target } if target == step.new_target_path => {}
                ActivationEntrySnapshot::Missing => create_relocate_entry(
                    &step.new_target_path,
                    &step.entry_path,
                    "forward Link relocation",
                )?,
                ActivationEntrySnapshot::Symlink { target } if target == step.old_target_path => {
                    fs::remove_file(&step.entry_path).map_err(|source| FileSystemError::Io {
                        operation: "remove stale Link relocation entry",
                        path: step.entry_path.clone(),
                        source,
                    })?;
                    create_relocate_entry(
                        &step.new_target_path,
                        &step.entry_path,
                        "forward Link relocation",
                    )?;
                }
                _ => {
                    return Err(FileSystemError::RecoveryRequired {
                        operation: "recover Link relocation",
                        path: step.entry_path.clone(),
                        message: "the Activation entry is occupied by external content".into(),
                    });
                }
            }
            continue;
        }
        match (&step.initial_entry, entry) {
            (RelocateInitialEntry::Missing, ActivationEntrySnapshot::Missing) => {}
            (RelocateInitialEntry::Missing, ActivationEntrySnapshot::Symlink { target })
                if target == step.new_target_path =>
            {
                fs::remove_file(&step.entry_path).map_err(|source| FileSystemError::Io {
                    operation: "roll back Link relocation entry",
                    path: step.entry_path.clone(),
                    source,
                })?;
            }
            (
                RelocateInitialEntry::Symlink { old_target },
                ActivationEntrySnapshot::Symlink { target },
            ) if target == step.new_target_path => {
                fs::remove_file(&step.entry_path).map_err(|source| FileSystemError::Io {
                    operation: "roll back Link relocation entry",
                    path: step.entry_path.clone(),
                    source,
                })?;
                create_relocate_entry(old_target, &step.entry_path, "roll back Link relocation")?;
            }
            (RelocateInitialEntry::Symlink { old_target }, ActivationEntrySnapshot::Missing) => {
                create_relocate_entry(old_target, &step.entry_path, "roll back Link relocation")?
            }
            (
                RelocateInitialEntry::Symlink { old_target },
                ActivationEntrySnapshot::Symlink { target },
            ) if target == *old_target => {}
            _ => {
                return Err(FileSystemError::RecoveryRequired {
                    operation: "recover Link relocation",
                    path: step.entry_path.clone(),
                    message: "the Activation entry changed while the relocation was interrupted"
                        .into(),
                });
            }
        }
    }
    Ok(())
}

fn create_relocate_entry(
    target: &Path,
    entry: &Path,
    operation: &'static str,
) -> Result<(), FileSystemError> {
    std::os::unix::fs::symlink(target, entry).map_err(|source| FileSystemError::Io {
        operation,
        path: entry.to_path_buf(),
        source,
    })
}

/// Roll an interrupted Remove back: restore the backed-up Install entity,
/// then recreate every Activation symlink that existed at plan time. Entries
/// that changed externally stop recovery with `RecoveryRequired`.
fn rollback_remove(journal: &RemoveJournal) -> Result<(), FileSystemError> {
    if let Some(backup_path) = &journal.backup_path {
        if fs::symlink_metadata(backup_path).is_ok() {
            if fs::symlink_metadata(&journal.final_entity_path).is_ok() {
                return Err(FileSystemError::RecoveryRequired {
                    operation: "roll back Remove",
                    path: journal.final_entity_path.clone(),
                    message: "the Library entity path changed while the Remove was interrupted"
                        .into(),
                });
            }
            move_directory_verified(backup_path, &journal.final_entity_path)?;
        }
    }
    for step in &journal.activations {
        match activation_entry_snapshot_at(&step.entry_path)? {
            ActivationEntrySnapshot::Symlink { target } if target == step.target_path => {}
            ActivationEntrySnapshot::Missing
                if step.initial_entry == RemoveInitialEntry::Symlink =>
            {
                create_relocate_entry(&step.target_path, &step.entry_path, "roll back Remove")?;
            }
            ActivationEntrySnapshot::Missing => {}
            _ => {
                return Err(FileSystemError::RecoveryRequired {
                    operation: "roll back Remove",
                    path: step.entry_path.clone(),
                    message: "the Activation entry changed while the Remove was interrupted".into(),
                });
            }
        }
    }
    Ok(())
}

/// Roll an interrupted Remove forward after the catalog commit: delete any
/// entity still at its owned Library path and discard the backup. Link
/// entities are external and never touched.
fn complete_remove(journal: &RemoveJournal, library_root: &Path) -> Result<(), FileSystemError> {
    if journal.source_kind == RemoveSourceKind::Install {
        let skills_root = library_root.join("skills");
        if journal.final_entity_path.parent() == Some(skills_root.as_path()) {
            remove_owned_directory_if_present(
                &journal.final_entity_path,
                None,
                "recover Remove entity",
            )?;
        }
    }
    if let Some(backup_path) = &journal.backup_path {
        remove_owned_directory_if_present(backup_path, None, "recover Remove backup")?;
    }
    Ok(())
}

fn restore_occupant_verified(journal: &ActivationReplaceJournal) -> Result<(), FileSystemError> {
    let backup = occupant_snapshot_at(&journal.backup_path)?;
    if !occupant_kind_matches(&journal.occupant.kind, &backup.kind) {
        return Err(FileSystemError::RecoveryRequired {
            operation: "restore Activation occupant",
            path: journal.backup_path.clone(),
            message: "the occupant backup no longer matches the journal".into(),
        });
    }
    match fs::symlink_metadata(&journal.entry_path) {
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(FileSystemError::RecoveryRequired {
                operation: "restore Activation occupant",
                path: journal.entry_path.clone(),
                message: "the Activation entry is occupied".into(),
            });
        }
        Err(source) => {
            return Err(FileSystemError::Io {
                operation: "inspect Activation restore entry",
                path: journal.entry_path.clone(),
                source,
            });
        }
    }
    move_entry_verified(&journal.backup_path, &journal.entry_path, &journal.occupant)
}

/// Finish an interrupted Undo: the backup either still holds the occupant
/// (restore it) or was already consumed (verify the restored entry).
/// Undo recovery never discards the backup.
fn complete_activation_replace_undo(
    journal: &ActivationReplaceJournal,
) -> Result<(), FileSystemError> {
    match fs::symlink_metadata(&journal.backup_path) {
        Ok(_) => {
            match activation_entry_snapshot_at(&journal.entry_path)? {
                ActivationEntrySnapshot::Missing => {}
                ActivationEntrySnapshot::Symlink { target } if target == journal.target_path => {
                    fs::remove_file(&journal.entry_path).map_err(|source| FileSystemError::Io {
                        operation: "remove interrupted Activation replace Undo entry",
                        path: journal.entry_path.clone(),
                        source,
                    })?;
                }
                _ => {
                    return Err(FileSystemError::RecoveryRequired {
                        operation: "complete Activation replace Undo",
                        path: journal.entry_path.clone(),
                        message: "the entry was externally occupied while Undo was interrupted; the backup is retained"
                            .into(),
                    });
                }
            }
            restore_occupant_verified(journal)
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            let current = occupant_snapshot_at(&journal.entry_path)?;
            if current == journal.occupant {
                Ok(())
            } else {
                Err(FileSystemError::RecoveryRequired {
                    operation: "complete Activation replace Undo",
                    path: journal.entry_path.clone(),
                    message:
                        "the restored occupant cannot be re-identified after Undo was interrupted"
                            .into(),
                })
            }
        }
        Err(source) => Err(FileSystemError::Io {
            operation: "inspect Activation replace Undo backup",
            path: journal.backup_path.clone(),
            source,
        }),
    }
}

fn copy_directory_verified(source: &Path, destination: &Path) -> Result<(), FileSystemError> {
    let copy = (|| {
        fs::create_dir(destination).map_err(|source_error| FileSystemError::Io {
            operation: "create Adopt copy destination",
            path: destination.to_path_buf(),
            source: source_error,
        })?;
        for entry in fs::read_dir(source).map_err(|source_error| FileSystemError::Io {
            operation: "enumerate Adopt copy source",
            path: source.to_path_buf(),
            source: source_error,
        })? {
            let entry = entry.map_err(|source_error| FileSystemError::Io {
                operation: "enumerate Adopt copy source",
                path: source.to_path_buf(),
                source: source_error,
            })?;
            let source_path = entry.path();
            let destination_path = destination.join(entry.file_name());
            let metadata =
                fs::symlink_metadata(&source_path).map_err(|source_error| FileSystemError::Io {
                    operation: "inspect Adopt copy source entry",
                    path: source_path.clone(),
                    source: source_error,
                })?;
            if metadata.file_type().is_symlink() {
                let target =
                    fs::read_link(&source_path).map_err(|source_error| FileSystemError::Io {
                        operation: "read Adopt copy source symlink",
                        path: source_path.clone(),
                        source: source_error,
                    })?;
                std::os::unix::fs::symlink(&target, &destination_path).map_err(|source_error| {
                    FileSystemError::Io {
                        operation: "copy Adopt source symlink",
                        path: source_path,
                        source: source_error,
                    }
                })?;
            } else if metadata.is_dir() {
                copy_directory_verified(&source_path, &destination_path)?;
            } else if metadata.is_file() {
                let mut input = fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
                    .open(&source_path)
                    .map_err(|source_error| FileSystemError::Io {
                        operation: "open Adopt copy source entry without following links",
                        path: source_path.clone(),
                        source: source_error,
                    })?;
                let opened_metadata =
                    input
                        .metadata()
                        .map_err(|source_error| FileSystemError::Io {
                            operation: "inspect opened Adopt copy source entry",
                            path: source_path.clone(),
                            source: source_error,
                        })?;
                if opened_metadata.dev() != metadata.dev()
                    || opened_metadata.ino() != metadata.ino()
                    || opened_metadata.len() != metadata.len()
                {
                    return Err(FileSystemError::PlanStale { path: source_path });
                }
                let mut output = fs::File::create(&destination_path).map_err(|source_error| {
                    FileSystemError::Io {
                        operation: "create Adopt copy destination entry",
                        path: destination_path.clone(),
                        source: source_error,
                    }
                })?;
                std::io::copy(&mut input, &mut output).map_err(|source_error| {
                    FileSystemError::Io {
                        operation: "copy Adopt source entry",
                        path: source_path.clone(),
                        source: source_error,
                    }
                })?;
                let finished_metadata =
                    input
                        .metadata()
                        .map_err(|source_error| FileSystemError::Io {
                            operation: "reinspect copied Adopt source entry",
                            path: source_path.clone(),
                            source: source_error,
                        })?;
                if finished_metadata.dev() != metadata.dev()
                    || finished_metadata.ino() != metadata.ino()
                    || finished_metadata.len() != metadata.len()
                    || finished_metadata.mtime() != metadata.mtime()
                    || finished_metadata.mtime_nsec() != metadata.mtime_nsec()
                {
                    return Err(FileSystemError::PlanStale { path: source_path });
                }
            } else {
                return Err(FileSystemError::Io {
                    operation: "inspect Adopt copy source entry",
                    path: source_path,
                    source: std::io::Error::new(
                        std::io::ErrorKind::Unsupported,
                        "unsupported entry type",
                    ),
                });
            }
        }
        Ok(())
    })();
    if copy.is_err() {
        let _ = fs::remove_dir_all(destination);
    }
    copy
}

fn validate_path_component(
    component: &str,
    operation: &'static str,
) -> Result<(), FileSystemError> {
    validate_path_component_bytes(component.as_bytes(), operation)
}

fn validate_path_component_bytes(
    component: &[u8],
    _operation: &'static str,
) -> Result<(), FileSystemError> {
    if component.is_empty()
        || component == b"."
        || component == b".."
        || component.contains(&b'/')
        || component.contains(&0)
    {
        return Err(FileSystemError::InvalidConfiguredPath {
            path: PathBuf::from(OsString::from_vec(component.to_vec())),
        });
    }
    Ok(())
}

fn open_directory_nofollow(
    path: &Path,
    operation: &'static str,
) -> Result<(OwnedFd, libc::stat), FileSystemError> {
    let encoded =
        CString::new(path.as_os_str().as_bytes()).map_err(|source| FileSystemError::Io {
            operation,
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, source),
        })?;
    let descriptor = unsafe {
        libc::open(
            encoded.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(FileSystemError::Io {
            operation,
            path: path.to_path_buf(),
            source: std::io::Error::last_os_error(),
        });
    }
    // SAFETY: `open` returned a new owned descriptor.
    let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let metadata = directory_descriptor_metadata(&descriptor, path)?;
    Ok((descriptor, metadata))
}

fn open_directory_at_nofollow(
    parent: &OwnedFd,
    name: &CString,
    path: &Path,
    operation: &'static str,
) -> Result<(OwnedFd, libc::stat), FileSystemError> {
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(FileSystemError::Io {
            operation,
            path: path.to_path_buf(),
            source: std::io::Error::last_os_error(),
        });
    }
    // SAFETY: `openat` returned a new owned descriptor.
    let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let metadata = directory_descriptor_metadata(&descriptor, path)?;
    Ok((descriptor, metadata))
}

fn mkdirat_if_missing(
    parent: &OwnedFd,
    name: &CString,
    path: &Path,
    operation: &'static str,
) -> Result<bool, FileSystemError> {
    let status = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), libc::S_IRWXU) };
    if status == 0 {
        return Ok(true);
    }
    let source = std::io::Error::last_os_error();
    if source.kind() == std::io::ErrorKind::AlreadyExists {
        Ok(false)
    } else {
        Err(FileSystemError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        })
    }
}

fn sync_descriptor(
    directory: &OwnedFd,
    path: &Path,
    operation: &'static str,
) -> Result<(), FileSystemError> {
    let status = unsafe { libc::fsync(directory.as_raw_fd()) };
    if status == 0 {
        Ok(())
    } else {
        Err(FileSystemError::Io {
            operation,
            path: path.to_path_buf(),
            source: std::io::Error::last_os_error(),
        })
    }
}

fn file_system_error_is_not_found(error: &FileSystemError) -> bool {
    matches!(
        error,
        FileSystemError::Io { source, .. }
            if source.kind() == std::io::ErrorKind::NotFound
    )
}

fn ensure_entry_missing_at(
    parent: &OwnedFd,
    name: &CString,
    path: &Path,
    operation: &'static str,
) -> Result<(), FileSystemError> {
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    let status = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if status == 0 {
        return Err(FileSystemError::Io {
            operation,
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "the owned destination is already occupied",
            ),
        });
    }
    let source = std::io::Error::last_os_error();
    if source.kind() == std::io::ErrorKind::NotFound {
        Ok(())
    } else {
        Err(FileSystemError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        })
    }
}

fn open_adopt_operation_nofollow(
    library_root: &Path,
    operation_id: &str,
    expected: &DirectoryFingerprint,
) -> Result<(OwnedFd, OwnedFd, libc::stat), FileSystemError> {
    validate_adopt_operation_id(operation_id)?;
    let (library, library_metadata) = open_directory_nofollow(
        library_root,
        "open Adopt Library root without following links",
    )?;
    let staging_name = CString::new("staging").expect("static path component");
    let staging_path = library_root.join("staging");
    let (staging, staging_metadata) = open_directory_at_nofollow(
        &library,
        &staging_name,
        &staging_path,
        "open Adopt staging root without following links",
    )?;
    if staging_metadata.st_dev != library_metadata.st_dev {
        return Err(FileSystemError::RecoveryRequired {
            operation: "open Adopt staging operation",
            path: staging_path,
            message: "the staging root crosses the owned Library filesystem boundary".into(),
        });
    }
    let operation_name =
        CString::new(operation_id.as_bytes()).map_err(|source| FileSystemError::Io {
            operation: "encode Adopt operation identifier",
            path: PathBuf::from(operation_id),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, source),
        })?;
    let operation_path = library_root.join("staging").join(operation_id);
    let (operation, operation_metadata) = open_directory_at_nofollow(
        &staging,
        &operation_name,
        &operation_path,
        "open Adopt operation staging without following links",
    )?;
    if operation_metadata.st_dev != staging_metadata.st_dev
        || expected.canonical_path != operation_path
        || expected.device != operation_metadata.st_dev as u64
        || expected.inode != operation_metadata.st_ino
    {
        return Err(FileSystemError::PlanStale {
            path: operation_path,
        });
    }
    Ok((staging, operation, operation_metadata))
}

fn ensure_adopt_operation_path_matches(
    library_root: &Path,
    operation_id: &str,
    expected: &DirectoryFingerprint,
) -> Result<(), FileSystemError> {
    let _ = open_adopt_operation_nofollow(library_root, operation_id, expected)?;
    Ok(())
}

fn fingerprint_child_directory(
    parent: &OwnedFd,
    name: &CString,
    path: PathBuf,
    operation: &'static str,
) -> Result<DirectoryFingerprint, FileSystemError> {
    let (child, metadata) = open_directory_at_nofollow(parent, name, &path, operation)?;
    let parent_metadata = directory_descriptor_metadata(parent, path.parent().unwrap_or(&path))?;
    if metadata.st_dev != parent_metadata.st_dev {
        return Err(FileSystemError::RecoveryRequired {
            operation,
            path,
            message: "the staged directory crosses the operation filesystem boundary".into(),
        });
    }
    drop(child);
    Ok(DirectoryFingerprint {
        canonical_path: path,
        device: metadata.st_dev as u64,
        inode: metadata.st_ino,
    })
}

fn remove_child_directory_at(
    parent: &OwnedFd,
    expected_device: libc::dev_t,
    expected_inode: libc::ino_t,
    name: &CString,
    path: &Path,
    operation: &'static str,
) -> Result<(), FileSystemError> {
    let metadata = metadata_at_nofollow(parent, name, path, operation)?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFDIR || metadata.st_dev != expected_device {
        return Err(FileSystemError::RecoveryRequired {
            operation,
            path: path.to_path_buf(),
            message: "the owned entry is not a real directory on the expected filesystem".into(),
        });
    }
    if metadata.st_ino != expected_inode {
        return Err(FileSystemError::PlanStale {
            path: path.to_path_buf(),
        });
    }
    let (child, opened) = open_directory_at_nofollow(parent, name, path, operation)?;
    if opened.st_dev != expected_device || opened.st_ino != expected_inode {
        return Err(FileSystemError::PlanStale {
            path: path.to_path_buf(),
        });
    }
    remove_directory_contents_at(&child, opened.st_dev, path, operation, None, None)?;
    let current = metadata_at_nofollow(parent, name, path, operation)?;
    if current.st_dev != expected_device || current.st_ino != expected_inode {
        return Err(FileSystemError::PlanStale {
            path: path.to_path_buf(),
        });
    }
    let status = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) };
    if status == 0 {
        Ok(())
    } else {
        Err(FileSystemError::Io {
            operation,
            path: path.to_path_buf(),
            source: std::io::Error::last_os_error(),
        })
    }
}

fn isolate_copied_adopt_source_at(
    parent: &OwnedFd,
    source_name: &CString,
    source_path: &Path,
    operation_id: &str,
    expected_source: &DirectoryFingerprint,
    expected_tree: &StagedTreeSnapshot,
) -> Result<(), FileSystemError> {
    let (isolated_name, encoded_isolated_name) =
        adopt_isolated_source_name(operation_id, expected_source.inode, source_path)?;
    let parent_path =
        source_path
            .parent()
            .ok_or_else(|| FileSystemError::InvalidConfiguredPath {
                path: source_path.to_path_buf(),
            })?;
    let isolated_path = parent_path.join(&isolated_name);
    ensure_entry_missing_at(
        parent,
        &encoded_isolated_name,
        &isolated_path,
        "preflight isolated Adopt source",
    )?;

    let status = unsafe {
        libc::renameatx_np(
            parent.as_raw_fd(),
            source_name.as_ptr(),
            parent.as_raw_fd(),
            encoded_isolated_name.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if status != 0 {
        return Err(FileSystemError::Io {
            operation: "isolate copied Adopt source",
            path: source_path.to_path_buf(),
            source: std::io::Error::last_os_error(),
        });
    }

    let restore = |reason: FileSystemError| {
        let restore_status = unsafe {
            libc::renameatx_np(
                parent.as_raw_fd(),
                encoded_isolated_name.as_ptr(),
                parent.as_raw_fd(),
                source_name.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        if restore_status != 0 {
            return FileSystemError::RecoveryRequired {
                operation: "restore isolated Adopt source",
                path: isolated_path.clone(),
                message: format!(
                    "isolation validation failed with {reason}; source restoration also failed: {}",
                    std::io::Error::last_os_error()
                ),
            };
        }
        if let Err(error) = sync_descriptor(
            parent,
            parent_path,
            "sync restored isolated Adopt source parent",
        ) {
            return FileSystemError::RecoveryRequired {
                operation: "persist restored isolated Adopt source",
                path: source_path.to_path_buf(),
                message: format!("isolation validation failed with {reason}; {error}"),
            };
        }
        reason
    };

    let isolated = match fingerprint_child_directory(
        parent,
        &encoded_isolated_name,
        isolated_path.clone(),
        "reidentify isolated Adopt source",
    ) {
        Ok(isolated) => isolated,
        Err(error) => return Err(restore(error)),
    };
    if isolated.device != expected_source.device || isolated.inode != expected_source.inode {
        return Err(restore(FileSystemError::PlanStale {
            path: source_path.to_path_buf(),
        }));
    }
    let isolated_tree = match staged_tree_snapshot_at(&isolated_path) {
        Ok(snapshot) => snapshot,
        Err(error) => return Err(restore(error)),
    };
    if isolated_tree.content_hash != expected_tree.content_hash
        || isolated_tree.total_file_bytes != expected_tree.total_file_bytes
    {
        return Err(restore(FileSystemError::PlanStale {
            path: source_path.to_path_buf(),
        }));
    }

    sync_descriptor(parent, parent_path, "sync isolated copied Adopt source")
}

fn adopt_isolated_source_name(
    operation_id: &str,
    source_inode: libc::ino_t,
    error_path: &Path,
) -> Result<(String, CString), FileSystemError> {
    validate_adopt_operation_id(operation_id)?;
    let name = format!(".{operation_id}-source-{source_inode}");
    validate_path_component(&name, "isolate copied Adopt source")?;
    let encoded = CString::new(name.as_bytes()).map_err(|source_error| FileSystemError::Io {
        operation: "encode isolated Adopt source name",
        path: error_path.to_path_buf(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, source_error),
    })?;
    Ok((name, encoded))
}

fn restore_isolated_adopt_source_at(
    original_path: &Path,
    operation_id: &str,
    expected_source: &DirectoryFingerprint,
) -> Result<(), FileSystemError> {
    let parent_path =
        original_path
            .parent()
            .ok_or_else(|| FileSystemError::InvalidConfiguredPath {
                path: original_path.to_path_buf(),
            })?;
    let source_name =
        original_path
            .file_name()
            .ok_or_else(|| FileSystemError::InvalidConfiguredPath {
                path: original_path.to_path_buf(),
            })?;
    let encoded_source_name =
        CString::new(source_name.as_bytes()).map_err(|source_error| FileSystemError::Io {
            operation: "encode isolated Adopt restore destination",
            path: original_path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, source_error),
        })?;
    let (isolated_name, encoded_isolated_name) =
        adopt_isolated_source_name(operation_id, expected_source.inode, original_path)?;
    let isolated_path = parent_path.join(isolated_name);
    let (parent, _) = open_directory_nofollow(
        parent_path,
        "open isolated Adopt source parent without following links",
    )?;
    let isolated_metadata = metadata_at_nofollow(
        &parent,
        &encoded_isolated_name,
        &isolated_path,
        "reidentify isolated Adopt source for recovery",
    )?;
    if isolated_metadata.st_mode & libc::S_IFMT != libc::S_IFDIR
        || isolated_metadata.st_dev as u64 != expected_source.device
        || isolated_metadata.st_ino != expected_source.inode
    {
        return Err(FileSystemError::RecoveryRequired {
            operation: "restore isolated Adopt source",
            path: isolated_path,
            message: "the isolated source identity changed before recovery".into(),
        });
    }
    ensure_entry_missing_at(
        &parent,
        &encoded_source_name,
        original_path,
        "preflight isolated Adopt source restore",
    )?;
    let status = unsafe {
        libc::renameatx_np(
            parent.as_raw_fd(),
            encoded_isolated_name.as_ptr(),
            parent.as_raw_fd(),
            encoded_source_name.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if status != 0 {
        return Err(FileSystemError::Io {
            operation: "restore isolated Adopt source",
            path: isolated_path,
            source: std::io::Error::last_os_error(),
        });
    }
    let restored = fingerprint_child_directory(
        &parent,
        &encoded_source_name,
        original_path.to_path_buf(),
        "verify restored isolated Adopt source",
    )?;
    if restored.device != expected_source.device || restored.inode != expected_source.inode {
        return Err(FileSystemError::RecoveryRequired {
            operation: "verify restored isolated Adopt source",
            path: original_path.to_path_buf(),
            message: "the restored source identity changed".into(),
        });
    }
    sync_descriptor(
        &parent,
        parent_path,
        "sync restored isolated Adopt source parent",
    )
}

fn copy_directory_verified_to_at(
    source: &Path,
    destination_parent: &OwnedFd,
    destination_name: &CString,
    destination_path: &Path,
) -> Result<libc::stat, FileSystemError> {
    let parent_metadata = directory_descriptor_metadata(
        destination_parent,
        destination_path.parent().unwrap_or(destination_path),
    )?;
    let status = unsafe {
        libc::mkdirat(
            destination_parent.as_raw_fd(),
            destination_name.as_ptr(),
            libc::S_IRWXU,
        )
    };
    if status != 0 {
        return Err(FileSystemError::Io {
            operation: "create Adopt cross-volume copy destination",
            path: destination_path.to_path_buf(),
            source: std::io::Error::last_os_error(),
        });
    }
    let (destination, destination_metadata) = open_directory_at_nofollow(
        destination_parent,
        destination_name,
        destination_path,
        "open Adopt cross-volume copy destination",
    )?;
    if destination_metadata.st_dev != parent_metadata.st_dev {
        return Err(FileSystemError::RecoveryRequired {
            operation: "copy Adopt source directory",
            path: destination_path.to_path_buf(),
            message: "the copy destination crosses the owned staging filesystem boundary".into(),
        });
    }
    let copied = (|| {
        copy_directory_contents_verified_to_at(source, &destination, destination_path)?;
        sync_descriptor(
            &destination,
            destination_path,
            "sync Adopt copied directory",
        )
    })();
    if copied.is_err() {
        let _ = remove_child_directory_at(
            destination_parent,
            destination_metadata.st_dev,
            destination_metadata.st_ino,
            destination_name,
            destination_path,
            "discard failed Adopt cross-volume copy",
        );
    }
    copied.map(|()| destination_metadata)
}

fn copy_directory_contents_verified_to_at(
    source: &Path,
    destination: &OwnedFd,
    destination_path: &Path,
) -> Result<(), FileSystemError> {
    for entry in fs::read_dir(source).map_err(|source_error| FileSystemError::Io {
        operation: "enumerate Adopt copy source",
        path: source.to_path_buf(),
        source: source_error,
    })? {
        let entry = entry.map_err(|source_error| FileSystemError::Io {
            operation: "enumerate Adopt copy source",
            path: source.to_path_buf(),
            source: source_error,
        })?;
        let name = entry.file_name();
        validate_path_component_bytes(name.as_bytes(), "copy Adopt source entry")?;
        let encoded_name =
            CString::new(name.as_bytes()).map_err(|source_error| FileSystemError::Io {
                operation: "encode Adopt copy destination entry",
                path: destination_path.join(&name),
                source: std::io::Error::new(std::io::ErrorKind::InvalidInput, source_error),
            })?;
        let source_path = entry.path();
        let copied_path = destination_path.join(&name);
        let metadata =
            fs::symlink_metadata(&source_path).map_err(|source_error| FileSystemError::Io {
                operation: "inspect Adopt copy source entry",
                path: source_path.clone(),
                source: source_error,
            })?;
        if metadata.file_type().is_symlink() {
            let target =
                fs::read_link(&source_path).map_err(|source_error| FileSystemError::Io {
                    operation: "read Adopt copy source symlink",
                    path: source_path.clone(),
                    source: source_error,
                })?;
            let encoded_target =
                CString::new(target.as_os_str().as_bytes()).map_err(|source_error| {
                    FileSystemError::Io {
                        operation: "encode Adopt copy source symlink",
                        path: source_path.clone(),
                        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, source_error),
                    }
                })?;
            let status = unsafe {
                libc::symlinkat(
                    encoded_target.as_ptr(),
                    destination.as_raw_fd(),
                    encoded_name.as_ptr(),
                )
            };
            if status != 0 {
                return Err(FileSystemError::Io {
                    operation: "copy Adopt source symlink",
                    path: copied_path,
                    source: std::io::Error::last_os_error(),
                });
            }
            let finished =
                fs::symlink_metadata(&source_path).map_err(|source_error| FileSystemError::Io {
                    operation: "reinspect Adopt copy source symlink",
                    path: source_path.clone(),
                    source: source_error,
                })?;
            if finished.dev() != metadata.dev()
                || finished.ino() != metadata.ino()
                || fs::read_link(&source_path).map_err(|source_error| FileSystemError::Io {
                    operation: "reread Adopt copy source symlink",
                    path: source_path.clone(),
                    source: source_error,
                })? != target
            {
                return Err(FileSystemError::PlanStale { path: source_path });
            }
        } else if metadata.is_dir() {
            let _ = copy_directory_verified_to_at(
                &source_path,
                destination,
                &encoded_name,
                &copied_path,
            )?;
            let finished =
                fs::symlink_metadata(&source_path).map_err(|source_error| FileSystemError::Io {
                    operation: "reinspect Adopt copy source directory",
                    path: source_path.clone(),
                    source: source_error,
                })?;
            if finished.dev() != metadata.dev() || finished.ino() != metadata.ino() {
                return Err(FileSystemError::PlanStale { path: source_path });
            }
        } else if metadata.is_file() {
            let mut input = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
                .open(&source_path)
                .map_err(|source_error| FileSystemError::Io {
                    operation: "open Adopt copy source entry without following links",
                    path: source_path.clone(),
                    source: source_error,
                })?;
            let opened_metadata = input
                .metadata()
                .map_err(|source_error| FileSystemError::Io {
                    operation: "inspect opened Adopt copy source entry",
                    path: source_path.clone(),
                    source: source_error,
                })?;
            if opened_metadata.dev() != metadata.dev()
                || opened_metadata.ino() != metadata.ino()
                || opened_metadata.len() != metadata.len()
            {
                return Err(FileSystemError::PlanStale { path: source_path });
            }
            let descriptor = unsafe {
                libc::openat(
                    destination.as_raw_fd(),
                    encoded_name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    metadata.mode() & 0o777,
                )
            };
            if descriptor < 0 {
                return Err(FileSystemError::Io {
                    operation: "create Adopt copy destination entry",
                    path: copied_path,
                    source: std::io::Error::last_os_error(),
                });
            }
            // SAFETY: `openat` returned a new owned descriptor.
            let output = unsafe { OwnedFd::from_raw_fd(descriptor) };
            let mut output = fs::File::from(output);
            std::io::copy(&mut input, &mut output).map_err(|source_error| FileSystemError::Io {
                operation: "copy Adopt source entry",
                path: source_path.clone(),
                source: source_error,
            })?;
            output
                .sync_all()
                .map_err(|source_error| FileSystemError::Io {
                    operation: "sync Adopt copy destination entry",
                    path: destination_path.join(&name),
                    source: source_error,
                })?;
            let finished_metadata =
                input
                    .metadata()
                    .map_err(|source_error| FileSystemError::Io {
                        operation: "reinspect copied Adopt source entry",
                        path: source_path.clone(),
                        source: source_error,
                    })?;
            if finished_metadata.dev() != metadata.dev()
                || finished_metadata.ino() != metadata.ino()
                || finished_metadata.len() != metadata.len()
                || finished_metadata.mtime() != metadata.mtime()
                || finished_metadata.mtime_nsec() != metadata.mtime_nsec()
            {
                return Err(FileSystemError::PlanStale { path: source_path });
            }
        } else {
            return Err(FileSystemError::Io {
                operation: "inspect Adopt copy source entry",
                path: source_path,
                source: std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "unsupported entry type",
                ),
            });
        }
    }
    Ok(())
}

fn reject_orphaned_adopt_staging(library_root: &Path) -> Result<(), FileSystemError> {
    let staging_root = library_root.join("staging");
    let entries = match fs::read_dir(&staging_root) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(FileSystemError::Io {
                operation: "enumerate orphaned Adopt staging",
                path: staging_root,
                source,
            });
        }
    };
    for entry in entries {
        let entry = entry.map_err(|source| FileSystemError::Io {
            operation: "enumerate orphaned Adopt staging",
            path: staging_root.clone(),
            source,
        })?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("adopt-"))
        {
            return Err(FileSystemError::RecoveryRequired {
                operation: "recover orphaned Adopt staging",
                path: entry.path(),
                message: "staging has no matching Adopt journal; it is retained because it may contain the only copy of an Untracked Skill"
                    .into(),
            });
        }
    }
    Ok(())
}

fn discard_orphaned_staging(
    library_root: &Path,
    operation_prefixes: &[&str],
) -> Result<(), FileSystemError> {
    let protected_operation_ids = adopt_journal_operation_ids(library_root)?;
    let staging_root = library_root.join("staging");
    let encoded = CString::new(staging_root.as_os_str().as_bytes()).map_err(|source| {
        FileSystemError::Io {
            operation: "encode file Import staging root",
            path: staging_root.clone(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, source),
        }
    })?;
    // The descriptor pins the owned staging root and O_NOFOLLOW rejects a root
    // replaced by a symlink before recovery. All traversal and deletion below is
    // relative to this descriptor, so an intermediate-path swap cannot escape.
    let descriptor = unsafe {
        libc::open(
            encoded.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        let source = std::io::Error::last_os_error();
        if source.kind() == std::io::ErrorKind::NotFound {
            return Ok(());
        }
        return Err(FileSystemError::Io {
            operation: "open file Import staging root without following links",
            path: staging_root,
            source,
        });
    }
    // SAFETY: `open` returned a new owned descriptor.
    let descriptor = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let root_metadata = directory_descriptor_metadata(&descriptor, &staging_root)?;
    remove_directory_contents_at(
        &descriptor,
        root_metadata.st_dev,
        &staging_root,
        "discard orphaned file Import staging",
        Some(operation_prefixes),
        Some(&protected_operation_ids),
    )?;
    let status = unsafe { libc::fsync(descriptor.as_raw_fd()) };
    if status == 0 {
        Ok(())
    } else {
        Err(FileSystemError::Io {
            operation: "sync cleaned file Import staging",
            path: staging_root,
            source: std::io::Error::last_os_error(),
        })
    }
}

fn remove_directory_contents_at(
    directory: &OwnedFd,
    owned_device: libc::dev_t,
    display_path: &Path,
    operation: &'static str,
    operation_prefixes: Option<&[&str]>,
    protected_operation_ids: Option<&[OsString]>,
) -> Result<(), FileSystemError> {
    let names = directory_entry_names(directory, display_path, operation)?;
    for name in names {
        if operation_prefixes.is_some_and(|prefixes| {
            name.to_str()
                .is_none_or(|name| !prefixes.iter().any(|prefix| name.starts_with(prefix)))
        }) {
            continue;
        }
        if protected_operation_ids.is_some_and(|protected| protected.contains(&name)) {
            continue;
        }
        let child_path = display_path.join(&name);
        let encoded = CString::new(name.as_bytes()).map_err(|source| FileSystemError::Io {
            operation,
            path: child_path.clone(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, source),
        })?;
        let metadata = metadata_at_nofollow(directory, &encoded, &child_path, operation)?;
        if metadata.st_mode & libc::S_IFMT == libc::S_IFDIR {
            if metadata.st_dev != owned_device {
                return Err(FileSystemError::Io {
                    operation,
                    path: child_path,
                    source: std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "owned staging cleanup refuses to cross a filesystem boundary",
                    ),
                });
            }
            let child_descriptor = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    encoded.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if child_descriptor < 0 {
                return Err(FileSystemError::Io {
                    operation,
                    path: child_path,
                    source: std::io::Error::last_os_error(),
                });
            }
            // SAFETY: `openat` returned a new owned descriptor.
            let child_descriptor = unsafe { OwnedFd::from_raw_fd(child_descriptor) };
            let opened_metadata = directory_descriptor_metadata(&child_descriptor, &child_path)?;
            if opened_metadata.st_dev != metadata.st_dev
                || opened_metadata.st_ino != metadata.st_ino
            {
                return Err(FileSystemError::PlanStale { path: child_path });
            }
            remove_directory_contents_at(
                &child_descriptor,
                owned_device,
                &child_path,
                operation,
                None,
                None,
            )?;
            let current = metadata_at_nofollow(directory, &encoded, &child_path, operation)?;
            if current.st_dev != opened_metadata.st_dev || current.st_ino != opened_metadata.st_ino
            {
                return Err(FileSystemError::PlanStale { path: child_path });
            }
            let status = unsafe {
                libc::unlinkat(directory.as_raw_fd(), encoded.as_ptr(), libc::AT_REMOVEDIR)
            };
            if status != 0 {
                return Err(FileSystemError::Io {
                    operation,
                    path: child_path,
                    source: std::io::Error::last_os_error(),
                });
            }
        } else {
            let status = unsafe { libc::unlinkat(directory.as_raw_fd(), encoded.as_ptr(), 0) };
            if status != 0 {
                return Err(FileSystemError::Io {
                    operation,
                    path: child_path,
                    source: std::io::Error::last_os_error(),
                });
            }
        }
    }
    Ok(())
}

fn adopt_journal_operation_ids(library_root: &Path) -> Result<Vec<OsString>, FileSystemError> {
    let operations_root = library_root.join("operations");
    let entries = match fs::read_dir(&operations_root) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(FileSystemError::Io {
                operation: "enumerate journals before file Import staging cleanup",
                path: operations_root,
                source,
            });
        }
    };
    let mut protected = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| FileSystemError::Io {
            operation: "enumerate journals before file Import staging cleanup",
            path: operations_root.clone(),
            source,
        })?;
        let marker = entry.path().join("adopt-journal.json");
        match fs::symlink_metadata(&marker) {
            Ok(_) => protected.push(entry.file_name()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(FileSystemError::Io {
                    operation: "inspect Adopt journal before file Import staging cleanup",
                    path: marker,
                    source,
                });
            }
        }
    }
    Ok(protected)
}

fn directory_entry_names(
    directory: &OwnedFd,
    path: &Path,
    operation: &'static str,
) -> Result<Vec<OsString>, FileSystemError> {
    let duplicated = unsafe { libc::dup(directory.as_raw_fd()) };
    if duplicated < 0 {
        return Err(FileSystemError::Io {
            operation,
            path: path.to_path_buf(),
            source: std::io::Error::last_os_error(),
        });
    }
    let stream = unsafe { libc::fdopendir(duplicated) };
    if stream.is_null() {
        let source = std::io::Error::last_os_error();
        unsafe {
            libc::close(duplicated);
        }
        return Err(FileSystemError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        });
    }
    let mut names = Vec::new();
    let read_error = loop {
        // SAFETY: macOS exposes thread-local errno through __error.
        unsafe {
            *libc::__error() = 0;
        }
        let entry = unsafe { libc::readdir(stream) };
        if entry.is_null() {
            let source = std::io::Error::last_os_error();
            break (source.raw_os_error() != Some(0)).then_some(source);
        }
        let bytes = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if bytes != b"." && bytes != b".." {
            names.push(OsString::from_vec(bytes.to_vec()));
        }
    };
    unsafe {
        libc::closedir(stream);
    }
    match read_error {
        Some(source) => Err(FileSystemError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        }),
        None => Ok(names),
    }
}

fn metadata_at_nofollow(
    directory: &OwnedFd,
    name: &CString,
    path: &Path,
    operation: &'static str,
) -> Result<libc::stat, FileSystemError> {
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    let status = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if status == 0 {
        // SAFETY: a zero return from fstatat initializes the output structure.
        Ok(unsafe { metadata.assume_init() })
    } else {
        Err(FileSystemError::Io {
            operation,
            path: path.to_path_buf(),
            source: std::io::Error::last_os_error(),
        })
    }
}

fn directory_descriptor_metadata(
    directory: &OwnedFd,
    path: &Path,
) -> Result<libc::stat, FileSystemError> {
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    let status = unsafe { libc::fstat(directory.as_raw_fd(), metadata.as_mut_ptr()) };
    if status == 0 {
        // SAFETY: a zero return from fstat initializes the output structure.
        Ok(unsafe { metadata.assume_init() })
    } else {
        Err(FileSystemError::Io {
            operation: "inspect owned staging directory descriptor",
            path: path.to_path_buf(),
            source: std::io::Error::last_os_error(),
        })
    }
}

fn ensure_directory_fingerprint(
    path: &Path,
    expected: &DirectoryFingerprint,
) -> Result<(), FileSystemError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| FileSystemError::Io {
        operation: "recheck owned directory",
        path: path.to_path_buf(),
        source,
    })?;
    let canonical_path = path.canonicalize().map_err(|source| FileSystemError::Io {
        operation: "canonicalize owned directory",
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.dev() != expected.device
        || metadata.ino() != expected.inode
        || canonical_path != expected.canonical_path
    {
        return Err(FileSystemError::Io {
            operation: "recheck owned directory",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "owned directory identity changed after it was planned",
            ),
        });
    }
    Ok(())
}

fn real_directory_exists(path: &Path) -> Result<bool, FileSystemError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(FileSystemError::Io {
            operation: "inspect file Import recovery path",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "recovery path is not an owned directory",
            ),
        }),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(FileSystemError::Io {
            operation: "inspect file Import recovery path",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn ensure_directory_identity(
    path: &Path,
    expected: &DirectoryFingerprint,
) -> Result<(), FileSystemError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| FileSystemError::Io {
        operation: "verify restored directory identity",
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.is_dir()
        && !metadata.file_type().is_symlink()
        && metadata.dev() == expected.device
        && metadata.ino() == expected.inode
    {
        Ok(())
    } else {
        Err(FileSystemError::Io {
            operation: "verify restored directory identity",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "restored directory does not match its backup fingerprint",
            ),
        })
    }
}

fn ensure_moved_tree_matches(
    actual: &StagedTreeSnapshot,
    expected: &StagedTreeSnapshot,
    path: &Path,
) -> Result<(), FileSystemError> {
    if actual.root.device == expected.root.device
        && actual.root.inode == expected.root.inode
        && actual.entries == expected.entries
        && actual.content_hash == expected.content_hash
        && actual.total_file_bytes == expected.total_file_bytes
    {
        Ok(())
    } else {
        Err(stale_tree_entry(path))
    }
}

fn remove_empty_backup_directory(
    backup_path: &Path,
    operations_root: &Path,
) -> Result<(), FileSystemError> {
    let backup_root =
        backup_path
            .parent()
            .ok_or_else(|| FileSystemError::InvalidConfiguredPath {
                path: backup_path.to_path_buf(),
            })?;
    let operation_root =
        backup_root
            .parent()
            .ok_or_else(|| FileSystemError::InvalidConfiguredPath {
                path: backup_root.to_path_buf(),
            })?;
    if operation_root.parent() != Some(operations_root) {
        return Err(FileSystemError::InvalidConfiguredPath {
            path: operation_root.to_path_buf(),
        });
    }
    match fs::remove_dir(backup_root) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(FileSystemError::Io {
            operation: "remove empty file reinstall backup directory",
            path: backup_root.to_path_buf(),
            source,
        }),
    }
}

fn validate_operation_id(operation_id: &str) -> Result<(), FileSystemError> {
    let path = Path::new(operation_id);
    let mut components = path.components();
    if operation_id.is_empty()
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(FileSystemError::InvalidConfiguredPath {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn validate_adopt_operation_id(operation_id: &str) -> Result<(), FileSystemError> {
    validate_operation_id(operation_id)?;
    if !operation_id.starts_with("adopt-") {
        return Err(FileSystemError::InvalidConfiguredPath {
            path: PathBuf::from(operation_id),
        });
    }
    Ok(())
}

fn sync_directory(path: &Path, operation: &'static str) -> Result<(), FileSystemError> {
    let directory = fs::File::open(path).map_err(|source| FileSystemError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    })?;
    directory.sync_all().map_err(|source| FileSystemError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_relative_adopt_copy_preserves_the_verified_tree() {
        let root = tempfile::tempdir().expect("temporary descriptor copy root");
        let source = root.path().join("source");
        let destination_parent = root.path().join("destination");
        fs::create_dir_all(source.join("nested")).expect("create source tree");
        fs::create_dir_all(&destination_parent).expect("create destination parent");
        fs::write(source.join("SKILL.md"), "# Descriptor copy\n").expect("write Skill");
        fs::write(source.join("nested/helper.txt"), "helper\n").expect("write nested file");
        std::os::unix::fs::symlink("nested/helper.txt", source.join("helper-link"))
            .expect("create contained relative symlink");
        let expected = staged_tree_snapshot_at(&source).expect("snapshot source tree");
        let (destination, _) =
            open_directory_nofollow(&destination_parent, "open test destination")
                .expect("open destination parent");
        let name = CString::new("copied").expect("static destination name");
        let copied = destination_parent.join("copied");

        copy_directory_verified_to_at(&source, &destination, &name, &copied)
            .expect("descriptor-relative copy");

        let actual = staged_tree_snapshot_at(&copied).expect("snapshot copied tree");
        assert_eq!(actual.content_hash, expected.content_hash);
        assert_eq!(actual.total_file_bytes, expected.total_file_bytes);
        assert_eq!(
            fs::read_link(copied.join("helper-link")).expect("copied symlink"),
            PathBuf::from("nested/helper.txt")
        );
    }

    #[test]
    fn descriptor_relative_delete_rejects_a_replaced_source_directory() {
        let root = tempfile::tempdir().expect("temporary descriptor delete root");
        let parent_path = root.path().join("parent");
        let source = parent_path.join("source");
        let preserved = parent_path.join("preserved");
        fs::create_dir_all(&source).expect("create original source");
        fs::write(source.join("original.txt"), "original\n").expect("write original source");
        let (parent, _) = open_directory_nofollow(&parent_path, "open test source parent")
            .expect("open source parent");
        let name = CString::new("source").expect("static source name");
        let expected = metadata_at_nofollow(
            &parent,
            &name,
            &source,
            "snapshot original source for delete",
        )
        .expect("snapshot original source");

        fs::rename(&source, &preserved).expect("move original source aside");
        fs::create_dir(&source).expect("create replacement source");
        fs::write(source.join("replacement.txt"), "replacement\n")
            .expect("write replacement source");

        let result = remove_child_directory_at(
            &parent,
            expected.st_dev,
            expected.st_ino,
            &name,
            &source,
            "remove expected source",
        );

        assert!(matches!(result, Err(FileSystemError::PlanStale { .. })));
        assert!(source.join("replacement.txt").is_file());
        assert!(preserved.join("original.txt").is_file());
    }

    #[test]
    fn copied_adopt_source_is_restored_when_its_contents_change_before_removal() {
        let root = tempfile::tempdir().expect("temporary copied source root");
        let parent_path = root.path().join("parent");
        let source = parent_path.join("source");
        fs::create_dir_all(&source).expect("create copied source");
        fs::write(source.join("SKILL.md"), "# Original\n").expect("write copied source");
        let expected_tree = staged_tree_snapshot_at(&source).expect("snapshot copied source");
        let (parent, _) = open_directory_nofollow(&parent_path, "open copied source parent")
            .expect("open copied source parent");
        let name = CString::new("source").expect("static source name");
        let metadata =
            metadata_at_nofollow(&parent, &name, &source, "snapshot copied source identity")
                .expect("snapshot copied source identity");
        let expected_source = DirectoryFingerprint {
            canonical_path: source.clone(),
            device: metadata.st_dev as u64,
            inode: metadata.st_ino,
        };

        fs::write(source.join("late.txt"), "must survive\n")
            .expect("change copied source after verification");

        let result = isolate_copied_adopt_source_at(
            &parent,
            &name,
            &source,
            "adopt-test-1",
            &expected_source,
            &expected_tree,
        );

        assert!(matches!(result, Err(FileSystemError::PlanStale { .. })));
        assert_eq!(
            fs::read_to_string(source.join("late.txt")).expect("late content remains"),
            "must survive\n"
        );
    }
}
