//! Scan Run lifecycle integration tests (spec §4.10, ADR-0020; issue #82):
//! the real adapters — MacOsFileSystem, SystemLocalGitProbe, the temp-dir
//! fault-injecting Evidence Store — driven end to end over a real Bound
//! Home. Covers streaming evidence + atomic publish, Root atomicity /
//! Incomplete Reports, single-flight, cancel keeping the old Report,
//! supersede on a product write, and the typed zero-progress Unresponsive
//! isolation (with a shortened test window).

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use skill_man_lib::adapters::local_git_probe::SystemLocalGitProbe;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::scan_evidence_store::FaultInjectingScanEvidenceStoreFactory;
use skill_man_lib::adapters::system_clock::SystemClock;
use skill_man_lib::core::scan::mutation::ScanMutationCoordinator;
use skill_man_lib::core::scan::{ReportFreshness, ScanCoordinator, ScanRunState, ScanTrigger};
use skill_man_lib::core::write_gate::{ClosedReason, ReadOnlyReason, WriteGateState};
use skill_man_lib::seams::agent_configuration_store::{
    AgentConfigurationStore, AgentConfigurationStoreChange, AgentConfigurationStoreError,
    AgentConfigurationStoreSnapshot, RecentProjectFolder, StoredGlobalSkillRoot,
};
use skill_man_lib::seams::app_state_store::AppStateStore;
use skill_man_lib::seams::clock::Clock;
use skill_man_lib::seams::filesystem::{
    FileSystem, FileSystemError, ScannedSkillEntry, TreeScanEntry,
};
use skill_man_lib::seams::installer_lock_store::EmptyInstallerLockStore;
use skill_man_lib::seams::scan_evidence_store::{
    CurrentManifestRead, ScanEvidenceStore, ScanEvidenceStoreFactory, ScanReportCursor,
    ScanReportRow, ScanReportSection, ScanRootCoverageRecord,
};
use skill_man_lib::seams::scan_managed_facts::{ManagedSkillPathFact, ScanManagedFactsReader};

use common::{BoundTestHome, HOME_ID};

/// Page the Roots section of a Report through the unique `report_page`
/// contract (spec §4.10): the tests never reach into the summary for
/// per-Root detail.
fn page_roots(
    coordinator: &ScanCoordinator,
    summary: &skill_man_lib::core::scan::ScanReportSummary,
) -> Vec<ScanRootCoverageRecord> {
    let mut rows = Vec::new();
    let mut offset = 0_u64;
    loop {
        let page = coordinator
            .report_page(
                ScanReportCursor {
                    report_content_identity: summary.content_identity.clone(),
                    run_id: summary.run_id.clone(),
                    generation: summary.generation,
                    section: ScanReportSection::Roots,
                    offset,
                },
                64,
            )
            .expect("roots page");
        if page.rows.is_empty() {
            break;
        }
        rows.extend(page.rows.into_iter().map(|row| match row {
            ScanReportRow::RootCoverage(root) => root,
            other => panic!("unexpected Roots row {other:?}"),
        }));
        match page.next_offset {
            Some(next) => offset = next,
            None => break,
        }
    }
    rows
}

/// Page any section through `report_page`; a `limit` of 0 opens the first
/// page with the default size.
fn page_section(
    coordinator: &ScanCoordinator,
    summary: &skill_man_lib::core::scan::ScanReportSummary,
    section: ScanReportSection,
    limit: u32,
) -> (Vec<ScanReportRow>, Option<u64>) {
    let page = coordinator
        .report_page(
            ScanReportCursor {
                report_content_identity: summary.content_identity.clone(),
                run_id: summary.run_id.clone(),
                generation: summary.generation,
                section,
                offset: 0,
            },
            limit as usize,
        )
        .expect("report page");
    (page.rows, page.next_offset)
}

/// Test Agent Configuration store: one configured Root per entry.
struct StubAgentStore {
    roots: Vec<StoredGlobalSkillRoot>,
    configurations: Vec<skill_man_lib::seams::agent_configuration_store::StoredAgentConfiguration>,
    version: u64,
}

impl AgentConfigurationStore for StubAgentStore {
    fn agent_configuration_snapshot(
        &self,
    ) -> Result<AgentConfigurationStoreSnapshot, AgentConfigurationStoreError> {
        Ok(AgentConfigurationStoreSnapshot {
            snapshot_version: self.version,
            configurations: self.configurations.clone(),
            roots: self.roots.clone(),
        })
    }
    fn apply_agent_configuration_change(
        &self,
        _expected_snapshot_version: u64,
        _change: AgentConfigurationStoreChange,
    ) -> Result<u64, AgentConfigurationStoreError> {
        unreachable!("scan never applies configuration changes")
    }
    fn list_recent_project_folders(
        &self,
    ) -> Result<Vec<RecentProjectFolder>, AgentConfigurationStoreError> {
        unreachable!("scan never reads project folders")
    }
    fn record_recent_project_folder(
        &self,
        _folder: RecentProjectFolder,
    ) -> Result<(), AgentConfigurationStoreError> {
        unreachable!("scan never records project folders")
    }
    fn remove_recent_project_folder(
        &self,
        _canonical_path_key: &str,
    ) -> Result<(), AgentConfigurationStoreError> {
        unreachable!("scan never clears project folders")
    }

    fn clear_recent_project_folders(&self) -> Result<(), AgentConfigurationStoreError> {
        unreachable!("scan never clears project folders")
    }
}

/// Test managed facts reader: the classification pass fails closed when no
/// reader is available, so tests provide a fresh empty one per coordinator
/// (an unavailable Catalog is also a fail-closed Run failure).
struct StubManagedFacts;

impl ScanManagedFactsReader for StubManagedFacts {
    fn read(&self) -> Result<Vec<ManagedSkillPathFact>, String> {
        Ok(Vec::new())
    }
}

/// Delegating FileSystem: every call forwards to the real adapter, except
/// `scan_skills_directory_stream`, which can be gated by the tests (block
/// until released) and counts calls.
struct TestScanFileSystem {
    inner: MacOsFileSystem,
    /// The canonical path currently stalled (`None` = no stall). A stalled
    /// walk yields zero progress but keeps checking the flag in 20ms
    /// slices, so cancel, supersede and the watchdog can always interrupt:
    /// the Run never blocks inside a filesystem call forever.
    stall: Arc<Mutex<Option<PathBuf>>>,
    synthetic_non_utf8_root: Arc<Mutex<Option<PathBuf>>>,
}

impl TestScanFileSystem {
    fn new(home_directory: PathBuf) -> Self {
        Self {
            inner: MacOsFileSystem::new(home_directory),
            stall: Arc::new(Mutex::new(None)),
            synthetic_non_utf8_root: Arc::new(Mutex::new(None)),
        }
    }
    fn stall_path(&self, path: PathBuf) {
        // The walker sees the canonicalized root (macOS /var → /private/var).
        *self.stall.lock().unwrap() = Some(std::fs::canonicalize(&path).unwrap_or(path));
    }
    fn unblock_all(&self) {
        *self.stall.lock().unwrap() = None;
    }
    fn inject_non_utf8_entry(&self, root: &Path) {
        *self.synthetic_non_utf8_root.lock().unwrap() =
            Some(std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()));
    }
    fn is_stalled(&self, path: &Path) -> bool {
        self.stall
            .lock()
            .unwrap()
            .as_deref()
            .map(|stalled| stalled == path)
            .unwrap_or(false)
    }
    fn is_synthetic_non_utf8_root(&self, path: &Path) -> bool {
        self.synthetic_non_utf8_root
            .lock()
            .unwrap()
            .as_deref()
            .map(|root| root == path)
            .unwrap_or(false)
    }
}

impl FileSystem for TestScanFileSystem {
    fn read_entropy(&self, buffer: &mut [u8]) -> Result<(), FileSystemError> {
        self.inner.read_entropy(buffer)
    }

    fn inspect_link_source(
        &self,
        path: &Path,
    ) -> Result<skill_man_lib::seams::filesystem::LinkSourceSnapshot, FileSystemError> {
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
    ) -> Result<skill_man_lib::seams::filesystem::DirectoryFingerprint, FileSystemError> {
        self.inner.directory_fingerprint(path)
    }

    fn activation_snapshot(
        &self,
        entry_path: &Path,
    ) -> Result<skill_man_lib::seams::filesystem::ActivationEntrySnapshot, FileSystemError> {
        self.inner.activation_snapshot(entry_path)
    }

    fn skill_directory_is_readable(&self, path: &Path) -> Result<bool, FileSystemError> {
        self.inner.skill_directory_is_readable(path)
    }

    fn skill_fingerprint(
        &self,
        path: &Path,
    ) -> Result<skill_man_lib::seams::filesystem::SkillFingerprint, FileSystemError> {
        self.inner.skill_fingerprint(path)
    }

    fn read_skill_document(&self, path: &Path) -> Result<String, FileSystemError> {
        self.inner.read_skill_document(path)
    }

    fn tree_hash(&self, path: &Path) -> Result<String, FileSystemError> {
        self.inner.tree_hash(path)
    }

    fn staged_tree_snapshot(
        &self,
        path: &Path,
    ) -> Result<skill_man_lib::seams::filesystem::StagedTreeSnapshot, FileSystemError> {
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
        expected_staged_tree: &skill_man_lib::seams::filesystem::StagedTreeSnapshot,
    ) -> Result<skill_man_lib::seams::filesystem::DirectoryFingerprint, FileSystemError> {
        self.inner.install_staged_skill(
            staged_skill_path,
            final_entity_path,
            library_root,
            operation_id,
            expected_staged_tree,
        )
    }

