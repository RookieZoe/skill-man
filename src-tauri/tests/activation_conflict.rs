//! Activation Conflict 三选一 (#25): when Enable finds the target entry
//! occupied, the sheet offers Adopt existing item / Remove then replace /
//! Cancel. These tests cover conflict details (the Adopt gate), the
//! Remove-then-replace journal + backup flow, Undo, and startup recovery of
//! interrupted replaces.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::core::activation::{ActivationError, ActivationService, OccupierKind};
use skill_man_lib::core::domain::{ActivationObservedState, AgentId, AgentKind, SkillId};
use skill_man_lib::seams::activation_store::{
    ActivationContext, ActivationObservation, ActivationRecord, ActivationStore,
    ActivationStoreError, ConfiguredAgentPath, DesiredActivation,
};
use skill_man_lib::seams::filesystem::{
    ActivationRecoveryBaseline, ActivationReplaceJournal, ActivationReplacePhase, FileSystem,
    FileSystemError,
};
use skill_man_lib::tauri_adapter::activation_api::ActivationApi;
use skill_man_lib::tauri_adapter::dto::{
    ActivationConflictRequestDto, ApplyActivationReplaceRequestDto,
    CancelActivationReplaceRequestDto, FinalizeActivationReplaceRequestDto,
    PlanActivationReplaceRequestDto, UndoActivationReplaceRequestDto,
};

fn library_root(home: &Path) -> PathBuf {
    home.join("Library/Application Support/skill-man")
}

fn seed_managed_skill(home: &Path) -> PathBuf {
    let skill_root = library_root(home).join("skills/skill-authoring");
    std::fs::create_dir_all(&skill_root).expect("create Managed Skill");
    std::fs::write(skill_root.join("SKILL.md"), "# Skill authoring\n").expect("write SKILL.md");
    skill_root
}

fn claude_root(home: &Path) -> PathBuf {
    let root = home.join(".claude/skills");
    std::fs::create_dir_all(&root).expect("create Claude skills directory");
    root
}

fn service_with(home: &Path, store: Arc<dyn ActivationStore>) -> ActivationService {
    ActivationService::new(
        store,
        Arc::new(MacOsFileSystem::new(home.to_path_buf())),
        library_root(home),
    )
}

/// The canonical Agent entry for the seeded Skill.
fn expected_entry(home: &Path) -> PathBuf {
    claude_root(home)
        .canonicalize()
        .expect("canonical Claude skills directory")
        .join("skill-authoring")
}

fn occupy_with_real_directory(home: &Path) {
    let entry = claude_root(home).join("skill-authoring");
    std::fs::create_dir_all(&entry).expect("create occupying directory");
    std::fs::write(entry.join("keep.txt"), "user content").expect("write occupant content");
}

fn occupy_with_skill_directory(home: &Path) {
    let entry = claude_root(home).join("skill-authoring");
    std::fs::create_dir_all(&entry).expect("create occupying Skill directory");
    std::fs::write(entry.join("SKILL.md"), "# Occupant Skill\n").expect("write occupant SKILL.md");
    std::fs::write(entry.join("keep.txt"), "user content").expect("write occupant content");
}

// -- Conflict details ------------------------------------------------------

#[test]
fn conflict_details_describe_an_adoptable_untracked_skill() {
    let home = tempfile::tempdir().expect("temporary home");
    seed_managed_skill(home.path());
    occupy_with_skill_directory(home.path());
    let store = Arc::new(TestActivationStore::new(activation_context(
        home.path(),
        false,
        None,
    )));
    let service = service_with(home.path(), store.clone());

    let details = service
        .conflict_details(
            &SkillId("skill-authoring".into()),
            &AgentId("claude-code".into()),
        )
        .expect("conflict details");

    assert_eq!(details.entry_path, expected_entry(home.path()));
    assert_eq!(details.occupier.kind, OccupierKind::RealDirectory);
    assert!(details.occupier.is_skill);
    assert!(details.occupier.adoptable);
    assert_eq!(details.occupier.not_adoptable_reason, None);
    let canonical_entry = expected_entry(home.path());
    assert_eq!(
        details.occupier.final_entity_path.as_deref(),
        Some(canonical_entry.as_path())
    );
}

