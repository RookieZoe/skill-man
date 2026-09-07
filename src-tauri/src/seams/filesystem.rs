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
    pub target_root_id: String,
    pub expected_entry_path: PathBuf,
    pub expected_target_path: PathBuf,
}

/// The closed per-cell action of the Enable Module (spec §4.9): a plain
/// Enable/Disable/Repair, a Managed ownership Switch or a Remove-then-replace
/// of an Untracked occupant. Every action shares the same journal so one
/// interrupted operation has one recovery surface.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnableCellAction {
    Enable,
    Disable,
    Repair,
    Switch,
    /// Replace an Untracked occupier (symlink / real directory / file).
    Replace,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct EnableJournalCell {
    pub cell_index: u32,
    pub action: EnableCellAction,
    pub skill_id: String,
    pub target_root_id: String,
    pub directory_identity_key: String,
    pub entry_path: PathBuf,
    /// The canonical final entity path the Activation points at.
    pub target_path: PathBuf,
    /// The canonical identity of the Activation Target parent. `None` is
    /// retained only for legacy/project journals whose target was created
    /// after the journal was first written.
    #[serde(default)]
    pub target_parent: Option<DirectoryFingerprint>,
    /// The catalog desired state the commit point writes.
    pub after_desired: bool,
    /// The catalog desired state when the operation was planned.
    pub before_desired: bool,
    /// The occupant moved to backup before the write (`replace`/`switch`
    /// cells only); `None` for plain create/remove cells.
    pub backup_path: Option<PathBuf>,
    pub occupant: Option<OccupantSnapshot>,
    pub phase: ActivationReplacePhase,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct EnableJournal {
    pub version: u32,
    pub operation_id: String,
    pub phase: ActivationReplacePhase,
    pub cells: Vec<EnableJournalCell>,
}

/// One catalog row fact recovered at startup (or Undo): the Enable journal
/// recovery consults this set to decide rollback vs roll-forward per cell.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnableRecoveryFact {
    pub skill_id: String,
    pub target_root_id: String,
    pub desired_enabled: bool,
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

/// One child entry streamed by a Scan Run's tree walk (spec §3.6
/// "unlimited streaming evidence"): the relative path, the kind and the
/// byte length; symlink targets are the raw link text (never followed).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeScanEntry {
    pub relative_path: PathBuf,
    pub kind: TreeScanEntryKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TreeScanEntryKind {
    Directory,
    File { length: u64 },
    Symlink { target: PathBuf },
}

/// The accumulated facts of a streamed tree walk: file/byte counts and the
/// tree hash (`None` when the visitor asked the walk to stop — the entity
/// record then carries a hash fault instead of a partial hash).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StagedTreeSnapshot {
    pub root: DirectoryFingerprint,
    pub entries: Vec<StagedTreeEntry>,
    pub content_hash: String,
    pub total_file_bytes: u64,
}

/// Stream the full tree of one entity for a Scan Run: yields every child
/// entry (directories, regular files, symlinks — same rules as
/// `staged_tree_snapshot`, targets never followed), accumulates the
/// identical `tree-sha256-v1` content hash and the file/byte counts, and
/// never requires the whole listing in memory. The visitor returns `false`
/// to stop the walk (cancel/supersede). The default implementation buffers
/// through `staged_tree_snapshot` for test stubs; the system adapter walks
/// the tree level by level.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeScanStatistics {
    pub file_count: u64,
    pub byte_count: u64,
    pub tree_hash: Option<String>,
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
    pub target_root_id: String,
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
    pub target_root_id: String,
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
    pub target_root_id: String,
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

/// The durable parent manifest (`<Home>/remotes/<remote-id>/source.json`,
/// ADR-0013 §4.1): repository identity only, never checkout, credentials,
/// the whole lock or per-Skill versions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteParentManifest {
    /// Display-only plugin grouping from the selected commit. Never identity
    /// or ownership evidence; old manifests and journals default to ungrouped.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub member_plugins: std::collections::BTreeMap<String, String>,
    pub schema_version: u32,
    pub remote_id: String,
    pub canonical_url: String,
    /// Present only for a Git Repository Source.  An absent field is the
    /// historical v6 parent manifest and must remain Legacy during scans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_selected_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_release_id: Option<String>,
    pub aliases: Vec<String>,
    pub created_at: String,
}

/// Ownership Handoff per-Skill phases (spec §8.4, ADR-0013 §5): the exact
/// lock-entry CAS is the logical commit point. Recovery direction is
/// decided by this phase: below `OwnershipReleased` rolls back in place,
/// at or above it rolls forward under the recovery gate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffItemPhase {
    Planned,
    Staged,
    SourceIsolated,
    OwnershipReleased,
    ManagedCommitted,
    Finalized,
}

