use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use rusqlite::Connection;
use skill_man_lib::adapters::git_source::SystemGitSource;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::sqlite::SqliteCatalogStore;
use skill_man_lib::adapters::system_installer_lock_store::SystemInstallerLockStore;
use skill_man_lib::core::source_group_preview::{
    FetchLatestAndManageRequest, SourceGroupPreviewOutcome, SourceGroupPreviewService,
    SourceTrackingOverride,
};
use skill_man_lib::core::source_transition::{
    ConfirmSourceTransitionRequest, ConfirmSourceUpdateRequest, SourceTransitionError,
    SourceTransitionService,
};
use skill_man_lib::core::write_gate::WriteGate;
use skill_man_lib::seams::clock::Clock;
use skill_man_lib::seams::filesystem::FileSystem;
use skill_man_lib::seams::installer_lock_store::{
    InstallerLockError, InstallerLockStore, LockEntry, LockFileFault, LockFileReport,
};
use skill_man_lib::seams::source::{GitFetchReport, GitSource, GitTreeEntry, SourceError};
use skill_man_lib::seams::source_transition_store::{
    ExistingSourceFacts, ExistingSourceMember, SourceTransitionRecord, SourceTransitionStore,
    SourceTransitionStoreError,
};
use skill_man_lib::seams::source_update_store::SourceUpdateStore;

mod isolation_failure_tests {
    use super::*;
    use skill_man_lib::seams::filesystem::*;

    #[derive(Clone, Copy)]
    enum Failure {
        AfterRename,
        BeforeResultJournal,
    }

    struct IsolationFaultFs {
        inner: Arc<MacOsFileSystem>,
        failure: Failure,
        fail_name: String,
        renamed: AtomicBool,
        intent_was_durable: AtomicBool,
        library: PathBuf,
    }

    impl FileSystem for IsolationFaultFs {
        fn read_entropy(&self, buffer: &mut [u8]) -> Result<(), FileSystemError> {
            self.inner.read_entropy(buffer)
        }

        fn inspect_link_source(&self, path: &Path) -> Result<LinkSourceSnapshot, FileSystemError> {
            self.inner.inspect_link_source(path)
        }

        fn canonical_directory(&self, path: &Path) -> Result<PathBuf, FileSystemError> {
            self.inner.canonical_directory(path)
        }

        fn normalize_configured_path(&self, path: &Path) -> Result<PathBuf, FileSystemError> {
            self.inner.normalize_configured_path(path)
        }

        fn directory_fingerprint(
            &self,
            path: &Path,
        ) -> Result<DirectoryFingerprint, FileSystemError> {
            self.inner.directory_fingerprint(path)
        }

        fn activation_snapshot(
            &self,
            entry_path: &Path,
        ) -> Result<ActivationEntrySnapshot, FileSystemError> {
            self.inner.activation_snapshot(entry_path)
        }

        fn skill_directory_is_readable(&self, path: &Path) -> Result<bool, FileSystemError> {
            self.inner.skill_directory_is_readable(path)
        }

        fn skill_fingerprint(&self, path: &Path) -> Result<SkillFingerprint, FileSystemError> {
            self.inner.skill_fingerprint(path)
        }

        fn read_skill_document(&self, path: &Path) -> Result<String, FileSystemError> {
            self.inner.read_skill_document(path)
        }

        fn tree_hash(&self, path: &Path) -> Result<String, FileSystemError> {
            self.inner.tree_hash(path)
        }

        fn staged_tree_snapshot(&self, path: &Path) -> Result<StagedTreeSnapshot, FileSystemError> {
            self.inner.staged_tree_snapshot(path)
        }

        fn available_space(&self, path: &Path) -> Result<u64, FileSystemError> {
            self.inner.available_space(path)
        }

        fn staged_child_directories(&self, path: &Path) -> Result<Vec<PathBuf>, FileSystemError> {
            self.inner.staged_child_directories(path)
        }

        fn staged_has_skill_document(
            &self,
            directory: &Path,
            filename: &str,
        ) -> Result<bool, FileSystemError> {
            self.inner.staged_has_skill_document(directory, filename)
        }

        fn canonicalize_staged_path(&self, path: &Path) -> Result<PathBuf, FileSystemError> {
            self.inner.canonicalize_staged_path(path)
        }

        fn install_staged_skill(
            &self,
            staged_skill_path: &Path,
            final_entity_path: &Path,
            library_root: &Path,
            operation_id: &str,
            expected_staged_tree: &StagedTreeSnapshot,
        ) -> Result<DirectoryFingerprint, FileSystemError> {
            self.inner.install_staged_skill(
                staged_skill_path,
                final_entity_path,
                library_root,
                operation_id,
                expected_staged_tree,
            )
        }

        fn install_git_member_snapshot(
            &self,
            staged_skill_path: &Path,
            namespace_path: &Path,
            library_root: &Path,
            operation_id: &str,
            expected_staged_tree: &StagedTreeSnapshot,
        ) -> Result<DirectoryFingerprint, FileSystemError> {
            self.inner.install_git_member_snapshot(
                staged_skill_path,
                namespace_path,
                library_root,
                operation_id,
                expected_staged_tree,
            )
        }

        fn discard_staging(
            &self,
            staging_operation_root: &Path,
            library_root: &Path,
            expected: Option<&DirectoryFingerprint>,
        ) -> Result<(), FileSystemError> {
            self.inner
                .discard_staging(staging_operation_root, library_root, expected)
        }

        fn discard_installed_skill(
            &self,
            final_entity_path: &Path,
            library_root: &Path,
            expected: &DirectoryFingerprint,
        ) -> Result<(), FileSystemError> {
            self.inner
                .discard_installed_skill(final_entity_path, library_root, expected)
        }

        fn create_activation(
            &self,
            target_path: &Path,
            entry_path: &Path,
        ) -> Result<(), FileSystemError> {
            self.inner.create_activation(target_path, entry_path)
        }

        fn remove_activation(&self, entry_path: &Path) -> Result<(), FileSystemError> {
            self.inner.remove_activation(entry_path)
        }

        fn scan_skills_directory(
            &self,
            path: &Path,
        ) -> Result<Vec<ScannedSkillEntry>, FileSystemError> {
            self.inner.scan_skills_directory(path)
        }

        fn inspect_evidence_chain(&self, path: &Path) -> Result<EvidenceChain, FileSystemError> {
            self.inner.inspect_evidence_chain(path)
        }

        fn scan_skills_evidence(
            &self,
            path: &Path,
        ) -> Result<Vec<ScannedSkillEvidence>, FileSystemError> {
            self.inner.scan_skills_evidence(path)
        }

        fn create_temp_workspace(&self, purpose: &str) -> Result<PathBuf, FileSystemError> {
            self.inner.create_temp_workspace(purpose)
        }

        fn discard_temp_workspace(&self, path: &Path) -> Result<(), FileSystemError> {
            self.inner.discard_temp_workspace(path)
        }

        fn stage_external_directory(
            &self,
            source: &Path,
            staging_destination: &Path,
        ) -> Result<DirectoryFingerprint, FileSystemError> {
            self.inner
                .stage_external_directory(source, staging_destination)
        }

        fn create_adopt_staging_operation(
            &self,
            library_root: &Path,
            operation_id: &str,
        ) -> Result<DirectoryFingerprint, FileSystemError> {
            self.inner
                .create_adopt_staging_operation(library_root, operation_id)
        }

        fn restore_external_directory(
            &self,
            source: &Path,
            destination: &Path,
            expected: &DirectoryFingerprint,
        ) -> Result<(), FileSystemError> {
            self.inner
                .restore_external_directory(source, destination, expected)
        }

        fn apply_adopt_appearances(
            &self,
            appearances: &[AdoptAppearanceStep],
            activations: &[AdoptActivationStep],
        ) -> Result<(), FileSystemError> {
            self.inner.apply_adopt_appearances(appearances, activations)
        }

        fn write_adopt_journal(
            &self,
            library_root: &Path,
            journal: &AdoptJournal,
        ) -> Result<(), FileSystemError> {
            self.inner.write_adopt_journal(library_root, journal)
        }

        fn finish_adopt_journal(
            &self,
            library_root: &Path,
            operation_id: &str,
        ) -> Result<(), FileSystemError> {
            self.inner.finish_adopt_journal(library_root, operation_id)
        }

        fn recover_adopt_journals(
            &self,
            library_root: &Path,
            baselines: &[FileImportRecoveryBaseline],
            adopted_entities: &[FileImportRecoveryBaseline],
        ) -> Result<u32, FileSystemError> {
            self.inner
                .recover_adopt_journals(library_root, baselines, adopted_entities)
        }

        fn path_is_directory(&self, path: &Path) -> Result<bool, FileSystemError> {
            self.inner.path_is_directory(path)
        }

        fn path_is_occupied(&self, path: &Path) -> Result<bool, FileSystemError> {
            self.inner.path_is_occupied(path)
        }

        fn list_directory(&self, path: &Path) -> Result<Vec<DirectoryEntry>, FileSystemError> {
            self.inner.list_directory(path)
        }

        fn path_has_no_symlink_component(&self, path: &Path) -> Result<bool, FileSystemError> {
            self.inner.path_has_no_symlink_component(path)
        }

        fn remove_directory_verified_nofollow(
            &self,
            path: &Path,
            expected: &DirectoryFingerprint,
        ) -> Result<(), FileSystemError> {
            self.inner
                .remove_directory_verified_nofollow(path, expected)
        }

        fn restore_isolated_source(
            &self,
            isolated: &Path,
            source: &Path,
            expected_tree_hash: &str,
        ) -> Result<(), FileSystemError> {
            self.inner
                .restore_isolated_source(isolated, source, expected_tree_hash)
        }

        fn discard_isolated_source(&self, isolated: &Path) -> Result<(), FileSystemError> {
            self.inner.discard_isolated_source(isolated)
        }

        fn finish_source_transition_journal(
            &self,
            library_root: &Path,
            operation_id: &str,
        ) -> Result<(), FileSystemError> {
            self.inner
                .finish_source_transition_journal(library_root, operation_id)
        }

        fn list_source_transition_journals(
            &self,
            library_root: &Path,
        ) -> Result<Vec<SourceTransitionJournal>, FileSystemError> {
            self.inner.list_source_transition_journals(library_root)
        }

        fn isolate_external_source_verified(
            &self,
            source: &Path,
            operation_id: &str,
            expected: &DirectoryFingerprint,
            expected_tree_hash: &str,
        ) -> Result<PathBuf, FileSystemError> {
            let isolated = self.inner.isolate_external_source_verified(
                source,
                operation_id,
                expected,
                expected_tree_hash,
            )?;
            if source.file_name().and_then(|name| name.to_str()) == Some(self.fail_name.as_str()) {
                // Read what was durable BEFORE this method returns or fails.
                let journals = self.inner.list_source_transition_journals(&self.library)?;
                let journal = journals
                    .iter()
                    .find(|j| j.operation_id == operation_id)
                    .unwrap();
                let recorded = journal
                    .removed_members
                    .iter()
                    .find(|m| m.legacy_path == source)
                    .and_then(|m| m.isolated_path.as_ref())
                    .or_else(|| {
                        journal
                            .members
                            .iter()
                            .find(|m| m.canonical_entity.as_deref() == Some(source))
                            .and_then(|m| m.isolated_path.as_ref())
                    });
                self.intent_was_durable
                    .store(recorded == Some(&isolated), Ordering::SeqCst);
                self.renamed.store(true, Ordering::SeqCst);
                if matches!(self.failure, Failure::AfterRename) {
                    return Err(FileSystemError::Io {
                        operation: "injected failure after isolation rename",
                        path: isolated,
                        source: std::io::Error::other("post-rename fsync failed"),
                    });
                }
            }
            Ok(isolated)
        }

        fn write_source_transition_journal(
            &self,
            library_root: &Path,
            journal: &SourceTransitionJournal,
        ) -> Result<(), FileSystemError> {
            if matches!(self.failure, Failure::BeforeResultJournal)
                && self.renamed.load(Ordering::SeqCst)
            {
                // Simulate process death before the first post-rename write.
                panic!("injected crash before isolation result journal");
            }
            self.inner
                .write_source_transition_journal(library_root, journal)
        }
    }