#[test]
fn conflict_details_mark_files_and_dangling_symlinks_as_not_adoptable() {
    let home = tempfile::tempdir().expect("temporary home");
    seed_managed_skill(home.path());
    let entry = claude_root(home.path()).join("skill-authoring");
    std::fs::write(&entry, "not a directory").expect("occupy with a file");
    let file_store = Arc::new(TestActivationStore::new(activation_context(
        home.path(),
        false,
        None,
    )));
    let file_service = service_with(home.path(), file_store.clone());
    let details = file_service
        .conflict_details(
            &SkillId("skill-authoring".into()),
            &AgentId("claude-code".into()),
        )
        .expect("conflict details for a file occupant");
    assert_eq!(details.occupier.kind, OccupierKind::File);
    assert!(!details.occupier.is_skill);
    assert!(!details.occupier.adoptable);
    assert!(details.occupier.not_adoptable_reason.is_some());

    std::fs::remove_file(&entry).expect("remove file occupant");
    std::os::unix::fs::symlink(entry.join("missing-target"), &entry)
        .expect("occupy with a dangling symlink");
    let dangling_store = Arc::new(TestActivationStore::new(activation_context(
        home.path(),
        false,
        None,
    )));
    let dangling_service = service_with(home.path(), dangling_store.clone());
    let details = dangling_service
        .conflict_details(
            &SkillId("skill-authoring".into()),
            &AgentId("claude-code".into()),
        )
        .expect("conflict details for a dangling occupant");
    assert_eq!(details.occupier.kind, OccupierKind::Symlink);
    assert!(!details.occupier.adoptable);
    assert!(details.occupier.not_adoptable_reason.is_some());
}

#[test]
fn conflict_details_block_adopt_for_library_and_same_entity_targets() {
    let home = tempfile::tempdir().expect("temporary home");
    let skill_root = seed_managed_skill(home.path());
    let entry = claude_root(home.path()).join("skill-authoring");

    // A symlink whose final entity lives inside the Library is a Managed
    // reference, not an Untracked Skill: not adoptable.
    let library_entity = library_root(home.path()).join("skills/other-skill");
    std::fs::create_dir_all(&library_entity).expect("create Library entity");
    std::fs::write(library_entity.join("SKILL.md"), "# Other\n").expect("write SKILL.md");
    std::os::unix::fs::symlink(&library_entity, &entry).expect("symlink into the Library");
    let library_store = Arc::new(TestActivationStore::new(activation_context(
        home.path(),
        false,
        None,
    )));
    let library_service = service_with(home.path(), library_store.clone());
    let details = library_service
        .conflict_details(
            &SkillId("skill-authoring".into()),
            &AgentId("claude-code".into()),
        )
        .expect("conflict details for a Library target");
    assert!(details.occupier.is_skill);
    assert!(!details.occupier.adoptable);

    // A symlink already pointing at this Skill's entity is not something to
    // Adopt either.
    std::fs::remove_file(&entry).expect("remove Library symlink");
    std::os::unix::fs::symlink(
        skill_root.canonicalize().expect("canonical Skill target"),
        &entry,
    )
    .expect("symlink to the same entity");
    let same_store = Arc::new(TestActivationStore::new(activation_context(
        home.path(),
        false,
        None,
    )));
    let same_service = service_with(home.path(), same_store.clone());
    let details = same_service
        .conflict_details(
            &SkillId("skill-authoring".into()),
            &AgentId("claude-code".into()),
        )
        .expect("conflict details for a same-entity target");
    assert!(details.occupier.is_skill);
    assert!(!details.occupier.adoptable);
    assert!(
        details
            .occupier
            .not_adoptable_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("entity"))
    );
}

#[test]
fn conflict_details_report_when_the_entry_is_no_longer_occupied() {
    let home = tempfile::tempdir().expect("temporary home");
    seed_managed_skill(home.path());
    claude_root(home.path());
    let store = Arc::new(TestActivationStore::new(activation_context(
        home.path(),
        false,
        None,
    )));
    let service = service_with(home.path(), store.clone());

    let error = service
        .conflict_details(
            &SkillId("skill-authoring".into()),
            &AgentId("claude-code".into()),
        )
        .expect_err("an empty entry is not a Conflict");

    assert!(matches!(error, ActivationError::Validation(_)));
}

// -- Remove then replace ---------------------------------------------------

