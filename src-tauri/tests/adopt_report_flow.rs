//! Adopt report-plan integration tests (spec §4.6, §8.1–§8.2; issue #85):
//! the terminal Scan Report consumed end to end — plan/apply/undo/finalize
//! of keep-in-place Local Links and an explicit Conflict Set winner, over
//! real adapters (MacOsFileSystem, RuntimeCatalogStore, the temp-dir
//! Evidence Store). Covers the fail-closed plan gating (no report, running
//! scan, cross-startup cache, old generation), the typed per-selection
//! eligibility (Incomplete Report → `scan_incomplete`, already-Managed
//! never replaced, Git candidates handoff-only), the generation-bound
//! entity ref revalidation (canonical path/identity/tree/appearances,
//! WriteGate, Catalog generation) and the durable Link
//! journal/undo/finalize behavior.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skill_man_lib::adapters::app_state_store::AppStateStoreFileSystem;
use skill_man_lib::adapters::local_git_probe::SystemLocalGitProbe;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::scan_evidence_store::FaultInjectingScanEvidenceStoreFactory;
use skill_man_lib::adapters::system_clock::SystemClock;
use skill_man_lib::core::adopt::{
    ACTION_CONFLICT_WINNER, ACTION_LOCAL_LINK, AdoptError, AdoptReportPlanRequest,
    AdoptReportSelection, AdoptService,
};
use skill_man_lib::core::scan::mutation::ScanMutationCoordinator;
use skill_man_lib::core::scan::{
    ReportFreshness, ScanCoordinator, ScanReportState, ScanRunState, ScanTrigger,
};
use skill_man_lib::seams::agent_configuration_store::{
    AgentConfigurationStore, AgentConfigurationStoreChange, AgentConfigurationStoreError,
    AgentConfigurationStoreSnapshot, RecentProjectFolder, StoredAgentConfiguration,
    StoredGlobalSkillRoot,
};
use skill_man_lib::seams::filesystem::{ActivationEntrySnapshot, FileSystem};
use skill_man_lib::seams::installer_lock_store::EmptyInstallerLockStore;
use skill_man_lib::seams::scan_evidence_store::{
    ScanReportCursor, ScanReportRow, ScanReportSection,
};
use skill_man_lib::seams::scan_managed_facts::{ManagedSkillPathFact, ScanManagedFactsReader};

use common::BoundTestHome;

struct StubAgentStore {
    roots: Vec<StoredGlobalSkillRoot>,
    configurations: Vec<StoredAgentConfiguration>,
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
    fn clear_recent_project_folders(&self) -> Result<(), AgentConfigurationStoreError> {
        unreachable!("scan never clears project folders")
    }
}

#[derive(Clone)]
struct StubManagedFacts {
    facts: Vec<ManagedSkillPathFact>,
}

impl ScanManagedFactsReader for StubManagedFacts {
    fn read(&self) -> Result<Vec<ManagedSkillPathFact>, String> {
        Ok(self.facts.clone())
    }
}

#[derive(Clone, Debug)]
struct TestAgent {
    id: String,
    name: String,
    root: PathBuf,
}

impl TestAgent {
    fn root_id(&self) -> String {
        format!("root-{}", self.id)
    }
}

fn agent_store(agents: &[TestAgent], version: u64) -> StubAgentStore {
    let mut roots = Vec::new();
    let mut configurations = Vec::new();
    for agent in agents {
        std::fs::create_dir_all(&agent.root).expect("agent root");
        roots.push(StoredGlobalSkillRoot {
            root_id: agent.root_id(),
            configured_path: agent.root.clone(),
            path_identity_key: format!("root-{}", agent.id),
            consumer_agent_ids: vec![agent.id.clone()],
            activation_skill_ids: vec![],
        });
        configurations.push(StoredAgentConfiguration {
            agent_id: agent.id.clone(),
            origin: skill_man_lib::core::agent_configuration::AgentConfigurationOrigin::Preset,
            preset_key: Some("claude-code".into()),
            name: agent.name.clone(),
            name_identity_key: agent.name.to_lowercase(),
            compatibility: skill_man_lib::core::domain::Compatibility::Verified,
            project_skills_dir: None,
            created_at: "1970-01-01T00:00:00Z".into(),
            updated_at: "1970-01-01T00:00:00Z".into(),
            memberships: vec![],
        });
    }
    StubAgentStore {
        roots,
        configurations,
        version,
    }
}