    fn discard_staging(
        &self,
        staging_operation_root: &Path,
        library_root: &Path,
        expected: Option<&skill_man_lib::seams::filesystem::DirectoryFingerprint>,
    ) -> Result<(), FileSystemError> {
        self.inner
            .discard_staging(staging_operation_root, library_root, expected)
    }

    fn discard_installed_skill(
        &self,
        final_entity_path: &Path,
        library_root: &Path,
        expected: &skill_man_lib::seams::filesystem::DirectoryFingerprint,
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

    fn scan_skills_directory_stream(
        &self,
        path: &Path,
        visitor: &mut dyn FnMut(ScannedSkillEntry) -> Result<bool, FileSystemError>,
    ) -> Result<(), FileSystemError> {
        while self.is_stalled(path) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        self.inner.scan_skills_directory_stream(path, visitor)?;
        if self.is_synthetic_non_utf8_root(path) {
            visitor(ScannedSkillEntry {
                entry_path: path.join("synthetic-non-utf8"),
                name: "synthetic-non-utf8".into(),
                kind: skill_man_lib::seams::filesystem::LinkSourceEntryKind::Directory,
                final_entity_path: None,
                dangling: false,
            })?;
        }
        Ok(())
    }

    fn scan_tree_statistics(
        &self,
        path: &Path,
        visitor: &mut dyn FnMut(&TreeScanEntry) -> Result<bool, FileSystemError>,
    ) -> Result<skill_man_lib::seams::filesystem::TreeScanStatistics, FileSystemError> {
        self.inner.scan_tree_statistics(path, visitor)
    }
    fn inspect_evidence_chain(
        &self,
        path: &Path,
    ) -> Result<skill_man_lib::seams::filesystem::EvidenceChain, FileSystemError> {
        if path.file_name().and_then(|name| name.to_str()) == Some("synthetic-non-utf8") {
            return Ok(skill_man_lib::seams::filesystem::EvidenceChain {
                entry_path: path.to_path_buf(),
                entry_device: 1,
                entry_inode: 1,
                hops: Vec::new(),
                final_entity: None,
                fault: Some(skill_man_lib::seams::filesystem::ChainFault::NonUtf8 {
                    at: path.to_path_buf(),
                }),
            });
        }
        self.inner.inspect_evidence_chain(path)
    }

    fn scan_skills_evidence(
        &self,
        path: &Path,
    ) -> Result<Vec<skill_man_lib::seams::filesystem::ScannedSkillEvidence>, FileSystemError> {
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
        _source: &Path,
        _staging_destination: &Path,
    ) -> Result<skill_man_lib::seams::filesystem::DirectoryFingerprint, FileSystemError> {
        unreachable!("scan tests never call stage_external_directory")
    }

    fn restore_external_directory(
        &self,
        _source: &Path,
        _destination: &Path,
        _expected: &skill_man_lib::seams::filesystem::DirectoryFingerprint,
    ) -> Result<(), FileSystemError> {
        unreachable!("scan tests never call restore_external_directory")
    }

    fn apply_adopt_appearances(
        &self,
        _appearances: &[skill_man_lib::seams::filesystem::AdoptAppearanceStep],
        _activations: &[skill_man_lib::seams::filesystem::AdoptActivationStep],
    ) -> Result<(), FileSystemError> {
        unreachable!("scan tests never call apply_adopt_appearances")
    }

    fn write_adopt_journal(
        &self,
        _library_root: &Path,
        _journal: &skill_man_lib::seams::filesystem::AdoptJournal,
    ) -> Result<(), FileSystemError> {
        unreachable!("scan tests never call write_adopt_journal")
    }

    fn finish_adopt_journal(
        &self,
        _library_root: &Path,
        _operation_id: &str,
    ) -> Result<(), FileSystemError> {
        unreachable!("scan tests never call finish_adopt_journal")
    }

    fn recover_adopt_journals(
        &self,
        _library_root: &Path,
        _baselines: &[skill_man_lib::seams::filesystem::FileImportRecoveryBaseline],
        _adopted_entities: &[skill_man_lib::seams::filesystem::FileImportRecoveryBaseline],
    ) -> Result<u32, FileSystemError> {
        unreachable!("scan tests never call recover_adopt_journals")
    }
}

/// Wait until the Run leaves the in-flight states or the deadline passes.
fn wait_terminal(coordinator: &ScanCoordinator, deadline_ms: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(deadline_ms);
    loop {
        let snapshot = coordinator.snapshot();
        let terminal = snapshot
            .run
            .as_ref()
            .map(|run| run.state.is_terminal())
            .unwrap_or(true);
        if terminal {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Run did not reach a terminal state within {deadline_ms}ms"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn setup() -> (
    BoundTestHome,
    Arc<ScanCoordinator>,
    Arc<FaultInjectingScanEvidenceStoreFactory>,
    Arc<ScanMutationCoordinator>,
) {
    let home = BoundTestHome::new();
    let root = home.path().join("agent-skills");
    std::fs::create_dir_all(&root).expect("agent root");
    // Two skills: one directory entity, one symlink to an external entity.
    let alpha = root.join("alpha-skill");
    std::fs::create_dir_all(&alpha).expect("alpha entity");
    std::fs::write(alpha.join("SKILL.md"), "# Alpha\n").expect("SKILL.md");
    let external = home.path().join("Projects").join("beta-skill");
    std::fs::create_dir_all(&external).expect("beta entity");
    std::fs::write(external.join("SKILL.md"), "# Beta\n").expect("SKILL.md");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, root.join("beta-skill")).expect("beta symlink");

    let factory = Arc::new(FaultInjectingScanEvidenceStoreFactory::new(
        home.path().to_path_buf(),
    ));
    let mutation = Arc::new(ScanMutationCoordinator::new());
    let agent_store = Arc::new(StubAgentStore {
        roots: vec![StoredGlobalSkillRoot {
            root_id: "root-0".into(),
            configured_path: root,
            path_identity_key: "root-0".into(),
            consumer_agent_ids: vec!["a1".into()],
            activation_skill_ids: vec![],
        }],
        configurations: vec![],
        version: 1,
    });
    let coordinator = Arc::new(
        ScanCoordinator::new(
            factory.clone(),
            Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
            Arc::new(EmptyInstallerLockStore),
            Arc::new(SystemLocalGitProbe),
            home.write_gate.clone(),
            agent_store,
            mutation.clone(),
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
            Arc::new(StubManagedFacts),
            home.state_dir.clone(),
            Arc::new(SystemClock::new()),
        )
        .with_unresponsive_ms(1_000),
    );
    (home, coordinator, factory, mutation)
}

fn setup_with_test_scan_filesystem()
-> (BoundTestHome, Arc<ScanCoordinator>, Arc<TestScanFileSystem>) {
    let home = BoundTestHome::new();
    let root = home.path().join("agent-skills");
    std::fs::create_dir_all(&root).expect("agent root");
    let filesystem = Arc::new(TestScanFileSystem::new(home.path().to_path_buf()));
    let factory = Arc::new(FaultInjectingScanEvidenceStoreFactory::new(
        home.path().to_path_buf(),
    ));
    let coordinator = Arc::new(
        ScanCoordinator::new(
            factory,
            filesystem.clone(),
            Arc::new(EmptyInstallerLockStore),
            Arc::new(SystemLocalGitProbe),
            home.write_gate.clone(),
            Arc::new(StubAgentStore {
                roots: vec![StoredGlobalSkillRoot {
                    root_id: "root-0".into(),
                    configured_path: root,
                    path_identity_key: "root-0".into(),
                    consumer_agent_ids: vec!["a1".into()],
                    activation_skill_ids: vec![],
                }],
                configurations: vec![],
                version: 1,
            }),
            Arc::new(ScanMutationCoordinator::new()),
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
            Arc::new(StubManagedFacts),
            home.state_dir.clone(),
            Arc::new(SystemClock::new()),
        )
        .with_unresponsive_ms(1_000),
    );
    (home, coordinator, filesystem)
}

use skill_man_lib::adapters::app_state_store::AppStateStoreFileSystem;

#[test]
fn manual_rescan_streams_evidence_and_publishes_complete_report() {
    let (home, coordinator, _factory, _mutation) = setup();
    let started = coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("start");
    assert!(
        started
            .run
            .as_ref()
            .map(|run| matches!(run.state, ScanRunState::Queued | ScanRunState::Running))
            .unwrap_or(false),
        "the Run starts queued/running, never terminal"
    );
    wait_terminal(&coordinator, 10_000);
    let snapshot = coordinator.snapshot();
    let run = snapshot.run.expect("terminal run retained");
    assert_eq!(run.state, ScanRunState::Completed);
    assert!(
        run.counts.entries >= 2,
        "alpha + beta entries: {:?}",
        run.counts
    );
    assert!(run.counts.entities >= 2);
    assert!(run.counts.files >= 2);
    assert!(run.counts.bytes > 0);
    let report = snapshot.current_report.summary.expect("report");
    assert_eq!(
        report.state,
        skill_man_lib::core::scan::ScanReportState::Complete,
        "coverage: {:?}",
        report.coverage
    );
    assert_eq!(report.generation, 1);
    // Streaming artifacts on disk: the temp-dir factory stores under
    // <root>/<home_id>/scan.
    let scan_dir = home.path().join(HOME_ID).join("scan");
    let entries = std::fs::read_dir(scan_dir.join("runs")).expect("runs dir");
    let run_dir = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .next()
        .expect("a run directory");
    let root_dir = run_dir.join("roots/r0");
    let entry_lines =
        std::fs::read_to_string(root_dir.join("entries.jsonl")).expect("entries stream");
    assert!(entry_lines.contains("alpha-skill"));
    assert!(entry_lines.contains("beta-skill"));
    assert!(
        std::fs::read_to_string(root_dir.join("entities.jsonl"))
            .expect("entities stream")
            .contains("alpha-skill")
    );
    // Current manifest switch is the single current pointer.
    let store = skill_man_lib::adapters::scan_evidence_store::SystemScanEvidenceStore::new(
        scan_dir,
        home.home.home_id.0.clone(),
    );
    match store.current_manifest().unwrap() {
        CurrentManifestRead::Report(report) => {
            assert_eq!(report.run_id, run.run_id);
            assert_eq!(report.state, "complete");
        }
        other => panic!("expected a current Report, got {other:?}"),
    }
    // A new manual trigger after a terminal Run starts a new generation.
    let second = coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("second");
    assert_eq!(second.run.as_ref().unwrap().generation, 2);
    let _ = home.dir.path();
}

#[test]
fn ignoring_a_local_candidate_persists_and_excludes_only_that_entity() {
    let (home, coordinator, factory, mutation) = setup();
    coordinator.start_rescan(ScanTrigger::Manual).unwrap();
    wait_terminal(&coordinator, 10_000);
    let report = coordinator.snapshot().current_report.summary.unwrap();
    let (rows, _) = page_section(
        &coordinator,
        &report,
        ScanReportSection::LocalCandidates,
        64,
    );
    let ScanReportRow::SourceVerdict(candidate) = &rows[0] else {
        panic!("local candidate")
    };
    let original = std::fs::read(candidate.canonical_path.join("SKILL.md")).unwrap();
    assert!(
        coordinator
            .ignore_local_candidate("wrong report", report.generation, candidate.entity_seq)
            .is_err()
    );
    assert_eq!(mutation.generation(), 0);
    coordinator
        .ignore_local_candidate(
            &report.content_identity,
            report.generation,
            candidate.entity_seq,
        )
        .unwrap();
    assert_eq!(
        mutation.generation(),
        0,
        "ignoring a candidate does not change Skill files"
    );
    assert_eq!(
        coordinator.snapshot().current_report.freshness,
        ReportFreshness::Current
    );
    // A new store instance reads the durable choice, not process-local state.
    let bound = home.write_gate.bound_home().unwrap();
    assert_eq!(
        factory.store_for(&bound).unwrap().ignored_paths().unwrap(),
        vec![candidate.canonical_path.clone()]
    );
    coordinator.start_rescan(ScanTrigger::Manual).unwrap();
    wait_terminal(&coordinator, 10_000);
    let next = coordinator.snapshot().current_report.summary.unwrap();
    let (remaining, _) = page_section(&coordinator, &next, ScanReportSection::LocalCandidates, 64);
    assert_eq!(remaining.len(), rows.len() - 1);
    let (excluded, _) = page_section(&coordinator, &next, ScanReportSection::Excluded, 64);
    let ignored = excluded
        .iter()
        .find_map(|row| match row {
            ScanReportRow::SourceVerdict(row) if row.reason_kind.as_deref() == Some("ignored") => {
                Some(row)
            }
            _ => None,
        })
        .expect("ignored classification");
    assert_eq!(ignored.canonical_path, candidate.canonical_path);
    assert!(ignored.operations.is_empty());
    assert!(
        coordinator
            .ignore_local_candidate(&next.content_identity, next.generation, ignored.entity_seq)
            .is_err()
    );
    assert_eq!(
        std::fs::read(candidate.canonical_path.join("SKILL.md")).unwrap(),
        original
    );
}

/// Full #84 classification contract over the real engine: Local candidates
/// (in-place + move-required), an aggregated Git source group from supported
/// worktree metadata, fixture exclusion and the typed operation eligibility
/// of a complete Report. (spec §8.2, ADR-0017; no lock file needed — the
/// worktree hint alone is a source hint inside the control zone.)
#[test]
fn classification_verdicts_and_eligibility_of_a_complete_report() {
    let (home, coordinator, _factory, _mutation) = setup();
    // Extend the root with a Git worktree repository (supported provider)
    // and a fixture directory.
    let root = home.path().join("agent-skills");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).expect("repo dir");
    let repo_skill = repo.join("repo-skill");
    std::fs::create_dir_all(&repo_skill).expect("repo skill");
    std::fs::write(repo_skill.join("SKILL.md"), "# Repo\n").expect("SKILL.md");
    let gitdir = repo.join(".git");
    std::fs::create_dir_all(&gitdir).expect("gitdir");
    std::fs::write(
        gitdir.join("config"),
        "[remote \"origin\"]\n\turl = https://github.com/owner/example.git\n",
    )
    .expect("git config");
    let fixture = root.join("fixture-entities").join("fixture-skill");
    std::fs::create_dir_all(&fixture).expect("fixture");
    std::fs::write(fixture.join("SKILL.md"), "# Fixture\n").expect("fixture md");

    let started = coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("start");
    assert!(started.run.is_some());
    wait_terminal(&coordinator, 10_000);
    let snapshot = coordinator.snapshot();
    let run = snapshot.run.expect("terminal run retained");
    assert_eq!(run.state, ScanRunState::Completed);
    let report = snapshot.current_report.summary.expect("report");
    assert_eq!(
        report.state,
        skill_man_lib::core::scan::ScanReportState::Complete
    );

    // Four-card counts: entities under the control zone classify as Local
    // (move-required) or Git source group; the fixture is Excluded.
    assert!(
        report.source_counts.local_candidates >= 2,
        "{:?}",
        report.source_counts
    );
    assert_eq!(
        report.source_counts.git_groups, 1,
        "{:?}",
        report.source_counts
    );
    assert!(
        report.source_counts.excluded >= 1,
        "{:?}",
        report.source_counts
    );
    assert_eq!(report.source_counts.conflict_sets, 0);

    // Git group row: provider + canonical repository + the candidate
    // operation that requires complete coverage (allowed here).
    let (git_rows, _) = page_section(&coordinator, &report, ScanReportSection::GitSources, 64);
    assert_eq!(git_rows.len(), 1);
    match &git_rows[0] {
        ScanReportRow::GitSourceGroup(group) => {
            assert_eq!(group.provider, "github");
            assert_eq!(
                group.canonical_repository,
                "https://github.com/owner/example"
            );
            assert_eq!(group.status, "candidate");
            let fetch = group
                .operations
                .iter()
                .find(|op| op.operation == "git_fetch_and_manage")
                .expect("fetch operation");
            assert!(fetch.allowed);
            assert!(fetch.closed_reason.is_none());
        }
        other => panic!("expected GitSourceGroup, got {other:?}"),
    }

    // Local candidates: the in-place external link and the move-required
    // alpha entity; each carries typed eligibility.
    let (local_rows, _) = page_section(
        &coordinator,
        &report,
        ScanReportSection::LocalCandidates,
        64,
    );
    assert_eq!(local_rows.len(), 2);
    for row in &local_rows {
        let ScanReportRow::SourceVerdict(verdict) = row else {
            panic!("expected SourceVerdict, got {row:?}");
        };
        assert_eq!(verdict.verdict, "local");
        assert!(
            verdict
                .operations
                .iter()
                .any(|op| op.operation == "local_link" || op.operation == "local_link_with_move"),
            "{:?}",
            verdict.operations
        );
    }

    // Fixture entities are Excluded with no selection control.
    let (excluded_rows, _) = page_section(&coordinator, &report, ScanReportSection::Excluded, 64);
    assert!(!excluded_rows.is_empty());
    for row in &excluded_rows {
        let ScanReportRow::SourceVerdict(verdict) = row else {
            panic!("expected SourceVerdict for excluded, got {row:?}");
        };
        assert_eq!(verdict.verdict, "excluded");
        assert!(verdict.operations.is_empty());
    }
}

/// An Incomplete Report (a failed Root) disables destructive operation
/// eligibility with the Core closed reason `scan_incomplete`, while the
/// non-destructive Local Link stays allowed (spec §8.1).
#[test]
fn incomplete_report_blocks_destructive_eligibility() {
    let (home, _coordinator, factory, _mutation) = setup();
    // A second configured Root whose canonical identity cannot resolve:
    // its planned Root is Failed and the Report is Incomplete.
    const BROKEN_PATH: &str = "/definitely/not/a/real/path/scan-root-841";
    let agent_store = Arc::new(StubAgentStore {
        roots: vec![
            StoredGlobalSkillRoot {
                root_id: "root-0".into(),
                configured_path: home.path().join("agent-skills"),
                path_identity_key: "root-0".into(),
                consumer_agent_ids: vec!["a1".into()],
                activation_skill_ids: vec![],
            },
            StoredGlobalSkillRoot {
                root_id: "root-1".into(),
                configured_path: PathBuf::from(BROKEN_PATH),
                path_identity_key: "root-1".into(),
                consumer_agent_ids: vec!["a1".into()],
                activation_skill_ids: vec![],
            },
        ],
        configurations: vec![],
        version: 2,
    });
    let mutation = Arc::new(ScanMutationCoordinator::new());
    let coordinator = Arc::new(
        ScanCoordinator::new(
            factory,
            Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
            Arc::new(EmptyInstallerLockStore),
            Arc::new(SystemLocalGitProbe),
            home.write_gate.clone(),
            agent_store,
            mutation,
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
            Arc::new(StubManagedFacts),
            home.state_dir.clone(),
            Arc::new(SystemClock::new()),
        )
        .with_unresponsive_ms(1_000),
    );

    let started = coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("start");
    assert!(started.run.is_some());
    wait_terminal(&coordinator, 10_000);
    let snapshot = coordinator.snapshot();
    let run = snapshot.run.expect("terminal run retained");
    assert_eq!(run.state, ScanRunState::Completed);
    let report = snapshot.current_report.summary.expect("report");
    assert_eq!(
        report.state,
        skill_man_lib::core::scan::ScanReportState::Incomplete
    );
    assert!(report.incomplete);
    assert!(report.coverage.failed >= 1);

    // Local candidate: the in-place external beta-skill link (outside the
    // control zone) keeps its non-destructive Local Link eligibility.
    let (local_rows, _) = page_section(
        &coordinator,
        &report,
        ScanReportSection::LocalCandidates,
        64,
    );
    for row in &local_rows {
        let ScanReportRow::SourceVerdict(verdict) = row else {
            panic!("expected SourceVerdict, got {row:?}");
        };
        for op in &verdict.operations {
            if op.operation == "local_link" {
                assert!(op.allowed, "non-destructive Local Link must stay allowed");
            }
            if op.operation == "local_link_with_move" {
                assert!(!op.allowed, "move requires complete coverage");
                assert_eq!(
                    op.closed_reason.as_deref(),
                    Some("scan_incomplete"),
                    "the Core closed reason is consistent"
                );
            }
        }
    }
    // The failed Root diagnostic remains visible in coverage rows.
    let (root_rows, _) = page_section(&coordinator, &report, ScanReportSection::Roots, 64);
    assert!(
        root_rows.iter().any(|row| matches!(
            row,
            ScanReportRow::RootCoverage(root)
                if root.state == skill_man_lib::seams::scan_evidence_store::ScanRootState::Failed
        )),
        "failed coverage row with diagnostic must persist: {root_rows:?}"
    );
}

/// Git source hints from an applicable lock claim (no worktree needed):
/// the claim's declaration facts become the group's External Ownership
/// Claim evidence, and the group is aggregated by provider + canonical
/// repository (§8.2). The claim's `source_type` must agree with the URL.
#[test]
fn lock_claim_forms_git_source_group_with_external_ownership_evidence() {
    let home = BoundTestHome::new();
    let root = home.path().join("agent-skills");
    std::fs::create_dir_all(&root).expect("agent root");
    let alpha = root.join("alpha-skill");
    std::fs::create_dir_all(&alpha).expect("alpha entity");
    std::fs::write(alpha.join("SKILL.md"), "# Alpha\n").expect("SKILL.md");

    // v3 installer lock declaring the alpha entry from GitHub.
    let lock_dir = home.path().join(".agents");
    std::fs::create_dir_all(&lock_dir).expect("lock dir");
    std::fs::write(
        lock_dir.join(".skill-lock.json"),
        r#"{
  "version": 3,
  "skills": {
    "alpha-skill": {
      "sourceType": "github",
      "source": "https://github.com/owner/example",
      "sourceUrl": "https://github.com/owner/example",
      "skillPath": "skills/alpha-skill",
      "skillFolderHash": "deadbeef",
      "ref": "v1.0.0"
    }
  }
}"#,
    )
    .expect("lock json");