#[test]
fn cancel_after_plan_replace_leaves_zero_changes() {
    let home = tempfile::tempdir().expect("temporary home");
    seed_managed_skill(home.path());
    occupy_with_real_directory(home.path());
    let canonical_entry = expected_entry(home.path());
    let store = Arc::new(TestActivationStore::new(activation_context(
        home.path(),
        false,
        None,
    )));
    let service = service_with(home.path(), store.clone());

    let preview = service
        .plan_replace(
            &SkillId("skill-authoring".into()),
            &AgentId("claude-code".into()),
        )
        .expect("plan Remove-then-replace");
    assert_eq!(preview.occupant_kind, OccupierKind::RealDirectory);
    assert!(
        service
            .cancel_replace(&preview.plan_token)
            .expect("cancel Replace plan")
    );

    assert_eq!(
        std::fs::read_to_string(canonical_entry.join("keep.txt")).expect("occupant remains"),
        "user content"
    );
    assert!(store.recorded().is_none(), "no catalog write happened");
    let operations_root = library_root(home.path()).join("operations");
    if operations_root.exists() {
        let leftover = std::fs::read_dir(&operations_root)
            .expect("operations directory")
            .count();
        assert_eq!(leftover, 0, "Cancel leaves no journal or backup behind");
    }
}

#[test]
fn apply_replace_backs_up_the_occupant_and_creates_the_activation() {
    let home = tempfile::tempdir().expect("temporary home");
    let skill_root = seed_managed_skill(home.path());
    occupy_with_real_directory(home.path());
    let canonical_entry = expected_entry(home.path());
    let store = Arc::new(TestActivationStore::new(activation_context(
        home.path(),
        false,
        None,
    )));
    let service = service_with(home.path(), store.clone());

    let preview = service
        .plan_replace(
            &SkillId("skill-authoring".into()),
            &AgentId("claude-code".into()),
        )
        .expect("plan Remove-then-replace");
    let result = service
        .apply_replace(&preview.plan_token)
        .expect("apply Remove-then-replace");

    assert!(result.desired_enabled);
    assert_eq!(result.observed_state, ActivationObservedState::Present);
    let canonical_target = skill_root.canonicalize().expect("canonical Skill target");
    assert_eq!(
        std::fs::read_link(&canonical_entry).expect("Activation symlink"),
        canonical_target
    );
    // The occupant is preserved in the journaled backup.
    assert_eq!(
        std::fs::read_to_string(preview.backup_path.join("keep.txt"))
            .expect("backed-up occupant content"),
        "user content"
    );
    let canonical_library = library_root(home.path())
        .canonicalize()
        .expect("canonical Library root");
    assert!(
        canonical_library
            .join("operations")
            .join(&preview.operation_id)
            .join("activation-replace-journal.json")
            .is_file(),
        "the replace journal is durable on disk"
    );
    let record = store.recorded().expect("catalog record");
    assert!(record.desired_enabled);
    assert_eq!(record.observed_state, ActivationObservedState::Present);
    assert_eq!(record.expected_entry_path, canonical_entry);
    assert_eq!(record.expected_target_path, canonical_target);
}

#[test]
fn apply_replace_stops_when_the_occupant_changes_after_preview() {
    let home = tempfile::tempdir().expect("temporary home");
    seed_managed_skill(home.path());
    occupy_with_real_directory(home.path());
    let store = Arc::new(TestActivationStore::new(activation_context(
        home.path(),
        false,
        None,
    )));
    let service = service_with(home.path(), store.clone());
    let preview = service
        .plan_replace(
            &SkillId("skill-authoring".into()),
            &AgentId("claude-code".into()),
        )
        .expect("plan Remove-then-replace");
    // An external actor replaces the occupant before Apply.
    let entry = claude_root(home.path()).join("skill-authoring");
    std::fs::remove_dir_all(&entry).expect("remove occupant");
    std::fs::create_dir(&entry).expect("replace occupant with other content");
    std::fs::write(entry.join("other.txt"), "external").expect("write external content");

    let error = service
        .apply_replace(&preview.plan_token)
        .expect_err("stale Replace plan cannot write");

    assert!(matches!(error, ActivationError::PlanStale));
    assert_eq!(
        std::fs::read_to_string(entry.join("other.txt")).expect("external content remains"),
        "external"
    );
    assert!(store.recorded().is_none(), "no catalog write happened");
}