    fn fault_service(
        fixture: &Fixture,
        filesystem: Arc<dyn FileSystem>,
    ) -> SourceTransitionService {
        SourceTransitionService::new(
            fixture.preview.clone(),
            fixture.source.clone(),
            fixture.locks.clone(),
            fixture.catalog.clone(),
            filesystem,
            Arc::new(FixtureClock),
            fixture.library.clone(),
            fixture.home.clone(),
            fixture.write_gate.clone(),
        )
    }

    #[test]
    fn isolation_rename_then_error_restores_original_before_archiving() {
        for name in ["design", "source"] {
            let fixture = disappeared_claim_fixture();
            let original_claims = fixture.locks.discover().unwrap()[0].entries.clone();
            let original = fixture
                .filesystem
                .canonical_directory(&fixture.home.join(".agents/skills").join(name))
                .unwrap();
            let identity = fixture.filesystem.directory_fingerprint(&original).unwrap();
            let bytes = std::fs::read(original.join("SKILL.md")).unwrap();
            let filesystem = Arc::new(IsolationFaultFs {
                inner: fixture.filesystem.clone(),
                failure: Failure::AfterRename,
                fail_name: name.into(),
                renamed: AtomicBool::new(false),
                intent_was_durable: AtomicBool::new(false),
                library: fixture.library.clone(),
            });
            let result =
                fault_service(&fixture, filesystem.clone()).confirm(force_confirmation(&fixture));
            assert!(result.is_err());
            assert!(
                filesystem.renamed.load(Ordering::SeqCst),
                "fault must occur after real rename"
            );
            assert!(
                filesystem.intent_was_durable.load(Ordering::SeqCst),
                "rename must have a durable recovery path"
            );
            assert_eq!(
                fixture.filesystem.directory_fingerprint(&original).unwrap(),
                identity
            );
            assert_eq!(std::fs::read(original.join("SKILL.md")).unwrap(), bytes);
            assert_eq!(
                fixture.locks.discover().unwrap()[0].entries,
                original_claims
            );
            assert_eq!(count(&open_catalog(&fixture), "skills"), 0);
            assert!(
                fixture
                    .home
                    .join(".cursor/skills/design/SKILL.md")
                    .is_file()
            );
            assert!(
                fixture
                    .filesystem
                    .list_source_transition_journals(&fixture.library)
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn isolation_crash_before_result_journal_recovers_or_preserves_changed_copy() {
        for (name, replace_copy) in [("design", false), ("source", false), ("design", true)] {
            let fixture = disappeared_claim_fixture();
            let original_claims = fixture.locks.discover().unwrap()[0].entries.clone();
            let original = fixture
                .filesystem
                .canonical_directory(&fixture.home.join(".agents/skills").join(name))
                .unwrap();
            let identity = fixture.filesystem.directory_fingerprint(&original).unwrap();
            let bytes = std::fs::read(original.join("SKILL.md")).unwrap();
            let filesystem = Arc::new(IsolationFaultFs {
                inner: fixture.filesystem.clone(),
                failure: Failure::BeforeResultJournal,
                fail_name: name.into(),
                renamed: AtomicBool::new(false),
                intent_was_durable: AtomicBool::new(false),
                library: fixture.library.clone(),
            });
            let transition = fault_service(&fixture, filesystem.clone());
            let request = force_confirmation(&fixture);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                transition.confirm(request)
            }));
            assert!(
                result.is_err(),
                "injected crash must prevent normal compensation"
            );
            assert!(filesystem.renamed.load(Ordering::SeqCst));
            assert!(filesystem.intent_was_durable.load(Ordering::SeqCst));
            assert!(!original.exists());
            let journals = fixture
                .filesystem
                .list_source_transition_journals(&fixture.library)
                .unwrap();
            assert_eq!(journals.len(), 1);
            let journal = &journals[0];
            let isolated = journal
                .removed_members
                .iter()
                .find(|m| m.legacy_path == original)
                .and_then(|m| m.isolated_path.clone())
                .or_else(|| {
                    journal
                        .members
                        .iter()
                        .find(|m| m.canonical_entity.as_ref() == Some(&original))
                        .and_then(|m| m.isolated_path.clone())
                })
                .unwrap();
            // Equal bytes on a different inode must not be accepted as our copy.
            let preserved = fixture.home.join("preserved-original");
            if replace_copy {
                std::fs::rename(&isolated, &preserved).unwrap();
                std::fs::create_dir(&isolated).unwrap();
                std::fs::write(isolated.join("SKILL.md"), &bytes).unwrap();
            }
            let recovery = SourceTransitionService::new(
                fixture.preview.clone(),
                Arc::new(NoFetchGitSource),
                fixture.locks.clone(),
                fixture.catalog.clone(),
                fixture.filesystem.clone(),
                Arc::new(OffsetClock),
                fixture.library.clone(),
                fixture.home.clone(),
                Arc::new(WriteGate::open_for_tests()),
            );
            let result = recovery.recover_pending(&fixture.library);
            if replace_copy {
                assert!(result.is_err());
                assert!(
                    !fixture
                        .filesystem
                        .list_source_transition_journals(&fixture.library)
                        .unwrap()
                        .is_empty()
                );
                assert!(isolated.is_dir());
                assert_eq!(std::fs::read(preserved.join("SKILL.md")).unwrap(), bytes);
            } else {
                result.unwrap();
                assert_eq!(
                    fixture.filesystem.directory_fingerprint(&original).unwrap(),
                    identity
                );
                assert_eq!(std::fs::read(original.join("SKILL.md")).unwrap(), bytes);
                assert!(
                    fixture
                        .home
                        .join(".cursor/skills/design/SKILL.md")
                        .is_file()
                );
                assert!(
                    fixture
                        .filesystem
                        .list_source_transition_journals(&fixture.library)
                        .unwrap()
                        .is_empty()
                );
            }
            assert_eq!(
                fixture.locks.discover().unwrap()[0].entries,
                original_claims
            );
            assert_eq!(count(&open_catalog(&fixture), "skills"), 0);
        }
    }
}

struct FixtureGitSource {
    inner: SystemGitSource,
    fixture_url: String,
    edit_during_stage: Option<PathBuf>,
}

#[derive(Default)]
struct EmptyLocks;

impl InstallerLockStore for EmptyLocks {
    fn discover(&self) -> Result<Vec<LockFileReport>, InstallerLockError> {
        Ok(Vec::new())
    }
}

struct FaultedLocks;

struct RefusingRelease(Arc<SystemInstallerLockStore>);

impl InstallerLockStore for RefusingRelease {
    fn discover(&self) -> Result<Vec<LockFileReport>, InstallerLockError> {
        self.0.discover()
    }
    // The default release refuses before touching the lock.
}

impl InstallerLockStore for FaultedLocks {
    fn discover(&self) -> Result<Vec<LockFileReport>, InstallerLockError> {
        Ok(vec![LockFileReport {
            path: PathBuf::from("/tmp/.skill-lock.json"),
            fingerprint: "faulted".into(),
            byte_len: 0,
            version: 4,
            fault: Some(LockFileFault::UnsupportedVersion(4)),
            entries: Vec::new(),
            entry_faults: Vec::new(),
        }])
    }
}

struct ReappearingLocks {
    calls: AtomicUsize,
    report: LockFileReport,
}

impl InstallerLockStore for ReappearingLocks {
    fn discover(&self) -> Result<Vec<LockFileReport>, InstallerLockError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) >= 3 {
            Ok(vec![self.report.clone()])
        } else {
            Ok(Vec::new())
        }
    }
}

/// Post-CAS recovery must use only the frozen journal and must never touch
/// the remote again, even to resolve the already-fixed commit.
struct NoFetchGitSource;

impl GitSource for NoFetchGitSource {
    fn fetch_mirror(&self, _url: &str, _mirror_dir: &Path) -> Result<GitFetchReport, SourceError> {
        Err(SourceError::Git("recovery must not fetch Git".into()))
    }

    fn resolve_commit(
        &self,
        _mirror_dir: &Path,
        _rev: &str,
    ) -> Result<Option<String>, SourceError> {
        Err(SourceError::Git("recovery must not resolve Git".into()))
    }

    fn list_tree(
        &self,
        _mirror_dir: &Path,
        _commit: &str,
    ) -> Result<Vec<GitTreeEntry>, SourceError> {
        Err(SourceError::Git("recovery must not list Git".into()))
    }

    fn read_blob(
        &self,
        _mirror_dir: &Path,
        _commit: &str,
        _path: &str,
        _max_bytes: usize,
    ) -> Result<Option<Vec<u8>>, SourceError> {
        Err(SourceError::Git("recovery must not read Git".into()))
    }

    fn tree_summary(
        &self,
        _mirror_dir: &Path,
        _commit: &str,
        _skill_path: &str,
    ) -> Result<String, SourceError> {
        Err(SourceError::Git("recovery must not summarize Git".into()))
    }

    fn stage_skill(
        &self,
        _mirror_dir: &Path,
        _commit: &str,
        _skill_path: &str,
        _destination: &Path,
    ) -> Result<(), SourceError> {
        Err(SourceError::Git("recovery must not stage Git".into()))
    }
}

impl FixtureGitSource {
    fn new(repository: &Path) -> Self {
        Self {
            inner: SystemGitSource::new(),
            fixture_url: format!("file://{}", repository.display()),
            edit_during_stage: None,
        }
    }
}

impl GitSource for FixtureGitSource {
    fn validate_home_cache(&self, home: &Path, mirror: &Path) -> Result<(), SourceError> {
        self.inner.validate_home_cache(home, mirror)
    }

    fn fetch_mirror(&self, _url: &str, mirror_dir: &Path) -> Result<GitFetchReport, SourceError> {
        self.inner.fetch_mirror(&self.fixture_url, mirror_dir)
    }

    fn resolve_commit(&self, mirror_dir: &Path, rev: &str) -> Result<Option<String>, SourceError> {
        self.inner.resolve_commit(mirror_dir, rev)
    }

    fn list_tree(&self, mirror_dir: &Path, commit: &str) -> Result<Vec<GitTreeEntry>, SourceError> {
        self.inner.list_tree(mirror_dir, commit)
    }

    fn read_blob(
        &self,
        mirror_dir: &Path,
        commit: &str,
        path: &str,
        max_bytes: usize,
    ) -> Result<Option<Vec<u8>>, SourceError> {
        self.inner.read_blob(mirror_dir, commit, path, max_bytes)
    }

    fn tree_summary(
        &self,
        mirror_dir: &Path,
        commit: &str,
        skill_path: &str,
    ) -> Result<String, SourceError> {
        self.inner.tree_summary(mirror_dir, commit, skill_path)
    }

    fn stage_skill(
        &self,
        mirror_dir: &Path,
        commit: &str,
        skill_path: &str,
        destination: &Path,
    ) -> Result<(), SourceError> {
        self.inner
            .stage_skill(mirror_dir, commit, skill_path, destination)?;
        if let Some(path) = &self.edit_during_stage {
            std::fs::write(path, "Concurrent edit during staging").unwrap();
        }
        Ok(())
    }

    fn list_tags(
        &self,
        mirror_dir: &Path,
    ) -> Result<Vec<skill_man_lib::seams::source::GitTagFact>, SourceError> {
        self.inner.list_tags(mirror_dir)
    }

    fn is_ancestor(
        &self,
        mirror_dir: &Path,
        ancestor: &str,
        descendant: &str,
    ) -> Result<bool, SourceError> {
        self.inner.is_ancestor(mirror_dir, ancestor, descendant)
    }
}

#[derive(Default)]
struct FixtureClock;

impl Clock for FixtureClock {
    fn monotonic_millis(&self) -> u128 {
        1
    }

    fn unix_epoch_nanos(&self) -> u128 {
        1_725_000_000_000_000_000
    }
}

struct FailOnceSourceStore {
    inner: Arc<SqliteCatalogStore>,
    fail_commit_once: AtomicBool,
}