    let factory = Arc::new(FaultInjectingScanEvidenceStoreFactory::new(
        home.path().to_path_buf(),
    ));
    let mutation = Arc::new(ScanMutationCoordinator::new());
    let agent_store = Arc::new(StubAgentStore {
        roots: vec![StoredGlobalSkillRoot {
            root_id: "root-0".into(),
            configured_path: root,
            path_identity_key: "root-0".into(),
            consumer_agent_ids: vec!["a1".into()],
            activation_skill_ids: vec![],
        }],
        configurations: vec![],
        version: 1,
    });
    let coordinator = Arc::new(
        ScanCoordinator::new(
            factory,
            Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
            Arc::new(
                skill_man_lib::adapters::system_installer_lock_store::SystemInstallerLockStore::new(
                    home.path().to_path_buf(),
                ),
            ),
            Arc::new(SystemLocalGitProbe),
            home.write_gate.clone(),
            agent_store,
            mutation,
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
            Arc::new(StubManagedFacts),
            home.state_dir.clone(),
            Arc::new(SystemClock::new()),
        )
        .with_unresponsive_ms(1_000),
    );

    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("start");
    wait_terminal(&coordinator, 10_000);
    let snapshot = coordinator.snapshot();
    let run = snapshot.run.expect("terminal run retained");
    assert_eq!(run.state, ScanRunState::Completed);
    let report = snapshot.current_report.summary.expect("report");
    assert_eq!(
        report.source_counts.git_groups, 1,
        "{:?}",
        report.source_counts
    );
    let (git_rows, _) = page_section(&coordinator, &report, ScanReportSection::GitSources, 64);
    match git_rows.first().expect("a git group row") {
        ScanReportRow::GitSourceGroup(group) => {
            assert_eq!(group.provider, "github");
            assert_eq!(
                group.canonical_repository,
                "https://github.com/owner/example"
            );
            assert_eq!(group.status, "candidate");
            assert_eq!(group.refs, vec!["v1.0.0".to_owned()]);
            assert_eq!(group.lock_paths.len(), 1);
            let claim = group.lock_claims.first().expect("external ownership claim");
            assert_eq!(claim.entry_name, "alpha-skill");
            assert_eq!(claim.source_type.as_deref(), Some("github"));
            assert_eq!(claim.skill_path.as_deref(), Some("skills/alpha-skill"));
        }
        other => panic!("expected GitSourceGroup, got {other:?}"),
    }
}