#[test]
fn apply_replace_restores_the_occupant_when_the_state_write_fails() {
    let home = tempfile::tempdir().expect("temporary home");
    seed_managed_skill(home.path());
    occupy_with_real_directory(home.path());
    let canonical_entry = expected_entry(home.path());
    let store = Arc::new(FailingActivationStore::new(activation_context(
        home.path(),
        false,
        None,
    )));
    let service = service_with(home.path(), store.clone());
    let preview = service
        .plan_replace(
            &SkillId("skill-authoring".into()),
            &AgentId("claude-code".into()),
        )
        .expect("plan Remove-then-replace");

    let error = service
        .apply_replace(&preview.plan_token)
        .expect_err("failed state write surfaces");

    assert!(matches!(error, ActivationError::Store(_)));
    assert_eq!(
        std::fs::read_to_string(canonical_entry.join("keep.txt"))
            .expect("occupant restored to its entry"),
        "user content"
    );
    assert!(
        !preview.backup_path.exists(),
        "backup was consumed by the restore"
    );
}

#[test]
fn undo_replace_restores_the_occupant_and_disables_the_activation() {
    let home = tempfile::tempdir().expect("temporary home");
    seed_managed_skill(home.path());
    occupy_with_real_directory(home.path());
    let canonical_entry = expected_entry(home.path());
    let store = Arc::new(TestActivationStore::new(activation_context(
        home.path(),
        false,
        None,
    )));
    let service = service_with(home.path(), store.clone());
    let preview = service
        .plan_replace(
            &SkillId("skill-authoring".into()),
            &AgentId("claude-code".into()),
        )
        .expect("plan Remove-then-replace");
    let result = service
        .apply_replace(&preview.plan_token)
        .expect("apply Remove-then-replace");

    let undo = service
        .undo_replace(&result_operation_id(&preview, result.snapshot_version))
        .expect("Undo the Replace");

    assert!(undo.undone, "Undo restored the occupant");
    assert_eq!(
        std::fs::read_to_string(canonical_entry.join("keep.txt")).expect("occupant restored"),
        "user content"
    );
    let record = store.recorded().expect("catalog record after Undo");
    assert!(!record.desired_enabled);
    assert_eq!(record.observed_state, ActivationObservedState::Occupied);
    assert!(!preview.backup_path.exists(), "backup consumed by Undo");
}

fn result_operation_id(
    preview: &skill_man_lib::core::activation::ActivationReplacePreview,
    _snapshot_version: u64,
) -> String {
    preview.operation_id.clone()
}

#[test]
fn undo_replace_skips_when_the_entry_was_externally_replaced() {
    let home = tempfile::tempdir().expect("temporary home");
    seed_managed_skill(home.path());
    occupy_with_real_directory(home.path());
    let store = Arc::new(TestActivationStore::new(activation_context(
        home.path(),
        false,
        None,
    )));
    let service = service_with(home.path(), store.clone());
    let preview = service
        .plan_replace(
            &SkillId("skill-authoring".into()),
            &AgentId("claude-code".into()),
        )
        .expect("plan Remove-then-replace");
    service
        .apply_replace(&preview.plan_token)
        .expect("apply Remove-then-replace");
    // External content appears at the entry before Undo.
    let entry = claude_root(home.path()).join("skill-authoring");
    std::fs::remove_file(&entry).expect("remove Activation");
    std::fs::create_dir(&entry).expect("occupy entry externally");
    std::fs::write(entry.join("external.txt"), "external").expect("write external content");

    let undo = service
        .undo_replace(&preview.operation_id)
        .expect("Undo reports a skip, not a failure");

    assert!(!undo.undone);
    assert!(undo.error.is_some());
    assert_eq!(
        std::fs::read_to_string(entry.join("external.txt")).expect("external content remains"),
        "external"
    );
    assert_eq!(
        std::fs::read_to_string(preview.backup_path.join("keep.txt"))
            .expect("occupant remains backed up"),
        "user content"
    );
}