impl FailOnceSourceStore {
    fn new(inner: Arc<SqliteCatalogStore>, fail_commit_once: bool) -> Self {
        Self {
            inner,
            fail_commit_once: AtomicBool::new(fail_commit_once),
        }
    }
}

impl SourceTransitionStore for FailOnceSourceStore {
    fn transition_activations_match(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<bool, SourceTransitionStoreError> {
        self.inner.transition_activations_match(record)
    }
    fn activation_targets(
        &self,
    ) -> Result<
        Vec<skill_man_lib::seams::source_transition_store::SourceTransitionTarget>,
        SourceTransitionStoreError,
    > {
        self.inner.activation_targets()
    }
    fn existing_current_members(
        &self,
        canonical_url: &str,
    ) -> Result<Option<Vec<ExistingSourceMember>>, SourceTransitionStoreError> {
        self.inner.existing_current_members(canonical_url)
    }

    fn existing_source(
        &self,
        remote_id: &str,
    ) -> Result<Option<ExistingSourceFacts>, SourceTransitionStoreError> {
        self.inner.existing_source(remote_id)
    }

    fn validate_new_source_transition(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<(), SourceTransitionStoreError> {
        self.inner.validate_new_source_transition(record)
    }

    fn commit_source_transition(
        &self,
        record: SourceTransitionRecord,
    ) -> Result<u64, SourceTransitionStoreError> {
        if self.fail_commit_once.swap(false, Ordering::SeqCst) {
            return Err(SourceTransitionStoreError::Unavailable(
                "injected post-CAS Catalog failure".into(),
            ));
        }
        self.inner.commit_source_transition(record)
    }

    fn source_transition_is_committed(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<bool, SourceTransitionStoreError> {
        self.inner.source_transition_is_committed(record)
    }

    fn undo_source_transition(
        &self,
        record: &SourceTransitionRecord,
    ) -> Result<u64, SourceTransitionStoreError> {
        self.inner.undo_source_transition(record)
    }
}

fn git(repository: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repository)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_file(root: &Path, path: &str, contents: &str) {
    let target = root.join(path);
    std::fs::create_dir_all(target.parent().expect("parent")).expect("create parent");
    std::fs::write(target, contents).expect("write fixture");
}

fn fixture_repository(root: &Path) -> PathBuf {
    let repository = root.join("source-repository");
    std::fs::create_dir_all(&repository).expect("repository");
    write_file(
        &repository,
        "skills/source/SKILL.md",
        "---\nname: Source Root\ndescription: Root member\n---\n# Root\n",
    );
    write_file(
        &repository,
        "skills/beta/SKILL.md",
        "---\nname: Source Beta\ndescription: Nested member\n---\n# Beta\n",
    );
    git(&repository, &["init", "-q", "-b", "main"]);
    git(
        &repository,
        &["config", "user.email", "fixture@example.com"],
    );
    git(&repository, &["config", "user.name", "Fixture"]);
    git(&repository, &["add", "-A"]);
    git(
        &repository,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "release",
        ],
    );
    repository
}

struct OffsetClock;

impl Clock for OffsetClock {
    fn monotonic_millis(&self) -> u128 {
        2
    }

    fn unix_epoch_nanos(&self) -> u128 {
        1_725_000_000_000_000_001
    }
}

struct Fixture {
    _workspace: tempfile::TempDir,
    home: PathBuf,
    library: PathBuf,
    preview: Arc<SourceGroupPreviewService>,
    source: Arc<dyn GitSource>,
    locks: Arc<SystemInstallerLockStore>,
    filesystem: Arc<MacOsFileSystem>,
    catalog: Arc<SqliteCatalogStore>,
    write_gate: Arc<WriteGate>,
}

fn fixture() -> Fixture {
    let workspace = tempfile::tempdir().expect("workspace");
    let repository = fixture_repository(workspace.path());
    let home = workspace.path().join("home");
    let library = workspace.path().join("library");
    std::fs::create_dir_all(&library).expect("Library");
    let source_root = home.join(".agents/skills");
    write_file(
        &source_root.join("source"),
        "SKILL.md",
        "---\nname: Source Root\ndescription: Root member\n---\n# Root\n",
    );
    write_file(
        &source_root.join("beta"),
        "SKILL.md",
        "---\nname: Source Beta\ndescription: Nested member\n---\n# Beta\n",
    );
    let lock_path = home.join(".agents/.skill-lock.json");
    std::fs::create_dir_all(lock_path.parent().expect("lock parent")).expect("lock parent");
    std::fs::write(
        &lock_path,
        r#"{
  "version": 3,
  "skills": {
    "source": {
      "sourceType": "git",
      "source": "acme/source",
      "sourceUrl": "https://example.com/acme/source",
      "ref": "main",
      "skillPath": "skills/source",
      "skillFolderHash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    },
    "beta": {
      "sourceType": "git",
      "source": "acme/source",
      "sourceUrl": "https://example.com/acme/source",
      "ref": "main",
      "skillPath": "skills/beta",
      "skillFolderHash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    }
  }
}"#,
    )
    .expect("write lock");
    let source: Arc<dyn GitSource> = Arc::new(FixtureGitSource::new(&repository));
    let locks = Arc::new(SystemInstallerLockStore::new(home.clone()));
    let preview = Arc::new(SourceGroupPreviewService::new(
        source.clone(),
        locks.clone(),
    ));
    let filesystem = Arc::new(MacOsFileSystem::new(home.clone()));
    let catalog =
        Arc::new(SqliteCatalogStore::open(&library.join("skill-man.sqlite3")).expect("Catalog"));
    let write_gate = Arc::new(WriteGate::open_for_tests());
    Fixture {
        _workspace: workspace,
        home,
        library,
        preview,
        source,
        locks,
        filesystem,
        catalog,
        write_gate,
    }
}

fn confirmation(fixture: &Fixture) -> ConfirmSourceTransitionRequest {
    let outcome = fixture
        .preview
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: "https://example.com/acme/source".into(),
            tracking_policy: Some(SourceTrackingOverride {
                mode: "branch".into(),
                value: Some("main".into()),
            }),
        })
        .expect("preview");
    let SourceGroupPreviewOutcome::Preview(preview) = outcome else {
        panic!("expected clean preview");
    };
    assert!(!preview.members.is_empty());
    ConfirmSourceTransitionRequest {
        expected_removed_claims: Vec::new(),
        source_type: "git".into(),
        source_url: preview.source_url,
        tracking_policy: Some(SourceTrackingOverride {
            mode: "branch".into(),
            value: Some("main".into()),
        }),
        expected_selected_ref: preview.policy.selected_ref,
        expected_resolved_commit: preview.policy.resolved_commit,
    }
}

fn service(fixture: &Fixture, store: Arc<dyn SourceTransitionStore>) -> SourceTransitionService {
    service_with_locks(fixture, fixture.locks.clone(), store)
}

fn service_with_locks(
    fixture: &Fixture,
    locks: Arc<dyn InstallerLockStore>,
    store: Arc<dyn SourceTransitionStore>,
) -> SourceTransitionService {
    SourceTransitionService::new(
        fixture.preview.clone(),
        fixture.source.clone(),
        locks,
        store,
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
        fixture.write_gate.clone(),
    )
}

fn open_catalog(fixture: &Fixture) -> Connection {
    Connection::open(fixture.library.join("skill-man.sqlite3")).expect("Catalog connection")
}

fn count(connection: &Connection, table: &str) -> i64 {
    connection
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_or_else(|error| panic!("count rows in {table}: {error}"))
}

fn namespace_members(fixture: &Fixture, remote_id: &str) -> PathBuf {
    fixture.library.join("skills/git").join(remote_id)
}

fn configure_target(fixture: &Fixture, path: &Path, name: &str) {
    use skill_man_lib::adapters::agent_configuration_fs::MacOsAgentConfigurationFileSystem;
    use skill_man_lib::core::agent_configuration::{
        AgentConfigurationDraft, AgentConfigurationService, AgentRootDraft, AgentRootRole,
        PresetRegistry,
    };
    let service = AgentConfigurationService::new(
        fixture.catalog.clone(),
        Arc::new(MacOsAgentConfigurationFileSystem::new(fixture.home.clone())),
        fixture.write_gate.clone(),
        PresetRegistry::system(),
    );
    let plan = service
        .plan_create(AgentConfigurationDraft {
            preset_key: None,
            name: name.into(),
            roots: vec![AgentRootDraft {
                configured_path: path.into(),
                role: AgentRootRole::ActivationTarget,
            }],
            project_skills_dir: None,
        })
        .expect("plan target");
    service.apply(&plan.plan_token).expect("configure target");
}

fn disappeared_claim_fixture() -> Fixture {
    let fixture = fixture();
    let root = fixture.home.join(".agents/skills");
    std::fs::rename(root.join("beta"), root.join("design")).unwrap();
    write_file(
        &root.join("design"),
        "SKILL.md",
        "---\nname: design\ndescription: Old local design\n---\nold user bytes\n",
    );
    let lock_path = fixture.home.join(".agents/.skill-lock.json");
    let mut lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lock_path).unwrap()).unwrap();
    let mut entry = lock["skills"]
        .as_object_mut()
        .unwrap()
        .remove("beta")
        .unwrap();
    entry["skillPath"] = "skills/design/SKILL.md".into();
    lock["skills"]["design"] = entry;
    std::fs::write(lock_path, serde_json::to_vec(&lock).unwrap()).unwrap();
    let repository = fixture._workspace.path().join("source-repository");
    std::fs::rename(repository.join("skills/beta"), repository.join("skills/ui")).unwrap();
    write_file(
        &repository.join("skills/ui"),
        "SKILL.md",
        "---\nname: ui\ndescription: New remote UI\n---\nnew ui bytes\n",
    );
    git(&repository, &["add", "-A"]);
    git(
        &repository,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "replace design with ui",
        ],
    );
    configure_target(&fixture, &root, "General");
    let aliases = fixture.home.join(".cursor/skills");
    std::fs::create_dir_all(&aliases).unwrap();
    std::os::unix::fs::symlink("../../.agents/skills/design", aliases.join("design")).unwrap();
    configure_target(&fixture, &aliases, "Cursor");
    fixture
}

fn force_confirmation(fixture: &Fixture) -> ConfirmSourceTransitionRequest {
    ConfirmSourceTransitionRequest {
        expected_removed_claims: vec!["design".into()],
        ..confirmation(fixture)
    }
}