/// The frozen remote facts a handoff journal needs to roll forward without
/// any network access (spec §8.4 step 5: post-CAS recovery never re-fetches).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffRemoteJournal {
    pub canonical_url: String,
    pub requested_ref: String,
    pub verification_anchor_commit: String,
    pub original_commit_known: bool,
    pub skill_path: String,
    pub provider_hash: Option<String>,
    pub remote_baseline_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct HandoffJournalItem {
    pub skill_id: String,
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    /// The external canonical entity (the installer's directory).
    pub canonical_entity: PathBuf,
    /// The same-parent hidden operation path after Source Isolated; the
    /// short-term Undo isolation copy until the window closes or restarts.
    pub isolated_path: Option<PathBuf>,
    /// The staged tree (current bytes or the explicit Verification Anchor
    /// tree); `None` for Convert-to-Link (the tree moves, never stages).
    pub staged_root: PathBuf,
    pub staged_fingerprint: Option<DirectoryFingerprint>,
    pub final_entity_path: PathBuf,
    /// The frozen local tree hash (spec §8.1 TOCTOU baseline).
    pub source_tree_hash: String,
    /// The staged tree hash once Staged; `None` before staging.
    pub staged_tree_hash: Option<String>,
    pub lock_path: PathBuf,
    pub lock_fingerprint: String,
    pub lock_entry_name: String,
    /// The exact frozen lock entry (canonical JSON); the CAS writer
    /// re-compares it against the live file.
    pub lock_entry_json: String,
    /// The resolved parent id once the catalog commit runs; `None` until
    /// then and for Link intents. Undo uses it for last-child cleanup.
    #[serde(default)]
    pub remote_id: Option<String>,
    pub remote: Option<HandoffRemoteJournal>,
    pub current_baseline_hash: String,
    pub appearances: Vec<AdoptAppearanceStep>,
    pub activations: Vec<AdoptActivationStep>,
    /// The user-chosen stable directory for Convert-to-Link and
    /// Local-Link-with-Move; `None` for Home installs.
    pub target_directory: Option<PathBuf>,
    pub phase: HandoffItemPhase,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct HandoffJournal {
    pub version: u32,
    pub operation_id: String,
    pub staging_operation_root: PathBuf,
    pub staging_fingerprint: DirectoryFingerprint,
    pub items: Vec<HandoffJournalItem>,
}

/// A durable whole-repository transition. Unlike the legacy per-Skill
/// handoff journal, one phase governs every member and the frozen lock claim
/// set. It is the only startup authority for a post-CAS Source Transition;
/// recovery never needs to fetch a newer remote tip.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceTransitionPhase {
    Planned,
    MembersStaged,
    SourceIsolated,
    DestinationsReserved,
    OwnershipReleased,
    ManagedCommitted,
    Finalized,
    Undoing,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SourceTransitionJournalMember {
    pub skill_id: String,
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    /// The external canonical entity (the installer's directory). A new
    /// Source Member without an external declaration has none (a Promotion
    /// add follows the lock only when the installer claimed the same
    /// directory name).
    #[serde(default)]
    pub canonical_entity: Option<PathBuf>,
    /// The external canonical entity's real directory identity, frozen
    /// before isolation. A lock claim that is itself a symlink is rejected
    /// rather than canonicalized to its target.
    #[serde(default)]
    pub canonical_entity_fingerprint: Option<DirectoryFingerprint>,
    pub isolated_path: Option<PathBuf>,
    pub staged_root: PathBuf,
    pub staged_snapshot: Option<StagedTreeSnapshot>,
    /// The immutable namespace snapshot
    /// (`<Library>/skills/git/<remote_id>/<skill_id>`, ADR-0018).
    pub namespace_path: PathBuf,
    pub skill_path: String,
    pub tree_hash: String,
    /// Frozen external bytes before replacement, independent of the new
    /// release. Absent in older journals, which required both trees to match.
    #[serde(default)]
    pub external_tree_hash: Option<String>,
    pub provider_hash: Option<String>,
    /// `Added` enters the Library with this release; `Current` reuses the
    /// stable skill_id of the source's existing member (a promotion keeps a
    /// legacy member with the same `skill_path`).
    pub action: SourceTransitionMemberAction,
}

/// One `Current`/`Removed` member of the frozen complete manifest (ADR-0018
/// source-level lifecycle). For a clean transition both lists stay empty;
/// a Legacy Source Promotion freezes the matched/removed legacy members here.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceTransitionMemberAction {
    Added,
    Current,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SourceTransitionRemovedMember {
    /// Frozen direct external aliases removed after CAS and restored by Undo.
    /// These are never inserted as managed Activation records.
    #[serde(default)]
    pub external_links: Vec<crate::seams::source_transition_store::SourceTransitionActivation>,
    pub skill_id: String,
    pub directory_name: String,
    pub skill_path: String,
    /// The legacy Home entity path; absent from the target release.
    pub legacy_path: PathBuf,
    pub tree_hash: String,
    /// The legacy entity identity frozen before Source Transition
    /// isolation. Older journals without this fact recover fail-closed.
    #[serde(default)]
    pub legacy_fingerprint: Option<DirectoryFingerprint>,
    pub isolated_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SourceTransitionJournal {
    #[serde(default)]
    pub activations: Vec<crate::seams::source_transition_store::SourceTransitionActivation>,
    pub version: u32,
    pub operation_id: String,
    pub phase: SourceTransitionPhase,
    pub staging_operation_root: PathBuf,
    pub staging_fingerprint: Option<DirectoryFingerprint>,
    pub remote_id: String,
    pub release_id: String,
    pub provider: String,
    pub canonical_url: String,
    pub tracking_mode: String,
    pub tracking_value: Option<String>,
    pub selection_kind: String,
    pub selected_ref: String,
    pub resolved_commit: String,
    /// The exact managed-source manifest is frozen before the ownership CAS.
    /// Older journals predate Source Releases and recover through the
    /// transition service's compatibility path.
    #[serde(default)]
    pub target_manifest: Option<RemoteParentManifest>,
    pub lock_path: PathBuf,
    pub lock_fingerprint: String,
    /// lstat identity of the lock file at plan time; older journals may not
    /// carry it and are recovered fail-closed before any lock write.
    #[serde(default)]
    pub lock_identity: Option<crate::seams::installer_lock_store::LockFileIdentity>,
    pub lock_entries: Vec<crate::seams::installer_lock_store::LockEntry>,
    /// Original installer entry name -> relocated member path. Original lock
    /// entries remain byte-semantically unchanged for CAS and Undo.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub relocated_claim_paths: std::collections::BTreeMap<String, String>,
    pub members: Vec<SourceTransitionJournalMember>,
    /// Members absent from the target release, including explicitly confirmed
    /// disappeared external claims without a Promotion or Update audit.
    #[serde(default)]
    pub removed_members: Vec<SourceTransitionRemovedMember>,
    /// Frozen Legacy audit for a Source Promotion; recovered Undo restores
    /// the exact pre-Promotion Catalog state from these bytes.
    #[serde(default)]
    pub promotion_legacy: Option<crate::seams::source_promotion_store::LegacySourcePromotionRecord>,
    /// Frozen pre-Update facts for a v9 Source Update (ticket #93), as an
    /// opaque JSON document so the journal format stays stable.
    #[serde(default)]
    pub update_previous: Option<serde_json::Value>,
}

/// One frozen pre-Update member fact (primitive fields only).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceUpdatePreviousMemberFacts {
    pub skill_id: String,
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    pub skill_path: String,
    pub storage_relpath: String,
    /// `current` or `absent`.
    pub presence: String,
    pub tree_hash: Option<String>,
    /// `healthy|broken|modified|source_snapshot_mismatch`.
    pub health: String,
}

/// The frozen pre-Update source facts (primitive fields only).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceUpdatePreviousFacts {
    pub release_id: String,
    pub tracking_mode: String,
    pub tracking_value: Option<String>,
    pub selected_ref: String,
    pub resolved_commit: String,
    /// JSON document of the previous `remotes/<remote-id>/source.json`.
    pub manifest_json: Option<String>,
    pub members: Vec<SourceUpdatePreviousMemberFacts>,
}

impl SourceUpdatePreviousFacts {
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::json;
        json!({
            "releaseId": self.release_id,
            "trackingMode": self.tracking_mode,
            "trackingValue": self.tracking_value,
            "selectedRef": self.selected_ref,
            "resolvedCommit": self.resolved_commit,
            "manifestJson": self.manifest_json,
            "members": self.members.iter().map(|member| json!({
                "skillId": member.skill_id,
                "directoryName": member.directory_name,
                "identityKey": member.identity_key,
                "displayName": member.display_name,
                "description": member.description,
                "skillPath": member.skill_path,
                "storageRelpath": member.storage_relpath,
                "presence": member.presence,
                "treeHash": member.tree_hash,
                "health": member.health,
            })).collect::<Vec<_>>(),
        })
    }

    pub fn from_json(value: &serde_json::Value) -> Result<Self, String> {
        use serde_json::Value;
        let text = |key: &str| -> Result<String, String> {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("previous facts lack '{key}'"))
        };
        let members = value
            .get("members")
            .and_then(Value::as_array)
            .ok_or_else(|| "previous facts lack members".to_string())?
            .iter()
            .map(|member| {
                let field = |key: &str| -> Result<String, String> {
                    member
                        .get(key)
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .ok_or_else(|| format!("previous member lacks '{key}'"))
                };
                Ok(SourceUpdatePreviousMemberFacts {
                    skill_id: field("skillId")?,
                    directory_name: field("directoryName")?,
                    identity_key: field("identityKey")?,
                    display_name: field("displayName")?,
                    description: field("description")?,
                    skill_path: field("skillPath")?,
                    storage_relpath: field("storageRelpath")?,
                    presence: field("presence")?,
                    tree_hash: member
                        .get("treeHash")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    health: field("health")?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            release_id: text("releaseId")?,
            tracking_mode: text("trackingMode")?,
            tracking_value: value
                .get("trackingValue")
                .and_then(Value::as_str)
                .map(str::to_owned),
            selected_ref: text("selectedRef")?,
            resolved_commit: text("resolvedCommit")?,
            manifest_json: value
                .get("manifestJson")
                .and_then(Value::as_str)
                .map(str::to_owned),
            members,
        })
    }
}