#[test]
fn finalize_replace_discards_the_backup_and_archives_the_journal() {
    let home = tempfile::tempdir().expect("temporary home");
    seed_managed_skill(home.path());
    occupy_with_real_directory(home.path());
    let store = Arc::new(TestActivationStore::new(activation_context(
        home.path(),
        false,
        None,
    )));
    let service = service_with(home.path(), store.clone());
    let preview = service
        .plan_replace(
            &SkillId("skill-authoring".into()),
            &AgentId("claude-code".into()),
        )
        .expect("plan Remove-then-replace");
    service
        .apply_replace(&preview.plan_token)
        .expect("apply Remove-then-replace");
    let canonical_library = library_root(home.path())
        .canonicalize()
        .expect("canonical Library root");

    service
        .finalize_replace(&preview.operation_id)
        .expect("finalize the Replace");

    assert!(
        !preview.backup_path.exists(),
        "backup discarded on finalize"
    );
    assert!(
        !canonical_library
            .join("operations")
            .join(&preview.operation_id)
            .exists(),
        "operation directory removed"
    );
    assert!(
        canonical_library
            .join("operation-history")
            .join(format!("{}.activation-replace.json", preview.operation_id))
            .is_file(),
        "journal archived for audit"
    );
}

// -- Startup recovery ------------------------------------------------------

fn journal_for(
    home: &Path,
    operation_id: &str,
    phase: ActivationReplacePhase,
    occupant: skill_man_lib::seams::filesystem::OccupantSnapshot,
) -> ActivationReplaceJournal {
    let canonical_library = library_root(home)
        .canonicalize()
        .expect("canonical Library root");
    ActivationReplaceJournal {
        version: 1,
        operation_id: operation_id.into(),
        phase,
        skill_id: "skill-authoring".into(),
        agent_id: "claude-code".into(),
        entry_path: expected_entry(home),
        target_path: seed_managed_skill(home)
            .canonicalize()
            .expect("canonical Skill target"),
        backup_path: canonical_library
            .join("operations")
            .join(operation_id)
            .join("backup")
            .join("skill-authoring"),
        occupant,
    }
}

fn baseline_for(home: &Path) -> ActivationRecoveryBaseline {
    ActivationRecoveryBaseline {
        skill_id: "skill-authoring".into(),
        agent_id: "claude-code".into(),
        expected_entry_path: expected_entry(home),
        expected_target_path: seed_managed_skill(home)
            .canonicalize()
            .expect("canonical Skill target"),
    }
}

#[test]
fn recovery_rolls_back_an_uncommitted_replace() {
    let home = tempfile::tempdir().expect("temporary home");
    seed_managed_skill(home.path());
    occupy_with_real_directory(home.path());
    let fs = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let entry = expected_entry(home.path());
    let occupant = fs.occupant_snapshot(&entry).expect("occupant snapshot");
    let journal = journal_for(
        home.path(),
        "activation-replace-crash-1",
        ActivationReplacePhase::Applying,
        occupant.clone(),
    );
    // Simulate the crash point: occupant moved, Activation created, catalog
    // never committed (empty baselines).
    fs.write_activation_replace_journal(&library_root(home.path()), &journal)
        .expect("write journal");
    fs.move_occupant_to_backup(
        &entry,
        &journal.backup_path,
        &library_root(home.path()),
        &occupant,
    )
    .expect("move occupant to backup");
    fs.create_activation(&journal.target_path, &entry)
        .expect("create Activation");

    let recovered = fs
        .recover_activation_replace_journals(&library_root(home.path()), &[])
        .expect("recover uncommitted Replace");

    assert_eq!(recovered, 1);
    assert_eq!(
        std::fs::read_to_string(entry.join("keep.txt")).expect("occupant restored"),
        "user content"
    );
    assert!(
        std::fs::symlink_metadata(&entry)
            .expect("entry is a real directory again")
            .file_type()
            .is_dir()
    );
    assert!(
        !journal.backup_path.exists(),
        "backup consumed by the rollback"
    );
    assert!(
        !journal
            .backup_path
            .parent()
            .expect("backup parent")
            .exists()
    );
}