#[test]
fn disappeared_claim_preview_and_exact_force_acknowledgment_agree() {
    let fixture = disappeared_claim_fixture();
    let request = force_confirmation(&fixture);
    let SourceGroupPreviewOutcome::Preview(preview) = fixture
        .preview
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: request.source_type.clone(),
            source_url: request.source_url.clone(),
            tracking_policy: request.tracking_policy.clone(),
        })
        .unwrap()
    else {
        panic!("preview");
    };
    assert_eq!(preview.removed_external_claims, vec!["design"]);
    assert_eq!(preview.added_member_names, vec!["ui"]);
    let lock_path = fixture.home.join(".agents/.skill-lock.json");
    let before = std::fs::read(&lock_path).unwrap();
    for names in [
        vec![],
        vec!["ui".into()],
        vec!["design".into(), "design".into()],
        vec!["design".into(), "source".into()],
    ] {
        let error = service(&fixture, fixture.catalog.clone())
            .confirm(ConfirmSourceTransitionRequest {
                expected_removed_claims: names,
                ..request.clone()
            })
            .unwrap_err();
        assert!(matches!(error, SourceTransitionError::Validation(_)));
        assert_eq!(std::fs::read(&lock_path).unwrap(), before);
        assert!(
            fixture
                .home
                .join(".agents/skills/design/SKILL.md")
                .is_file()
        );
        assert!(fixture.home.join(".cursor/skills/design").is_symlink());
        assert_eq!(count(&open_catalog(&fixture), "skills"), 0);
        assert!(
            fixture
                .filesystem
                .list_source_transition_journals(&fixture.library)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn forced_disappeared_claim_removes_old_use_keeps_unchanged_and_undo_restores_all() {
    for recover_undo in [false, true] {
        let fixture = disappeared_claim_fixture();
        let original_claims = fixture.locks.discover().unwrap()[0].entries.clone();
        let transition = service(&fixture, fixture.catalog.clone());
        let result = transition.confirm(force_confirmation(&fixture)).unwrap();
        let root = fixture.home.join(".agents/skills");
        assert!(!root.join("design").exists());
        assert!(!fixture.home.join(".cursor/skills/design").is_symlink());
        assert!(!root.join("ui").exists());
        assert!(root.join("source").is_symlink());
        assert_eq!(count(&open_catalog(&fixture), "activations"), 1);
        assert_eq!(count(&open_catalog(&fixture), "skills"), 2);
        assert!(
            fixture
                .catalog
                .read_current(&result.remote_id)
                .unwrap()
                .unwrap()
                .members
                .iter()
                .any(|m| m.skill_path == "skills/ui")
        );
        if recover_undo {
            let mut journal = fixture
                .filesystem
                .list_source_transition_journals(&fixture.library)
                .unwrap()
                .remove(0);
            journal.phase = skill_man_lib::seams::filesystem::SourceTransitionPhase::Undoing;
            fixture
                .filesystem
                .write_source_transition_journal(&fixture.library, &journal)
                .unwrap();
            transition.recover_pending(&fixture.library).unwrap();
        } else {
            transition.undo(&result.operation_id).unwrap();
        }
        assert!(
            std::fs::read_to_string(root.join("design/SKILL.md"))
                .unwrap()
                .contains("old user bytes")
        );
        assert_eq!(
            std::fs::read_link(fixture.home.join(".cursor/skills/design")).unwrap(),
            PathBuf::from("../../.agents/skills/design")
        );
        assert!(root.join("source/SKILL.md").is_file());
        assert_eq!(count(&open_catalog(&fixture), "skills"), 0);
        assert_eq!(fixture.locks.discover().unwrap()[0].entries.len(), 2);
        assert_eq!(
            fixture.locks.discover().unwrap()[0].entries,
            original_claims
        );
    }
}

#[test]
fn forced_disappeared_claim_rejects_removed_bytes_changed_during_staging() {
    let mut fixture = disappeared_claim_fixture();
    let removed_document = fixture.home.join(".agents/skills/design/SKILL.md");
    let mut source = FixtureGitSource::new(&fixture._workspace.path().join("source-repository"));
    source.edit_during_stage = Some(removed_document.clone());
    fixture.source = Arc::new(source);
    let transition = service(&fixture, fixture.catalog.clone());
    assert!(transition.confirm(force_confirmation(&fixture)).is_err());
    assert_eq!(
        std::fs::read_to_string(removed_document).unwrap(),
        "Concurrent edit during staging"
    );
    assert_eq!(fixture.locks.discover().unwrap()[0].entries.len(), 2);
    assert_eq!(count(&open_catalog(&fixture), "skills"), 0);
    assert!(fixture.home.join(".cursor/skills/design").is_symlink());
}

#[test]
fn forced_disappeared_claim_recovers_post_cas_without_remote_reads() {
    let fixture = disappeared_claim_fixture();
    let transition = service(
        &fixture,
        Arc::new(FailOnceSourceStore::new(fixture.catalog.clone(), true)),
    );
    assert!(matches!(
        transition.confirm(force_confirmation(&fixture)),
        Err(SourceTransitionError::RecoveryRequired(_))
    ));
    let recovery = SourceTransitionService::new(
        fixture.preview.clone(),
        Arc::new(NoFetchGitSource),
        fixture.locks.clone(),
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(OffsetClock),
        fixture.library.clone(),
        fixture.home.clone(),
        Arc::new(WriteGate::open_for_tests()),
    );
    recovery.recover_pending(&fixture.library).unwrap();
    assert_eq!(count(&open_catalog(&fixture), "skills"), 2);
    assert_eq!(count(&open_catalog(&fixture), "activations"), 1);
    assert!(!fixture.home.join(".agents/skills/design").exists());
    assert!(!fixture.home.join(".cursor/skills/design").is_symlink());
    assert!(!fixture.home.join(".agents/skills/ui").exists());
}

#[test]
fn forced_disappeared_claim_pre_cas_failure_restores_entities_and_aliases() {
    let fixture = disappeared_claim_fixture();
    let transition = service_with_locks(
        &fixture,
        Arc::new(RefusingRelease(fixture.locks.clone())),
        fixture.catalog.clone(),
    );
    assert!(transition.confirm(force_confirmation(&fixture)).is_err());
    assert!(
        fixture
            .home
            .join(".agents/skills/design/SKILL.md")
            .is_file()
    );
    assert!(
        fixture
            .home
            .join(".cursor/skills/design/SKILL.md")
            .is_file()
    );
    assert_eq!(count(&open_catalog(&fixture), "skills"), 0);
    assert_eq!(fixture.locks.discover().unwrap()[0].entries.len(), 2);
}

#[test]
fn forced_disappeared_claim_refuses_symlink_entity_and_changed_undo_alias() {
    let fixture = disappeared_claim_fixture();
    let root = fixture.home.join(".agents/skills");
    let original = root.join("design");
    let saved = fixture.home.join("saved-design");
    std::fs::rename(&original, &saved).unwrap();
    std::os::unix::fs::symlink(&saved, &original).unwrap();
    let transition = service(&fixture, fixture.catalog.clone());
    assert!(transition.confirm(force_confirmation(&fixture)).is_err());
    assert_eq!(fixture.locks.discover().unwrap()[0].entries.len(), 2);
    std::fs::remove_file(&original).unwrap();
    std::fs::rename(&saved, &original).unwrap();
    let result = transition.confirm(force_confirmation(&fixture)).unwrap();
    let alias = fixture.home.join(".cursor/skills/design");
    std::os::unix::fs::symlink("unrelated-owner", &alias).unwrap();
    assert!(transition.undo(&result.operation_id).is_err());
    assert_eq!(
        std::fs::read_link(alias).unwrap(),
        PathBuf::from("unrelated-owner")
    );
    assert_eq!(count(&open_catalog(&fixture), "skills"), 2);
    assert!(!original.exists());
}

#[test]
fn forced_disappeared_claim_recovery_rejects_forged_removal_and_changed_copy() {
    for tamper_path in [true, false] {
        let fixture = disappeared_claim_fixture();
        let transition = service(
            &fixture,
            Arc::new(FailOnceSourceStore::new(fixture.catalog.clone(), true)),
        );
        assert!(transition.confirm(force_confirmation(&fixture)).is_err());
        let mut journal = fixture
            .filesystem
            .list_source_transition_journals(&fixture.library)
            .unwrap()
            .remove(0);
        if tamper_path {
            journal.removed_members[0].legacy_path = fixture.home.join(".agents/skills/unrelated");
            fixture
                .filesystem
                .write_source_transition_journal(&fixture.library, &journal)
                .unwrap();
        } else {
            write_file(
                journal.removed_members[0].isolated_path.as_ref().unwrap(),
                "SKILL.md",
                "changed preserved bytes",
            );
        }
        assert!(transition.recover_pending(&fixture.library).is_err());
        assert_eq!(count(&open_catalog(&fixture), "skills"), 0);
    }
}

#[test]
fn replacement_restores_previous_global_use_and_undo_restores_external_directory() {
    let fixture = fixture();
    let root = fixture.home.join(".agents/skills");
    configure_target(&fixture, &root, "General");
    let transition = service(&fixture, fixture.catalog.clone());
    let result = transition
        .confirm(confirmation(&fixture))
        .expect("replace source");
    let db = open_catalog(&fixture);
    assert_eq!(
        count(&db, "activations"),
        2,
        "previous target use must become managed Activations"
    );
    for name in ["source", "beta"] {
        let target = std::fs::read_link(root.join(name)).expect("direct activation link");
        assert!(target.starts_with(namespace_members(&fixture, &result.remote_id)));
        assert!(target.join("SKILL.md").is_file());
    }
    transition
        .undo(&result.operation_id)
        .expect("undo replacement");
    assert_eq!(count(&db, "activations"), 0);
    for name in ["source", "beta"] {
        assert!(!root.join(name).is_symlink());
        assert!(root.join(name).join("SKILL.md").is_file());
    }
}

#[test]
fn replacement_retargets_only_existing_aliases_and_undo_restores_raw_relative_link() {
    let fixture = fixture();
    let root = fixture.home.join(".cursor/skills");
    std::fs::create_dir_all(&root).unwrap();
    let previous = PathBuf::from("../../.agents/skills/source");
    std::os::unix::fs::symlink(&previous, root.join("source")).unwrap();
    configure_target(&fixture, &root, "Cursor");
    let transition = service(&fixture, fixture.catalog.clone());
    let result = transition.confirm(confirmation(&fixture)).expect("replace");
    assert_eq!(count(&open_catalog(&fixture), "activations"), 1);
    assert!(
        std::fs::read_link(root.join("source"))
            .unwrap()
            .starts_with(namespace_members(&fixture, &result.remote_id))
    );
    assert!(
        !root.join("beta").exists(),
        "unused members must not be enabled"
    );
    transition.undo(&result.operation_id).expect("undo aliases");
    assert_eq!(std::fs::read_link(root.join("source")).unwrap(), previous);
    assert!(root.join("source/SKILL.md").is_file());
}

#[test]
fn replacement_undo_refuses_changed_activation_without_deleting_catalog_or_files() {
    let fixture = fixture();
    let root = fixture.home.join(".agents/skills");
    configure_target(&fixture, &root, "General");
    let transition = service(&fixture, fixture.catalog.clone());
    let result = transition.confirm(confirmation(&fixture)).unwrap();
    std::fs::remove_file(root.join("source")).unwrap();
    write_file(&root.join("source"), "SKILL.md", "Unrelated occupant");
    assert!(transition.undo(&result.operation_id).is_err());
    assert_eq!(count(&open_catalog(&fixture), "activations"), 2);
    assert_eq!(
        std::fs::read_to_string(root.join("source/SKILL.md")).unwrap(),
        "Unrelated occupant"
    );
    assert!(namespace_members(&fixture, &result.remote_id).is_dir());
}

#[test]
fn pre_cas_failure_preserves_previous_use_without_activation_records() {
    let fixture = fixture();
    let root = fixture.home.join(".agents/skills");
    configure_target(&fixture, &root, "General");
    let transition = service_with_locks(
        &fixture,
        Arc::new(RefusingRelease(fixture.locks.clone())),
        fixture.catalog.clone(),
    );
    assert!(transition.confirm(confirmation(&fixture)).is_err());
    assert_eq!(count(&open_catalog(&fixture), "activations"), 0);
    for name in ["source", "beta"] {
        assert!(!root.join(name).is_symlink());
        assert!(root.join(name).join("SKILL.md").is_file());
    }
}

#[test]
fn recovery_does_not_reenable_a_removed_committed_activation() {
    let fixture = fixture();
    let root = fixture.home.join(".agents/skills");
    configure_target(&fixture, &root, "General");
    let transition = service(&fixture, fixture.catalog.clone());
    transition.confirm(confirmation(&fixture)).unwrap();
    std::fs::remove_file(root.join("source")).unwrap();
    assert!(transition.recover_pending(&fixture.library).is_err());
    assert!(!root.join("source").exists());
    assert_eq!(count(&open_catalog(&fixture), "skills"), 2);
}

#[test]
fn recovery_rejects_mismatched_release_before_recreating_activation() {
    let fixture = fixture();
    let root = fixture.home.join(".agents/skills");
    configure_target(&fixture, &root, "General");
    let transition = service(&fixture, fixture.catalog.clone());
    transition.confirm(confirmation(&fixture)).unwrap();
    std::fs::remove_file(root.join("source")).unwrap();
    let mut journal = fixture
        .filesystem
        .list_source_transition_journals(&fixture.library)
        .unwrap()
        .remove(0);
    journal.selected_ref = "different-ref".into();
    fixture
        .filesystem
        .write_source_transition_journal(&fixture.library, &journal)
        .unwrap();
    assert!(transition.recover_pending(&fixture.library).is_err());
    assert!(!root.join("source").exists());
    assert_eq!(count(&open_catalog(&fixture), "skills"), 2);
}

#[test]
fn undo_refuses_changed_activation_intent_without_removing_source() {
    let fixture = fixture();
    let root = fixture.home.join(".agents/skills");
    configure_target(&fixture, &root, "General");
    let transition = service(&fixture, fixture.catalog.clone());
    let result = transition.confirm(confirmation(&fixture)).unwrap();
    let db = open_catalog(&fixture);
    db.execute("UPDATE activations SET desired_enabled = 0", [])
        .unwrap();
    assert!(transition.undo(&result.operation_id).is_err());
    assert_eq!(count(&db, "skills"), 2);
    assert!(root.join("source").is_symlink());
}

#[test]
fn undo_rejects_tampered_previous_alias_target_before_any_mutation() {
    let fixture = fixture();
    let root = fixture.home.join(".cursor/skills");
    std::fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::symlink(
        fixture.home.join(".agents/skills/source"),
        root.join("source"),
    )
    .unwrap();
    configure_target(&fixture, &root, "Cursor");
    let transition = service(&fixture, fixture.catalog.clone());
    let result = transition.confirm(confirmation(&fixture)).unwrap();
    let original_link = std::fs::read_link(root.join("source")).unwrap();
    let mut journal = fixture
        .filesystem
        .list_source_transition_journals(&fixture.library)
        .unwrap()
        .remove(0);
    let unrelated = fixture.home.join("unrelated/source");
    write_file(&unrelated, "SKILL.md", "Unrelated Skill");
    journal.activations[0].previous_target = Some(unrelated);
    fixture
        .filesystem
        .write_source_transition_journal(&fixture.library, &journal)
        .unwrap();
    assert!(transition.undo(&result.operation_id).is_err());
    assert_eq!(
        std::fs::read_link(root.join("source")).unwrap(),
        original_link
    );
    assert_eq!(count(&open_catalog(&fixture), "activations"), 1);
}

#[test]
fn relocated_root_claim_replaces_original_entry_and_recovers_without_fetch() {
    for fail_commit in [false, true] {
        let fixture = fixture();
        configure_target(&fixture, &fixture.home.join(".agents/skills"), "General");
        let repository = fixture._workspace.path().join("source-repository");
        let document =
            "---\nname: source\ndescription: Skill moved from the repository root\n---\nSkill\n";
        write_file(&repository, "skills/source/SKILL.md", document);
        write_file(
            &fixture.home.join(".agents/skills/source"),
            "SKILL.md",
            document,
        );
        git(&repository, &["add", "-A"]);
        git(
            &repository,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "moved root skill",
            ],
        );
        let lock_path = fixture.home.join(".agents/.skill-lock.json");
        let mut lock: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&lock_path).unwrap()).unwrap();
        lock["skills"]["source"]["skillPath"] = "SKILL.md".into();
        std::fs::write(&lock_path, serde_json::to_vec(&lock).unwrap()).unwrap();
        let store: Arc<dyn SourceTransitionStore> = Arc::new(FailOnceSourceStore::new(
            fixture.catalog.clone(),
            fail_commit,
        ));
        let transition = service(&fixture, store.clone());
        let result = transition.confirm(confirmation(&fixture));
        if fail_commit {
            assert!(
                matches!(result, Err(SourceTransitionError::RecoveryRequired(_))),
                "{result:?}"
            );
        } else {
            result.expect("old root claim maps to current named member");
        }
        let journal = fixture
            .filesystem
            .list_source_transition_journals(&fixture.library)
            .unwrap()
            .remove(0);
        assert_eq!(
            journal.relocated_claim_paths.get("source").unwrap(),
            "skills/source"
        );
        assert_eq!(
            journal
                .lock_entries
                .iter()
                .find(|e| e.name == "source")
                .unwrap()
                .skill_path,
            "SKILL.md"
        );
        if fail_commit {
            SourceTransitionService::new(
                fixture.preview.clone(),
                Arc::new(NoFetchGitSource),
                fixture.locks.clone(),
                store,
                fixture.filesystem.clone(),
                Arc::new(FixtureClock),
                fixture.library.clone(),
                fixture.home.clone(),
                fixture.write_gate.clone(),
            )
            .recover_pending(&fixture.library)
            .expect("frozen relocation recovery");
        }
        let member = journal
            .members
            .iter()
            .find(|m| m.skill_path == "skills/source")
            .unwrap();
        assert_eq!(
            std::fs::read_link(fixture.home.join(".agents/skills/source")).unwrap(),
            member.namespace_path
        );
        if !fail_commit {
            let mut tampered = journal.clone();
            tampered
                .relocated_claim_paths
                .insert("source".into(), "skills/beta".into());
            fixture
                .filesystem
                .write_source_transition_journal(&fixture.library, &tampered)
                .unwrap();
            assert!(transition.undo(&journal.operation_id).is_err());
            fixture
                .filesystem
                .write_source_transition_journal(&fixture.library, &journal)
                .unwrap();
            transition
                .undo(&journal.operation_id)
                .expect("restore original lock path and local Skill");
            assert_eq!(
                std::fs::read_to_string(fixture.home.join(".agents/skills/source/SKILL.md"))
                    .unwrap(),
                document
            );
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&std::fs::read(&lock_path).unwrap())
                    .unwrap(),
                lock
            );
        }
    }
}