/// One durable source-lifecycle journal (Restore / Local Copy / Remove).
/// JSON handled by explicit `to_json`/`from_json` mappers (no derive
/// machinery, keeping the journal format small and stable).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceLifecycleJournal {
    Restore(RestoreSourceJournal),
    LocalCopy(LocalCopyJournal),
    RemoveSource(RemoveSourceJournal),
}

impl SourceLifecycleJournal {
    pub fn operation_id(&self) -> &str {
        match self {
            Self::Restore(journal) => &journal.operation_id,
            Self::LocalCopy(journal) => &journal.operation_id,
            Self::RemoveSource(journal) => &journal.operation_id,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            Self::Restore(journal) => json!({
                "kind": "restore",
                "version": journal.version,
                "operationId": journal.operation_id,
                "phase": journal.phase.as_str(),
                "remoteId": journal.remote_id,
                "releaseId": journal.release_id,
                "resolvedCommit": journal.resolved_commit,
                "stagingOperationRoot": journal.staging_operation_root.to_string_lossy(),
                "members": journal.members.iter().map(|member| json!({
                    "skillId": member.skill_id,
                    "directoryName": member.directory_name,
                    "skillPath": member.skill_path,
                    "namespacePath": member.namespace_path.to_string_lossy(),
                    "stagedRoot": member.staged_root.to_string_lossy(),
                    "releaseTreeHash": member.release_tree_hash,
                    "observedTreeHash": member.observed_tree_hash,
                    "backupPath": member.backup_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                    "restored": member.restored,
                })).collect::<Vec<_>>(),
            }),
            Self::LocalCopy(journal) => json!({
                "kind": "local_copy",
                "version": journal.version,
                "operationId": journal.operation_id,
                "phase": journal.phase.as_str(),
                "remoteId": journal.remote_id,
                "skillId": journal.skill_id,
                "directoryName": journal.directory_name,
                "identityKey": journal.identity_key,
                "displayName": journal.display_name,
                "description": journal.description,
                "sourcePath": journal.source_path.to_string_lossy(),
                "destination": journal.destination.to_string_lossy(),
                "destinationParent": journal.destination_parent.as_ref().map(|parent| json!({
                    "canonicalPath": parent.canonical_path.to_string_lossy(),
                    "device": parent.device,
                    "inode": parent.inode,
                })),
                "destinationFingerprint": journal.destination_fingerprint.as_ref().map(|destination| json!({
                    "canonicalPath": destination.canonical_path.to_string_lossy(),
                    "device": destination.device,
                    "inode": destination.inode,
                })),
                "stagedPath": journal.staged_path.to_string_lossy(),
                "contentHash": journal.content_hash,
            }),
            Self::RemoveSource(journal) => json!({
                "kind": "remove_source",
                "version": journal.version,
                "operationId": journal.operation_id,
                "phase": journal.phase.as_str(),
                "remoteId": journal.remote_id,
                "canonicalUrl": journal.canonical_url,
                "stagingOperationRoot": journal.staging_operation_root.to_string_lossy(),
                "members": journal.members.iter().map(|member| json!({
                    "skillId": member.skill_id,
                    "directoryName": member.directory_name,
                    "namespacePath": member.namespace_path.to_string_lossy(),
                    "observedTreeHash": member.observed_tree_hash,
                    "isolatedPath": member.isolated_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                })).collect::<Vec<_>>(),
                "activations": journal.activations.iter().map(|activation| json!({
                    "skillId": activation.skill_id,
                    "entryPath": activation.entry_path.to_string_lossy(),
                    "targetPath": activation.target_path.to_string_lossy(),
                })).collect::<Vec<_>>(),
            }),
        }
    }