#[test]
fn recovery_discards_the_backup_for_a_committed_replace() {
    let home = tempfile::tempdir().expect("temporary home");
    seed_managed_skill(home.path());
    occupy_with_real_directory(home.path());
    let fs = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let entry = expected_entry(home.path());
    let occupant = fs.occupant_snapshot(&entry).expect("occupant snapshot");
    let journal = journal_for(
        home.path(),
        "activation-replace-committed-1",
        ActivationReplacePhase::Committed,
        occupant.clone(),
    );
    fs.write_activation_replace_journal(&library_root(home.path()), &journal)
        .expect("write journal");
    fs.move_occupant_to_backup(
        &entry,
        &journal.backup_path,
        &library_root(home.path()),
        &occupant,
    )
    .expect("move occupant to backup");
    fs.create_activation(&journal.target_path, &entry)
        .expect("create Activation");

    let recovered = fs
        .recover_activation_replace_journals(
            &library_root(home.path()),
            &[baseline_for(home.path())],
        )
        .expect("recover committed Replace");

    assert_eq!(recovered, 1);
    assert_eq!(
        std::fs::read_link(&entry).expect("Activation remains"),
        journal.target_path
    );
    assert!(
        !journal.backup_path.exists(),
        "committed backup discarded by recovery"
    );
}

#[test]
fn recovery_completes_an_interrupted_undo() {
    let home = tempfile::tempdir().expect("temporary home");
    seed_managed_skill(home.path());
    occupy_with_real_directory(home.path());
    let fs = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let entry = expected_entry(home.path());
    let occupant = fs.occupant_snapshot(&entry).expect("occupant snapshot");
    let journal = journal_for(
        home.path(),
        "activation-replace-undo-crash-1",
        ActivationReplacePhase::Undoing,
        occupant.clone(),
    );
    fs.write_activation_replace_journal(&library_root(home.path()), &journal)
        .expect("write journal");
    fs.move_occupant_to_backup(
        &entry,
        &journal.backup_path,
        &library_root(home.path()),
        &occupant,
    )
    .expect("move occupant to backup");
    fs.create_activation(&journal.target_path, &entry)
        .expect("create Activation");
    // The undo removed the Activation but crashed before restoring the occupant.
    fs.remove_activation(&entry).expect("remove Activation");

    let recovered = fs
        .recover_activation_replace_journals(
            &library_root(home.path()),
            &[baseline_for(home.path())],
        )
        .expect("recover interrupted Undo");

    assert_eq!(recovered, 1);
    assert_eq!(
        std::fs::read_to_string(entry.join("keep.txt")).expect("occupant restored"),
        "user content"
    );
    assert!(
        !journal.backup_path.exists(),
        "backup consumed by the restore"
    );
}

#[test]
fn recovery_finishes_cleanly_when_the_crash_happened_before_the_move() {
    let home = tempfile::tempdir().expect("temporary home");
    seed_managed_skill(home.path());
    occupy_with_real_directory(home.path());
    let fs = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let entry = expected_entry(home.path());
    let occupant = fs.occupant_snapshot(&entry).expect("occupant snapshot");
    let journal = journal_for(
        home.path(),
        "activation-replace-before-move-1",
        ActivationReplacePhase::Applying,
        occupant.clone(),
    );
    // Crash right after the journal was fsync'd, before anything moved.
    fs.write_activation_replace_journal(&library_root(home.path()), &journal)
        .expect("write journal");

    let recovered = fs
        .recover_activation_replace_journals(&library_root(home.path()), &[])
        .expect("unapplied Replace recovers without requiring attention");

    assert_eq!(recovered, 1);
    assert_eq!(
        std::fs::read_to_string(entry.join("keep.txt")).expect("occupant untouched"),
        "user content"
    );
    assert!(!journal.backup_path.exists(), "no backup was created");
}

#[test]
fn conflict_details_block_adopt_when_an_identity_conflict_exists() {
    let home = tempfile::tempdir().expect("temporary home");
    seed_managed_skill(home.path());
    occupy_with_skill_directory(home.path());
    let store = Arc::new(TestActivationStore::new(activation_context(
        home.path(),
        false,
        None,
    )));
    // The checker reports the Managed Skill that owns this directory identity
    // (the Skill being enabled) with a different entity — §8.4 blocks Adopt.
    struct IdentityConflictChecker;
    impl skill_man_lib::core::activation::ActivationConflictChecker for IdentityConflictChecker {
        fn library_identity_conflict(
            &self,
            identity_key: &str,
        ) -> Result<
            Option<skill_man_lib::seams::adopt_store::LibraryConflict>,
            skill_man_lib::seams::activation_store::ActivationStoreError,
        > {
            Ok(Some(skill_man_lib::seams::adopt_store::LibraryConflict {
                skill_id: SkillId("skill-authoring".into()),
                directory_name: identity_key.to_owned(),
                final_entity_path: PathBuf::from("/Library/skills/skill-authoring"),
            }))
        }
    }
    let service = ActivationService::new(
        store,
        Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
        library_root(home.path()),
    )
    .with_conflict_checker(Arc::new(IdentityConflictChecker));

    let details = service
        .conflict_details(
            &SkillId("skill-authoring".into()),
            &AgentId("claude-code".into()),
        )
        .expect("conflict details");

    assert!(details.occupier.is_skill);
    assert!(!details.occupier.adoptable);
    assert!(
        details
            .occupier
            .not_adoptable_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("already uses this directory identity"))
    );
}