fn coordinator_for(
    home: &BoundTestHome,
    agent_store: Arc<StubAgentStore>,
    managed: StubManagedFacts,
) -> Arc<ScanCoordinator> {
    Arc::new(
        ScanCoordinator::new(
            Arc::new(FaultInjectingScanEvidenceStoreFactory::new(
                home.path().to_path_buf(),
            )),
            Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
            Arc::new(EmptyInstallerLockStore),
            Arc::new(SystemLocalGitProbe),
            home.write_gate.clone(),
            agent_store,
            Arc::new(ScanMutationCoordinator::new()),
            Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone())),
            Arc::new(managed),
            home.state_dir.clone(),
            Arc::new(SystemClock::new()),
        )
        .with_unresponsive_ms(1_000),
    )
}

fn adopt_service(home: &BoundTestHome, coordinator: Arc<ScanCoordinator>) -> AdoptService {
    AdoptService::new(
        home.runtime.clone(),
        Arc::new(MacOsFileSystem::new(home.path().to_path_buf())),
        Arc::new(SystemClock::new()),
        home.library_root.clone(),
        home.path().to_path_buf(),
        home.write_gate.clone(),
    )
    .with_home_context(home.write_gate.clone())
    .with_scan_coordinator(coordinator)
}

fn wait_terminal(coordinator: &ScanCoordinator, deadline_ms: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(deadline_ms);
    loop {
        if let Some(run) = coordinator.snapshot().run {
            if run.state.is_terminal() {
                return;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Scan Run did not finish in time"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn scan_and_wait(
    coordinator: &Arc<ScanCoordinator>,
) -> skill_man_lib::core::scan::ScanReportSummary {
    let started = coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("start");
    assert!(started.run.is_some());
    wait_terminal(coordinator, 10_000);
    let snapshot = coordinator.snapshot();
    let run = snapshot.run.expect("terminal run retained");
    assert_eq!(run.state, ScanRunState::Completed);
    snapshot.current_report.summary.expect("report summary")
}

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

fn entity_ref(summary: &skill_man_lib::core::scan::ScanReportSummary, seq: u64) -> String {
    skill_man_lib::core::adopt::AdoptEntityRef::new(
        summary.content_identity.clone(),
        summary.generation,
        seq,
    )
    .encode()
}

fn plan_request(
    summary: &skill_man_lib::core::scan::ScanReportSummary,
    entity_ref: String,
    action: &str,
) -> AdoptReportPlanRequest {
    AdoptReportPlanRequest {
        report_generation: summary.generation,
        selections: vec![AdoptReportSelection {
            entity_ref,
            action: action.into(),
        }],
    }
}

fn plan_service_err(service: &AdoptService) -> Result<(), AdoptError> {
    // A request against a nonexistent report identity is always PlanStale.
    let request = plan_request(
        &skill_man_lib::core::scan::ScanReportSummary {
            generation: 999_999,
            run_id: "no-run".into(),
            content_identity: "no-report".into(),
            trigger: skill_man_lib::core::scan::ScanTrigger::Manual,
            state: ScanReportState::Complete,
            coverage: Default::default(),
            counts: Default::default(),
            incomplete: false,
            published_at_ms: 0,
            agent_configuration_generation: 0,
            configured_root_snapshot_fingerprint: "none".into(),
            started_at_ms: 0,
            slow: false,
            source_counts: Default::default(),
        },
        "nobody@1@1".into(),
        ACTION_LOCAL_LINK,
    );
    service.plan_report(&request).map(|_| ())
}

/// One agent root with an external entity reachable through a symlink
/// appearance plus a controlled real directory.
fn setup_standard(home: &BoundTestHome) -> (Arc<ScanCoordinator>, TestAgent) {
    let agent = TestAgent {
        id: "a1".into(),
        name: "A1".into(),
        root: home.path().join("agent-skills"),
    };
    home.seed_agent(
        &agent.id,
        &agent.name,
        "claude_preset",
        &agent.root.to_string_lossy(),
        "verified",
    );
    (
        coordinator_for(
            home,
            Arc::new(agent_store(std::slice::from_ref(&agent), 1)),
            StubManagedFacts { facts: vec![] },
        ),
        agent,
    )
}

fn write_external_entity(entity: &Path, name: &str) {
    std::fs::create_dir_all(entity).expect("entity dir");
    std::fs::write(entity.join("SKILL.md"), format!("# {name}\n")).expect("SKILL.md");
}

/// The scan resolves macOS /var → /private/var; tests compare against the
/// canonical spelling the Report records.
fn canonical(path: &std::path::Path) -> PathBuf {
    std::fs::canonicalize(path).expect("canonicalize")
}

#[test]
fn plan_without_a_report_is_stale() {
    let home = BoundTestHome::new();
    let (coordinator, _agent) = setup_standard(&home);
    let service = adopt_service(&home, coordinator);
    assert!(matches!(
        plan_service_err(&service),
        Err(AdoptError::PlanStale)
    ));
}

#[test]
fn local_link_apply_keeps_entity_in_place_without_baseline_and_undo_restores() {
    let home = BoundTestHome::new();
    let (coordinator, agent) = setup_standard(&home);
    let external = home.path().join("Projects").join("networking");
    write_external_entity(&external, "Networking");
    let external = canonical(&external);
    std::fs::write(external.join("notes.txt"), "keep me\n").expect("extra file");
    std::os::unix::fs::symlink(&external, agent.root.join("networking")).expect("symlink");

    let service = adopt_service(&home, coordinator.clone());
    let summary = scan_and_wait(&coordinator);
    assert_eq!(summary.state, ScanReportState::Complete);
    let (rows, _) = page_section(
        &coordinator,
        &summary,
        ScanReportSection::LocalCandidates,
        64,
    );
    assert_eq!(rows.len(), 1, "exactly one Local candidate: {rows:?}");
    let ScanReportRow::SourceVerdict(verdict) = &rows[0] else {
        panic!("expected SourceVerdict, got {:?}", rows[0]);
    };
    assert_eq!(verdict.canonical_path, external);
    assert!(
        verdict
            .operations
            .iter()
            .any(|op| op.operation == "local_link" && op.allowed),
        "keep-in-place Local Link eligibility: {:?}",
        verdict.operations
    );

    let ref_token = entity_ref(&summary, verdict.entity_seq);
    let plan = service
        .plan_report(&plan_request(&summary, ref_token, ACTION_LOCAL_LINK))
        .expect("plan");
    assert!(plan.can_apply);
    assert_eq!(plan.items.len(), 1);
    assert_eq!(plan.items[0].action, ACTION_LOCAL_LINK);
    assert_eq!(plan.items[0].appearances.len(), 1);
    assert!(
        !plan.items[0].appearances[0].chain.is_empty(),
        "the symlink hop evidence chain is presented"
    );
    let bytes_before = std::fs::read(external.join("SKILL.md")).expect("bytes");
    assert_eq!(bytes_before, b"# Networking\n");

    let result = service.apply(&plan.plan_token).expect("apply");
    assert!(result.undo_available);
    assert_eq!(result.items.len(), 1);
    assert!(result.items[0].adopted, "{:?}", result.items[0]);

    // The entity stayed exactly in place: same bytes, no move/copy/rewrite,
    // nothing written into the Library skills tree.
    assert_eq!(
        std::fs::read(external.join("SKILL.md")).unwrap(),
        bytes_before
    );
    assert!(external.join("notes.txt").exists());
    assert!(
        !home.library_root.join("skills").join("networking").exists(),
        "Link never copies or moves the entity"
    );
    let rows: Vec<(Option<String>, Option<String>, Option<String>)> = {
        let mut collected = Vec::new();
        home.with_sql("read adopted Link", |connection| {
            let mut stmt = connection
                .prepare(
                    "SELECT final_entity_path, library_entry_path, recorded_content_hash FROM skills WHERE directory_name = 'networking'",
                )
                .expect("prepare");
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })
                .expect("query")
                .collect::<Result<Vec<_>, _>>()
                .expect("rows");
            rows.into_iter().for_each(|row| collected.push(row));
        });
        collected
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].0.as_deref(),
        Some(external.to_string_lossy().as_ref())
    );
    assert_eq!(rows[0].1, None, "Link records no library entry");
    assert_eq!(rows[0].2, None, "Link records no content baseline");

    // The Agent appearance became a direct Activation symlink.
    let entry = agent.root.join("networking");
    match home
        .filesystem
        .activation_snapshot(&entry)
        .expect("snapshot")
    {
        ActivationEntrySnapshot::Symlink { target } => {
            assert_eq!(target, external, "activation points directly at the entity")
        }
        other => panic!("expected activation symlink, got {other:?}"),
    }

    // Undo removes the Catalog row + activation and restores the original
    // appearance; the entity never moved.
    let undo = service.undo(&result.operation_id).expect("undo");
    assert_eq!(undo.items.len(), 1);
    assert!(undo.items[0].undone);
    let remaining: Vec<(String,)> = {
        let mut collected = Vec::new();
        home.with_sql("read after undo", |connection| {
            let mut stmt = connection
                .prepare("SELECT directory_name FROM skills WHERE directory_name = 'networking'")
                .expect("prepare");
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?,)))
                .expect("query")
                .collect::<Result<Vec<_>, _>>()
                .expect("rows");
            rows.into_iter().for_each(|row| collected.push(row));
        });
        collected
    };
    assert!(remaining.is_empty(), "Catalog row removed after Undo");
    match home
        .filesystem
        .activation_snapshot(&entry)
        .expect("snapshot")
    {
        ActivationEntrySnapshot::Symlink { target } => {
            assert_eq!(target, external, "original appearance restored")
        }
        other => panic!("expected restored symlink, got {other:?}"),
    }
    assert_eq!(
        std::fs::read(external.join("SKILL.md")).unwrap(),
        bytes_before
    );

    // Finalize closes the result window: further Undo is rejected.
    service.finalize(&result.operation_id).expect("finalize");
    assert!(matches!(
        service.undo(&result.operation_id),
        Err(AdoptError::PlanNotFound)
    ));
}

