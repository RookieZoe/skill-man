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
use skill_man_lib::core::scan::{ScanCoordinator, ScanRunState, ScanTrigger};
use skill_man_lib::seams::agent_configuration_store::{
    AgentConfigurationStore, AgentConfigurationStoreChange, AgentConfigurationStoreError,
    AgentConfigurationStoreSnapshot, RecentProjectFolder, StoredAgentConfiguration,
    StoredGlobalSkillRoot,
};
use skill_man_lib::seams::app_state_store::AppStateStore;
use skill_man_lib::seams::clock::Clock;
use skill_man_lib::seams::filesystem::{
    FileSystem, FileSystemError, ScannedSkillEntry, TreeScanEntry,
};
use skill_man_lib::seams::installer_lock_store::EmptyInstallerLockStore;
use skill_man_lib::seams::scan_evidence_store::{CurrentManifestRead, ScanEvidenceStore};

use common::{BoundTestHome, HOME_ID};

/// Test Agent Configuration store: one configured Root per entry.
struct StubAgentStore {
    roots: Vec<StoredGlobalSkillRoot>,
    version: u64,
}

impl AgentConfigurationStore for StubAgentStore {
    fn agent_configuration_snapshot(
        &self,
    ) -> Result<AgentConfigurationStoreSnapshot, AgentConfigurationStoreError> {
        Ok(AgentConfigurationStoreSnapshot {
            snapshot_version: self.version,
            configurations: Vec::<StoredAgentConfiguration>::new(),
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
    fn clear_recent_project_folders(&self) -> Result<(), AgentConfigurationStoreError> {
        unreachable!("scan never clears project folders")
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
}

impl TestScanFileSystem {
    fn new(home_directory: PathBuf) -> Self {
        Self {
            inner: MacOsFileSystem::new(home_directory),
            stall: Arc::new(Mutex::new(None)),
        }
    }
    fn stall_path(&self, path: PathBuf) {
        // The walker sees the canonicalized root (macOS /var → /private/var).
        *self.stall.lock().unwrap() = Some(std::fs::canonicalize(&path).unwrap_or(path));
    }
    fn unblock_all(&self) {
        *self.stall.lock().unwrap() = None;
    }
    fn is_stalled(&self, path: &Path) -> bool {
        self.stall
            .lock()
            .unwrap()
            .as_deref()
            .map(|stalled| stalled == path)
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
        self.inner.scan_skills_directory_stream(path, visitor)
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
            Arc::new(SystemClock::new()),
        )
        .with_unresponsive_ms(1_000),
    );
    (home, coordinator, factory, mutation)
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
        "roots: {:?}",
        report
            .roots
            .iter()
            .map(|r| (&r.state, &r.diagnostic, r.counts))
            .collect::<Vec<_>>()
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
    // The failed Root carries a diagnostic, not fabricated coverage.
    let lost = report
        .roots
        .iter()
        .find(|root| root.index == 1)
        .expect("failed root row");
    assert_eq!(
        lost.state,
        skill_man_lib::core::scan::ScanRootCoverageState::Failed
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
                version: 1,
            }),
            mutation.clone(),
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
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
                version: 1,
            }),
            Arc::new(ScanMutationCoordinator::new()),
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
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
    assert_eq!(
        report.roots[0].state,
        skill_man_lib::core::scan::ScanRootCoverageState::Unresponsive
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
                version: 1,
            }),
            Arc::new(ScanMutationCoordinator::new()),
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
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
        "{:?}",
        report.roots
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
        r#"{{"schema_version":1,"home_id":"{}","run_id":"{}","generation":9,"trigger":"manual","frozen":{{"home_id":"{}","write_gate_generation":0,"agent_configuration_generation":1,"mutation_generation":0,"roots_fingerprint":"roots<0>"}},"roots":[],"started_at_ms":1}}"#,
        home.home.home_id.0, orphan_id, home.home.home_id.0
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
                version: 1,
            }),
            Arc::new(ScanMutationCoordinator::new()),
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
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
    let healthy = report
        .roots
        .iter()
        .find(|root| root.index == 0)
        .expect("healthy root row");
    assert_eq!(
        healthy.state,
        skill_man_lib::core::scan::ScanRootCoverageState::Completed
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
                version: 1,
            }),
            Arc::new(ScanMutationCoordinator::new()),
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
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
