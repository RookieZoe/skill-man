use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActivationEntrySnapshot {
    Missing,
    Symlink { target: PathBuf },
    Other,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DirectoryFingerprint {
    pub canonical_path: PathBuf,
    pub device: u64,
    pub inode: u64,
}

/// One immediate directory entry, as listed by `FileSystem::list_directory`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectoryEntry {
    pub name: String,
    pub is_directory: bool,
    pub len: u64,
}

/// The content occupying an Activation entry when Remove-then-replace is
/// planned. The snapshot identifies the entry before it is moved to backup;
/// same-volume moves preserve device+inode so the moved object can be
/// re-identified at Undo and during startup recovery.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OccupantKind {
    RealDirectory,
    Symlink { target: PathBuf },
    File { length: u64 },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct OccupantSnapshot {
    pub kind: OccupantKind,
    pub device: u64,
    pub inode: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationReplacePhase {
    /// The occupant has been moved to backup (or the move is pending); the
    /// catalog write decides whether recovery rolls forward or back.
    Applying,
    /// The replace succeeded; the backup is retained only while the result
    /// window is open, then discarded.
    Committed,
    /// Undo is in progress: the Activation may already be removed. Recovery
    /// completes the restore and never discards the backup.
    Undoing,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ActivationReplaceJournal {
    pub version: u32,
    pub operation_id: String,
    pub phase: ActivationReplacePhase,
    pub skill_id: String,
    pub agent_id: String,
    pub entry_path: PathBuf,
    pub target_path: PathBuf,
    pub backup_path: PathBuf,
    pub occupant: OccupantSnapshot,
}

/// A desired Activation as recorded in the catalog; used at startup to decide
/// whether an interrupted Remove-then-replace had committed its catalog write.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActivationRecoveryBaseline {
    pub skill_id: String,
    pub agent_id: String,
    pub expected_entry_path: PathBuf,
    pub expected_target_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillFingerprint {
    pub directory: DirectoryFingerprint,
    pub document_device: u64,
    pub document_inode: u64,
    pub document_length: u64,
    pub document_modified_seconds: i64,
    pub document_modified_nanoseconds: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum StagedEntryKind {
    Directory,
    File { length: u64 },
    Symlink { target: PathBuf },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StagedTreeEntry {
    pub relative_path: PathBuf,
    pub kind: StagedEntryKind,
    pub device: u64,
    pub inode: u64,
    pub modified_seconds: i64,
    pub modified_nanoseconds: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StagedTreeSnapshot {
    pub root: DirectoryFingerprint,
    pub entries: Vec<StagedTreeEntry>,
    pub content_hash: String,
    pub total_file_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileReplacement {
    pub final_entity_path: PathBuf,
    pub installed_fingerprint: DirectoryFingerprint,
    pub backup_path: PathBuf,
    pub backup_fingerprint: DirectoryFingerprint,
    pub original_tree_snapshot: StagedTreeSnapshot,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileImportJournalPhase {
    Planned,
    FileSystemApplied,
    CatalogCommitted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileImportJournalItem {
    pub skill_id: String,
    pub final_entity_path: PathBuf,
    pub expected_content_hash: String,
    pub staged_root_fingerprint: DirectoryFingerprint,
    pub replacement_planned: bool,
    #[serde(default)]
    pub replacement_original_tree: Option<StagedTreeSnapshot>,
    pub installed_fingerprint: Option<DirectoryFingerprint>,
    pub replacement: Option<FileReplacement>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileImportJournal {
    pub version: u32,
    pub operation_id: String,
    pub phase: FileImportJournalPhase,
    pub staging_operation_root: PathBuf,
    pub staging_fingerprint: DirectoryFingerprint,
    pub items: Vec<FileImportJournalItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileImportRecoveryBaseline {
    pub skill_id: String,
    pub final_entity_path: PathBuf,
    pub recorded_content_hash: String,
}

/// The persisted Skill pointer at startup; relocation recovery decides
/// whether the interrupted operation had committed by comparing the recorded
/// final entity against the journal's new path.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RelocateRecoveryBaseline {
    pub skill_id: String,
    pub final_entity_path: PathBuf,
}

/// What the Activation entry was when the Remove was planned: entries that
/// were already missing are never recreated by rollback/compensation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoveInitialEntry {
    Missing,
    Symlink,
}

/// One desired Activation removed while removing a Skill from the Library.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RemoveActivationStep {
    pub agent_id: String,
    pub entry_path: PathBuf,
    pub target_path: PathBuf,
    pub initial_entry: RemoveInitialEntry,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoveSourceKind {
    /// The entity lives outside the Library; removal never touches it.
    Link,
    /// The entity is owned by the Library; removal backs it up, then
    /// discards the backup after the catalog commit.
    Install,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoveJournalPhase {
    /// Activations may already be removed and the entity may already be
    /// backed up; the catalog delete decides whether recovery rolls forward
    /// or back.
    Applying,
    /// The catalog row is gone; only filesystem cleanup remains.
    Committed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RemoveJournal {
    pub version: u32,
    pub operation_id: String,
    pub phase: RemoveJournalPhase,
    pub skill_id: String,
    pub source_kind: RemoveSourceKind,
    pub final_entity_path: PathBuf,
    /// Install entities are moved here before the catalog commit so an
    /// interrupted Remove can roll back; `None` for Links.
    pub backup_path: Option<PathBuf>,
    pub backup_fingerprint: Option<DirectoryFingerprint>,
    pub activations: Vec<RemoveActivationStep>,
}

/// Catalog row existence decides whether an interrupted Remove had
/// committed: a surviving row rolls back, a vanished row rolls forward.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RemoveRecoveryBaseline {
    pub skill_id: String,
}

/// A single desired Activation as it exists when a Link relocation is
/// planned: the entry to rewrite, the old (Broken) target and the new one.
/// `initial_entry` records what the entry was at plan time (Missing or a
/// symlink to the old target) so startup recovery can roll forward or back
/// deterministically.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelocateInitialEntry {
    Missing,
    Symlink { old_target: PathBuf },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RelocateActivationStep {
    pub agent_id: String,
    pub entry_path: PathBuf,
    pub old_target_path: PathBuf,
    pub new_target_path: PathBuf,
    pub initial_entry: RelocateInitialEntry,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelocateJournalPhase {
    /// At least one Activation may already point at the new entity; the
    /// catalog write decides whether recovery rolls forward or back.
    Applying,
    /// The catalog committed the new pointer; only the symlinks remain to
    /// be verified and the journal archived.
    Committed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RelocateJournal {
    pub version: u32,
    pub operation_id: String,
    pub phase: RelocateJournalPhase,
    pub skill_id: String,
    pub old_final_entity_path: PathBuf,
    pub new_final_entity_path: PathBuf,
    pub activations: Vec<RelocateActivationStep>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptJournalKind {
    /// The entity was moved into the Library (file Install).
    Migrate,
    /// The entity stays outside; the Library records a pointer (Link).
    Link,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptAppearanceKind {
    RealDirectory,
    Symlink {
        original_target: PathBuf,
    },
    /// Entry under a shared/legacy scan source; never an Activation target.
    SharedEntry,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AdoptAppearanceStep {
    pub entry_path: PathBuf,
    pub kind: AdoptAppearanceKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AdoptActivationStep {
    pub agent_id: String,
    pub entry_path: PathBuf,
    pub target_path: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptItemPhase {
    /// Apply has not moved or registered this Skill yet.
    Planned,
    /// Staged in staging/<op>/<name>; nothing applied yet.
    Staged,
    /// The entity was installed at its stable Library path.
    EntityInstalled,
    /// The catalog row was committed.
    CatalogCommitted,
    /// Old appearances were replaced.
    AppearancesApplied,
    Done,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptJournalPhase {
    Planned,
    Applying,
    Committed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AdoptJournalItem {
    pub skill_id: String,
    pub directory_name: String,
    pub kind: AdoptJournalKind,
    pub staged_root: PathBuf,
    /// Identity of the external source before a Migrate item is staged. New
    /// v2 journals persist it so interrupted cross-volume isolation can be
    /// recovered without guessing which copy is authoritative.
    #[serde(default)]
    pub source_fingerprint: Option<DirectoryFingerprint>,
    pub staged_fingerprint: DirectoryFingerprint,
    pub final_entity_path: PathBuf,
    /// Empty for Link registrations.
    pub recorded_content_hash: String,
    pub original_path: PathBuf,
    pub original_filename: String,
    pub appearances: Vec<AdoptAppearanceStep>,
    pub activations: Vec<AdoptActivationStep>,
    pub phase: AdoptItemPhase,
    pub installed_fingerprint: Option<DirectoryFingerprint>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct AdoptJournal {
    pub version: u32,
    pub operation_id: String,
    pub phase: AdoptJournalPhase,
    pub staging_operation_root: PathBuf,
    pub staging_fingerprint: DirectoryFingerprint,
    pub items: Vec<AdoptJournalItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LinkSourceEntryKind {
    Directory,
    Symlink { target: PathBuf },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkSourceHop {
    pub path: PathBuf,
    pub target: PathBuf,
    pub device: u64,
    pub inode: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkSourceSnapshot {
    pub entry_path: PathBuf,
    pub directory_name: String,
    pub entry_device: u64,
    pub entry_inode: u64,
    pub entry_kind: LinkSourceEntryKind,
    pub symlink_chain: Vec<LinkSourceHop>,
    pub final_entity_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScannedSkillEntry {
    pub entry_path: PathBuf,
    pub name: String,
    pub kind: LinkSourceEntryKind,
    pub final_entity_path: Option<PathBuf>,
    pub dangling: bool,
}

#[derive(Debug, Error)]
pub enum FileSystemError {
    #[error("{operation} failed for '{}': {source}", path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("'{}' is not a directory", path.display())]
    NotDirectory { path: PathBuf },
    #[error("'{}' is not an absolute configured path", path.display())]
    InvalidConfiguredPath { path: PathBuf },
    #[error("'{}' changed after its operation was planned", path.display())]
    PlanStale { path: PathBuf },
    #[error("{operation} requires recovery for '{}': {message}", path.display())]
    RecoveryRequired {
        operation: &'static str,
        path: PathBuf,
        message: String,
    },
}

pub trait FileSystem: Send + Sync {
    /// Fill `buffer` with OS entropy (e.g. `/dev/urandom`): the randomness
    /// source for generated identities. Behind the seam so Core never
    /// touches the filesystem directly (core-boundary contract).
    fn read_entropy(&self, buffer: &mut [u8]) -> Result<(), FileSystemError>;

    fn inspect_link_source(&self, path: &Path) -> Result<LinkSourceSnapshot, FileSystemError>;

    fn canonical_directory(&self, path: &Path) -> Result<PathBuf, FileSystemError>;

    fn normalize_configured_path(&self, path: &Path) -> Result<PathBuf, FileSystemError>;

    fn directory_fingerprint(&self, path: &Path) -> Result<DirectoryFingerprint, FileSystemError>;

    fn activation_snapshot(
        &self,
        entry_path: &Path,
    ) -> Result<ActivationEntrySnapshot, FileSystemError>;

    fn skill_directory_is_readable(&self, path: &Path) -> Result<bool, FileSystemError>;

    fn skill_fingerprint(&self, path: &Path) -> Result<SkillFingerprint, FileSystemError>;

    fn read_skill_document(&self, path: &Path) -> Result<String, FileSystemError>;

    fn tree_hash(&self, path: &Path) -> Result<String, FileSystemError>;

    fn staged_tree_snapshot(&self, path: &Path) -> Result<StagedTreeSnapshot, FileSystemError>;

    fn available_space(&self, path: &Path) -> Result<u64, FileSystemError>;

    fn staged_child_directories(&self, path: &Path) -> Result<Vec<PathBuf>, FileSystemError>;

    fn staged_has_skill_document(
        &self,
        directory: &Path,
        filename: &str,
    ) -> Result<bool, FileSystemError>;

    fn canonicalize_staged_path(&self, path: &Path) -> Result<PathBuf, FileSystemError>;

    fn install_staged_skill(
        &self,
        staged_skill_path: &Path,
        final_entity_path: &Path,
        library_root: &Path,
        operation_id: &str,
        expected_staged_tree: &StagedTreeSnapshot,
    ) -> Result<DirectoryFingerprint, FileSystemError>;

    fn discard_staging(
        &self,
        staging_operation_root: &Path,
        library_root: &Path,
        expected: Option<&DirectoryFingerprint>,
    ) -> Result<(), FileSystemError>;

    fn discard_installed_skill(
        &self,
        final_entity_path: &Path,
        library_root: &Path,
        expected: &DirectoryFingerprint,
    ) -> Result<(), FileSystemError>;

    fn replace_staged_skill(
        &self,
        staged_skill_path: &Path,
        final_entity_path: &Path,
        library_root: &Path,
        operation_id: &str,
        expected_staged_tree: &StagedTreeSnapshot,
        expected_existing_tree: &StagedTreeSnapshot,
    ) -> Result<FileReplacement, FileSystemError> {
        let _ = (
            staged_skill_path,
            final_entity_path,
            library_root,
            operation_id,
            expected_staged_tree,
            expected_existing_tree,
        );
        Err(FileSystemError::Io {
            operation: "replace staged Skill",
            path: final_entity_path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "stable replacement is not supported by this filesystem",
            ),
        })
    }

    fn commit_replaced_skill(
        &self,
        replacement: &FileReplacement,
        library_root: &Path,
    ) -> Result<(), FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "commit replaced Skill",
            path: replacement.backup_path.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "stable replacement is not supported by this filesystem",
            ),
        })
    }

    fn rollback_replaced_skill(
        &self,
        replacement: &FileReplacement,
        library_root: &Path,
    ) -> Result<(), FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "roll back replaced Skill",
            path: replacement.final_entity_path.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "stable replacement is not supported by this filesystem",
            ),
        })
    }

    fn write_file_import_journal(
        &self,
        library_root: &Path,
        journal: &FileImportJournal,
    ) -> Result<(), FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "write file Import journal",
            path: PathBuf::from(&journal.operation_id),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "operation journals are not supported by this filesystem",
            ),
        })
    }

    fn finish_file_import_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "finish file Import journal",
            path: PathBuf::from(operation_id),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "operation journals are not supported by this filesystem",
            ),
        })
    }

    fn recover_file_import_journals(
        &self,
        library_root: &Path,
        baselines: &[FileImportRecoveryBaseline],
    ) -> Result<u32, FileSystemError> {
        let _ = baselines;
        Err(FileSystemError::Io {
            operation: "recover file Import journals",
            path: library_root.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "operation journal recovery is not supported by this filesystem",
            ),
        })
    }

    fn create_activation(
        &self,
        target_path: &Path,
        entry_path: &Path,
    ) -> Result<(), FileSystemError>;

    fn remove_activation(&self, entry_path: &Path) -> Result<(), FileSystemError>;

    /// Snapshot the content occupying an Activation entry: kind, symlink
    /// target or file length, and the device+inode identity.
    fn occupant_snapshot(&self, path: &Path) -> Result<OccupantSnapshot, FileSystemError> {
        let _ = path;
        Err(FileSystemError::Io {
            operation: "snapshot Activation occupant",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Activation occupant snapshots are not supported by this filesystem",
            ),
        })
    }

    /// Move the occupying entry to the operation backup (same-volume rename,
    /// cross-volume verified copy); the entry must still match `expected`.
    fn move_occupant_to_backup(
        &self,
        entry_path: &Path,
        backup_path: &Path,
        library_root: &Path,
        expected: &OccupantSnapshot,
    ) -> Result<(), FileSystemError> {
        let _ = (entry_path, backup_path, library_root, expected);
        Err(FileSystemError::Io {
            operation: "back up Activation occupant",
            path: entry_path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Activation occupant backups are not supported by this filesystem",
            ),
        })
    }

    /// Move the backed-up occupant back to its entry; the entry must be
    /// absent and the backup must still match `expected`.
    fn restore_occupant_from_backup(
        &self,
        backup_path: &Path,
        entry_path: &Path,
        library_root: &Path,
        expected: &OccupantSnapshot,
    ) -> Result<(), FileSystemError> {
        let _ = (backup_path, entry_path, library_root, expected);
        Err(FileSystemError::Io {
            operation: "restore Activation occupant",
            path: backup_path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Activation occupant restores are not supported by this filesystem",
            ),
        })
    }

    /// Discard a committed backup after verifying it still matches `expected`
    /// (or is already gone). Never used while an Undo is in progress.
    fn discard_replace_backup(
        &self,
        backup_path: &Path,
        library_root: &Path,
        expected: &OccupantSnapshot,
    ) -> Result<(), FileSystemError> {
        let _ = (backup_path, library_root, expected);
        Err(FileSystemError::Io {
            operation: "discard Activation occupant backup",
            path: backup_path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Activation occupant backups are not supported by this filesystem",
            ),
        })
    }

    fn write_activation_replace_journal(
        &self,
        library_root: &Path,
        journal: &ActivationReplaceJournal,
    ) -> Result<(), FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "write Activation replace journal",
            path: PathBuf::from(&journal.operation_id),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Activation replace journals are not supported by this filesystem",
            ),
        })
    }

    fn finish_activation_replace_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "finish Activation replace journal",
            path: PathBuf::from(operation_id),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Activation replace journals are not supported by this filesystem",
            ),
        })
    }

    fn recover_activation_replace_journals(
        &self,
        library_root: &Path,
        baselines: &[ActivationRecoveryBaseline],
    ) -> Result<u32, FileSystemError> {
        let _ = (library_root, baselines);
        Err(FileSystemError::Io {
            operation: "recover Activation replace journals",
            path: library_root.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Activation replace journal recovery is not supported by this filesystem",
            ),
        })
    }

    /// List the top-level entries of an Agent skills directory for the
    /// Adopt scan: real directories and symlinks (resolved with the same
    /// loop/depth guards as Link sources); dangling entries are reported
    /// with `dangling = true` and no final entity. Files are skipped.
    fn scan_skills_directory(&self, path: &Path)
    -> Result<Vec<ScannedSkillEntry>, FileSystemError>;

    /// Create an Agent skills directory at a configured path (explicit
    /// user-confirmed onboarding action, spec §8.7); the path must not exist.
    fn create_directory(&self, path: &Path) -> Result<(), FileSystemError> {
        let _ = path;
        Err(FileSystemError::Io {
            operation: "create Agent skills directory",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "directory creation is not supported by this filesystem",
            ),
        })
    }

    /// Move a real (non-symlink) directory from an external scan source into
    /// the staging root: same-volume rename, cross-volume copy with per-file
    /// verification then delete. Returns the staged fingerprint.
    fn stage_external_directory(
        &self,
        source: &Path,
        staging_destination: &Path,
    ) -> Result<DirectoryFingerprint, FileSystemError>;

    /// Create one Adopt operation directory beneath the owned Library staging
    /// root without following a replacement symlink. The returned fingerprint
    /// pins the directory that later source moves must target.
    fn create_adopt_staging_operation(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<DirectoryFingerprint, FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "create Adopt staging operation",
            path: PathBuf::from(operation_id),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "safe Adopt staging is not supported by this filesystem",
            ),
        })
    }

    /// Move an external source into a pinned Adopt operation directory. Both
    /// the operation directory and source entity are re-identified before the
    /// descriptor-relative move, closing the mkdir-to-move TOCTOU window.
    #[allow(clippy::too_many_arguments)]
    fn stage_external_directory_in_adopt_operation(
        &self,
        source: &Path,
        library_root: &Path,
        operation_id: &str,
        directory_name: &str,
        expected_operation_root: &DirectoryFingerprint,
        expected_source: &DirectoryFingerprint,
    ) -> Result<DirectoryFingerprint, FileSystemError> {
        let _ = (
            source,
            library_root,
            operation_id,
            expected_operation_root,
            expected_source,
        );
        Err(FileSystemError::Io {
            operation: "stage external directory for Adopt",
            path: PathBuf::from(directory_name),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "safe Adopt staging is not supported by this filesystem",
            ),
        })
    }

    /// Remove the original source retained under the deterministic
    /// cross-volume isolation name after the Staged journal cursor is durable.
    /// Same-volume migrations have no isolated source and return success.
    fn discard_isolated_adopt_source(
        &self,
        source: &Path,
        operation_id: &str,
        expected_source: &DirectoryFingerprint,
    ) -> Result<(), FileSystemError> {
        let _ = operation_id;
        Err(FileSystemError::Io {
            operation: "discard isolated Adopt source",
            path: source.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                format!(
                    "safe isolated Adopt source cleanup is not supported (expected inode {})",
                    expected_source.inode
                ),
            ),
        })
    }

    /// Reverse of `stage_external_directory` for Undo: move the directory
    /// back to its original entry path. The destination must be absent.
    fn restore_external_directory(
        &self,
        source: &Path,
        destination: &Path,
        expected: &DirectoryFingerprint,
    ) -> Result<(), FileSystemError>;

    /// Replace the old appearance entries (removing verified symlinks) and
    /// create every planned Activation; idempotent so interrupted Adopt
    /// operations can continue forward during recovery.
    fn apply_adopt_appearances(
        &self,
        appearances: &[AdoptAppearanceStep],
        activations: &[AdoptActivationStep],
    ) -> Result<(), FileSystemError>;

    fn write_adopt_journal(
        &self,
        library_root: &Path,
        journal: &AdoptJournal,
    ) -> Result<(), FileSystemError>;

    fn finish_adopt_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError>;

    fn recover_adopt_journals(
        &self,
        library_root: &Path,
        baselines: &[FileImportRecoveryBaseline],
        adopted_entities: &[FileImportRecoveryBaseline],
    ) -> Result<u32, FileSystemError>;

    /// Persist a Link relocation journal before the first filesystem step;
    /// progress is re-written after every Activation, and the journal is
    /// archived once the catalog commit succeeds or compensation completes.
    fn write_relocate_journal(
        &self,
        library_root: &Path,
        journal: &RelocateJournal,
    ) -> Result<(), FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "write Link relocation journal",
            path: PathBuf::from(&journal.operation_id),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "relocation journals are not supported by this filesystem",
            ),
        })
    }

    fn finish_relocate_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "finish Link relocation journal",
            path: PathBuf::from(operation_id),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "relocation journals are not supported by this filesystem",
            ),
        })
    }

    /// Replay interrupted Link relocations at startup: when the catalog
    /// recorded the new pointer, roll forward (rewrite symlinks to the new
    /// entity); otherwise roll back to each entry's original state.
    fn recover_relocate_journals(
        &self,
        library_root: &Path,
        baselines: &[RelocateRecoveryBaseline],
    ) -> Result<u32, FileSystemError> {
        let _ = (library_root, baselines);
        Err(FileSystemError::Io {
            operation: "recover Link relocation journals",
            path: library_root.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "relocation journal recovery is not supported by this filesystem",
            ),
        })
    }

    /// Persist a Remove journal before the first filesystem step; the entity
    /// backup path and fingerprint are recorded before the catalog commit,
    /// and the journal is archived once the removal completes or is
    /// compensated.
    fn write_remove_journal(
        &self,
        library_root: &Path,
        journal: &RemoveJournal,
    ) -> Result<(), FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "write Remove journal",
            path: PathBuf::from(&journal.operation_id),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Remove journals are not supported by this filesystem",
            ),
        })
    }

    fn finish_remove_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "finish Remove journal",
            path: PathBuf::from(operation_id),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Remove journals are not supported by this filesystem",
            ),
        })
    }

    /// Replay interrupted Removes at startup: a surviving catalog row rolls
    /// back (restore the backed-up entity, recreate removed Activations);
    /// a vanished row rolls forward (finish entity cleanup).
    fn recover_remove_journals(
        &self,
        library_root: &Path,
        baselines: &[RemoveRecoveryBaseline],
    ) -> Result<u32, FileSystemError> {
        let _ = (library_root, baselines);
        Err(FileSystemError::Io {
            operation: "recover Remove journals",
            path: library_root.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Remove journal recovery is not supported by this filesystem",
            ),
        })
    }

    /// Move an owned Install entity into the operation backup so an
    /// interrupted Remove can roll back. Same-volume rename; the destination
    /// must not exist. Returns the backup fingerprint for later verification.
    fn backup_library_entity(
        &self,
        final_entity_path: &Path,
        backup_path: &Path,
        library_root: &Path,
    ) -> Result<DirectoryFingerprint, FileSystemError> {
        let _ = (final_entity_path, backup_path, library_root);
        Err(FileSystemError::Io {
            operation: "back up Library entity",
            path: final_entity_path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Library entity backups are not supported by this filesystem",
            ),
        })
    }

    /// Restore a backed-up Install entity to its stable Library path during
    /// rollback; the destination must be absent and the backup must still
    /// match `expected`.
    fn restore_library_entity(
        &self,
        backup_path: &Path,
        final_entity_path: &Path,
        library_root: &Path,
        expected: &DirectoryFingerprint,
    ) -> Result<(), FileSystemError> {
        let _ = (backup_path, final_entity_path, library_root, expected);
        Err(FileSystemError::Io {
            operation: "restore Library entity",
            path: backup_path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Library entity restores are not supported by this filesystem",
            ),
        })
    }

    /// Discard a committed entity backup; verifies identity when `expected`
    /// is present. Never used while a rollback may still need the backup.
    fn discard_library_entity_backup(
        &self,
        backup_path: &Path,
        library_root: &Path,
        expected: Option<&DirectoryFingerprint>,
    ) -> Result<(), FileSystemError> {
        let _ = (backup_path, library_root, expected);
        Err(FileSystemError::Io {
            operation: "discard Library entity backup",
            path: backup_path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Library entity backups are not supported by this filesystem",
            ),
        })
    }

    /// Read a UTF-8 text file; `Ok(None)` when the file does not exist. Used
    /// by the bootstrap authority for the Home marker (spec §3.3) so a
    /// missing marker is a closed mismatch, not an I/O crash.
    fn read_utf8_file(&self, path: &Path) -> Result<Option<String>, FileSystemError> {
        let _ = path;
        Err(FileSystemError::Io {
            operation: "read UTF-8 file",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "UTF-8 file reads are not supported by this filesystem",
            ),
        })
    }

    /// Write a UTF-8 text file; the parent directory must already exist.
    /// Test composition writes Home markers through this seam so core stays
    /// free of direct filesystem access (capability boundary).
    fn write_utf8_file(&self, path: &Path, content: &str) -> Result<(), FileSystemError> {
        let _ = (path, content);
        Err(FileSystemError::Io {
            operation: "write UTF-8 file",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "UTF-8 file writes are not supported by this filesystem",
            ),
        })
    }

    /// Whether `path` is an existing directory; `Ok(false)` when it does not
    /// exist or is not a directory. Used by bootstrap for the read-only
    /// Legacy detection check (spec §3.3).
    fn path_is_directory(&self, path: &Path) -> Result<bool, FileSystemError> {
        let _ = path;
        Err(FileSystemError::Io {
            operation: "inspect directory existence",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "directory existence checks are not supported by this filesystem",
            ),
        })
    }

    /// Create a directory and all missing ancestors. Used for the prepared
    /// Home layout; a path that already exists is an error (never reuse).
    fn create_directory_all(&self, path: &Path) -> Result<(), FileSystemError> {
        let _ = path;
        Err(FileSystemError::Io {
            operation: "create directory tree",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "directory creation is not supported by this filesystem",
            ),
        })
    }

    /// Atomically rename a directory to a same-volume sibling that must not
    /// exist. The recovery module uses this for the whole-Home snapshot and
    /// the prepared-Home promote.
    fn rename_directory(&self, from: &Path, to: &Path) -> Result<(), FileSystemError> {
        let _ = (from, to);
        Err(FileSystemError::Io {
            operation: "rename directory",
            path: from.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "directory rename is not supported by this filesystem",
            ),
        })
    }

    /// fsync a directory so a completed rename is durable (spec §3.2
    /// protocol; the recovery snapshot/promote/commit protocol relies on it).
    fn fsync_directory(&self, path: &Path) -> Result<(), FileSystemError> {
        let _ = path;
        Err(FileSystemError::Io {
            operation: "fsync directory",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "directory fsync is not supported by this filesystem",
            ),
        })
    }

    /// Remove an app-created recovery artifact — a `<home>.snapshot-<op>` or
    /// `<home>.prepared-<op>` sibling of `home_root`. Bounded: the adapter
    /// verifies the artifact is a direct sibling with the exact generated
    /// name pattern; anything else is refused.
    fn remove_recovery_artifact(
        &self,
        artifact: &Path,
        home_root: &Path,
    ) -> Result<(), FileSystemError> {
        let _ = (artifact, home_root);
        Err(FileSystemError::Io {
            operation: "remove recovery artifact",
            path: artifact.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "recovery artifact removal is not supported by this filesystem",
            ),
        })
    }

    /// Prove no other process holds a SQLite WAL-index lock on `shm_path`:
    /// try to acquire an exclusive advisory lock without blocking.
    /// `Ok(true)` = lock acquired (no writer), `Ok(false)` = busy (a writer
    /// may be active), `Err` = cannot probe (fail closed, treat as busy).
    fn try_lock_wal_index_exclusive(&self, shm_path: &Path) -> Result<bool, FileSystemError> {
        let _ = shm_path;
        Err(FileSystemError::Io {
            operation: "probe WAL-index lock",
            path: shm_path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "WAL-index lock probing is not supported by this filesystem",
            ),
        })
    }

    /// List one directory level. The recovery module scans the Home's parent
    /// for Safety Snapshot siblings and probes the external app-state
    /// directory; entries are returned sorted by name.
    fn list_directory(&self, path: &Path) -> Result<Vec<DirectoryEntry>, FileSystemError> {
        let _ = path;
        Err(FileSystemError::Io {
            operation: "list directory",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "directory listing is not supported by this filesystem",
            ),
        })
    }

    /// Tree hash that skips files whose name is in `excluded` (SQLite
    /// WAL/SHM sidecars are derived artifacts; the recovery manifest must
    /// not depend on whether a read-only probe recreated them).
    fn tree_hash_excluding(
        &self,
        path: &Path,
        excluded: &[String],
    ) -> Result<String, FileSystemError> {
        let _ = (path, excluded);
        Err(FileSystemError::Io {
            operation: "hash tree excluding sidecars",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "excluded tree hashing is not supported by this filesystem",
            ),
        })
    }

    /// Create `path` as a directory when it does not exist (parents
    /// included); when it already exists it must be a real directory.
    /// Home Binding uses this to build the standard layout inside a fresh
    /// candidate and to fill legacy layout gaps without ever treating an
    /// existing file as a directory.
    fn ensure_directory(&self, path: &Path) -> Result<(), FileSystemError> {
        let _ = path;
        Err(FileSystemError::Io {
            operation: "ensure directory",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "directory creation is not supported by this filesystem",
            ),
        })
    }

    /// `Ok(true)` when no component of `path` that currently exists is a
    /// symlink (the final component included); components that do not exist
    /// yet cannot be symlinks and are skipped. Home Candidate validation
    /// refuses any path whose resolved components could change identity.
    fn path_has_no_symlink_component(&self, path: &Path) -> Result<bool, FileSystemError> {
        let _ = path;
        Err(FileSystemError::Io {
            operation: "inspect path components for symlinks",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "symlink component inspection is not supported by this filesystem",
            ),
        })
    }

    /// `Ok(true)` when the directory exists and the current user may create
    /// entries in it (Home Candidate validation checks the parent before a
    /// confirmation could create a new Home there).
    fn path_is_writable(&self, path: &Path) -> Result<bool, FileSystemError> {
        let _ = path;
        Err(FileSystemError::Io {
            operation: "inspect directory writability",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "writability inspection is not supported by this filesystem",
            ),
        })
    }

    /// Copy one whole tree to a new destination: directories, regular files
    /// and symlinks (symlinks are recreated as symlinks, never followed),
    /// with per-entry TOCTOU verification and cleanup of the partial copy
    /// on failure. The destination must not exist or must be an empty
    /// directory (the empty directory is removed first). Home Binding's
    /// Legacy copy transition relies on this for the SQLite+WAL+SHM
    /// consistent set plus every other Home entry.
    fn copy_tree_verified(&self, source: &Path, destination: &Path) -> Result<(), FileSystemError> {
        let _ = (source, destination);
        Err(FileSystemError::Io {
            operation: "copy tree",
            path: source.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "tree copy is not supported by this filesystem",
            ),
        })
    }

    /// Total bytes of every regular file in the tree (symlink targets are
    /// not followed; their link text counts). Used to preflight the Legacy
    /// copy destination's free space.
    fn tree_size(&self, path: &Path) -> Result<u64, FileSystemError> {
        let _ = path;
        Err(FileSystemError::Io {
            operation: "measure tree size",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "tree size measurement is not supported by this filesystem",
            ),
        })
    }

    /// Remove a directory tree. Home Binding calls this only after the
    /// caller has proven the directory is an operation-created candidate
    /// (ledger identity plus pure-layout contents); the capability is
    /// deliberately narrow so no other module can delete arbitrary trees.
    fn remove_directory_verified(&self, path: &Path) -> Result<(), FileSystemError> {
        let _ = path;
        Err(FileSystemError::Io {
            operation: "remove candidate directory",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "directory removal is not supported by this filesystem",
            ),
        })
    }
}