#[test]
fn plan_rejects_old_generation_and_cross_startup_cache() {
    let home = BoundTestHome::new();
    let (coordinator, agent) = setup_standard(&home);
    let external = home.path().join("Projects").join("networking");
    write_external_entity(&external, "Networking");
    let external = canonical(&external);
    std::os::unix::fs::symlink(&external, agent.root.join("networking")).expect("symlink");
    let service = adopt_service(&home, coordinator.clone());

    let summary = scan_and_wait(&coordinator);
    let (rows, _) = page_section(
        &coordinator,
        &summary,
        ScanReportSection::LocalCandidates,
        64,
    );
    let ScanReportRow::SourceVerdict(verdict) = &rows[0] else {
        panic!("expected SourceVerdict");
    };
    let ref_token = entity_ref(&summary, verdict.entity_seq);

    // A second scan publishes a new generation: the old Report identity is
    // stale — PlanStale, never a fallback.
    let second = scan_and_wait(&coordinator);
    assert!(second.generation > summary.generation);
    assert!(matches!(
        service.plan_report(&plan_request(
            &summary,
            ref_token.clone(),
            ACTION_LOCAL_LINK
        )),
        Err(AdoptError::PlanStale)
    ));
    // The current generation plans fine.
    let plan = service
        .plan_report(&plan_request(
            &second,
            entity_ref(&second, verdict.entity_seq),
            ACTION_LOCAL_LINK,
        ))
        .expect("current generation plan");
    assert!(plan.can_apply);

    // Cross-startup: a fresh coordinator over the same Home sees the Report
    // as a cross-startup cache — never a planning authority.
    let fresh = coordinator_for(
        &home,
        Arc::new(StubAgentStore {
            roots: vec![],
            configurations: vec![],
            version: 1,
        }),
        StubManagedFacts { facts: vec![] },
    );
    let fresh_view = fresh.snapshot();
    assert_eq!(fresh_view.current_report.freshness, ReportFreshness::Stale);
    assert_eq!(
        fresh_view
            .current_report
            .summary
            .as_ref()
            .unwrap()
            .content_identity,
        second.content_identity,
        "the same Report loads from disk"
    );
    let cross_service = adopt_service(&home, fresh);
    assert!(matches!(
        cross_service.plan_report(&plan_request(
            &second,
            entity_ref(&second, verdict.entity_seq),
            ACTION_LOCAL_LINK,
        )),
        Err(AdoptError::PlanStale)
    ));
}