#[test]
fn installer_names_must_not_alias_the_same_repository_member() {
    let fixture = fixture();
    let lock_path = fixture.home.join(".agents/.skill-lock.json");
    let mut lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lock_path).unwrap()).unwrap();
    lock["skills"]["beta"]["skillPath"] = "skills/source/SKILL.md".into();
    let bytes = serde_json::to_vec(&lock).unwrap();
    std::fs::write(&lock_path, &bytes).unwrap();
    assert!(matches!(
        service(&fixture, fixture.catalog.clone()).confirm(confirmation(&fixture)),
        Err(SourceTransitionError::Validation(_))
    ));
    assert_eq!(std::fs::read(&lock_path).unwrap(), bytes);
    assert!(
        fixture
            .home
            .join(".agents/skills/source/SKILL.md")
            .is_file()
    );
    assert!(fixture.home.join(".agents/skills/beta/SKILL.md").is_file());
    assert_eq!(count(&open_catalog(&fixture), "skills"), 0);
}

#[test]
fn installer_entry_name_can_differ_from_repository_directory() {
    for fail_commit in [false, true] {
        let fixture = fixture();
        configure_target(&fixture, &fixture.home.join(".agents/skills"), "General");
        let lock_path = fixture.home.join(".agents/.skill-lock.json");
        let mut lock: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&lock_path).unwrap()).unwrap();
        let claim = lock["skills"]
            .as_object_mut()
            .unwrap()
            .remove("source")
            .unwrap();
        lock["skills"]["mmx-cli"] = claim;
        std::fs::write(&lock_path, serde_json::to_vec(&lock).unwrap()).unwrap();
        std::fs::rename(
            fixture.home.join(".agents/skills/source"),
            fixture.home.join(".agents/skills/mmx-cli"),
        )
        .unwrap();
        let store: Arc<dyn SourceTransitionStore> = Arc::new(FailOnceSourceStore::new(
            fixture.catalog.clone(),
            fail_commit,
        ));
        let transition =
            service(&fixture, store.clone()).with_update_store(fixture.catalog.clone());
        let result = transition.confirm(confirmation(&fixture));
        let journal = fixture
            .filesystem
            .list_source_transition_journals(&fixture.library)
            .unwrap()
            .remove(0);
        if fail_commit {
            assert!(matches!(
                result,
                Err(SourceTransitionError::RecoveryRequired(_))
            ));
            SourceTransitionService::new(
                fixture.preview.clone(),
                Arc::new(NoFetchGitSource),
                fixture.locks.clone(),
                store,
                fixture.filesystem.clone(),
                Arc::new(FixtureClock),
                fixture.library.clone(),
                fixture.home.clone(),
                fixture.write_gate.clone(),
            )
            .recover_pending(&fixture.library)
            .expect("recover installer name without refetch");
        } else {
            result.expect(
                "repository path identifies the member independently of installer entry name",
            );
        }
        let member = journal
            .members
            .iter()
            .find(|m| m.skill_path == "skills/source")
            .unwrap();
        assert_eq!(member.directory_name, "mmx-cli");
        assert_eq!(
            std::fs::read_link(fixture.home.join(".agents/skills/mmx-cli")).unwrap(),
            member.namespace_path
        );
        assert!(!fixture.home.join(".agents/skills/source").exists());
        if fail_commit {
            continue;
        } // Recovery finalizes and archives its journal.
        let repository = fixture._workspace.path().join("source-repository");
        write_file(
            &repository,
            "skills/source/SKILL.md",
            "---\nname: Different display label\ndescription: Updated member\n---\nUpdated\n",
        );
        git(&repository, &["add", "-A"]);
        git(
            &repository,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "update renamed entry",
            ],
        );
        let request = confirmation(&fixture);
        let updated = transition
            .confirm_update(ConfirmSourceUpdateRequest {
                remote_id: journal.remote_id.clone(),
                tracking_policy: request.tracking_policy,
                expected_selected_ref: request.expected_selected_ref,
                expected_resolved_commit: request.expected_resolved_commit,
            })
            .expect("update preserves installer directory identity");
        let current = fixture
            .catalog
            .read_current(&journal.remote_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            current
                .members
                .iter()
                .find(|m| m.skill_path == "skills/source")
                .unwrap()
                .directory_name,
            "mmx-cli"
        );
        assert_eq!(
            std::fs::read_link(fixture.home.join(".agents/skills/mmx-cli")).unwrap(),
            member.namespace_path
        );
        transition
            .undo(&updated.operation_id)
            .expect("undo update preserves installer name");
        transition
            .undo(&journal.operation_id)
            .expect("undo keeps original installer name");
        assert!(
            fixture
                .home
                .join(".agents/skills/mmx-cli/SKILL.md")
                .is_file()
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&lock_path).unwrap())
                .unwrap(),
            lock
        );
    }
}

#[test]
fn document_path_claims_can_transition_and_undo_without_rewriting_claims() {
    let fixture = fixture();
    let lock_path = fixture.home.join(".agents/.skill-lock.json");
    let mut lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lock_path).unwrap()).unwrap();
    for name in ["source", "beta"] {
        lock["skills"][name]["skillPath"] = format!("skills/{name}/SKILL.md").into();
    }
    std::fs::write(&lock_path, serde_json::to_vec(&lock).unwrap()).unwrap();
    let service = service(&fixture, fixture.catalog.clone());
    let result = service
        .confirm(confirmation(&fixture))
        .expect("document-path claims");
    assert_eq!(count(&open_catalog(&fixture), "skills"), 2);
    service
        .undo(&result.operation_id)
        .expect("undo document claims");
    let restored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lock_path).unwrap()).unwrap();
    assert_eq!(restored, lock);
}

#[test]
fn root_document_claim_can_transition_and_undo() {
    let fixture = fixture();
    let repository = fixture._workspace.path().join("source-repository");
    write_file(
        &repository,
        "SKILL.md",
        "---\nname: source\ndescription: Root skill\n---\nRoot skill\n",
    );
    git(&repository, &["add", "-A"]);
    git(
        &repository,
        &["-c", "commit.gpgsign=false", "commit", "-qm", "root skill"],
    );
    let lock_path = fixture.home.join(".agents/.skill-lock.json");
    let mut lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lock_path).unwrap()).unwrap();
    lock["skills"].as_object_mut().unwrap().remove("beta");
    lock["skills"]["source"]["skillPath"] = "SKILL.md".into();
    std::fs::write(&lock_path, serde_json::to_vec(&lock).unwrap()).unwrap();
    let external = fixture.home.join(".agents/skills/source");
    let old_document = std::fs::read(external.join("SKILL.md")).unwrap();
    let service = service(&fixture, fixture.catalog.clone());
    let result = service
        .confirm(confirmation(&fixture))
        .expect("root document claim");
    assert_eq!(count(&open_catalog(&fixture), "skills"), 1);
    let journals = fixture
        .filesystem
        .list_source_transition_journals(&fixture.library)
        .unwrap();
    let root_member = journals[0]
        .members
        .iter()
        .find(|member| member.skill_path.is_empty())
        .unwrap();
    assert_eq!(
        std::fs::read(root_member.namespace_path.join("SKILL.md")).unwrap(),
        std::fs::read(repository.join("SKILL.md")).unwrap()
    );
    service.undo(&result.operation_id).expect("undo root claim");
    let restored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lock_path).unwrap()).unwrap();
    assert_eq!(restored, lock);
    assert_eq!(
        std::fs::read(external.join("SKILL.md")).unwrap(),
        old_document
    );
    assert!(!external.join("skills").exists());
}

