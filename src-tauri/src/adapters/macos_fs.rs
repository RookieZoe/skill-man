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
    ActivationEntrySnapshot, AdoptActivationStep, AdoptAppearanceKind, AdoptAppearanceStep,
    AdoptItemPhase, AdoptJournal, AdoptJournalItem, AdoptJournalKind, AdoptJournalPhase,
    DirectoryFingerprint, FileImportJournal, FileImportJournalPhase, FileImportRecoveryBaseline,
    FileReplacement, FileSystem, FileSystemError, LinkSourceEntryKind, LinkSourceHop,
    LinkSourceSnapshot, ScannedSkillEntry, SkillFingerprint, StagedEntryKind, StagedTreeEntry,
    StagedTreeSnapshot,
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

    /// Reverse-compensate an interrupted, not-yet-committed Adopt item: move
    /// the migrated entity (staged or installed) back to its real-directory
    /// appearance. Link items never moved anything.
    fn restore_uncommitted_adopt_item(
        &self,
        item: &AdoptJournalItem,
    ) -> Result<(), FileSystemError> {
        if !matches!(item.kind, AdoptJournalKind::Migrate) {
            return Ok(());
        }
        let Some(real_appearance) = item
            .appearances
            .iter()
            .find(|appearance| matches!(appearance.kind, AdoptAppearanceKind::RealDirectory))
        else {
            return Ok(());
        };
        let (source, expected) = match &item.installed_fingerprint {
            Some(fingerprint) => (&item.final_entity_path, fingerprint),
            None => (&item.staged_root, &item.staged_fingerprint),
        };
        match fs::symlink_metadata(source) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Ok(_) => {}
            Err(error) => {
                return Err(FileSystemError::Io {
                    operation: "inspect interrupted Adopt entity",
                    path: source.clone(),
                    source: error,
                });
            }
        }
        self.restore_external_directory(source, &real_appearance.entry_path, expected)
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
        let staging_root = library_root.join("staging");
        let path = self.normalize_configured_path(staging_operation_root)?;
        if path.parent() != Some(staging_root.as_path()) {
            return Err(FileSystemError::InvalidConfiguredPath { path });
        }
        remove_owned_directory_if_present(&path, expected, "discard file Import staging")
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
                discard_orphaned_staging(&library_root)?;
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
        discard_orphaned_staging(&library_root)?;
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

    fn remove_activation(&self, entry_path: &Path) -> Result<(), FileSystemError> {
        fs::remove_file(entry_path).map_err(|source| FileSystemError::Io {
            operation: "remove Activation",
            path: entry_path.to_path_buf(),
            source,
        })
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
        validate_operation_id(&journal.operation_id)?;
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
        validate_operation_id(operation_id)?;
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
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(0),
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
            validate_operation_id(&journal.operation_id)?;
            if operation_root.file_name().and_then(|name| name.to_str())
                != Some(journal.operation_id.as_str())
            {
                return Err(FileSystemError::InvalidConfiguredPath {
                    path: operation_root,
                });
            }
            match journal.phase {
                AdoptJournalPhase::Planned => {
                    // The plan-time staging move already relocated the user's
                    // entity; reverse-compensate instead of deleting (§6.4).
                    for item in &journal.items {
                        self.restore_uncommitted_adopt_item(item)?;
                    }
                }
                AdoptJournalPhase::Committed => {}
                AdoptJournalPhase::Applying => {
                    for item in &mut journal.items {
                        match item.phase {
                            AdoptItemPhase::Staged | AdoptItemPhase::Done => {}
                            AdoptItemPhase::EntityInstalled
                            | AdoptItemPhase::CatalogCommitted
                            | AdoptItemPhase::AppearancesApplied => {
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
                                    self.restore_uncommitted_adopt_item(item)?;
                                    item.installed_fingerprint = None;
                                    item.phase = AdoptItemPhase::Staged;
                                } else {
                                    self.apply_adopt_appearances(
                                        &item.appearances,
                                        &item.activations,
                                    )?;
                                    item.phase = AdoptItemPhase::Done;
                                }
                            }
                        }
                    }
                    self.write_adopt_journal(&library_root, &journal)?;
                }
            }
            self.discard_staging(
                &journal.staging_operation_root,
                &library_root,
                Some(&journal.staging_fingerprint),
            )?;
            self.finish_adopt_journal(&library_root, &journal.operation_id)?;
            recovered = recovered.saturating_add(1);
        }
        discard_orphaned_staging(&library_root)?;
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

fn staged_tree_snapshot_at(root: &Path) -> Result<StagedTreeSnapshot, FileSystemError> {
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

fn discard_orphaned_staging(library_root: &Path) -> Result<(), FileSystemError> {
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
) -> Result<(), FileSystemError> {
    let names = directory_entry_names(directory, display_path, operation)?;
    for name in names {
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
            remove_directory_contents_at(&child_descriptor, owned_device, &child_path, operation)?;
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