#[test]
fn plan_rejects_running_scan() {
    let home = BoundTestHome::new();
    let (coordinator, agent) = setup_standard(&home);
    let external = home.path().join("Projects").join("networking");
    write_external_entity(&external, "Networking");
    let external = canonical(&external);
    std::os::unix::fs::symlink(&external, agent.root.join("networking")).expect("symlink");
    let service = adopt_service(&home, coordinator.clone());

    let summary = scan_and_wait(&coordinator);
    let (rows, _) = page_section(
        &coordinator,
        &summary,
        ScanReportSection::LocalCandidates,
        64,
    );
    let ScanReportRow::SourceVerdict(verdict) = &rows[0] else {
        panic!("expected SourceVerdict");
    };
    let ref_token = entity_ref(&summary, verdict.entity_seq);

    // A new Run (whether still active or already re-published) invalidates
    // the previously issued identity: either way the plan request is typed
    // PlanStale — no plan, no token.
    let started = coordinator
        .start_rescan(ScanTrigger::Manual)
        .expect("start");
    let _ = started;
    let request = plan_request(&summary, ref_token, ACTION_LOCAL_LINK);
    assert!(matches!(
        service.plan_report(&request),
        Err(AdoptError::PlanStale)
    ));
    wait_terminal(&coordinator, 10_000);
}