/// The classification index write fault point (spec §11): a classification
/// write failure fails the whole Run store-wide and keeps the old Report —
/// a torn classification is never published.
#[test]
fn classification_write_failure_fails_run_and_keeps_old_report() {
    let home = BoundTestHome::new();
    let root = home.path().join("agent-skills");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::create_dir_all(root.join("alpha-skill")).expect("alpha");
    std::fs::write(root.join("alpha-skill/SKILL.md"), "# Alpha\n").expect("md");

    let factory = Arc::new(FaultInjectingScanEvidenceStoreFactory::new(
        home.path().to_path_buf(),
    ));
    let mutation = Arc::new(ScanMutationCoordinator::new());
    let agent_store = Arc::new(StubAgentStore {
        roots: vec![StoredGlobalSkillRoot {
            root_id: "root-0".into(),
            configured_path: root,
            path_identity_key: "root-0".into(),
            consumer_agent_ids: vec!["a1".into()],
            activation_skill_ids: vec![],
        }],
        configurations: vec![],
        version: 1,
    });
    let coordinator = Arc::new(
        ScanCoordinator::new(
            factory.clone(),
            Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
            Arc::new(EmptyInstallerLockStore),
            Arc::new(SystemLocalGitProbe),
            home.write_gate.clone(),
            agent_store,
            mutation,
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
            Arc::new(StubManagedFacts),
            home.state_dir.clone(),
            Arc::new(SystemClock::new()),
        )
        .with_unresponsive_ms(1_000),
    );

    // First a clean Run publishes a Report (the one that must survive).
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("start");
    wait_terminal(&coordinator, 10_000);
    let first = coordinator.snapshot();
    assert_eq!(first.run.as_ref().unwrap().state, ScanRunState::Completed);
    let first_identity = first
        .current_report
        .summary
        .as_ref()
        .expect("first report")
        .content_identity
        .clone();

    // The next Run fails at the classification write — old Report intact.
    factory.fail_next(skill_man_lib::seams::scan_evidence_store::fault_points::CLASSIFICATION);
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("start");
    wait_terminal(&coordinator, 10_000);
    let second = coordinator.snapshot();
    assert_eq!(
        second.run.as_ref().unwrap().state,
        ScanRunState::Failed,
        "classification failure is store-wide"
    );
    assert!(
        second
            .run
            .as_ref()
            .unwrap()
            .diagnostic
            .as_deref()
            .map(|detail| detail.contains("source classification failed"))
            .unwrap_or(false)
    );
    let report = second.current_report.summary.as_ref().expect("old report");
    assert_eq!(
        report.content_identity, first_identity,
        "old Report is kept"
    );
}

#[test]
fn failed_root_forms_incomplete_report_with_healthy_root_evidence() {
    let home = BoundTestHome::new();
    let good_root = home.path().join("good-skills");
    std::fs::create_dir_all(&good_root).expect("good root");
    let skill = good_root.join("skill-a");
    std::fs::create_dir_all(&skill).expect("skill a");
    std::fs::write(skill.join("SKILL.md"), "# A\n").expect("doc");
    let factory = Arc::new(FaultInjectingScanEvidenceStoreFactory::new(
        home.path().to_path_buf(),
    ));
    let mutation = Arc::new(ScanMutationCoordinator::new());
    let agent_store = Arc::new(StubAgentStore {
        roots: vec![
            StoredGlobalSkillRoot {
                root_id: "good".into(),
                configured_path: good_root,
                path_identity_key: "good".into(),
                consumer_agent_ids: vec!["a1".into()],
                activation_skill_ids: vec![],
            },
            StoredGlobalSkillRoot {
                root_id: "lost".into(),
                configured_path: home.path().join("gone-skills"),
                path_identity_key: "lost".into(),
                consumer_agent_ids: vec!["a2".into()],
                activation_skill_ids: vec![],
            },
        ],
        configurations: vec![],
        version: 1,
    });
    let coordinator = Arc::new(
        ScanCoordinator::new(
            factory.clone(),
            Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
            Arc::new(EmptyInstallerLockStore),
            Arc::new(SystemLocalGitProbe),
            home.write_gate.clone(),
            agent_store,
            mutation,
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
            Arc::new(StubManagedFacts),
            home.state_dir.clone(),
            Arc::new(SystemClock::new()),
        )
        .with_unresponsive_ms(1_000),
    );
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("start");
    wait_terminal(&coordinator, 10_000);
    let snapshot = coordinator.snapshot();
    let run = snapshot.run.as_ref().expect("run");
    assert_eq!(
        run.state,
        ScanRunState::Completed,
        "the Run itself completes"
    );
    let report = snapshot.current_report.summary.expect("report");
    assert_eq!(
        report.state,
        skill_man_lib::core::scan::ScanReportState::Incomplete,
        "a failed Root must yield an Incomplete Report"
    );
    assert_eq!(report.counts.failed_roots, 1);
    // The healthy Root still produced evidence.
    assert!(report.counts.entries >= 1, "{:?}", report.counts);
    assert_eq!(report.coverage.completed, 1);
    assert_eq!(report.coverage.failed, 1);
    // The failed Root carries a diagnostic, not fabricated coverage.
    let roots = page_roots(&coordinator, &report);
    let lost = roots
        .iter()
        .find(|root| root.index == 1)
        .expect("failed root row");
    assert_eq!(
        lost.state,
        skill_man_lib::seams::scan_evidence_store::ScanRootState::Failed
    );
    assert!(lost.diagnostic.is_some());
}