    pub fn from_json(value: &serde_json::Value) -> Result<Self, String> {
        use serde_json::Value;
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| "source lifecycle journal has no kind".to_string())?;
        let text = |key: &str| -> Result<String, String> {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("source lifecycle journal lacks '{key}'"))
        };
        let path = |key: &str| -> Result<PathBuf, String> { text(key).map(PathBuf::from) };
        match kind {
            "restore" => {
                let members = value
                    .get("members")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "restore journal lacks members".to_string())?
                    .iter()
                    .map(|member| {
                        let field = |key: &str| -> Result<String, String> {
                            member
                                .get(key)
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                                .ok_or_else(|| format!("member lacks '{key}'"))
                        };
                        Ok(RestoreSourceMember {
                            skill_id: field("skillId")?,
                            directory_name: field("directoryName")?,
                            skill_path: field("skillPath")?,
                            namespace_path: field("namespacePath")?.into(),
                            staged_root: field("stagedRoot")?.into(),
                            release_tree_hash: field("releaseTreeHash")?,
                            observed_tree_hash: field("observedTreeHash")?,
                            staged_snapshot: None,
                            backup_path: member
                                .get("backupPath")
                                .and_then(Value::as_str)
                                .map(PathBuf::from),
                            restored: member
                                .get("restored")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                Ok(Self::Restore(RestoreSourceJournal {
                    version: value.get("version").and_then(Value::as_u64).unwrap_or(0) as u32,
                    operation_id: text("operationId")?,
                    phase: RestoreSourcePhase::parse(&text("phase")?)?,
                    remote_id: text("remoteId")?,
                    release_id: text("releaseId")?,
                    resolved_commit: text("resolvedCommit")?,
                    staging_operation_root: path("stagingOperationRoot")?,
                    staging_fingerprint: None,
                    members,
                }))
            }
            "local_copy" => Ok(Self::LocalCopy(LocalCopyJournal {
                version: value.get("version").and_then(Value::as_u64).unwrap_or(0) as u32,
                operation_id: text("operationId")?,
                phase: LocalCopyPhase::parse(&text("phase")?)?,
                remote_id: text("remoteId")?,
                skill_id: text("skillId")?,
                directory_name: text("directoryName")?,
                identity_key: text("identityKey")?,
                display_name: text("displayName")?,
                description: text("description")?,
                source_path: path("sourcePath")?,
                destination: path("destination")?,
                destination_parent: value
                    .get("destinationParent")
                    .and_then(Value::as_object)
                    .map(|parent| {
                        Ok::<DirectoryFingerprint, String>(DirectoryFingerprint {
                            canonical_path: parent
                                .get("canonicalPath")
                                .and_then(Value::as_str)
                                .map(PathBuf::from)
                                .ok_or_else(|| {
                                    "local copy journal destination parent lacks canonicalPath"
                                        .to_string()
                                })?,
                            device: parent.get("device").and_then(Value::as_u64).ok_or_else(
                                || "local copy journal destination parent lacks device".to_string(),
                            )?,
                            inode: parent.get("inode").and_then(Value::as_u64).ok_or_else(
                                || "local copy journal destination parent lacks inode".to_string(),
                            )?,
                        })
                    })
                    .transpose()?,
                destination_fingerprint: value
                    .get("destinationFingerprint")
                    .and_then(Value::as_object)
                    .map(|destination| {
                        Ok::<DirectoryFingerprint, String>(DirectoryFingerprint {
                            canonical_path: destination
                                .get("canonicalPath")
                                .and_then(Value::as_str)
                                .map(PathBuf::from)
                                .ok_or_else(|| {
                                    "local copy journal destination fingerprint lacks canonicalPath"
                                        .to_string()
                                })?,
                            device: destination
                                .get("device")
                                .and_then(Value::as_u64)
                                .ok_or_else(|| {
                                    "local copy journal destination fingerprint lacks device"
                                        .to_string()
                                })?,
                            inode: destination
                                .get("inode")
                                .and_then(Value::as_u64)
                                .ok_or_else(|| {
                                    "local copy journal destination fingerprint lacks inode"
                                        .to_string()
                                })?,
                        })
                    })
                    .transpose()?,
                staged_path: path("stagedPath")?,
                content_hash: text("contentHash")?,
            })),
            "remove_source" => {
                let members = value
                    .get("members")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "remove journal lacks members".to_string())?
                    .iter()
                    .map(|member| {
                        let field = |key: &str| -> Result<String, String> {
                            member
                                .get(key)
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                                .ok_or_else(|| format!("member lacks '{key}'"))
                        };
                        Ok(RemoveSourceMemberJournal {
                            skill_id: field("skillId")?,
                            directory_name: field("directoryName")?,
                            namespace_path: field("namespacePath")?.into(),
                            observed_tree_hash: field("observedTreeHash")?,
                            isolated_path: member
                                .get("isolatedPath")
                                .and_then(Value::as_str)
                                .map(PathBuf::from),
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let activations = value
                    .get("activations")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "remove journal lacks activations".to_string())?
                    .iter()
                    .map(|activation| {
                        let field = |key: &str| -> Result<String, String> {
                            activation
                                .get(key)
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                                .ok_or_else(|| format!("activation lacks '{key}'"))
                        };
                        Ok(RemoveSourceActivationJournal {
                            skill_id: field("skillId")?,
                            entry_path: field("entryPath")?.into(),
                            target_path: field("targetPath")?.into(),
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                Ok(Self::RemoveSource(RemoveSourceJournal {
                    version: value.get("version").and_then(Value::as_u64).unwrap_or(0) as u32,
                    operation_id: text("operationId")?,
                    phase: RemoveSourcePhase::parse(&text("phase")?)?,
                    remote_id: text("remoteId")?,
                    canonical_url: text("canonicalUrl")?,
                    staging_operation_root: path("stagingOperationRoot")?,
                    staging_fingerprint: None,
                    members,
                    activations,
                }))
            }
            other => Err(format!("unknown source lifecycle journal kind '{other}'")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreSourcePhase {
    Planned,
    Restaged,
    BackedUp,
    Restored,
    Committed,
    Finalized,
}

impl RestoreSourcePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Restaged => "restaged",
            Self::BackedUp => "backed_up",
            Self::Restored => "restored",
            Self::Committed => "committed",
            Self::Finalized => "finalized",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        Ok(match value {
            "planned" => Self::Planned,
            "restaged" => Self::Restaged,
            "backed_up" => Self::BackedUp,
            "restored" => Self::Restored,
            "committed" => Self::Committed,
            "finalized" => Self::Finalized,
            other => return Err(format!("unknown restore phase '{other}'")),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreSourceMember {
    pub skill_id: String,
    pub directory_name: String,
    /// Repository-relative skill path of the frozen release.
    pub skill_path: String,
    /// `<Home>/skills/git/<remote_id>/<skill_id>`.
    pub namespace_path: PathBuf,
    pub staged_root: PathBuf,
    /// The frozen current Source Release tree hash.
    pub release_tree_hash: String,
    /// The observed byte hash when the restore was planned (mismatch).
    pub observed_tree_hash: String,
    pub staged_snapshot: Option<StagedTreeSnapshot>,
    /// The isolated observed bytes (`.skill-man-source-transition-…`).
    pub backup_path: Option<PathBuf>,
    pub restored: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreSourceJournal {
    pub version: u32,
    pub operation_id: String,
    pub phase: RestoreSourcePhase,
    pub remote_id: String,
    pub release_id: String,
    pub resolved_commit: String,
    pub staging_operation_root: PathBuf,
    pub staging_fingerprint: Option<DirectoryFingerprint>,
    pub members: Vec<RestoreSourceMember>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalCopyPhase {
    Planned,
    Copied,
    Registered,
    Finalized,
}

impl LocalCopyPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Copied => "copied",
            Self::Registered => "registered",
            Self::Finalized => "finalized",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        Ok(match value {
            "planned" => Self::Planned,
            "copied" => Self::Copied,
            "registered" => Self::Registered,
            "finalized" => Self::Finalized,
            other => return Err(format!("unknown local copy phase '{other}'")),
        })
    }
}

#[allow(clippy::struct_field_names)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalCopyJournal {
    pub version: u32,
    pub operation_id: String,
    pub phase: LocalCopyPhase,
    pub remote_id: String,
    pub skill_id: String,
    pub directory_name: String,
    pub identity_key: String,
    pub display_name: String,
    pub description: String,
    /// The frozen observed source bytes (`<Home>/skills/git/<id>/<skill>`).
    pub source_path: PathBuf,
    /// The user-chosen final (canonical) destination outside Home/Agent/
    /// installer roots.
    pub destination: PathBuf,
    /// The destination parent identity frozen before any copy bytes are
    /// written. This prevents an ancestor replacement from redirecting the
    /// staged copy or its final rename.
    pub destination_parent: Option<DirectoryFingerprint>,
    /// The copied destination identity after the final rename. Rollback
    /// refuses to delete the path without this fact.
    pub destination_fingerprint: Option<DirectoryFingerprint>,
    pub staged_path: PathBuf,
    pub content_hash: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoveSourcePhase {
    Planned,
    MembersIsolated,
    ActivationsRemoved,
    CatalogCommitted,
    Finalized,
}

impl RemoveSourcePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::MembersIsolated => "members_isolated",
            Self::ActivationsRemoved => "activations_removed",
            Self::CatalogCommitted => "catalog_committed",
            Self::Finalized => "finalized",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        Ok(match value {
            "planned" => Self::Planned,
            "members_isolated" => Self::MembersIsolated,
            "activations_removed" => Self::ActivationsRemoved,
            "catalog_committed" => Self::CatalogCommitted,
            "finalized" => Self::Finalized,
            other => return Err(format!("unknown remove phase '{other}'")),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoveSourceMemberJournal {
    pub skill_id: String,
    pub directory_name: String,
    pub namespace_path: PathBuf,
    /// The frozen current release tree hash (used to verify + restore the
    /// isolated snapshot on rollback).
    pub observed_tree_hash: String,
    pub isolated_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoveSourceActivationJournal {
    pub skill_id: String,
    pub entry_path: PathBuf,
    pub target_path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoveSourceJournal {
    pub version: u32,
    pub operation_id: String,
    pub phase: RemoveSourcePhase,
    pub remote_id: String,
    pub canonical_url: String,
    pub staging_operation_root: PathBuf,
    pub staging_fingerprint: Option<DirectoryFingerprint>,
    pub members: Vec<RemoveSourceMemberJournal>,
    pub activations: Vec<RemoveSourceActivationJournal>,
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

/// One step of a bounded evidence chain walk (spec §8.1). The walk records
/// every hop with its raw symlink target and entry identity; a failure stops
/// at the exact hop without guessing a final entity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EvidenceChainHopKind {
    /// A real directory component on the path to the entity.
    Directory,
    /// A symlink component; `target` is the raw target text as stored on
    /// disk, never resolved or rewritten.
    Symlink { target: PathBuf },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceChainHop {
    pub path: PathBuf,
    pub kind: EvidenceChainHopKind,
    pub device: u64,
    pub inode: u64,
}

/// Closed reasons why a chain walk stops before a final entity (spec §8.1):
/// every one yields a Blocked verdict and never a partial fingerprint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChainFault {
    /// The target of the hop does not exist.
    Dangling { at: PathBuf },
    /// The walk revisited a symlink it already followed.
    Cycle { at: PathBuf },
    /// The walk exceeded the bounded hop limit (16).
    HopLimit { at: PathBuf },
    /// A path component or symlink target is not valid UTF-8.
    NonUtf8 { at: PathBuf },
    /// A component could not be inspected or read.
    ReadFailed { at: PathBuf, detail: String },
    /// A component exists but is not a directory.
    NotDirectory { at: PathBuf },
    /// The entry changed identity while it was being walked (TOCTOU).
    IdentityReplaced { at: PathBuf },
}

/// The read-only result of walking one Adopt appearance to its final entity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceChain {
    pub entry_path: PathBuf,
    pub entry_device: u64,
    pub entry_inode: u64,
    /// Hops in walk order; the first hop is the entry itself when it is a
    /// symlink.
    pub hops: Vec<EvidenceChainHop>,
    /// The resolved final entity; `None` while `fault` is present.
    pub final_entity: Option<PathBuf>,
    pub fault: Option<ChainFault>,
}

/// Faults from bounded project symlink resolution (spec §4.9; ADR-0015; #89).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectTargetFault {
    OutsideProjectRoot,
    SymlinkCycle,
    HopLimitExceeded,
    TargetNotDirectory,
    TargetUnavailable { diagnostic: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectTargetResolution {
    pub resolved_container: PathBuf,
    pub hops: Vec<EvidenceChainHop>,
    pub create_steps: Vec<PathBuf>,
    pub fault: Option<ProjectTargetFault>,
}

/// One skill-named entry discovered in an Adopt scan source, with its full
/// evidence chain (spec §8.1). Non-UTF-8 entry names are reported as a
/// `NonUtf8` fault at the entry hop instead of being silently skipped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScannedSkillEvidence {
    pub entry_path: PathBuf,
    pub name: String,
    pub chain: EvidenceChain,
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

    /// Install one staged member snapshot into the immutable Git member
    /// namespace `<library_root>/skills/git/<remote_id>/<skill_id>`
    /// (ADR-0018). The namespace path is validated against the derived Git
    /// namespace exactly like `install_staged_skill` validates the flat
    /// allocation.
    fn install_git_member_snapshot(
        &self,
        staged_skill_path: &Path,
        namespace_path: &Path,
        library_root: &Path,
        operation_id: &str,
        expected_staged_tree: &StagedTreeSnapshot,
    ) -> Result<DirectoryFingerprint, FileSystemError> {
        let _ = (
            staged_skill_path,
            namespace_path,
            library_root,
            operation_id,
            expected_staged_tree,
        );
        Err(FileSystemError::Io {
            operation: "install Git member snapshot",
            path: namespace_path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Git member snapshot install is not supported by this filesystem",
            ),
        })
    }

    /// Resolve one Agent's `project_skills_dir` relative to a project root
    /// with bounded symlink walk and containment enforcement (spec §4.9;
    /// ADR-0015; ADR-0019; #89).
    fn resolve_project_target(
        &self,
        canonical_project_root: &Path,
        configured_relative: &Path,
    ) -> Result<ProjectTargetResolution, FileSystemError> {
        let _ = (canonical_project_root, configured_relative);
        Err(FileSystemError::Io {
            operation: "resolve project skills directory",
            path: configured_relative.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "project target resolution is not supported by this filesystem",
            ),
        })
    }

    /// Ensure a directory tree exists for project activation (creates parent
    /// directories safely if missing).
    fn ensure_directory_tree(&self, path: &Path) -> Result<(), FileSystemError> {
        let _ = path;
        Err(FileSystemError::Io {
            operation: "ensure directory tree exists",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "ensure directory tree is not supported by this filesystem",
            ),
        })
    }

    /// Create a Project Enable entry while binding the project root and
    /// resolved target through no-follow, descriptor-relative operations.
    /// The system adapter also rechecks every planned missing component.
    #[allow(clippy::too_many_arguments)]
    fn create_project_activation(
        &self,
        project_root: &DirectoryFingerprint,
        expected_targets: &[(PathBuf, ProjectTargetResolution)],
        target_path: &Path,
        create_steps: &[PathBuf],
        entry_path: &Path,
        final_entity_path: &Path,
        expected_target: Option<&DirectoryFingerprint>,
    ) -> Result<DirectoryFingerprint, FileSystemError> {
        for (configured_relative_path, expected) in expected_targets {
            let current = self
                .resolve_project_target(&project_root.canonical_path, configured_relative_path)?;
            let matches = current == *expected
                || (!expected.create_steps.is_empty()
                    && current.fault.is_none()
                    && current.resolved_container == target_path
                    && current.create_steps.is_empty());
            if !matches || current.resolved_container != target_path {
                return Err(FileSystemError::PlanStale {
                    path: target_path.to_path_buf(),
                });
            }
        }
        let actual_root = self.directory_fingerprint(&project_root.canonical_path)?;
        if actual_root != *project_root {
            return Err(FileSystemError::PlanStale {
                path: project_root.canonical_path.clone(),
            });
        }
        self.ensure_directory_tree(target_path)?;
        let target = self.directory_fingerprint(target_path)?;
        if let Some(expected) = expected_target
            && &target != expected
        {
            return Err(FileSystemError::PlanStale {
                path: target_path.to_path_buf(),
            });
        }
        self.create_activation_nofollow(final_entity_path, entry_path, &target)?;
        let _ = create_steps;
        Ok(target)
    }

    /// Copy a tree into a newly-created child of an identity-pinned
    /// destination parent. The macOS implementation uses `openat`/`mkdirat`
    /// with `O_NOFOLLOW`; the fallback is for in-memory test adapters.
    fn copy_tree_verified_nofollow(
        &self,
        source: &Path,
        destination: &Path,
        expected_destination_parent: &DirectoryFingerprint,
    ) -> Result<(), FileSystemError> {
        let parent =
            destination
                .parent()
                .ok_or_else(|| FileSystemError::InvalidConfiguredPath {
                    path: destination.to_path_buf(),
                })?;
        let actual_parent = self.directory_fingerprint(parent)?;
        if actual_parent != *expected_destination_parent {
            return Err(FileSystemError::PlanStale {
                path: parent.to_path_buf(),
            });
        }
        self.copy_tree_verified(source, destination)
    }

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

    /// Create an Activation entry after checking the identity of its parent.
    /// Implementations must not follow a symlink in the parent chain.
    fn create_activation_nofollow(
        &self,
        target_path: &Path,
        entry_path: &Path,
        expected_parent: &DirectoryFingerprint,
    ) -> Result<(), FileSystemError> {
        let parent = entry_path
            .parent()
            .ok_or_else(|| FileSystemError::InvalidConfiguredPath {
                path: entry_path.to_path_buf(),
            })?;
        if self.directory_fingerprint(parent)? != *expected_parent {
            return Err(FileSystemError::PlanStale {
                path: parent.to_path_buf(),
            });
        }
        self.create_activation(target_path, entry_path)
    }

    fn remove_activation(&self, entry_path: &Path) -> Result<(), FileSystemError>;

    /// Remove only the expected Activation symlink from an identity-pinned
    /// parent. This is the checked counterpart used by Enable undo.
    fn remove_activation_nofollow(
        &self,
        target_path: &Path,
        entry_path: &Path,
        expected_parent: &DirectoryFingerprint,
    ) -> Result<(), FileSystemError> {
        let parent = entry_path
            .parent()
            .ok_or_else(|| FileSystemError::InvalidConfiguredPath {
                path: entry_path.to_path_buf(),
            })?;
        if self.directory_fingerprint(parent)? != *expected_parent {
            return Err(FileSystemError::PlanStale {
                path: parent.to_path_buf(),
            });
        }
        match self.activation_snapshot(entry_path)? {
            ActivationEntrySnapshot::Symlink { target } if target == target_path => {
                self.remove_activation(entry_path)
            }
            _ => Err(FileSystemError::PlanStale {
                path: entry_path.to_path_buf(),
            }),
        }
    }

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

    /// Back up an Activation occupant after checking the identity of the
    /// entry parent. The system adapter additionally performs the move with
    /// descriptor-relative paths.
    fn move_occupant_to_backup_nofollow(
        &self,
        entry_path: &Path,
        backup_path: &Path,
        library_root: &Path,
        expected: &OccupantSnapshot,
        expected_entry_parent: &DirectoryFingerprint,
    ) -> Result<(), FileSystemError> {
        let parent = entry_path
            .parent()
            .ok_or_else(|| FileSystemError::InvalidConfiguredPath {
                path: entry_path.to_path_buf(),
            })?;
        if self.directory_fingerprint(parent)? != *expected_entry_parent {
            return Err(FileSystemError::PlanStale {
                path: parent.to_path_buf(),
            });
        }
        self.move_occupant_to_backup(entry_path, backup_path, library_root, expected)
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

    /// Restore an Activation occupant after checking the identity of the
    /// entry parent. The system adapter keeps the restore descriptor-relative.
    fn restore_occupant_from_backup_nofollow(
        &self,
        backup_path: &Path,
        entry_path: &Path,
        library_root: &Path,
        expected: &OccupantSnapshot,
        expected_entry_parent: &DirectoryFingerprint,
    ) -> Result<(), FileSystemError> {
        let parent = entry_path
            .parent()
            .ok_or_else(|| FileSystemError::InvalidConfiguredPath {
                path: entry_path.to_path_buf(),
            })?;
        if self.directory_fingerprint(parent)? != *expected_entry_parent {
            return Err(FileSystemError::PlanStale {
                path: parent.to_path_buf(),
            });
        }
        self.restore_occupant_from_backup(backup_path, entry_path, library_root, expected)
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

    /// Persist one Enable operation journal (spec §4.9) before the first
    /// filesystem mutation; the journal is rewritten after every committed
    /// cell so startup recovery can decide per-cell rollback/roll-forward.
    fn write_enable_journal(
        &self,
        library_root: &Path,
        journal: &EnableJournal,
    ) -> Result<(), FileSystemError> {
        let _ = (library_root, journal);
        Err(FileSystemError::Io {
            operation: "write Enable journal",
            path: PathBuf::from(&journal.operation_id),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "operation journals are not supported by this filesystem",
            ),
        })
    }

    /// Archive and remove one finished Enable journal (finalize / Undo /
    /// recovery); refuses while a backup still holds content.
    fn finish_enable_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "finish Enable journal",
            path: PathBuf::from(operation_id),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "operation journals are not supported by this filesystem",
            ),
        })
    }

    /// Recover every interrupted Enable journal: per cell, consult the
    /// catalog facts (current desired state) — a fact that matches the
    /// cell's commit point rolls the cell forward, anything else rolls it
    /// back. External changes stop recovery with `RecoveryRequired`.
    fn recover_enable_journals(
        &self,
        library_root: &Path,
        facts: &[EnableRecoveryFact],
    ) -> Result<u32, FileSystemError> {
        let _ = (library_root, facts);
        Err(FileSystemError::Io {
            operation: "recover Enable journals",
            path: library_root.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "operation journal recovery is not supported by this filesystem",
            ),
        })
    }

    /// List the top-level entries of an Agent skills directory for the
    /// Adopt scan: real directories and symlinks (resolved with the same
    /// loop/depth guards as Link sources); dangling entries are reported
    /// with `dangling = true` and no final entity. Files are skipped.
    fn scan_skills_directory(&self, path: &Path)
    -> Result<Vec<ScannedSkillEntry>, FileSystemError>;

    /// Stream the top-level entries of an Agent skills directory for a Scan
    /// Run (spec §3.6/§10.2 "unlimited streaming evidence"): the same
    /// filtering rules as `scan_skills_directory` but the caller never
    /// requires the full listing in memory. The visitor returns `false` to
    /// stop the walk (cancel/supersede). The default implementation buffers
    /// through `scan_skills_directory` for test stubs; the system adapter
    /// streams directly from `read_dir`.
    fn scan_skills_directory_stream(
        &self,
        path: &Path,
        visitor: &mut dyn FnMut(ScannedSkillEntry) -> Result<bool, FileSystemError>,
    ) -> Result<(), FileSystemError> {
        for entry in self.scan_skills_directory(path)? {
            if !visitor(entry)? {
                break;
            }
        }
        Ok(())
    }

    /// Stream the full tree of one entity for a Scan Run (spec §3.6
    /// "unlimited streaming evidence"): yields every child entry with the
    /// same rules as `staged_tree_snapshot` (targets never followed),
    /// accumulates the identical `tree-sha256-v1` content hash and the
    /// file/byte counts, and never requires the whole listing in memory.
    /// The visitor returns `false` to stop the walk (cancel/supersede),
    /// yielding a partial hash (`None`) so the entity record carries a hash
    /// fault instead of pretending completeness. The default implementation
    /// buffers through `staged_tree_snapshot` for test stubs; the system
    /// adapter walks the tree level by level.
    fn scan_tree_statistics(
        &self,
        path: &Path,
        visitor: &mut dyn FnMut(&TreeScanEntry) -> Result<bool, FileSystemError>,
    ) -> Result<TreeScanStatistics, FileSystemError> {
        let snapshot = self.staged_tree_snapshot(path)?;
        let mut file_count = 0_u64;
        let mut byte_count = 0_u64;
        for entry in &snapshot.entries {
            let kind = match &entry.kind {
                StagedEntryKind::Directory => TreeScanEntryKind::Directory,
                StagedEntryKind::File { length } => {
                    file_count += 1;
                    byte_count += *length;
                    TreeScanEntryKind::File { length: *length }
                }
                StagedEntryKind::Symlink { target } => TreeScanEntryKind::Symlink {
                    target: target.clone(),
                },
            };
            if !visitor(&TreeScanEntry {
                relative_path: entry.relative_path.clone(),
                kind,
            })? {
                return Ok(TreeScanStatistics {
                    file_count,
                    byte_count,
                    tree_hash: None,
                });
            }
        }
        Ok(TreeScanStatistics {
            file_count,
            byte_count,
            tree_hash: Some(snapshot.content_hash.clone()),
        })
    }

    /// Walk one Adopt appearance to its final entity with full per-hop
    /// evidence (spec §8.1): bounded at 16 hops, cycle-detecting, and
    /// failing at the exact hop on dangling/cycle/hop-limit/non-UTF-8/read
    /// errors or identity replacement. Never produces a partial fingerprint.
    fn inspect_evidence_chain(&self, path: &Path) -> Result<EvidenceChain, FileSystemError>;

    /// Enumerate the Skill entries of an Adopt scan source with their full
    /// evidence chains; a non-UTF-8 entry name yields a `NonUtf8` fault
    /// entry instead of being dropped.
    fn scan_skills_evidence(
        &self,
        path: &Path,
    ) -> Result<Vec<ScannedSkillEvidence>, FileSystemError>;

    /// Create a disposable temp workspace for read-only Adopt verification
    /// (remote mirrors and materialized subtrees). The caller MUST discard
    /// it with `discard_temp_workspace`.
    fn create_temp_workspace(&self, purpose: &str) -> Result<PathBuf, FileSystemError>;

    /// Remove a temp workspace created by `create_temp_workspace`; refuses
    /// to remove anything outside the Skill Man temp namespace.
    fn discard_temp_workspace(&self, path: &Path) -> Result<(), FileSystemError>;

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

    /// Whether any filesystem entry occupies `path`. Unlike
    /// `path_is_directory`, a symlink, regular file, or other non-directory
    /// entry is still occupied. Source Transition uses this exact fact at
    /// its ownership and Undo guards, where treating a non-directory as
    /// absent could overwrite an external owner.
    fn path_is_occupied(&self, path: &Path) -> Result<bool, FileSystemError> {
        let _ = path;
        Err(FileSystemError::Io {
            operation: "inspect path occupancy",
            path: path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "path occupancy checks are not supported by this filesystem",
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

    /// Move a verified directory without resolving either its parent at the
    /// point of mutation.  Source Promotion uses this narrow capability for
    /// Local Link so a replacement of an external target parent cannot
    /// redirect the library entity.  Implementations must bind both parent
    /// directories by descriptor, require an absent destination, and reject
    /// a source whose identity/content no longer match `expected`.
    fn move_directory_nofollow(
        &self,
        from: &Path,
        to: &Path,
        expected: &StagedTreeSnapshot,
        expected_source_parent: &DirectoryFingerprint,
        expected_destination_parent: &DirectoryFingerprint,
    ) -> Result<DirectoryFingerprint, FileSystemError> {
        let _ = (
            from,
            to,
            expected,
            expected_source_parent,
            expected_destination_parent,
        );
        Err(FileSystemError::Io {
            operation: "move verified directory without following links",
            path: from.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "descriptor-relative directory moves are not supported by this filesystem",
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

    /// Remove a directory only when its final entry is still the recorded
    /// real directory. The system adapter performs the recursive cleanup
    /// relative to an identity-pinned parent descriptor.
    fn remove_directory_verified_nofollow(
        &self,
        path: &Path,
        expected: &DirectoryFingerprint,
    ) -> Result<(), FileSystemError> {
        if self.directory_fingerprint(path)? != *expected {
            return Err(FileSystemError::PlanStale {
                path: path.to_path_buf(),
            });
        }
        self.remove_directory_verified(path)
    }

    /// Atomically write the parent manifest `source.json` (temp → fsync →
    /// rename → parent fsync, spec §3.2 protocol; ADR-0013 §4.1). The
    /// remotes root is `<Home>/remotes`; the remote-id directory is
    /// created when absent.
    fn write_remote_parent_manifest(
        &self,
        remotes_root: &Path,
        manifest: &RemoteParentManifest,
    ) -> Result<(), FileSystemError> {
        let _ = (remotes_root, manifest);
        Err(FileSystemError::Io {
            operation: "write remote parent manifest",
            path: PathBuf::new(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "remote parent manifests are not supported by this filesystem",
            ),
        })
    }

    /// Read the parent manifest; `Ok(None)` when the manifest does not
    /// exist (parent integrity check, ADR-0013 §4.3).
    fn read_remote_parent_manifest(
        &self,
        remotes_root: &Path,
        remote_id: &str,
    ) -> Result<Option<RemoteParentManifest>, FileSystemError> {
        let _ = (remotes_root, remote_id);
        Err(FileSystemError::Io {
            operation: "read remote parent manifest",
            path: PathBuf::new(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "remote parent manifests are not supported by this filesystem",
            ),
        })
    }

    /// Remove an empty parent's manifest directory after its last child
    /// Binding was removed (ADR-0013 §4.2). Bounded: the directory must be
    /// `<remotes_root>/<remote-id>` and may only contain `source.json`.
    fn remove_remote_parent_manifest(
        &self,
        remotes_root: &Path,
        remote_id: &str,
    ) -> Result<(), FileSystemError> {
        let _ = (remotes_root, remote_id);
        Err(FileSystemError::Io {
            operation: "remove remote parent manifest",
            path: PathBuf::new(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "remote parent manifests are not supported by this filesystem",
            ),
        })
    }

    /// Ownership Handoff step 3: atomically rename the external canonical
    /// directory to a same-parent hidden operation path, freezing the
    /// external source. Returns the hidden path. Any pre-CAS failure
    /// restores it with `restore_isolated_source`.
    fn isolate_external_source(
        &self,
        source: &Path,
        operation_id: &str,
    ) -> Result<PathBuf, FileSystemError> {
        let _ = (source, operation_id);
        Err(FileSystemError::Io {
            operation: "isolate external source",
            path: source.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "source isolation is not supported by this filesystem",
            ),
        })
    }

    /// Verified Source Transition isolation. The system adapter binds the
    /// expected inode/tree facts to the descriptor-relative rename; the
    /// default is retained for lightweight test adapters.
    /// An error does not prove rename had no effect: post-rename checks or
    /// fsync may fail. Callers must durably record the derived isolation
    /// path before invoking this method and retain that intent until the
    /// original entity is verified restored or the transition commits.
    fn isolate_external_source_verified(
        &self,
        source: &Path,
        operation_id: &str,
        expected: &DirectoryFingerprint,
        expected_tree_hash: &str,
    ) -> Result<PathBuf, FileSystemError> {
        if self.directory_fingerprint(source)? != *expected
            || self.tree_hash(source)? != expected_tree_hash
        {
            return Err(FileSystemError::PlanStale {
                path: source.to_path_buf(),
            });
        }
        self.isolate_external_source(source, operation_id)
    }

    /// Reverse of `isolate_external_source`: rename the hidden operation
    /// path back to the canonical location, which must be absent. The
    /// isolated tree must still match `expected_tree_hash`.
    fn restore_isolated_source(
        &self,
        isolated: &Path,
        source: &Path,
        expected_tree_hash: &str,
    ) -> Result<(), FileSystemError> {
        let _ = (isolated, source, expected_tree_hash);
        Err(FileSystemError::Io {
            operation: "restore isolated source",
            path: isolated.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "source isolation is not supported by this filesystem",
            ),
        })
    }

    /// Delete the hidden isolation copy (post-CAS roll-forward cleanup or
    /// Undo-after-restore). Bounded to the operation-name pattern.
    fn discard_isolated_source(&self, isolated: &Path) -> Result<(), FileSystemError> {
        let _ = isolated;
        Err(FileSystemError::Io {
            operation: "discard isolated source",
            path: isolated.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "source isolation is not supported by this filesystem",
            ),
        })
    }

    /// Persist the handoff journal before the first filesystem step;
    /// progress is re-written after every item phase transition.
    fn write_handoff_journal(
        &self,
        library_root: &Path,
        journal: &HandoffJournal,
    ) -> Result<(), FileSystemError> {
        let _ = (library_root, journal);
        Err(FileSystemError::Io {
            operation: "write handoff journal",
            path: PathBuf::from(&journal.operation_id),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "handoff journals are not supported by this filesystem",
            ),
        })
    }

    /// Archive a completed handoff journal and remove its operation
    /// directory (window close, Undo or startup recovery completion).
    fn finish_handoff_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError> {
        let _ = (library_root, operation_id);
        Err(FileSystemError::Io {
            operation: "finish handoff journal",
            path: PathBuf::from(operation_id),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "handoff journals are not supported by this filesystem",
            ),
        })
    }

    /// Every pending handoff journal (one per operation root), for the
    /// startup recovery gate (spec §8.4: CAS-before journals roll back,
    /// CAS-after journals roll forward).
    fn list_handoff_journals(
        &self,
        library_root: &Path,
    ) -> Result<Vec<HandoffJournal>, FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "list handoff journals",
            path: library_root.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "handoff journals are not supported by this filesystem",
            ),
        })
    }

    /// Post-CAS roll-forward, filesystem side: publish the staged Home
    /// entity (idempotent when the final entity already matches), flatten
    /// the real Agent appearances into Activations, discard the staging
    /// tree and the isolation copy (the result window is closed by a
    /// restart), and mark the item Finalized.
    fn roll_forward_handoff_item(
        &self,
        library_root: &Path,
        journal: &HandoffJournal,
        item: &mut HandoffJournalItem,
    ) -> Result<(), FileSystemError> {
        let _ = (library_root, journal, item);
        Err(FileSystemError::Io {
            operation: "roll forward handoff item",
            path: journal.staging_operation_root.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "handoff journals are not supported by this filesystem",
            ),
        })
    }

    /// Pre-CAS rollback: restore the external canonical directory from the
    /// isolation copy when it was isolated (the tree must still match the
    /// frozen source hash), discard the staged tree, and mark the item
    /// Planned again. Catalog and lock were never touched.
    fn rollback_handoff_item(
        &self,
        library_root: &Path,
        item: &mut HandoffJournalItem,
    ) -> Result<(), FileSystemError> {
        let canonical = item.canonical_entity.clone();
        let _ = (library_root, item);
        Err(FileSystemError::Io {
            operation: "rollback handoff item",
            path: canonical,
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "handoff journals are not supported by this filesystem",
            ),
        })
    }

    /// Persist the whole-source transition intent and each durable cursor.
    /// The journal must be on disk before any member is staged or isolated.
    fn write_source_transition_journal(
        &self,
        library_root: &Path,
        journal: &SourceTransitionJournal,
    ) -> Result<(), FileSystemError> {
        let _ = (library_root, journal);
        Err(FileSystemError::Io {
            operation: "write Source Transition journal",
            path: PathBuf::new(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Source Transition journals are not supported by this filesystem",
            ),
        })
    }

    /// Archive a completed Source Transition journal and close its result
    /// window. The operation becomes ineligible for Source Undo afterwards.
    fn finish_source_transition_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError> {
        let _ = (library_root, operation_id);
        Err(FileSystemError::Io {
            operation: "finish Source Transition journal",
            path: PathBuf::from(operation_id),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Source Transition journals are not supported by this filesystem",
            ),
        })
    }

    /// Pending whole-source journals only; legacy Ownership Handoff journals
    /// remain on their own recovery path.
    fn list_source_transition_journals(
        &self,
        library_root: &Path,
    ) -> Result<Vec<SourceTransitionJournal>, FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "list Source Transition journals",
            path: PathBuf::new(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Source Transition journals are not supported by this filesystem",
            ),
        })
    }

    /// Atomic durable write of one source-lifecycle journal into
    /// `<library_root>/operations/<operation-id>/`.
    fn write_source_lifecycle_journal(
        &self,
        library_root: &Path,
        journal: &SourceLifecycleJournal,
    ) -> Result<(), FileSystemError> {
        let _ = (library_root, journal);
        Err(FileSystemError::Io {
            operation: "write source lifecycle journal",
            path: PathBuf::new(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "source lifecycle journals are not supported by this filesystem",
            ),
        })
    }

    /// All source-lifecycle journals whose operation directory still exists.
    fn list_source_lifecycle_journals(
        &self,
        library_root: &Path,
    ) -> Result<Vec<SourceLifecycleJournal>, FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "list source lifecycle journals",
            path: PathBuf::new(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "source lifecycle journals are not supported by this filesystem",
            ),
        })
    }

    /// Archive (finish) one source-lifecycle journal and remove its
    /// operation directory; a missing operation is a no-op.
    fn finish_source_lifecycle_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError> {
        let _ = (library_root, operation_id);
        Err(FileSystemError::Io {
            operation: "finish source lifecycle journal",
            path: PathBuf::new(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "source lifecycle journals are not supported by this filesystem",
            ),
        })
    }

    /// Persist an opaque, versioned Source Promotion journal. Promotion is a
    /// Source Transition over an already-managed Legacy source, so it needs
    /// the same durable whole-source recovery authority without teaching the
    /// filesystem seam about Catalog domain records.
    fn write_source_promotion_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
        bytes: &[u8],
    ) -> Result<(), FileSystemError> {
        let _ = (library_root, operation_id, bytes);
        Err(FileSystemError::Io {
            operation: "write Source Promotion journal",
            path: PathBuf::new(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Source Promotion journals are not supported by this filesystem",
            ),
        })
    }

    /// Archive a completed Source Promotion journal and close its Source
    /// Undo result window.
    fn finish_source_promotion_journal(
        &self,
        library_root: &Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError> {
        let _ = (library_root, operation_id);
        Err(FileSystemError::Io {
            operation: "finish Source Promotion journal",
            path: PathBuf::from(operation_id),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Source Promotion journals are not supported by this filesystem",
            ),
        })
    }

    /// Return only pending whole-source Promotion journals. The opaque bytes
    /// are decoded and version-checked by Core so their domain schema can
    /// evolve independently of this physical filesystem seam.
    fn list_source_promotion_journals(
        &self,
        library_root: &Path,
    ) -> Result<Vec<(String, Vec<u8>)>, FileSystemError> {
        let _ = library_root;
        Err(FileSystemError::Io {
            operation: "list Source Promotion journals",
            path: PathBuf::new(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Source Promotion journals are not supported by this filesystem",
            ),
        })
    }

    /// Whether the lock file still carries `entry_name`, resolved from the
    /// strict v3 parse. Startup recovery uses this to decide the direction
    /// of an item interrupted between the exact-entry CAS and the durable
    /// journal phase write (spec §8.4: the CAS is the commit point, so an
    /// already-released entry must roll forward, never back). `Ok(false)`
    /// when the lock file is absent or the entry is gone; a faulted file
    /// fails closed.
    fn lock_entry_present(
        &self,
        lock_path: &Path,
        entry_name: &str,
    ) -> Result<bool, FileSystemError> {
        let _ = (lock_path, entry_name);
        Err(FileSystemError::Io {
            operation: "probe installer lock entry",
            path: lock_path.to_path_buf(),
            source: std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "lock entry probing is not supported by this filesystem",
            ),
        })
    }
}