#[test]
fn partial_claims_refuse_unmatched_paths_before_any_external_change() {
    let fixture = fixture();
    let lock_path = fixture.home.join(".agents/.skill-lock.json");
    let mut lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lock_path).unwrap()).unwrap();
    lock["skills"].as_object_mut().unwrap().remove("beta");
    lock["skills"]["source"]["skillPath"] = "plugins/not-the-same-skill".into();
    let original_lock = serde_json::to_vec(&lock).unwrap();
    std::fs::write(&lock_path, &original_lock).unwrap();
    let external = fixture.home.join(".agents/skills/source/SKILL.md");
    let original = std::fs::read(&external).unwrap();
    let result = service(&fixture, fixture.catalog.clone()).confirm(confirmation(&fixture));
    assert!(matches!(result, Err(SourceTransitionError::Validation(_))));
    assert_eq!(std::fs::read(&lock_path).unwrap(), original_lock);
    assert_eq!(std::fs::read(&external).unwrap(), original);
    assert_eq!(count(&open_catalog(&fixture), "skills"), 0);
    assert!(
        fixture
            .filesystem
            .list_source_transition_journals(&fixture.library)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn partial_claims_recover_without_refetching_or_touching_unclaimed_files() {
    let fixture = fixture();
    configure_target(&fixture, &fixture.home.join(".agents/skills"), "General");
    let lock_path = fixture.home.join(".agents/.skill-lock.json");
    let mut lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lock_path).unwrap()).unwrap();
    lock["skills"].as_object_mut().unwrap().remove("beta");
    std::fs::write(&lock_path, serde_json::to_vec(&lock).unwrap()).unwrap();
    let unclaimed = fixture.home.join(".agents/skills/beta/SKILL.md");
    let original = std::fs::read(&unclaimed).unwrap();
    let old_document = "---\nname: source\n---\nOld local changes\n";
    write_file(
        &fixture.home.join(".agents/skills/source"),
        "SKILL.md",
        old_document,
    );
    let flaky: Arc<dyn SourceTransitionStore> =
        Arc::new(FailOnceSourceStore::new(fixture.catalog.clone(), true));
    let error = service(&fixture, flaky.clone())
        .confirm(confirmation(&fixture))
        .expect_err("injected failure");
    assert!(matches!(error, SourceTransitionError::RecoveryRequired(_)));
    let frozen = fixture
        .filesystem
        .list_source_transition_journals(&fixture.library)
        .unwrap();
    let member = frozen[0]
        .members
        .iter()
        .find(|member| member.directory_name == "source")
        .unwrap();
    let installed_path = member.namespace_path.clone();
    let recovery = SourceTransitionService::new(
        fixture.preview.clone(),
        Arc::new(NoFetchGitSource),
        fixture.locks.clone(),
        flaky,
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
        fixture.write_gate.clone(),
    );
    recovery
        .recover_pending(&fixture.library)
        .expect("partial claim recovery");
    assert_eq!(count(&open_catalog(&fixture), "skills"), 2);
    assert_eq!(std::fs::read(&unclaimed).unwrap(), original);
    assert_ne!(
        std::fs::read_to_string(installed_path.join("SKILL.md")).unwrap(),
        old_document
    );
    assert_eq!(count(&open_catalog(&fixture), "activations"), 1);
    assert_eq!(
        std::fs::read_link(fixture.home.join(".agents/skills/source")).unwrap(),
        installed_path
    );
}

#[test]
fn replacement_undo_refuses_changed_preservation_copy() {
    let fixture = fixture();
    write_file(
        &fixture.home.join(".agents/skills/source"),
        "SKILL.md",
        "Old local content",
    );
    let transition = service(&fixture, fixture.catalog.clone());
    let result = transition.confirm(confirmation(&fixture)).unwrap();
    let journals = fixture
        .filesystem
        .list_source_transition_journals(&fixture.library)
        .unwrap();
    let member = journals[0]
        .members
        .iter()
        .find(|member| member.directory_name == "source")
        .unwrap();
    write_file(
        member.isolated_path.as_ref().unwrap(),
        "SKILL.md",
        "Concurrent edit",
    );
    assert!(transition.undo(&result.operation_id).is_err());
    assert!(transition.finalize(&result.operation_id).is_err());
    assert_eq!(
        std::fs::read_to_string(member.isolated_path.as_ref().unwrap().join("SKILL.md")).unwrap(),
        "Concurrent edit"
    );
    assert_eq!(count(&open_catalog(&fixture), "skills"), 2);
    assert!(!fixture.home.join(".agents/skills/source").exists());
}

#[test]
fn replacement_rolls_back_old_bytes_when_lock_release_fails() {
    let fixture = fixture();
    let external = fixture.home.join(".agents/skills/source");
    write_file(&external, "SKILL.md", "Old local content");
    let lock_path = fixture.home.join(".agents/.skill-lock.json");
    let original_lock = std::fs::read(&lock_path).unwrap();
    let transition = service_with_locks(
        &fixture,
        Arc::new(RefusingRelease(fixture.locks.clone())),
        fixture.catalog.clone(),
    );
    assert!(transition.confirm(confirmation(&fixture)).is_err());
    assert_eq!(
        std::fs::read_to_string(external.join("SKILL.md")).unwrap(),
        "Old local content"
    );
    assert_eq!(std::fs::read(&lock_path).unwrap(), original_lock);
    assert_eq!(count(&open_catalog(&fixture), "skills"), 0);
    assert!(
        fixture
            .filesystem
            .list_source_transition_journals(&fixture.library)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn older_equal_content_journals_still_undo_without_external_hash() {
    let fixture = fixture();
    let transition = service(&fixture, fixture.catalog.clone());
    let result = transition.confirm(confirmation(&fixture)).unwrap();
    let mut journals = fixture
        .filesystem
        .list_source_transition_journals(&fixture.library)
        .unwrap();
    for member in &mut journals[0].members {
        member.external_tree_hash = None;
    }
    fixture
        .filesystem
        .write_source_transition_journal(&fixture.library, &journals[0])
        .unwrap();
    transition
        .undo(&result.operation_id)
        .expect("legacy equal-content journal");
    assert_eq!(count(&open_catalog(&fixture), "skills"), 0);
}

#[test]
fn replacement_refuses_external_edits_during_staging() {
    let mut fixture = fixture();
    let external = fixture.home.join(".agents/skills/source/SKILL.md");
    let mut git = FixtureGitSource::new(&fixture._workspace.path().join("source-repository"));
    git.edit_during_stage = Some(external.clone());
    fixture.source = Arc::new(git);
    let lock_path = fixture.home.join(".agents/.skill-lock.json");
    let old_lock = std::fs::read(&lock_path).unwrap();
    let result = service(&fixture, fixture.catalog.clone()).confirm(confirmation(&fixture));
    assert!(
        matches!(result, Err(SourceTransitionError::Validation(_))),
        "{result:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&external).unwrap(),
        "Concurrent edit during staging"
    );
    assert_eq!(std::fs::read(&lock_path).unwrap(), old_lock);
    assert_eq!(count(&open_catalog(&fixture), "skills"), 0);
}

#[test]
fn partial_claims_install_same_named_members_and_preserve_unclaimed_entities() {
    let fixture = fixture();
    let repository = fixture._workspace.path().join("source-repository");
    write_file(
        &repository,
        "skills/plugins/source/SKILL.md",
        "---\nname: Other Source\ndescription: Another path\n---\n# Other\n",
    );
    git(&repository, &["add", "-A"]);
    git(
        &repository,
        &["-c", "commit.gpgsign=false", "commit", "-qm", "same name"],
    );
    let lock_path = fixture.home.join(".agents/.skill-lock.json");
    let mut lock: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lock_path).unwrap()).unwrap();
    lock["skills"].as_object_mut().unwrap().remove("beta");
    let original_lock = serde_json::to_vec(&lock).unwrap();
    std::fs::write(&lock_path, &original_lock).unwrap();
    let unclaimed = fixture.home.join(".agents/skills/beta/SKILL.md");
    let original_bytes = std::fs::read(&unclaimed).unwrap();
    let transition = service(&fixture, fixture.catalog.clone());
    let result = transition
        .confirm(confirmation(&fixture))
        .expect("partial ownership transition");
    assert_eq!(result.member_count, 3);
    assert_eq!(count(&open_catalog(&fixture), "skills"), 3);
    assert_eq!(std::fs::read(&unclaimed).unwrap(), original_bytes);
    transition
        .undo(&result.operation_id)
        .expect("undo partial claims");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&lock_path).unwrap()).unwrap(),
        lock
    );
    assert_eq!(std::fs::read(&unclaimed).unwrap(), original_bytes);
    assert!(
        fixture
            .home
            .join(".agents/skills/source/SKILL.md")
            .is_file()
    );
}

#[test]
fn confirms_and_undoes_the_complete_source_in_one_release() {
    let fixture = fixture();
    let transition = service(&fixture, fixture.catalog.clone());
    let result = transition
        .confirm(confirmation(&fixture))
        .expect("v9 transition");

    let mirror = skill_man_lib::core::git_source::git_mirror_path(
        &fixture.library.join("cache"),
        "https://example.com/acme/source",
    );
    assert!(
        mirror.join("HEAD").is_file(),
        "installation retains its reusable Git mirror"
    );
    assert_eq!(result.member_count, 2);
    assert!(result.undo_available);
    let connection = open_catalog(&fixture);
    let runtime = skill_man_lib::adapters::runtime_catalog::RuntimeCatalogStore::new(
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
    );
    let api = skill_man_lib::tauri_adapter::catalog_api::CatalogApi::new(
        skill_man_lib::core::catalog::CatalogService::new(Arc::new(runtime)),
    );
    let listed = api
        .list_skills(skill_man_lib::tauri_adapter::dto::ListSkillsRequestDto {
            filter: skill_man_lib::tauri_adapter::dto::CatalogFilterDto::All,
        })
        .unwrap();
    for member in listed.items {
        let detail = api
            .inspect_skill(member.id)
            .expect("installed Git member is readable");
        assert!(
            Path::new(&detail.final_entity_path)
                .canonicalize()
                .unwrap()
                .starts_with(fixture.library.canonicalize().unwrap())
        );
        assert!(!detail.skill_markdown.is_empty());
    }
    assert_eq!(count(&connection, "remote_source_parents"), 1);
    assert_eq!(count(&connection, "git_repository_sources"), 1);
    assert_eq!(count(&connection, "git_source_releases"), 1);
    assert_eq!(count(&connection, "git_source_release_members"), 2);
    assert_eq!(count(&connection, "git_source_members"), 2);
    assert_eq!(count(&connection, "skills"), 2);
    let source: (String, String, String, String) = connection
        .query_row(
            "SELECT tracking_mode, tracking_value, current_selected_ref, current_release_id
             FROM git_repository_sources",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("source row");
    assert_eq!(source.0, "branch");
    assert_eq!(source.1, "main");
    assert_eq!(source.2, "main");
    assert!(source.3.starts_with("source-release-"));
    let selection_kind: String = connection
        .query_row(
            "SELECT selection_kind FROM git_source_releases",
            [],
            |row| row.get(0),
        )
        .expect("release row");
    assert_eq!(selection_kind, "branch");
    let storage: Vec<String> = connection
        .prepare("SELECT storage_relpath FROM git_source_members ORDER BY skill_path")
        .expect("members")
        .query_map([], |row| row.get(0))
        .expect("map")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect");
    assert_eq!(storage.len(), 2);
    assert!(
        storage
            .iter()
            .all(|path| { path.starts_with(&format!("skills/git/{}/", result.remote_id)) })
    );
    drop(connection);

    // Immutable namespace snapshots exist under skills/git/<remote_id>/<skill_id>.
    let namespace = namespace_members(&fixture, &result.remote_id);
    let member_ids: Vec<String> = open_catalog(&fixture)
        .prepare("SELECT skill_id FROM git_source_members ORDER BY skill_path")
        .expect("ids")
        .query_map([], |row| row.get(0))
        .expect("map")
        .collect::<Result<Vec<_>, _>>()
        .expect("ids");
    for skill_id in member_ids {
        assert!(
            namespace.join(&skill_id).is_dir(),
            "namespace snapshot for {skill_id} must be published"
        );
    }
    // The single full-file CAS released both claims.
    assert!(
        fixture.locks.discover().expect("read released lock")[0]
            .entries
            .is_empty(),
        "one CAS released both claims"
    );
    // The manifest freezes policy + selected ref + release.
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            fixture
                .library
                .join("remotes")
                .join(&result.remote_id)
                .join("source.json"),
        )
        .expect("source manifest"),
    )
    .expect("parse manifest");
    assert_eq!(manifest["trackingMode"], "branch");
    assert_eq!(manifest["trackingValue"], "main");
    assert_eq!(manifest["currentSelectedRef"], "main");
    assert_eq!(manifest["currentReleaseId"], result.release_id.clone());

    // Source Undo restores the whole source, the lock and the external
    // entities without touching other Home state.
    let undo = transition
        .undo(&result.operation_id)
        .expect("v9 source undo");
    assert_eq!(undo.member_count, 2);
    let connection = open_catalog(&fixture);
    assert_eq!(count(&connection, "remote_source_parents"), 0);
    assert_eq!(count(&connection, "git_repository_sources"), 0);
    assert_eq!(count(&connection, "git_source_releases"), 0);
    assert_eq!(count(&connection, "skills"), 0);
    drop(connection);
    assert!(
        !namespace_members(&fixture, &result.remote_id).exists(),
        "the namespace tree is removed on Undo"
    );
    assert!(
        fixture
            .home
            .join(".agents/skills/source/SKILL.md")
            .is_file()
    );
    assert!(fixture.home.join(".agents/skills/beta/SKILL.md").is_file());
    assert_eq!(
        fixture.locks.discover().expect("read restored lock")[0]
            .entries
            .len(),
        2,
        "Undo restores the exact claim set"
    );
    assert!(
        fixture
            .filesystem
            .list_source_transition_journals(&fixture.library)
            .expect("journal cleanup")
            .is_empty()
    );
}