#[test]
fn recovery_keeps_the_backup_when_undo_was_interrupted_by_external_content() {
    let home = tempfile::tempdir().expect("temporary home");
    seed_managed_skill(home.path());
    occupy_with_real_directory(home.path());
    let fs = Arc::new(MacOsFileSystem::new(home.path().to_path_buf()));
    let entry = expected_entry(home.path());
    let occupant = fs.occupant_snapshot(&entry).expect("occupant snapshot");
    let journal = journal_for(
        home.path(),
        "activation-replace-undo-blocked-1",
        ActivationReplacePhase::Undoing,
        occupant.clone(),
    );
    fs.write_activation_replace_journal(&library_root(home.path()), &journal)
        .expect("write journal");
    fs.move_occupant_to_backup(
        &entry,
        &journal.backup_path,
        &library_root(home.path()),
        &occupant,
    )
    .expect("move occupant to backup");
    // External content appears where the Activation used to be.
    std::fs::create_dir(&entry).expect("occupy entry externally");
    std::fs::write(entry.join("external.txt"), "external").expect("write external content");

    let error = fs
        .recover_activation_replace_journals(
            &library_root(home.path()),
            &[baseline_for(home.path())],
        )
        .expect_err("ambiguous Undo requires attention");

    assert!(matches!(error, FileSystemError::RecoveryRequired { .. }));
    assert!(
        journal.backup_path.join("keep.txt").is_file(),
        "the occupant backup is retained"
    );
}

// -- Tauri adapter ---------------------------------------------------------

#[test]
fn tauri_adapter_exposes_typed_conflict_and_replace_results() {
    let home = tempfile::tempdir().expect("temporary home");
    seed_managed_skill(home.path());
    occupy_with_skill_directory(home.path());
    let store = Arc::new(TestActivationStore::new(activation_context(
        home.path(),
        false,
        None,
    )));
    let service = service_with(home.path(), store.clone());
    let api = ActivationApi::new(service);

    let details = api
        .activation_conflict_details(ActivationConflictRequestDto {
            skill_id: "skill-authoring".into(),
            agent_id: "claude-code".into(),
        })
        .expect("typed conflict details");
    assert!(matches!(
        details.occupier.kind,
        skill_man_lib::tauri_adapter::dto::OccupierKindDto::RealDirectory
    ));
    assert!(details.occupier.adoptable);

    let preview = api
        .plan_activation_replace(PlanActivationReplaceRequestDto {
            skill_id: "skill-authoring".into(),
            agent_id: "claude-code".into(),
        })
        .expect("typed Replace preview");
    let result = api
        .apply_activation_replace(ApplyActivationReplaceRequestDto {
            plan_token: preview.plan_token.clone(),
        })
        .expect("typed Replace result");
    assert!(result.desired_enabled);

    let undo = api
        .undo_activation_replace(UndoActivationReplaceRequestDto {
            operation_id: preview.operation_id.clone(),
        })
        .expect("typed Undo result");
    assert!(undo.undone);

    api.cancel_activation_replace(CancelActivationReplaceRequestDto {
        plan_token: preview.plan_token,
    })
    .expect("cancel after Undo is harmless");
    let finalize_error = api
        .finalize_activation_replace(FinalizeActivationReplaceRequestDto {
            operation_id: preview.operation_id,
        })
        .expect_err("an undone operation is already finished");
    assert_eq!(finalize_error.code, "plan_stale");
}

// -- Test doubles ----------------------------------------------------------