#[test]
fn incomplete_report_allows_only_keep_in_place_local_links() {
    let home = BoundTestHome::new();
    let agent = TestAgent {
        id: "a1".into(),
        name: "A1".into(),
        root: home.path().join("agent-skills"),
    };
    home.seed_agent(
        &agent.id,
        &agent.name,
        "claude_preset",
        &agent.root.to_string_lossy(),
        "verified",
    );
    std::fs::create_dir_all(&agent.root).expect("agent root");
    // External keep-in-place entity (symlink appearance) and a real
    // directory inside the control zone (requires move).
    let external = home.path().join("Projects").join("networking");
    write_external_entity(&external, "Networking");
    let external = canonical(&external);
    std::os::unix::fs::symlink(&external, agent.root.join("networking")).expect("symlink");
    let controlled = agent.root.join("alpha-skill");
    std::fs::create_dir_all(&controlled).expect("controlled entity");
    std::fs::write(controlled.join("SKILL.md"), "# Alpha\n").expect("SKILL.md");

    // A second configured Root that cannot canonicalize → Incomplete.
    let broken = PathBuf::from("/definitely/not/a/real/path/adopt-root-85");
    let mut store = agent_store(&[agent], 2);
    store.roots.push(StoredGlobalSkillRoot {
        root_id: "root-broken".into(),
        configured_path: broken,
        path_identity_key: "root-broken".into(),
        consumer_agent_ids: vec!["a1".into()],
        activation_skill_ids: vec![],
    });
    let coordinator = coordinator_for(&home, Arc::new(store), StubManagedFacts { facts: vec![] });
    let service = adopt_service(&home, coordinator.clone());

    let summary = scan_and_wait(&coordinator);
    assert_eq!(summary.state, ScanReportState::Incomplete);
    assert!(summary.incomplete);
    assert!(summary.coverage.failed >= 1);

    let (rows, _) = page_section(
        &coordinator,
        &summary,
        ScanReportSection::LocalCandidates,
        64,
    );
    let mut in_place_seq = None;
    let mut move_seq = None;
    for row in &rows {
        let ScanReportRow::SourceVerdict(verdict) = row else {
            continue;
        };
        if verdict
            .operations
            .iter()
            .any(|op| op.operation == "local_link" && op.allowed)
        {
            in_place_seq = Some(verdict.entity_seq);
        }
        if verdict
            .operations
            .iter()
            .any(|op| op.operation == "local_link_with_move")
            && verdict
                .operations
                .iter()
                .any(|op| op.operation == "local_link_with_move" && !op.allowed)
        {
            move_seq = Some(verdict.entity_seq);
        }
    }
    let in_place_seq = in_place_seq.expect("in-place Local candidate");
    // Keep-in-place Local Link plans even in an Incomplete Report.
    let plan = service
        .plan_report(&plan_request(
            &summary,
            entity_ref(&summary, in_place_seq),
            ACTION_LOCAL_LINK,
        ))
        .expect("in-place Link plans");
    assert!(plan.can_apply);
    // The controlled-zone entity requires a deliberate move-eligibility
    // intent: the plan surface refuses it with the Core closed reason.
    let move_seq = move_seq.expect("move-required candidate");
    let err = service
        .plan_report(&plan_request(
            &summary,
            entity_ref(&summary, move_seq),
            ACTION_LOCAL_LINK,
        ))
        .unwrap_err();
    match err {
        AdoptError::Eligibility {
            code, entity_seq, ..
        } => {
            assert_eq!(code, "scan_coverage_incomplete");
            assert_eq!(entity_seq, move_seq);
        }
        other => panic!("expected typed eligibility, got {other:?}"),
    }
}

#[test]
fn already_managed_is_never_replaced_by_local_adopt() {
    let home = BoundTestHome::new();
    let (_coordinator, agent) = setup_standard(&home);
    // A Managed Link Skill registered on the same external entity.
    let managed = home.path().join("Projects").join("networking");
    write_external_entity(&managed, "Networking");
    let managed = canonical(&managed);
    home.seed_link_skill(
        "networking",
        "Networking",
        "Managed",
        "1970-01-01T00:00:00Z",
    );
    std::os::unix::fs::symlink(&managed, agent.root.join("networking")).expect("symlink");
    let managed_facts = vec![ManagedSkillPathFact {
        directory_name: "networking".into(),
        final_entity_path: managed,
    }];
    let coordinator = coordinator_for(
        &home,
        Arc::new(agent_store(&[agent], 1)),
        StubManagedFacts {
            facts: managed_facts,
        },
    );
    let service = adopt_service(&home, coordinator.clone());

    let summary = scan_and_wait(&coordinator);
    let (rows, _) = page_section(&coordinator, &summary, ScanReportSection::Excluded, 64);
    let mut managed_seq = None;
    for row in &rows {
        let ScanReportRow::SourceVerdict(verdict) = row else {
            continue;
        };
        if verdict.verdict == "already_managed" {
            managed_seq = Some(verdict.entity_seq);
        }
    }
    let managed_seq = managed_seq.expect("already_managed row");
    let err = service
        .plan_report(&plan_request(
            &summary,
            entity_ref(&summary, managed_seq),
            ACTION_LOCAL_LINK,
        ))
        .unwrap_err();
    match err {
        AdoptError::Eligibility { code, .. } => assert_eq!(code, "already_managed"),
        other => panic!("expected typed eligibility, got {other:?}"),
    }
}