#[test]
fn plugin_categories_survive_install_and_a_fresh_capability_read() {
    use skill_man_lib::adapters::git_source_capability::SqliteGitSourceCapabilityReader;
    use skill_man_lib::core::git_source_capability::GitSourceCapabilityScan;
    let fixture = fixture();
    let repository = fixture._workspace.path().join("source-repository");
    write_file(
        &repository,
        ".claude-plugin/plugin.json",
        r#"{"name":"mattpocock-skills","skills":["./skills/source"]}"#,
    );
    git(&repository, &["add", "-A"]);
    git(
        &repository,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "plugin categories",
        ],
    );
    let transition = service(&fixture, fixture.catalog.clone());
    let request = confirmation(&fixture);
    let result = transition.confirm(request).unwrap();
    let read = || {
        GitSourceCapabilityScan::new(Arc::new(SqliteGitSourceCapabilityReader::new(
            Arc::new(WriteGate::new(
                skill_man_lib::core::write_gate::WriteGateState::Open(
                    skill_man_lib::core::home::BoundHome::test_value(
                        "category-read",
                        fixture.library.clone(),
                    ),
                ),
            )),
            "skill-man.sqlite3",
            fixture.filesystem.clone(),
        )))
        .scan()
        .unwrap()
    };
    let report = read();
    let members = &report.sources[0].members;
    assert_eq!(
        members
            .iter()
            .find(|m| m.skill_path == "skills/source")
            .unwrap()
            .plugin_name
            .as_deref(),
        Some("mattpocock-skills")
    );
    assert_eq!(
        members
            .iter()
            .find(|m| m.skill_path == "skills/beta")
            .unwrap()
            .plugin_name,
        None
    );
    transition.undo(&result.operation_id).unwrap();
    assert!(read().sources.is_empty());
}

#[test]
fn confirms_a_worktree_only_source_without_fabricating_ownership_claims() {
    let fixture = fixture();
    let locks: Arc<dyn InstallerLockStore> = Arc::new(EmptyLocks);
    let preview = Arc::new(SourceGroupPreviewService::new(
        fixture.source.clone(),
        locks.clone(),
    ));
    let transition = SourceTransitionService::new(
        preview.clone(),
        fixture.source.clone(),
        locks,
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
        fixture.write_gate.clone(),
    );
    let outcome = preview
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: "https://example.com/acme/source".into(),
            tracking_policy: Some(SourceTrackingOverride {
                mode: "branch".into(),
                value: Some("main".into()),
            }),
        })
        .expect("preview");
    let SourceGroupPreviewOutcome::Preview(preview) = outcome else {
        panic!("expected a worktree-only preview");
    };
    assert!(preview.external_ownership_claims.is_empty());
    let result = transition
        .confirm(ConfirmSourceTransitionRequest {
            expected_removed_claims: Vec::new(),
            source_type: "git".into(),
            source_url: preview.source_url,
            tracking_policy: Some(SourceTrackingOverride {
                mode: "branch".into(),
                value: Some("main".into()),
            }),
            expected_selected_ref: preview.policy.selected_ref,
            expected_resolved_commit: preview.policy.resolved_commit,
        })
        .expect("unowned source converts into a managed release");
    assert_eq!(result.member_count, 2);
    assert!(
        fixture
            .filesystem
            .read_remote_parent_manifest(&fixture.library.join("remotes"), &result.remote_id)
            .expect("read source manifest")
            .is_some()
    );
    assert_eq!(count(&open_catalog(&fixture), "git_repository_sources"), 1);
    transition
        .undo(&result.operation_id)
        .expect("unowned source Undo");
    assert_eq!(count(&open_catalog(&fixture), "git_repository_sources"), 0);
}

#[test]
fn refuses_lockless_conversion_when_an_installer_lock_is_faulted() {
    let fixture = fixture();
    let locks: Arc<dyn InstallerLockStore> = Arc::new(FaultedLocks);
    let preview = Arc::new(SourceGroupPreviewService::new(
        fixture.source.clone(),
        locks.clone(),
    ));
    let transition = SourceTransitionService::new(
        preview.clone(),
        fixture.source.clone(),
        locks,
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
        fixture.write_gate.clone(),
    );
    let outcome = preview
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: "https://example.com/acme/source".into(),
            tracking_policy: Some(SourceTrackingOverride {
                mode: "branch".into(),
                value: Some("main".into()),
            }),
        })
        .expect("preview");
    let SourceGroupPreviewOutcome::Preview(preview) = outcome else {
        panic!("the faulted lock is hidden from the read-only preview");
    };
    let error = transition
        .confirm(ConfirmSourceTransitionRequest {
            expected_removed_claims: Vec::new(),
            source_type: "git".into(),
            source_url: preview.source_url,
            tracking_policy: Some(SourceTrackingOverride {
                mode: "branch".into(),
                value: Some("main".into()),
            }),
            expected_selected_ref: preview.policy.selected_ref,
            expected_resolved_commit: preview.policy.resolved_commit,
        })
        .expect_err("a faulted lock cannot be treated as no ownership");
    assert!(
        matches!(&error, SourceTransitionError::Validation(message) if message.contains("lock")),
        "expected a closed lock validation, got {error:?}"
    );
    assert_eq!(count(&open_catalog(&fixture), "git_repository_sources"), 0);
}

#[test]
fn second_confirmation_is_closed_to_update_until_ticket_93() {
    let fixture = fixture();
    let transition = service(&fixture, fixture.catalog.clone());
    transition
        .confirm(confirmation(&fixture))
        .expect("first transition");
    let error = transition
        .confirm(confirmation(&fixture))
        .expect_err("a managed source never re-enters Fetch Latest and Manage");
    let matches_closed = matches!(
        &error,
        SourceTransitionError::Validation(message) if message.contains("already managed")
    );
    assert!(
        matches_closed,
        "a whole-source Update belongs to ticket #93: {error}"
    );
    let connection = open_catalog(&fixture);
    assert_eq!(count(&connection, "git_repository_sources"), 1);
    assert_eq!(count(&connection, "skills"), 2);
    drop(connection);
    assert_eq!(
        fixture.locks.discover().expect("lock")[0].entries.len(),
        0,
        "the managed source's claims stay released"
    );
}

#[test]
fn update_rechecks_external_ownership_before_catalog_commit() {
    let fixture = fixture();
    let transition = service(&fixture, fixture.catalog.clone());
    let remote_id = transition
        .confirm(confirmation(&fixture))
        .expect("initial transition")
        .remote_id;
    let old_commit = fixture
        .catalog
        .read_current(&remote_id)
        .expect("read current source")
        .expect("source")
        .resolved_commit;
    let repository = fixture._workspace.path().join("source-repository");

    write_file(
        &repository,
        "skills/source/SKILL.md",
        "---\nname: Source Root\ndescription: Updated member\n---\n# Updated\n",
    );
    git(&repository, &["add", "-A"]);
    git(
        &repository,
        &["-c", "commit.gpgSign=false", "commit", "-q", "-m", "update"],
    );

    let report = LockFileReport {
        path: fixture.home.join(".agents/.skill-lock.json"),
        fingerprint: "reappeared".into(),
        byte_len: 0,
        version: 3,
        fault: None,
        entries: vec![LockEntry {
            name: "source".into(),
            source_type: "git".into(),
            source: "acme/source".into(),
            source_url: "https://example.com/acme/source".into(),
            requested_ref: Some("main".into()),
            skill_path: "skills/source".into(),
            skill_folder_hash: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            installed_at: None,
            updated_at: None,
            plugin_name: None,
        }],
        entry_faults: Vec::new(),
    };
    let locks = Arc::new(ReappearingLocks {
        calls: AtomicUsize::new(0),
        report,
    });
    let preview = Arc::new(SourceGroupPreviewService::new(
        fixture.source.clone(),
        locks.clone(),
    ));
    let transition = SourceTransitionService::new(
        preview.clone(),
        fixture.source.clone(),
        locks,
        fixture.catalog.clone(),
        fixture.filesystem.clone(),
        Arc::new(OffsetClock),
        fixture.library.clone(),
        fixture.home.clone(),
        fixture.write_gate.clone(),
    )
    .with_update_store(fixture.catalog.clone());
    let SourceGroupPreviewOutcome::Preview(preview) = preview
        .fetch_latest_and_manage(FetchLatestAndManageRequest {
            source_type: "git".into(),
            source_url: "https://example.com/acme/source".into(),
            tracking_policy: Some(SourceTrackingOverride {
                mode: "branch".into(),
                value: Some("main".into()),
            }),
        })
        .expect("updated preview")
    else {
        panic!("expected updated source preview");
    };

    let error = transition
        .confirm_update(ConfirmSourceUpdateRequest {
            remote_id: remote_id.clone(),
            tracking_policy: Some(SourceTrackingOverride {
                mode: "branch".into(),
                value: Some("main".into()),
            }),
            expected_selected_ref: preview.policy.selected_ref,
            expected_resolved_commit: preview.policy.resolved_commit,
        })
        .expect_err("ownership reappearance must block Update");
    assert!(
        matches!(&error, SourceTransitionError::ExternalOwnershipReappeared)
            || matches!(
                &error,
                SourceTransitionError::Validation(message) if message.contains("installer lock")
            ),
        "unexpected Update failure: {error:?}"
    );
    assert_eq!(
        fixture
            .catalog
            .read_current(&remote_id)
            .expect("read current source")
            .expect("source")
            .resolved_commit,
        old_commit,
        "a reappearing owner must prevent the Catalog release switch"
    );
}