#[test]
fn cancel_keeps_the_previous_report_untouched() {
    let (home, coordinator, factory, _mutation) = setup();
    // First run completes normally: an old current Report exists.
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("first");
    wait_terminal(&coordinator, 10_000);
    let old_run_id = coordinator
        .snapshot()
        .run
        .as_ref()
        .expect("first run")
        .run_id
        .clone();
    // Re-compose with a stalled FileSystem to hold the second Run in flight.
    let gated = Arc::new(TestScanFileSystem::new(home.path().to_path_buf()));
    gated.stall_path(home.path().join("agent-skills"));
    let mutation = Arc::new(ScanMutationCoordinator::new());
    let agent_store = Arc::new(StubAgentStore {
        roots: vec![StoredGlobalSkillRoot {
            root_id: "root-0".into(),
            configured_path: home.path().join("agent-skills"),
            path_identity_key: "root-0".into(),
            consumer_agent_ids: vec!["a1".into()],
            activation_skill_ids: vec![],
        }],
        configurations: vec![],
        version: 1,
    });
    let coordinator_gated = Arc::new(
        ScanCoordinator::new(
            factory.clone(),
            gated.clone(),
            Arc::new(EmptyInstallerLockStore),
            Arc::new(SystemLocalGitProbe),
            home.write_gate.clone(),
            agent_store,
            mutation,
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
            Arc::new(StubManagedFacts),
            home.state_dir.clone(),
            Arc::new(SystemClock::new()),
        )
        .with_unresponsive_ms(60_000),
    );
    coordinator_gated
        .start_rescan(ScanTrigger::Manual)
        .expect("second");
    // Wait until the Run is in flight (walker stalled inside the stream).
    let wait = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let state = coordinator_gated
            .snapshot()
            .run
            .as_ref()
            .map(|run| run.state)
            .unwrap_or(ScanRunState::Completed);
        if state != ScanRunState::Queued {
            break;
        }
        assert!(std::time::Instant::now() < wait, "run stalled before walk");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let run_id = coordinator_gated
        .snapshot()
        .run
        .as_ref()
        .expect("in-flight run")
        .run_id
        .clone();
    let cancelled = coordinator_gated.cancel_rescan(&run_id).expect("cancel");
    assert_eq!(
        cancelled.run.as_ref().unwrap().state,
        ScanRunState::Cancelling
    );
    // Unstall so the workers stop cooperatively at the next record
    // boundary: cancel is never blocked inside a filesystem call.
    gated.unblock_all();
    wait_terminal(&coordinator_gated, 10_000);
    let terminal = coordinator_gated.snapshot();
    assert_eq!(
        terminal.run.as_ref().unwrap().state,
        ScanRunState::Cancelled
    );
    // The previous Report is untouched.
    assert_eq!(
        terminal.current_report.summary.as_ref().unwrap().run_id,
        old_run_id,
        "cancel must keep the old current Report"
    );
    let store = skill_man_lib::adapters::scan_evidence_store::SystemScanEvidenceStore::new(
        home.path().join(HOME_ID).join("scan"),
        home.home.home_id.0.clone(),
    );
    match store.current_manifest().unwrap() {
        CurrentManifestRead::Report(report) => assert_eq!(report.run_id, old_run_id),
        other => panic!("old Report must remain current, got {other:?}"),
    }
}