#[test]
fn git_candidate_group_is_handoff_only() {
    let home = BoundTestHome::new();
    let (coordinator, agent) = setup_standard(&home);
    // A real directory inside the control zone with a worktree hint: a Git
    // Repository Source candidate (its members are never per-member
    // planable; the group is handed to the source modules).
    let member = agent.root.join("git-member");
    std::fs::create_dir_all(&member).expect("member");
    std::fs::write(member.join("SKILL.md"), "# Member\n").expect("SKILL.md");
    std::fs::create_dir_all(agent.root.join(".git")).expect("gitdir");
    std::fs::write(
        agent.root.join(".git/config"),
        "[remote \"origin\"]\n\turl = https://github.com/example/repo.git\n",
    )
    .expect("git config");
    std::fs::write(agent.root.join(".git/HEAD"), "ref: refs/heads/main\n").expect("HEAD");

    let service = adopt_service(&home, coordinator.clone());
    let summary = scan_and_wait(&coordinator);
    let (rows, _) = page_section(&coordinator, &summary, ScanReportSection::GitSources, 16);
    assert!(
        !rows.is_empty(),
        "a Git Repository Source group aggregates the worktree hint"
    );
    // The member is a Git candidate member: not a Local candidate.
    let (locals, _) = page_section(
        &coordinator,
        &summary,
        ScanReportSection::LocalCandidates,
        64,
    );
    assert!(
        !locals.iter().any(|row| {
            matches!(row, ScanReportRow::SourceVerdict(verdict) if verdict.directory_names.first() == Some(&"git-member".to_string()))
        }),
        "Git members never enter Local candidates"
    );
    // The group row carries the closed fetch-and-manage operation gated to
    // a complete report (the handoff evidence handoff of #92).
    let group = rows.into_iter().find_map(|row| match row {
        ScanReportRow::GitSourceGroup(group) => Some(group),
        _ => None,
    });
    let group = group.expect("git group row");
    assert_eq!(group.status, "candidate");
    assert!(
        group
            .operations
            .iter()
            .any(|op| op.operation == "git_fetch_and_manage" && op.allowed)
    );
    let _ = service;
}

#[test]
fn apply_revalidates_identity_tree_appearances_and_catalog_generation() {
    let home = BoundTestHome::new();
    let (coordinator, agent) = setup_standard(&home);
    let external = home.path().join("Projects").join("networking");
    write_external_entity(&external, "Networking");
    let external = canonical(&external);
    std::os::unix::fs::symlink(&external, agent.root.join("networking")).expect("symlink");
    // A second external entity for the Catalog-generation case.
    let gamma = home.path().join("Projects").join("gamma-skill");
    write_external_entity(&gamma, "Gamma");
    let gamma = canonical(&gamma);
    std::os::unix::fs::symlink(&gamma, agent.root.join("gamma-skill")).expect("gamma symlink");

    let service = adopt_service(&home, coordinator.clone());
    let summary = scan_and_wait(&coordinator);
    let (rows, _) = page_section(
        &coordinator,
        &summary,
        ScanReportSection::LocalCandidates,
        64,
    );
    let mut networking_seq = None;
    let mut gamma_seq = None;
    for row in &rows {
        let ScanReportRow::SourceVerdict(verdict) = row else {
            continue;
        };
        if verdict.directory_names.first() == Some(&"networking".to_string()) {
            networking_seq = Some(verdict.entity_seq);
        }
        if verdict.directory_names.first() == Some(&"gamma-skill".to_string()) {
            gamma_seq = Some(verdict.entity_seq);
        }
    }
    let networking_seq = networking_seq.expect("networking candidate");
    let gamma_seq = gamma_seq.expect("gamma candidate");

    // Catalog-generation case FIRST (before any filesystem change): plan
    // networking, apply gamma (a real Catalog write bumps the generation),
    // then the frozen networking plan is PlanStale.
    let plan = service
        .plan_report(&plan_request(
            &summary,
            entity_ref(&summary, networking_seq),
            ACTION_LOCAL_LINK,
        ))
        .expect("plan");
    let gamma_plan = service
        .plan_report(&plan_request(
            &summary,
            entity_ref(&summary, gamma_seq),
            ACTION_LOCAL_LINK,
        ))
        .expect("gamma plan");
    let gamma_result = service.apply(&gamma_plan.plan_token).expect("gamma apply");
    assert!(gamma_result.undo_available);
    assert!(matches!(
        service.apply(&plan.plan_token),
        Err(AdoptError::PlanStale)
    ));

    // Tree change after planning → PlanStale.
    let plan = service
        .plan_report(&plan_request(
            &summary,
            entity_ref(&summary, networking_seq),
            ACTION_LOCAL_LINK,
        ))
        .expect("plan");
    std::fs::write(external.join("SKILL.md"), "# Networking\nchanged\n").expect("edit");
    assert!(matches!(
        service.apply(&plan.plan_token),
        Err(AdoptError::PlanStale)
    ));

    // Restore and plan again; then replace the entity (identity change) →
    // apply is PlanStale.
    std::fs::write(external.join("SKILL.md"), "# Networking\n").expect("restore");
    let plan = service
        .plan_report(&plan_request(
            &summary,
            entity_ref(&summary, networking_seq),
            ACTION_LOCAL_LINK,
        ))
        .expect("plan 2");
    std::fs::remove_dir_all(&external).expect("remove entity");
    write_external_entity(&external, "Networking");
    assert!(matches!(
        service.apply(&plan.plan_token),
        Err(AdoptError::PlanStale)
    ));
}