#[test]
fn journal_freezes_policy_manifest_and_namespace_before_the_commit_point() {
    let fixture = fixture();
    let transition = service(&fixture, fixture.catalog.clone());
    transition
        .confirm(confirmation(&fixture))
        .expect("transition");
    let journal = fixture
        .filesystem
        .list_source_transition_journals(&fixture.library)
        .expect("frozen journal")
        .into_iter()
        .next()
        .expect("one journal");
    assert_eq!(journal.version, 2);
    assert_eq!(journal.tracking_mode, "branch");
    assert_eq!(journal.tracking_value.as_deref(), Some("main"));
    assert_eq!(journal.selection_kind, "branch");
    assert_eq!(journal.selected_ref, "main");
    assert_eq!(journal.members.len(), 2);
    for member in &journal.members {
        assert!(member.tree_hash.starts_with("tree-sha256-v1:"));
        assert!(
            member
                .namespace_path
                .starts_with(fixture.library.join("skills/git").join(&journal.remote_id))
        );
        assert_eq!(
            member.namespace_path.to_string_lossy(),
            format!(
                "{}/skills/git/{}/{}",
                fixture.library.to_string_lossy(),
                journal.remote_id,
                member.skill_id
            )
        );
    }
}

#[test]
fn confirmation_refuses_a_home_destination_before_releasing_external_ownership() {
    let fixture = fixture();
    // A plain file at the namespace root makes every member destination
    // unresolvable before the ownership CAS.
    std::fs::create_dir_all(fixture.library.join("skills")).expect("skills root");
    std::fs::write(fixture.library.join("skills/git"), "occupied").expect("occupant file");
    let transition = service(&fixture, fixture.catalog.clone());

    let error = transition
        .confirm(confirmation(&fixture))
        .expect_err("a Home destination collision is pre-CAS stale evidence");

    assert!(
        matches!(
            error,
            SourceTransitionError::Source(_)
                | SourceTransitionError::Validation(_)
                | SourceTransitionError::FileSystem(_)
        ),
        "unexpected error: {error}"
    );
    let connection = open_catalog(&fixture);
    assert_eq!(count(&connection, "git_repository_sources"), 0);
    assert_eq!(count(&connection, "skills"), 0);
    drop(connection);
    assert_eq!(
        fixture.locks.discover().expect("lock after rejection")[0]
            .entries
            .len(),
        2,
        "the one source lock remains untouched before its CAS",
    );
    assert!(
        fixture
            .filesystem
            .list_source_transition_journals(&fixture.library)
            .expect("journal cleanup")
            .is_empty(),
        "journal remains after a pre-CAS refusal: {error}"
    );
}

#[test]
fn post_cas_failure_recovers_only_the_frozen_journal_without_refetching() {
    let fixture = fixture();
    let flaky: Arc<dyn SourceTransitionStore> =
        Arc::new(FailOnceSourceStore::new(fixture.catalog.clone(), true));
    let transition = service(&fixture, flaky.clone());
    let error = transition
        .confirm(confirmation(&fixture))
        .expect_err("injected Catalog failure must require recovery");
    assert!(matches!(error, SourceTransitionError::RecoveryRequired(_)));
    assert_eq!(
        fixture
            .filesystem
            .list_source_transition_journals(&fixture.library)
            .expect("pending journal")
            .len(),
        1
    );
    assert!(
        fixture.locks.discover().expect("lock")[0]
            .entries
            .is_empty()
    );

    let recovery = SourceTransitionService::new(
        fixture.preview.clone(),
        Arc::new(NoFetchGitSource),
        fixture.locks.clone(),
        flaky,
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
        fixture.write_gate.clone(),
    );
    recovery
        .recover_pending(&fixture.library)
        .expect("roll forward the frozen journal");
    let connection = open_catalog(&fixture);
    assert_eq!(count(&connection, "git_repository_sources"), 1);
    assert_eq!(count(&connection, "git_source_releases"), 1);
    assert_eq!(count(&connection, "skills"), 2);
    drop(connection);
    assert!(
        fixture
            .filesystem
            .list_source_transition_journals(&fixture.library)
            .expect("finished journal")
            .is_empty()
    );
}

#[test]
fn recovery_refuses_a_new_repository_claim_under_a_different_lock_key() {
    let fixture = fixture();
    let flaky: Arc<dyn SourceTransitionStore> =
        Arc::new(FailOnceSourceStore::new(fixture.catalog.clone(), true));
    let transition = service(&fixture, flaky.clone());
    transition
        .confirm(confirmation(&fixture))
        .expect_err("leave a post-CAS journal for recovery");
    std::fs::write(
        fixture.home.join(".agents/.skill-lock.json"),
        r#"{
  "version": 3,
  "skills": {
    "replacement": {
      "sourceType": "git",
      "source": "acme/source",
      "sourceUrl": "https://example.com/acme/source",
      "ref": "main",
      "skillPath": "skills/replacement",
      "skillFolderHash": "cccccccccccccccccccccccccccccccccccccccc"
    }
  }
}"#,
    )
    .expect("simulate a new external repository claim");
    let recovery = SourceTransitionService::new(
        fixture.preview.clone(),
        Arc::new(NoFetchGitSource),
        fixture.locks.clone(),
        flaky,
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
        fixture.write_gate.clone(),
    );

    let error = recovery
        .recover_pending(&fixture.library)
        .expect_err("a new source claim is never accepted as a released claim");

    assert!(matches!(error, SourceTransitionError::RecoveryRequired(_)));
    assert!(
        fixture
            .filesystem
            .list_source_transition_journals(&fixture.library)
            .expect("pending journal remains")
            .len()
            == 1
    );
}

#[test]
fn recovery_refuses_a_frozen_claim_replaced_under_its_original_key() {
    let fixture = fixture();
    let flaky: Arc<dyn SourceTransitionStore> =
        Arc::new(FailOnceSourceStore::new(fixture.catalog.clone(), true));
    let transition = service(&fixture, flaky.clone());
    transition
        .confirm(confirmation(&fixture))
        .expect_err("leave a post-CAS journal for recovery");
    std::fs::write(
        fixture.home.join(".agents/.skill-lock.json"),
        r#"{
  "version": 3,
  "skills": {
    "source": {
      "sourceType": "git",
      "source": "acme/source",
      "sourceUrl": "https://example.com/acme/source",
      "ref": "main",
      "skillPath": "skills/source",
      "skillFolderHash": "cccccccccccccccccccccccccccccccccccccccc"
    },
    "beta": {
      "sourceType": "git",
      "source": "acme/source",
      "sourceUrl": "https://example.com/acme/source",
      "ref": "main",
      "skillPath": "skills/beta",
      "skillFolderHash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    }
  }
}"#,
    )
    .expect("replace one frozen claim under the same key");
    let recovery = SourceTransitionService::new(
        fixture.preview.clone(),
        Arc::new(NoFetchGitSource),
        fixture.locks.clone(),
        flaky,
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
        fixture.write_gate.clone(),
    );

    let error = recovery
        .recover_pending(&fixture.library)
        .expect_err("a same-key claim replacement is not the frozen pre-CAS state");

    assert!(matches!(error, SourceTransitionError::RecoveryRequired(_)));
}

#[test]
fn undo_rechecks_external_occupancy_before_mutating_the_catalog() {
    let fixture = fixture();
    let transition = service(&fixture, fixture.catalog.clone());
    let result = transition
        .confirm(confirmation(&fixture))
        .expect("transition");
    // A concurrent external owner reappears before Undo.
    std::fs::create_dir_all(fixture.home.join(".agents/skills/source")).expect("reappear");
    std::fs::write(
        fixture.home.join(".agents/skills/source/SKILL.md"),
        "---\nname: Source Root\n---\n# external\n",
    )
    .expect("external bytes");

    let error = transition
        .undo(&result.operation_id)
        .expect_err("an occupied external location blocks the whole Undo");

    assert!(matches!(error, SourceTransitionError::Validation(_)));
    let connection = open_catalog(&fixture);
    assert_eq!(count(&connection, "git_repository_sources"), 1);
    assert_eq!(count(&connection, "skills"), 2);
    drop(connection);
}

#[test]
fn recovery_refuses_a_journal_isolation_path_outside_the_derived_source_root() {
    let fixture = fixture();
    let flaky: Arc<dyn SourceTransitionStore> =
        Arc::new(FailOnceSourceStore::new(fixture.catalog.clone(), true));
    let transition = service(&fixture, flaky.clone());
    transition
        .confirm(confirmation(&fixture))
        .expect_err("leave a post-CAS journal for recovery validation");
    let journal = fixture
        .filesystem
        .list_source_transition_journals(&fixture.library)
        .expect("frozen journal")
        .into_iter()
        .next()
        .expect("one frozen journal");
    let journal_path = fixture
        .library
        .join("operations")
        .join(&journal.operation_id)
        .join("source-transition-journal.json");
    let forged = fixture
        .library
        .parent()
        .expect("workspace")
        .join(".skill-man-source-transition-forged");
    std::fs::create_dir_all(&forged).expect("forged isolation directory");
    std::fs::write(forged.join("keep"), "must not be deleted").expect("forged content");
    let mut journal_value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&journal_path).expect("read journal"))
            .expect("journal JSON");
    journal_value["members"][0]["isolated_path"] =
        serde_json::Value::String(forged.to_string_lossy().into_owned());
    std::fs::write(
        &journal_path,
        serde_json::to_vec_pretty(&journal_value).expect("rewrite forged journal"),
    )
    .expect("write forged journal");
    let recovery = SourceTransitionService::new(
        fixture.preview.clone(),
        Arc::new(NoFetchGitSource),
        fixture.locks.clone(),
        flaky,
        fixture.filesystem.clone(),
        Arc::new(FixtureClock),
        fixture.library.clone(),
        fixture.home.clone(),
        fixture.write_gate.clone(),
    );

    let error = recovery
        .recover_pending(&fixture.library)
        .expect_err("journal-owned isolation paths must be exact");

    assert!(matches!(error, SourceTransitionError::RecoveryRequired(_)));
    assert!(
        forged.join("keep").is_file(),
        "forged path was never touched"
    );
}

#[test]
fn journal_writes_refuse_a_symlinked_operations_root() {
    let fixture = fixture();
    let outside = fixture.library.parent().expect("workspace").join("outside");
    std::fs::create_dir_all(&outside).expect("outside directory");
    std::os::unix::fs::symlink(&outside, fixture.library.join("operations"))
        .expect("replace owned operations root with a symlink");
    let transition = service(&fixture, fixture.catalog.clone());

    transition
        .confirm(confirmation(&fixture))
        .expect_err("journal writer must reject the symlink before any transition work");

    assert!(
        fixture
            .home
            .join(".agents/skills/source/SKILL.md")
            .is_file()
    );
    assert_eq!(
        fixture.locks.discover().expect("lock after refusal")[0]
            .entries
            .len(),
        2
    );
    assert!(
        std::fs::read_dir(&outside)
            .expect("outside remains readable")
            .next()
            .is_none(),
        "the descriptor-relative journal writer never followed the symlink"
    );
}

#[test]
fn repository_rename_and_path_aliases_never_move_the_storage_path() {
    let fixture = fixture();
    let transition = service(&fixture, fixture.catalog.clone());
    let result = transition
        .confirm(confirmation(&fixture))
        .expect("transition");
    // The storage path is keyed by remote_id + skill_id only: neither the
    // canonical URL nor a future alias affects it.
    let expected: Vec<String> = open_catalog(&fixture)
        .prepare("SELECT storage_relpath FROM git_source_members ORDER BY skill_path")
        .expect("members")
        .query_map([], |row| row.get(0))
        .expect("map")
        .collect::<Result<Vec<_>, _>>()
        .expect("paths");
    assert_eq!(expected.len(), 2);
    assert!(
        expected
            .iter()
            .all(|path| path.starts_with(&format!("skills/git/{}/", result.remote_id))),
        "every storage path is keyed by the stable remote_id"
    );
    assert!(
        expected.iter().all(|path| path.matches('/').count() == 3),
        "storage paths are exactly skills/git/<remote_id>/<skill_id>"
    );
}