struct TestActivationStore {
    context: Mutex<ActivationContext>,
    agent_paths: Vec<ConfiguredAgentPath>,
    record: Mutex<Option<ActivationRecord>>,
}

impl TestActivationStore {
    fn new(context: ActivationContext) -> Self {
        let agent_paths = vec![ConfiguredAgentPath {
            agent_id: AgentId("claude-code".into()),
            skills_path: context.agent_skills_path.clone(),
        }];
        Self {
            context: Mutex::new(context),
            agent_paths,
            record: Mutex::new(None),
        }
    }

    fn recorded(&self) -> Option<ActivationRecord> {
        self.record.lock().expect("record lock").clone()
    }
}

impl ActivationStore for TestActivationStore {
    fn load(
        &self,
        _skill_id: &SkillId,
        _agent_id: &AgentId,
    ) -> Result<Option<ActivationContext>, ActivationStoreError> {
        Ok(Some(self.context.lock().expect("context lock").clone()))
    }

    fn configured_agent_paths(&self) -> Result<Vec<ConfiguredAgentPath>, ActivationStoreError> {
        Ok(self.agent_paths.clone())
    }

    fn record(&self, record: ActivationRecord) -> Result<u64, ActivationStoreError> {
        *self.record.lock().expect("record lock") = Some(record.clone());
        let mut context = self.context.lock().expect("context lock");
        context.desired_enabled = record.desired_enabled;
        context.expected_target_path = Some(record.expected_target_path);
        Ok(8)
    }

    fn desired_activations(&self) -> Result<Vec<DesiredActivation>, ActivationStoreError> {
        let context = self.context.lock().expect("context lock");
        if !context.desired_enabled {
            return Ok(Vec::new());
        }
        Ok(vec![DesiredActivation {
            skill_id: context.skill_id.clone(),
            agent_id: context.agent_id.clone(),
            expected_entry_path: context.agent_skills_path.join(&context.directory_name),
            expected_target_path: context
                .expected_target_path
                .clone()
                .expect("desired test Activation target"),
        }])
    }

    fn record_observations(
        &self,
        _observations: &[ActivationObservation],
    ) -> Result<u64, ActivationStoreError> {
        Ok(8)
    }
}

struct FailingActivationStore {
    context: ActivationContext,
    agent_paths: Vec<ConfiguredAgentPath>,
}

impl FailingActivationStore {
    fn new(context: ActivationContext) -> Self {
        let agent_paths = vec![ConfiguredAgentPath {
            agent_id: AgentId("claude-code".into()),
            skills_path: context.agent_skills_path.clone(),
        }];
        Self {
            context,
            agent_paths,
        }
    }
}

impl ActivationStore for FailingActivationStore {
    fn load(
        &self,
        _skill_id: &SkillId,
        _agent_id: &AgentId,
    ) -> Result<Option<ActivationContext>, ActivationStoreError> {
        Ok(Some(self.context.clone()))
    }

    fn configured_agent_paths(&self) -> Result<Vec<ConfiguredAgentPath>, ActivationStoreError> {
        Ok(self.agent_paths.clone())
    }

    fn record(&self, _record: ActivationRecord) -> Result<u64, ActivationStoreError> {
        Err(ActivationStoreError::Unavailable(
            "injected state failure".into(),
        ))
    }

    fn desired_activations(&self) -> Result<Vec<DesiredActivation>, ActivationStoreError> {
        Err(ActivationStoreError::Unavailable(
            "injected state failure".into(),
        ))
    }

    fn record_observations(
        &self,
        _observations: &[ActivationObservation],
    ) -> Result<u64, ActivationStoreError> {
        Err(ActivationStoreError::Unavailable(
            "injected state failure".into(),
        ))
    }
}

fn activation_context(
    home: &Path,
    desired_enabled: bool,
    expected_target_path: Option<PathBuf>,
) -> ActivationContext {
    ActivationContext {
        skill_id: SkillId("skill-authoring".into()),
        directory_name: "skill-authoring".into(),
        final_entity_path: seed_managed_skill(home),
        agent_id: AgentId("claude-code".into()),
        agent_name: "Claude Code".into(),
        agent_kind: AgentKind::ClaudePreset,
        agent_skills_path: claude_root(home),
        desired_enabled,
        expected_target_path,
    }
}