#[test]
fn conflict_set_requires_exactly_one_explicit_winner() {
    let home = BoundTestHome::new();
    // Two Agent roots (different physical Roots): the same casefolded
    // Directory Identity on a case-insensitive filesystem can only coexist
    // across Roots — that is exactly the Local↔Local Conflict Set.
    let agent_left = TestAgent {
        id: "a1".into(),
        name: "A1".into(),
        root: home.path().join("agent-skills-a"),
    };
    let agent_right = TestAgent {
        id: "a2".into(),
        name: "A2".into(),
        root: home.path().join("agent-skills-b"),
    };
    for agent in [&agent_left, &agent_right] {
        home.seed_agent(
            &agent.id,
            &agent.name,
            "claude_preset",
            &agent.root.to_string_lossy(),
            "verified",
        );
    }
    let coordinator = coordinator_for(
        &home,
        Arc::new(agent_store(&[agent_left.clone(), agent_right.clone()], 1)),
        StubManagedFacts { facts: vec![] },
    );
    let left = home.path().join("Projects").join("left").join("networking");
    let right = home
        .path()
        .join("Projects")
        .join("right")
        .join("Networking");
    write_external_entity(&left, "Left");
    write_external_entity(&right, "Right");
    let left = std::fs::canonicalize(&left).expect("canonical left");
    let right = std::fs::canonicalize(&right).expect("canonical right");
    std::os::unix::fs::symlink(&left, agent_left.root.join("networking")).expect("left symlink");
    std::os::unix::fs::symlink(&right, agent_right.root.join("Networking")).expect("right symlink");

    let service = adopt_service(&home, coordinator.clone());
    let summary = scan_and_wait(&coordinator);
    let (set_rows, _) = page_section(&coordinator, &summary, ScanReportSection::ConflictSets, 16);
    let member_seqs = set_rows
        .iter()
        .filter_map(|row| match row {
            ScanReportRow::ConflictSet(set) => Some(set.member_entity_seqs.clone()),
            _ => None,
        })
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(member_seqs.len(), 2, "one Conflict Set with two members");

    // `local_link` on a Conflict Set member is refused: the winner must be
    // explicit (typed closed code).
    let left_ref = entity_ref(&summary, member_seqs[0]);
    let err = service
        .plan_report(&plan_request(&summary, left_ref.clone(), ACTION_LOCAL_LINK))
        .unwrap_err();
    match &err {
        AdoptError::Eligibility { code, .. } => assert_eq!(code, "conflict_winner_required"),
        other => panic!("expected eligibility refusal, got {other:?}"),
    }

    // A second winner for the same set in the SAME request is refused
    // (exactly one explicit winner).
    let winner_plan = service
        .plan_report(&plan_request(
            &summary,
            left_ref.clone(),
            ACTION_CONFLICT_WINNER,
        ))
        .expect("winner plan");
    assert_eq!(winner_plan.items[0].action, ACTION_CONFLICT_WINNER);
    let both_request = AdoptReportPlanRequest {
        report_generation: summary.generation,
        selections: vec![
            AdoptReportSelection {
                entity_ref: entity_ref(&summary, member_seqs[0]),
                action: ACTION_CONFLICT_WINNER.into(),
            },
            AdoptReportSelection {
                entity_ref: entity_ref(&summary, member_seqs[1]),
                action: ACTION_CONFLICT_WINNER.into(),
            },
        ],
    };
    let err = service.plan_report(&both_request).unwrap_err();
    match err {
        AdoptError::Eligibility { code, .. } => assert_eq!(code, "duplicate_conflict_winner"),
        other => panic!("expected duplicate winner refusal, got {other:?}"),
    }

    // The explicit winner applies; the other member stays Untracked.
    let result = service
        .apply(&winner_plan.plan_token)
        .expect("winner apply");
    assert!(result.undo_available);
    let rows: Vec<(String, String)> = {
        let mut collected = Vec::new();
        home.with_sql("read winner", |connection| {
            let mut stmt = connection
                .prepare("SELECT directory_name, final_entity_path FROM skills WHERE directory_name IN ('networking', 'Networking')")
                .expect("prepare");
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .expect("query")
                .collect::<Result<Vec<_>, _>>()
                .expect("rows");
            rows.into_iter().for_each(|row| collected.push(row));
        });
        collected
    };
    assert_eq!(rows.len(), 1, "only the explicit winner is Managed");
    assert_eq!(rows[0].0, "networking");
    assert_eq!(rows[0].1, left.to_string_lossy());
    assert!(right.is_dir(), "the loser stays in place, Untracked");
}