#[test]
fn mutation_generation_supersedes_a_running_run() {
    let home = BoundTestHome::new();
    let root = home.path().join("agent-skills");
    std::fs::create_dir_all(&root).expect("root");
    let gated = Arc::new(TestScanFileSystem::new(home.path().to_path_buf()));
    gated.stall_path(root.clone());
    let factory = Arc::new(FaultInjectingScanEvidenceStoreFactory::new(
        home.path().to_path_buf(),
    ));
    let mutation = Arc::new(ScanMutationCoordinator::new());
    let coordinator = Arc::new(
        ScanCoordinator::new(
            factory.clone(),
            gated.clone(),
            Arc::new(EmptyInstallerLockStore),
            Arc::new(SystemLocalGitProbe),
            home.write_gate.clone(),
            Arc::new(StubAgentStore {
                roots: vec![StoredGlobalSkillRoot {
                    root_id: "root-0".into(),
                    configured_path: root,
                    path_identity_key: "root-0".into(),
                    consumer_agent_ids: vec!["a1".into()],
                    activation_skill_ids: vec![],
                }],
                configurations: vec![],
                version: 1,
            }),
            mutation.clone(),
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
            Arc::new(StubManagedFacts),
            home.state_dir.clone(),
            Arc::new(SystemClock::new()),
        )
        .with_unresponsive_ms(60_000),
    );
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("start");
    // Wait for the Run to leave Queued (planning complete).
    let wait = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let run = coordinator.snapshot().run.expect("run");
        if run.state != ScanRunState::Queued {
            break;
        }
        assert!(std::time::Instant::now() < wait, "run stalled");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // A product write bumps the mutation generation; the watchdog must
    // Supersede the Run within one tick.
    mutation.bump();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let run = coordinator.snapshot().run.as_ref().expect("run").clone();
        if run.state == ScanRunState::Superseded {
            break;
        }
        if run.state.is_terminal() {
            panic!("expected Superseded, got {:?}", run.state);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no supersede observed"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    gated.unblock_all();
    wait_terminal(&coordinator, 10_000);
    let terminal = coordinator.snapshot();
    assert_eq!(
        terminal.run.as_ref().unwrap().state,
        ScanRunState::Superseded
    );
    assert!(
        terminal.current_report.summary.is_none(),
        "no report was ever published"
    );
}

/// The typed zero-progress isolation: a Root that produces no entry/byte/
/// probe progress for the window is marked Unresponsive (not silently
/// dropped) and the Run still completes as Incomplete.
#[test]
fn zero_progress_root_is_typed_unresponsive_and_isolated() {
    let home = BoundTestHome::new();
    let root = home.path().join("agent-skills");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::create_dir_all(root.join("skill-a")).expect("skill");
    // The gated FS blocks the walk: zero progress for the 1s test window.
    let gated = Arc::new(TestScanFileSystem::new(home.path().to_path_buf()));
    gated.stall_path(root.clone());
    let factory = Arc::new(FaultInjectingScanEvidenceStoreFactory::new(
        home.path().to_path_buf(),
    ));
    let coordinator = Arc::new(
        ScanCoordinator::new(
            factory.clone(),
            gated.clone(),
            Arc::new(EmptyInstallerLockStore),
            Arc::new(SystemLocalGitProbe),
            home.write_gate.clone(),
            Arc::new(StubAgentStore {
                roots: vec![StoredGlobalSkillRoot {
                    root_id: "root-0".into(),
                    configured_path: root,
                    path_identity_key: "root-0".into(),
                    consumer_agent_ids: vec!["a1".into()],
                    activation_skill_ids: vec![],
                }],
                configurations: vec![],
                version: 1,
            }),
            Arc::new(ScanMutationCoordinator::new()),
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
            Arc::new(StubManagedFacts),
            home.state_dir.clone(),
            Arc::new(SystemClock::new()),
        )
        .with_unresponsive_ms(1_000),
    );
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("start");
    // Watch for the typed Unresponsive state while the gate still holds.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let (index, diagnostic) = loop {
        let snapshot = coordinator.snapshot();
        let found = snapshot.run.as_ref().and_then(|run| {
            run.roots
                .iter()
                .find(|root| {
                    root.state == skill_man_lib::core::scan::ScanRootViewState::Unresponsive
                })
                .map(|root| (root.index, root.diagnostic.clone()))
        });
        if let Some(found) = found {
            break found;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Unresponsive isolation never fired"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    assert_eq!(index, 0);
    assert!(
        diagnostic
            .unwrap_or_default()
            .contains("no entry/byte/probe progress")
    );
    gated.unblock_all();
    wait_terminal(&coordinator, 10_000);
    let terminal = coordinator.snapshot();
    assert_eq!(
        terminal.run.as_ref().unwrap().state,
        ScanRunState::Completed
    );
    let report = terminal.current_report.summary.expect("report");
    assert_eq!(
        report.state,
        skill_man_lib::core::scan::ScanReportState::Incomplete
    );
    assert_eq!(report.counts.failed_roots, 1);
    assert_eq!(report.coverage.unresponsive, 1);
    assert_eq!(report.coverage.completed, 0);
    let roots = page_roots(&coordinator, &report);
    assert_eq!(
        roots[0].state,
        skill_man_lib::seams::scan_evidence_store::ScanRootState::Unresponsive
    );
}

#[test]
fn repeated_triggers_reuse_the_active_run_single_flight() {
    let home = BoundTestHome::new();
    let root = home.path().join("agent-skills");
    std::fs::create_dir_all(&root).expect("root");
    let gated = Arc::new(TestScanFileSystem::new(home.path().to_path_buf()));
    gated.stall_path(root.clone());
    let factory = Arc::new(FaultInjectingScanEvidenceStoreFactory::new(
        home.path().to_path_buf(),
    ));
    let coordinator = Arc::new(
        ScanCoordinator::new(
            factory.clone(),
            gated.clone(),
            Arc::new(EmptyInstallerLockStore),
            Arc::new(SystemLocalGitProbe),
            home.write_gate.clone(),
            Arc::new(StubAgentStore {
                roots: vec![],
                configurations: vec![],
                version: 1,
            }),
            Arc::new(ScanMutationCoordinator::new()),
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
            Arc::new(StubManagedFacts),
            home.state_dir.clone(),
            Arc::new(SystemClock::new()),
        )
        .with_unresponsive_ms(60_000),
    );
    let first = coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("first");
    let first_run_id = first.run.as_ref().unwrap().run_id.clone();
    let second = coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("second");
    assert_eq!(
        second.run.as_ref().unwrap().run_id,
        first_run_id,
        "a running Run is reused, not restarted"
    );
    assert_eq!(
        second.run.as_ref().unwrap().generation,
        first.run.as_ref().unwrap().generation
    );
    gated.unblock_all();
    wait_terminal(&coordinator, 10_000);
}

/// Fault matrix (spec §11): the store-wide publish failure must fail the
/// whole Run and keep the old current Report.
#[test]
fn manifest_switch_failure_fails_the_run_and_keeps_the_old_report() {
    let (_home, coordinator, factory, _mutation) = setup();
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("first");
    wait_terminal(&coordinator, 10_000);
    let old_run_id = coordinator
        .snapshot()
        .run
        .as_ref()
        .expect("first")
        .run_id
        .clone();
    factory.fail_next(skill_man_lib::seams::scan_evidence_store::fault_points::PUBLISH_MANIFEST);
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("second");
    wait_terminal(&coordinator, 10_000);
    let terminal = coordinator.snapshot();
    assert_eq!(
        terminal.run.as_ref().unwrap().state,
        ScanRunState::Failed,
        "manifest switch failure must fail the whole Run"
    );
    assert_eq!(
        terminal.current_report.summary.as_ref().unwrap().run_id,
        old_run_id,
        "the old current Report survives a store failure"
    );
    assert!(terminal.run.as_ref().unwrap().diagnostic.is_some());
}

/// Fault matrix: a disk-full store error is store-wide → whole Run Failed,
/// old Report kept; a root-local entry write failure fails only that Root.
#[test]
fn write_failures_classify_root_local_vs_store_wide() {
    let (_home, coordinator, factory, _mutation) = setup();
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("first");
    wait_terminal(&coordinator, 10_000);

    // Root-local: an injected entry write failure fails that Root only and
    // the Run still completes with an Incomplete Report.
    factory.fail_next(skill_man_lib::seams::scan_evidence_store::fault_points::WRITE_ENTRY);
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("second");
    wait_terminal(&coordinator, 10_000);
    let terminal = coordinator.snapshot();
    assert_eq!(
        terminal.run.as_ref().unwrap().state,
        ScanRunState::Completed
    );
    let report = terminal.current_report.summary.expect("report");
    assert_eq!(
        report.state,
        skill_man_lib::core::scan::ScanReportState::Incomplete,
        "coverage: {:?}",
        report.coverage
    );

    // Store-wide: disk full fails the whole Run and keeps the old Report.
    factory.fail_next(skill_man_lib::seams::scan_evidence_store::fault_points::DISK_FULL);
    let previous_run_id = report.run_id.clone();
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("third");
    wait_terminal(&coordinator, 10_000);
    let terminal = coordinator.snapshot();
    assert_eq!(terminal.run.as_ref().unwrap().state, ScanRunState::Failed);
    assert_eq!(
        terminal.current_report.summary.as_ref().unwrap().run_id,
        previous_run_id
    );
}

/// A terminal Root record is the Root commit point: its write failure marks
/// that Root failed instead of leaving the progress record Completed.
#[test]
fn root_terminal_write_failure_publishes_incomplete_report() {
    let (_home, coordinator, factory, _mutation) = setup();
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("first");
    wait_terminal(&coordinator, 10_000);

    factory.fail_next(skill_man_lib::seams::scan_evidence_store::fault_points::WRITE_ROOT);
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("second");
    wait_terminal(&coordinator, 10_000);

    let snapshot = coordinator.snapshot();
    assert_eq!(
        snapshot.run.as_ref().unwrap().state,
        ScanRunState::Completed
    );
    let report = snapshot.current_report.summary.expect("incomplete report");
    assert!(report.incomplete);
    assert_eq!(report.coverage.failed, 1);
    let roots = page_roots(&coordinator, &report);
    assert_eq!(roots.len(), 1);
    assert_eq!(
        roots[0].state,
        skill_man_lib::seams::scan_evidence_store::ScanRootState::Failed
    );
}

#[test]
fn read_only_and_closed_gates_browse_stale_report_without_starting_rescan() {
    let (home, coordinator, _factory, _mutation) = setup();
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("first");
    wait_terminal(&coordinator, 10_000);
    let report = coordinator
        .snapshot()
        .current_report
        .summary
        .expect("current report");

    home.write_gate
        .transition_to(WriteGateState::CatalogReadOnly {
            reason: ReadOnlyReason::OpenFailed,
        })
        .expect("read-only gate");
    let read_only = coordinator.snapshot();
    assert_eq!(
        read_only.current_report.freshness,
        skill_man_lib::core::scan::ReportFreshness::Stale
    );
    let page = coordinator
        .report_page(
            ScanReportCursor {
                report_content_identity: report.content_identity.clone(),
                run_id: report.run_id.clone(),
                generation: report.generation,
                section: ScanReportSection::Roots,
                offset: 0,
            },
            16,
        )
        .expect("read-only report page");
    assert_eq!(page.rows.len(), 1);
    assert!(
        coordinator.start_rescan(ScanTrigger::Manual).is_err(),
        "read-only gate must reject a new Run"
    );

    home.write_gate
        .transition_to(WriteGateState::Closed {
            reason: ClosedReason::HomeUnavailable,
        })
        .expect("closed gate");
    let page = coordinator
        .report_page(
            ScanReportCursor {
                report_content_identity: report.content_identity,
                run_id: report.run_id,
                generation: report.generation,
                section: ScanReportSection::Entities,
                offset: 0,
            },
            16,
        )
        .expect("closed report page");
    assert!(!page.rows.is_empty());
}

#[test]
fn non_utf8_root_entry_fails_coverage_with_typed_diagnostic() {
    let (home, coordinator, filesystem) = setup_with_test_scan_filesystem();
    let root = home.path().join("agent-skills");
    filesystem.inject_non_utf8_entry(&root);

    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("start");
    wait_terminal(&coordinator, 10_000);
    let snapshot = coordinator.snapshot();
    let report = snapshot.current_report.summary.expect("report");
    assert!(report.incomplete);
    assert_eq!(report.coverage.failed, 1);
    let roots = page_roots(&coordinator, &report);
    assert!(
        roots.iter().any(|root| root
            .diagnostic
            .as_deref()
            .is_some_and(|detail| { detail.contains("non_utf8_entry") })),
        "coverage failure must retain the typed non-UTF-8 diagnostic: {roots:?}"
    );
    let (diagnostics, _) = page_section(&coordinator, &report, ScanReportSection::Diagnostics, 16);
    assert!(diagnostics.iter().any(|row| {
        matches!(
            row,
            ScanReportRow::Diagnostic(diagnostic) if diagnostic.kind == "root_non_utf8"
        )
    }));
}

/// Orphaned temporary Runs are swept at the next run creation by
/// provenance (home_id + run_id + artifact identity).
#[test]
fn orphaned_temporary_run_is_swept_by_provenance() {
    let (home, coordinator, _factory, _mutation) = setup();
    // Leave a temporary Run artifact behind (a crash before publish): a job
    // like a killed process leaves `runs/<id>` without a current.json.
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("start");
    wait_terminal(&coordinator, 10_000);
    let scan_dir = home.path().join(HOME_ID).join("scan");
    let orphan_id = format!("orphan-{}", home.home.home_id.0.clone());
    let orphan_dir = scan_dir.join("runs").join(&orphan_id);
    std::fs::create_dir_all(orphan_dir.join("roots/r0")).expect("orphan dirs");
    let run_json = format!(
        r#"{{"schema_version":{},"home_id":"{}","run_id":"{}","generation":9,"trigger":"manual","frozen":{{"home_id":"{}","write_gate_generation":0,"agent_configuration_generation":1,"mutation_generation":0,"roots_fingerprint":"roots<0>","configured_agents":1,"declared_roots":1}},"roots":[],"started_at_ms":1}}"#,
        skill_man_lib::seams::scan_evidence_store::SCAN_STORE_SCHEMA_VERSION,
        home.home.home_id.0,
        orphan_id,
        home.home.home_id.0
    );
    std::fs::write(orphan_dir.join("run.json"), run_json).expect("orphan run.json");
    let store = skill_man_lib::adapters::scan_evidence_store::SystemScanEvidenceStore::new(
        scan_dir.clone(),
        home.home.home_id.0.clone(),
    );
    let removed = store.cleanup_temporary_runs().expect("sweep");
    assert_eq!(removed, 1, "the orphan temporary Run is proven and removed");
    assert!(!orphan_dir.exists());
    // The current Report still stands (the sweep does not touch it).
    assert!(matches!(
        store.current_manifest().unwrap(),
        CurrentManifestRead::Report(_)
    ));
    let _ = coordinator;
}

#[test]
fn unresponsive_isolation_keeps_other_roots_scanning() {
    // Two roots: the first is gated (zero progress → Unresponsive), the
    // second completes normally; the Run is Completed + Incomplete with the
    // healthy root's evidence present.
    let home = BoundTestHome::new();
    let good_root = home.path().join("good-skills");
    std::fs::create_dir_all(&good_root).expect("good root");
    std::fs::create_dir_all(good_root.join("skill-a")).expect("skill");
    let slow_root = home.path().join("slow-skills");
    std::fs::create_dir_all(&slow_root).expect("slow root");
    let gated = Arc::new(TestScanFileSystem::new(home.path().to_path_buf()));
    gated.stall_path(slow_root.clone());
    let factory = Arc::new(FaultInjectingScanEvidenceStoreFactory::new(
        home.path().to_path_buf(),
    ));
    let coordinator = Arc::new(
        ScanCoordinator::new(
            factory.clone(),
            gated.clone(),
            Arc::new(EmptyInstallerLockStore),
            Arc::new(SystemLocalGitProbe),
            home.write_gate.clone(),
            Arc::new(StubAgentStore {
                roots: vec![
                    StoredGlobalSkillRoot {
                        root_id: "good".into(),
                        configured_path: good_root,
                        path_identity_key: "good".into(),
                        consumer_agent_ids: vec!["a1".into()],
                        activation_skill_ids: vec![],
                    },
                    StoredGlobalSkillRoot {
                        root_id: "slow".into(),
                        configured_path: slow_root,
                        path_identity_key: "slow".into(),
                        consumer_agent_ids: vec!["a2".into()],
                        activation_skill_ids: vec![],
                    },
                ],
                configurations: vec![],
                version: 1,
            }),
            Arc::new(ScanMutationCoordinator::new()),
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
            Arc::new(StubManagedFacts),
            home.state_dir.clone(),
            Arc::new(SystemClock::new()),
        )
        .with_unresponsive_ms(800),
    );
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("start");
    // Wait for the Unresponsive state, then release the second walker.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let snapshot = coordinator.snapshot();
        let unresponsive = snapshot.run.as_ref().map(|run| {
            run.roots.iter().any(|root| {
                root.state == skill_man_lib::core::scan::ScanRootViewState::Unresponsive
            })
        });
        if unresponsive.unwrap_or(false) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "isolation never fired"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    gated.unblock_all();
    wait_terminal(&coordinator, 10_000);
    let terminal = coordinator.snapshot();
    assert_eq!(
        terminal.run.as_ref().unwrap().state,
        ScanRunState::Completed
    );
    let report = terminal.current_report.summary.expect("report");
    assert_eq!(
        report.state,
        skill_man_lib::core::scan::ScanReportState::Incomplete
    );
    let roots = page_roots(&coordinator, &report);
    let healthy = roots
        .iter()
        .find(|root| root.index == 0)
        .expect("healthy root row");
    assert_eq!(
        healthy.state,
        skill_man_lib::seams::scan_evidence_store::ScanRootState::Completed
    );
}

/// Fault matrix (spec §11 `scan.qualification.write`): a qualification
/// write failure never revokes the published Report — the Safety Snapshot
/// simply stays unqualified.
#[test]
fn qualification_write_failure_keeps_report_but_not_qualification() {
    let home = BoundTestHome::new();
    let root = home.path().join("agent-skills");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::create_dir_all(root.join("skill-a")).expect("skill");
    let factory = Arc::new(FaultInjectingScanEvidenceStoreFactory::new(
        home.path().to_path_buf(),
    ));
    let coordinator = Arc::new(
        ScanCoordinator::new(
            factory.clone(),
            Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
            Arc::new(EmptyInstallerLockStore),
            Arc::new(SystemLocalGitProbe),
            home.write_gate.clone(),
            Arc::new(StubAgentStore {
                roots: vec![StoredGlobalSkillRoot {
                    root_id: "root-0".into(),
                    configured_path: root,
                    path_identity_key: "root-0".into(),
                    consumer_agent_ids: vec!["a1".into()],
                    activation_skill_ids: vec![],
                }],
                configurations: vec![],
                version: 1,
            }),
            Arc::new(ScanMutationCoordinator::new()),
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
            Arc::new(StubManagedFacts),
            home.state_dir.clone(),
            Arc::new(SystemClock::new()),
        )
        .with_unresponsive_ms(1_000),
    );
    // A completed Restore record + startup marker make the manual Complete
    // Report eligible for qualification.
    let store = skill_man_lib::adapters::scan_evidence_store::SystemScanEvidenceStore::new(
        home.path().join(HOME_ID).join("scan"),
        home.home.home_id.0.clone(),
    );
    let marker = skill_man_lib::seams::scan_evidence_store::ScanStartupMarker {
        schema_version: skill_man_lib::seams::scan_evidence_store::SCAN_STORE_SCHEMA_VERSION,
        home_id: home.home.home_id.0.clone(),
        marked_at_ms: (SystemClock::new().unix_epoch_nanos() / 1_000_000) as u64 + 3_600_000,
    };
    store.write_startup_marker(&marker).expect("marker");
    let app_state = AppStateStoreFileSystem::new(home.state_dir.clone());
    let mut ledger = app_state.load().expect("ledger").recovery_ledger;
    ledger.completed.push(
        skill_man_lib::seams::app_state_store::RecoveryOperationRecord {
            operation_id: "op-q".into(),
            kind: "fixture_recovery".into(),
            home_id: Some(home.home.home_id.clone()),
            live_path: Some(home.library_root.clone()),
            snapshot_path: Some(
                home.library_root
                    .parent()
                    .unwrap()
                    .join("skill-man.snapshot-op-q"),
            ),
            prepared_path: None,
            manifest_hash: None,
            external_probe: None,
            cursor: Some("committed".into()),
            commit_point: Some("committed".into()),
            created_at: "2026-08-15T00:00:00Z".into(),
        },
    );
    app_state
        .write_recovery_ledger(&ledger)
        .expect("ledger write");
    factory.fail_next(skill_man_lib::seams::scan_evidence_store::fault_points::QUALIFICATION_WRITE);
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("start");
    wait_terminal(&coordinator, 10_000);
    let terminal = coordinator.snapshot();
    assert_eq!(
        terminal.run.as_ref().unwrap().state,
        ScanRunState::Completed,
        "a qualification write failure never fails the Run"
    );
    assert_eq!(
        terminal.current_report.summary.as_ref().unwrap().state,
        skill_man_lib::core::scan::ScanReportState::Complete
    );
    assert!(
        store.qualification().unwrap().is_none(),
        "the Snapshot stays unqualified when the qualification write fails"
    );
}

/// ADR-0017 §canonical entity: the same final file-system object reached
/// through multiple Roots and multiple symlinks forms exactly one
/// generation-bound canonical entity; every appearance is preserved.
#[test]
fn canonical_entity_aggregates_appearances_across_roots() {
    let home = BoundTestHome::new();
    let root_a = home.path().join("agent-skills");
    let root_b = home.path().join("agent-skills-b");
    let shared = home.path().join("shared-skill");
    std::fs::create_dir_all(&shared).expect("shared entity");
    std::fs::write(shared.join("SKILL.md"), "# Shared\n").expect("shared doc");
    std::fs::create_dir_all(&root_a).expect("root a");
    std::fs::create_dir_all(&root_b).expect("root b");
    std::fs::create_dir_all(root_a.join("skill-b")).expect("skill b");
    std::fs::write(root_a.join("skill-b").join("SKILL.md"), "# B\n").expect("b doc");
    // Root A: shared via one symlink. Root B: the same shared via another
    // symlink. Both + the real entry count three appearances of one entity.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&shared, root_a.join("shared-skill")).expect("link a");
        std::os::unix::fs::symlink(&shared, root_b.join("shared-skill")).expect("link b");
    }
    let factory = Arc::new(FaultInjectingScanEvidenceStoreFactory::new(
        home.path().to_path_buf(),
    ));
    let coordinator = Arc::new(
        ScanCoordinator::new(
            factory.clone(),
            Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
            Arc::new(EmptyInstallerLockStore),
            Arc::new(SystemLocalGitProbe),
            home.write_gate.clone(),
            Arc::new(StubAgentStore {
                roots: vec![
                    StoredGlobalSkillRoot {
                        root_id: "root-a".into(),
                        configured_path: root_a,
                        path_identity_key: "root-a".into(),
                        consumer_agent_ids: vec!["a1".into()],
                        activation_skill_ids: vec![],
                    },
                    StoredGlobalSkillRoot {
                        root_id: "root-b".into(),
                        configured_path: root_b,
                        path_identity_key: "root-b".into(),
                        consumer_agent_ids: vec!["a2".into()],
                        activation_skill_ids: vec![],
                    },
                ],
                configurations: vec![],
                version: 1,
            }),
            Arc::new(ScanMutationCoordinator::new()),
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
            Arc::new(StubManagedFacts),
            home.state_dir.clone(),
            Arc::new(SystemClock::new()),
        )
        .with_unresponsive_ms(1_000),
    );
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("start");
    wait_terminal(&coordinator, 10_000);
    let snapshot = coordinator.snapshot();
    assert_eq!(
        snapshot.run.as_ref().unwrap().state,
        ScanRunState::Completed
    );
    let report = snapshot.current_report.summary.expect("report");
    assert_eq!(report.counts.entries, 3, "three appearances total");
    assert_eq!(
        report.counts.entities, 2,
        "shared + skill-b: one canonical entity per distinct object"
    );
    let (rows, _) = page_section(&coordinator, &report, ScanReportSection::Entities, 16);
    let entities = rows
        .into_iter()
        .map(|row| match row {
            ScanReportRow::Entity(entity) => entity,
            other => panic!("unexpected Entity row {other:?}"),
        })
        .collect::<Vec<_>>();
    let expected_shared = std::fs::canonicalize(&shared).expect("canonical shared");
    let shared_entity = entities
        .iter()
        .find(|entity| entity.canonical_path == expected_shared)
        .expect("shared canonical entity");
    assert_eq!(
        shared_entity.appearances, 2,
        "both Roots' appearances aggregate into one entity"
    );
    // The appearances page preserves every appearance with the entity
    // binding (ADR-0017: 逐条保留).
    let (appearance_rows, _) =
        page_section(&coordinator, &report, ScanReportSection::Appearances, 16);
    let shared_appearances = appearance_rows
        .iter()
        .filter(|row| matches!(row, ScanReportRow::Appearance(appearance) if appearance.entity_seq == Some(shared_entity.entity_seq)))
        .count();
    assert_eq!(shared_appearances, 2);
}

/// The unique paged contract: stable cursors read the same generation,
/// rows are bounded per page, and a published Report switch makes the old
/// cursor typed stale instead of falling back.
#[test]
fn report_page_cursors_are_generation_bound() {
    let (_home, coordinator, _factory, _mutation) = setup();
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("first");
    wait_terminal(&coordinator, 10_000);
    let first = coordinator.snapshot();
    let report = first.current_report.summary.expect("report");
    assert_eq!(report.generation, 1);
    // Bounded first page: limit 1 returns exactly one row + continuation.
    let (rows, next) = page_section(&coordinator, &report, ScanReportSection::Entities, 1);
    assert_eq!(rows.len(), 1);
    assert!(next.is_some(), "a second entity remains");
    let continuation = coordinator
        .report_page(
            ScanReportCursor {
                report_content_identity: report.content_identity.clone(),
                run_id: report.run_id.clone(),
                generation: report.generation,
                section: ScanReportSection::Entities,
                offset: next.expect("next offset"),
            },
            1,
        )
        .expect("second entity page");
    assert_eq!(continuation.rows.len(), 1);
    assert_eq!(continuation.next_offset, None, "all entities are paged out");
    let (root_rows, _) = page_section(&coordinator, &report, ScanReportSection::Roots, 16);
    assert_eq!(root_rows.len(), 1);
    let (diagnostic_rows, _) =
        page_section(&coordinator, &report, ScanReportSection::Diagnostics, 16);
    assert_eq!(diagnostic_rows.len(), 0);
    // A new Report generation publishes: the old cursor is typed stale.
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("second");
    wait_terminal(&coordinator, 10_000);
    let second = coordinator.snapshot();
    let second_report = second.current_report.summary.expect("second report");
    assert_eq!(second_report.generation, 2);
    let stale = coordinator.report_page(
        ScanReportCursor {
            report_content_identity: report.content_identity.clone(),
            run_id: report.run_id.clone(),
            generation: report.generation,
            section: ScanReportSection::Entities,
            offset: 0,
        },
        8,
    );
    assert!(matches!(
        stale,
        Err(skill_man_lib::core::scan::ScanError::ReportPageStale(2))
    ));
}

/// Funnel counts (spec §4.6/ADR-0017): Configured Agents → declared roots →
/// canonical roots → appearances → canonical entities, all exact.
#[test]
fn funnel_counts_configured_to_canonical_layers() {
    let home = BoundTestHome::new();
    let root = home.path().join("agent-skills");
    std::fs::create_dir_all(&root).expect("root");
    let skill = root.join("skill-x");
    std::fs::create_dir_all(&skill).expect("skill");
    std::fs::write(skill.join("SKILL.md"), "# X\n").expect("doc");
    let factory = Arc::new(FaultInjectingScanEvidenceStoreFactory::new(
        home.path().to_path_buf(),
    ));
    let mut configurations = Vec::new();
    for index in 0..2 {
        configurations.push(
            skill_man_lib::seams::agent_configuration_store::StoredAgentConfiguration {
                agent_id: format!("agent-{index}"),
                origin: skill_man_lib::core::agent_configuration::AgentConfigurationOrigin::Custom,
                preset_key: None,
                name: format!("Agent {index}"),
                name_identity_key: format!("agent-{index}"),
                compatibility: skill_man_lib::core::domain::Compatibility::Verified,
                project_skills_dir: None,
                created_at: "2026-08-01T00:00:00Z".into(),
                updated_at: "2026-08-01T00:00:00Z".into(),
                memberships: vec![
                    skill_man_lib::seams::agent_configuration_store::StoredAgentRootMembership {
                        root_id: format!("root-{index}"),
                        role: skill_man_lib::core::agent_configuration::AgentRootRole::ScanOnly,
                    },
                ],
            },
        );
    }
    let coordinator = Arc::new(
        ScanCoordinator::new(
            factory.clone(),
            Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
            Arc::new(EmptyInstallerLockStore),
            Arc::new(SystemLocalGitProbe),
            home.write_gate.clone(),
            Arc::new(StubAgentStore {
                // Two different root_ids point at the same physical Root:
                // declared 2, canonical 1.
                roots: vec![
                    StoredGlobalSkillRoot {
                        root_id: "root-0".into(),
                        configured_path: root.clone(),
                        path_identity_key: "root-0".into(),
                        consumer_agent_ids: vec!["agent-0".into()],
                        activation_skill_ids: vec![],
                    },
                    StoredGlobalSkillRoot {
                        root_id: "root-1".into(),
                        configured_path: root.clone(),
                        path_identity_key: "root-1".into(),
                        consumer_agent_ids: vec!["agent-1".into()],
                        activation_skill_ids: vec![],
                    },
                ],
                configurations,
                version: 1,
            }),
            Arc::new(ScanMutationCoordinator::new()),
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
            Arc::new(StubManagedFacts),
            home.state_dir.clone(),
            Arc::new(SystemClock::new()),
        )
        .with_unresponsive_ms(1_000),
    );
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("start");
    wait_terminal(&coordinator, 10_000);
    let snapshot = coordinator.snapshot();
    assert_eq!(
        snapshot.run.as_ref().unwrap().state,
        ScanRunState::Completed
    );
    let report = snapshot.current_report.summary.expect("report");
    assert_eq!(report.counts.configured_agents, 2);
    assert_eq!(report.counts.declared_roots, 2);
    assert_eq!(
        report.counts.canonical_roots, 1,
        "one physical Root scanned once"
    );
    assert_eq!(report.counts.entries, 1);
    assert_eq!(report.counts.entities, 1);
    assert_eq!(report.coverage.completed, 1);
    assert!(!report.incomplete);
    assert_eq!(report.agent_configuration_generation, 1);
    assert!(
        report
            .configured_root_snapshot_fingerprint
            .starts_with("roots<")
    );
}

/// A synthetic large Root (beyond any old candidate cap) streams and pages:
/// hundreds of appearances over a handful of entities, bounded pages and
/// exact aggregate counts.
#[test]
fn synthetic_large_root_streams_and_pages_bounded() {
    let home = BoundTestHome::new();
    let root = home.path().join("agent-skills");
    std::fs::create_dir_all(&root).expect("root");
    for name in ["skill-a", "skill-b", "skill-c"] {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("skill dir");
        std::fs::write(dir.join("SKILL.md"), format!("# {name}\n")).expect("doc");
    }
    #[cfg(unix)]
    {
        for index in 0..300 {
            let target = root.join(format!("skill-{}", ["a", "b", "c"][index % 3]));
            std::os::unix::fs::symlink(&target, root.join(format!("alias-{index:03}")))
                .expect("alias symlink");
        }
    }
    let factory = Arc::new(FaultInjectingScanEvidenceStoreFactory::new(
        home.path().to_path_buf(),
    ));
    let coordinator = Arc::new(
        ScanCoordinator::new(
            factory.clone(),
            Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
            Arc::new(EmptyInstallerLockStore),
            Arc::new(SystemLocalGitProbe),
            home.write_gate.clone(),
            Arc::new(StubAgentStore {
                roots: vec![StoredGlobalSkillRoot {
                    root_id: "root-0".into(),
                    configured_path: root,
                    path_identity_key: "root-0".into(),
                    consumer_agent_ids: vec!["a1".into()],
                    activation_skill_ids: vec![],
                }],
                configurations: vec![],
                version: 1,
            }),
            Arc::new(ScanMutationCoordinator::new()),
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
            Arc::new(StubManagedFacts),
            home.state_dir.clone(),
            Arc::new(SystemClock::new()),
        )
        .with_unresponsive_ms(1_000),
    );
    coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("start");
    wait_terminal(&coordinator, 30_000);
    let snapshot = coordinator.snapshot();
    assert_eq!(
        snapshot.run.as_ref().unwrap().state,
        ScanRunState::Completed
    );
    let report = snapshot.current_report.summary.expect("report");
    assert_eq!(
        report.counts.entries, 303,
        "3 real + 300 aliases; coverage={:?}, summary={:?}",
        report.coverage, report
    );
    assert_eq!(
        report.counts.entities, 3,
        "exactly three distinct objects despite 303 appearances"
    );
    // Bounded pages: page 1 of 100 returns 100 rows and a continuation.
    let (rows, next) = page_section(&coordinator, &report, ScanReportSection::Appearances, 100);
    assert_eq!(rows.len(), 100);
    assert_eq!(
        next.map(|offset| offset > 0),
        Some(true),
        "continuation stays byte-precise"
    );
    // Every appearance row is bound to its canonical entity.
    let bound = rows.iter().all(|row| {
        matches!(
            row,
            ScanReportRow::Appearance(appearance) if appearance.entity_seq.is_some()
        )
    });
    assert!(bound, "all appearances bind to a canonical entity");
}