#[test]
fn failed_item_is_isolated_from_successful_siblings() {
    let home = BoundTestHome::new();
    let (coordinator, agent) = setup_standard(&home);
    let alpha = home.path().join("Projects").join("alpha-skill");
    let beta = home.path().join("Projects").join("beta-skill");
    write_external_entity(&alpha, "Alpha");
    write_external_entity(&beta, "Beta");
    std::os::unix::fs::symlink(&alpha, agent.root.join("alpha-skill")).expect("alpha symlink");
    std::os::unix::fs::symlink(&beta, agent.root.join("beta-skill")).expect("beta symlink");

    let service = adopt_service(&home, coordinator.clone());
    let summary = scan_and_wait(&coordinator);
    let (rows, _) = page_section(
        &coordinator,
        &summary,
        ScanReportSection::LocalCandidates,
        64,
    );
    let mut alpha_seq = None;
    let mut beta_seq = None;
    for row in &rows {
        let ScanReportRow::SourceVerdict(verdict) = row else {
            continue;
        };
        if verdict.directory_names.first() == Some(&"alpha-skill".to_string()) {
            alpha_seq = Some(verdict.entity_seq);
        }
        if verdict.directory_names.first() == Some(&"beta-skill".to_string()) {
            beta_seq = Some(verdict.entity_seq);
        }
    }
    let alpha_seq = alpha_seq.expect("alpha candidate");
    let beta_seq = beta_seq.expect("beta candidate");
    let request = AdoptReportPlanRequest {
        report_generation: summary.generation,
        selections: vec![
            AdoptReportSelection {
                entity_ref: entity_ref(&summary, alpha_seq),
                action: ACTION_LOCAL_LINK.into(),
            },
            AdoptReportSelection {
                entity_ref: entity_ref(&summary, beta_seq),
                action: ACTION_LOCAL_LINK.into(),
            },
        ],
    };
    let plan = service.plan_report(&request).expect("plan");
    assert_eq!(plan.items.len(), 2);
    // A Catalog collision surfaces at apply time for beta only: a skill row
    // with the same directory identity appears between plan and apply (raw
    // SQL seeds the Catalog without touching the frozen Report).
    let identity_key = skill_man_lib::core::domain::skill_identity_key("beta-skill");
    home.with_sql("seed collision", |connection| {
        connection
            .execute(
                "INSERT INTO skills (
                    id, directory_name, directory_identity_key, display_name, description,
                    source_kind, library_entry_path, final_entity_path,
                    recorded_content_hash, health, created_at, updated_at
                 ) VALUES (
                    'collision-1', 'beta-skill', ?1, 'Beta', 'collision',
                    'link', NULL, ?2, NULL, 'healthy', '1970-01-01T00:00:00Z', '1970-01-01T00:00:00Z'
                 )",
                rusqlite::params![identity_key, beta.to_string_lossy()],
            )
            .expect("seed collision row");
    });
    let result = service.apply(&plan.plan_token).expect("apply");
    assert_eq!(result.items.len(), 2);
    let alpha_result = result
        .items
        .iter()
        .find(|item| item.directory_name == "alpha-skill")
        .expect("alpha result");
    let beta_result = result
        .items
        .iter()
        .find(|item| item.directory_name == "beta-skill")
        .expect("beta result");
    assert!(
        alpha_result.adopted,
        "alpha stays adopted: {alpha_result:?}"
    );
    assert!(!beta_result.adopted, "beta fails without harming alpha");
    let rows: Vec<(String, String)> = {
        let mut collected = Vec::new();
        home.with_sql("read siblings", |connection| {
            let mut stmt = connection
                .prepare("SELECT id, directory_name FROM skills WHERE directory_name IN ('alpha-skill', 'beta-skill')")
                .expect("prepare");
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .expect("query")
                .collect::<Result<Vec<_>, _>>()
                .expect("rows");
            rows.into_iter().for_each(|row| collected.push(row));
        });
        collected
    };
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .any(|(id, name)| name == "alpha-skill" && id != "collision-1"),
        "alpha was adopted by the plan: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|(id, name)| name == "beta-skill" && id == "collision-1"),
        "beta is the pre-seeded collision row, not an adopted duplicate: {rows:?}"
    );
}
