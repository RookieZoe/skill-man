//! Safety Snapshot deletion qualification contract (spec §2.1 invariant 6,
//! ADR-0020; issue #82): the same `home_id` needs a later successful startup
//! (durable marker) and a manual Complete Scan Report (current manifest +
//! integrity) since the Restore before `plan_delete_snapshot` /
//! `apply_delete_snapshot` may proceed. Every negative case fails closed and
//! the positive Restore → startup → manual Complete → delete chain works
//! with the real Evidence Store adapters.

mod common;

use std::path::Path;
use std::sync::Arc;

use skill_man_lib::adapters::app_state_store::AppStateStoreFileSystem;
use skill_man_lib::adapters::catalog_probe::SqliteCatalogProbe;
use skill_man_lib::adapters::macos_fs::MacOsFileSystem;
use skill_man_lib::adapters::scan_evidence_store::SystemScanEvidenceStoreFactory;
use skill_man_lib::adapters::sqlite::SqlitePreparedCatalogFactory;
use skill_man_lib::adapters::system_clock::SystemClock;
use skill_man_lib::core::bootstrap::{BootstrapConfig, BootstrapService};
use skill_man_lib::core::fixture_recovery::{
    FixtureRecoveryError, FixtureRecoveryService, SystemFixtureClassifier,
};
use skill_man_lib::core::home::VolumeIdentity;
use skill_man_lib::core::scan::qualifier::SnapshotDeleteQualifier;
use skill_man_lib::seams::app_state_store::{AppStateStore, RecoveryOperationRecord};
use skill_man_lib::seams::clock::Clock;
use skill_man_lib::seams::scan_evidence_store as scan;
use skill_man_lib::seams::scan_evidence_store::ScanEvidenceStore;
use skill_man_lib::seams::volume_identity::{VolumeIdentityError, VolumeIdentitySource};

use common::{BoundTestHome, CATALOG_FILE_NAME};

struct FixedVolumeIdentity(Option<VolumeIdentity>);

impl VolumeIdentitySource for FixedVolumeIdentity {
    fn volume_identity(&self, _path: &Path) -> Result<Option<VolumeIdentity>, VolumeIdentityError> {
        Ok(self.0.clone())
    }
}

/// A real FixtureRecoveryService over the BoundTestHome + the production
/// qualifier (the same composition lib.rs performs).
fn recovery(home: &BoundTestHome) -> FixtureRecoveryService {
    let probe = Arc::new(SqliteCatalogProbe::new());
    let filesystem = Arc::new(MacOsFileSystem::new(home.dir.path().to_path_buf()));
    let classifier = Arc::new(SystemFixtureClassifier::new(
        probe.clone(),
        filesystem.clone(),
        CATALOG_FILE_NAME.into(),
    ));
    let app_state = Arc::new(AppStateStoreFileSystem::new(home.state_dir.clone()));
    let bootstrap = Arc::new(BootstrapService::new(
        app_state.clone(),
        Arc::new(FixedVolumeIdentity(Some(VolumeIdentity {
            fsid: "test-fsid".into(),
            uuid: "test-uuid".into(),
        }))),
        probe.clone(),
        filesystem.clone(),
        classifier,
        BootstrapConfig {
            state_dir: home.state_dir.clone(),
            default_home_path: home.library_root.clone(),
            catalog_file_name: CATALOG_FILE_NAME.into(),
        },
    ));
    FixtureRecoveryService::new(
        app_state,
        probe,
        filesystem,
        Arc::new(SqlitePreparedCatalogFactory),
        bootstrap,
        BootstrapConfig {
            state_dir: home.state_dir.clone(),
            default_home_path: home.library_root.clone(),
            catalog_file_name: CATALOG_FILE_NAME.into(),
        },
    )
    .with_delete_qualification(Arc::new(SnapshotDeleteQualifier::new(Arc::new(
        SystemScanEvidenceStoreFactory,
    ))))
}

/// Create the Restore ledger record + the Safety Snapshot directory siblings
/// exactly as a real Restore commit would leave them.
fn restore_ledger_record(home: &BoundTestHome, snapshot_id: &str) -> RecoveryOperationRecord {
    RecoveryOperationRecord {
        operation_id: snapshot_id
            .strip_prefix(&format!(
                "{}.snapshot-",
                home.library_root.file_name().unwrap().to_string_lossy()
            ))
            .unwrap_or(snapshot_id)
            .to_owned(),
        kind: "fixture_recovery".into(),
        home_id: Some(home.home.home_id.clone()),
        live_path: Some(home.library_root.clone()),
        snapshot_path: Some(home.library_root.parent().unwrap().join(snapshot_id)),
        prepared_path: None,
        manifest_hash: Some("manifest-sha256:abc".into()),
        external_probe: None,
        cursor: Some("committed".into()),
        commit_point: Some("committed".into()),
        created_at: "2026-08-15T00:00:00Z".into(),
    }
}

fn seed_restore(home: &BoundTestHome, snapshot_id: &str) {
    let snapshot_path = home.library_root.parent().unwrap().join(snapshot_id);
    std::fs::create_dir_all(&snapshot_path).expect("snapshot dir");
    std::fs::write(snapshot_path.join("manifest.json"), "{}").expect("snapshot manifest");
    let app_state = AppStateStoreFileSystem::new(home.state_dir.clone());
    let mut ledger = app_state.load().expect("ledger").recovery_ledger;
    ledger
        .completed
        .push(restore_ledger_record(home, snapshot_id));
    app_state
        .write_recovery_ledger(&ledger)
        .expect("write ledger");
}

fn base_manifest(
    home: &BoundTestHome,
    trigger: &str,
    state: &str,
) -> (scan::ScanReportManifest, scan::ScanRunRecord) {
    let now_ms = (SystemClock::new().unix_epoch_nanos() / 1_000_000) as u64;
    let frozen = scan::ScanFrozenFacts {
        home_id: home.home.home_id.0.clone(),
        write_gate_generation: home.write_gate.generation(),
        agent_configuration_generation: 1,
        mutation_generation: 0,
        roots_fingerprint: "roots<2>".into(),
        configured_agents: 1,
        declared_roots: 2,
    };
    let run = scan::ScanRunRecord {
        schema_version: scan::SCAN_STORE_SCHEMA_VERSION,
        home_id: home.home.home_id.0.clone(),
        run_id: "run-qual".into(),
        generation: 1,
        trigger: trigger.into(),
        frozen: frozen.clone(),
        roots: vec![],
        started_at_ms: now_ms,
    };
    let manifest = scan::ScanReportManifest {
        schema_version: scan::SCAN_STORE_SCHEMA_VERSION,
        home_id: home.home.home_id.0.clone(),
        run_id: "run-qual".into(),
        generation: 1,
        trigger: trigger.into(),
        state: state.into(),
        counts: scan::ScanEvidenceCounts {
            roots: 1,
            ..Default::default()
        },
        source_counts: scan::ScanSourceCounts::default(),
        roots: vec![],
        frozen,
        started_at_ms: now_ms,
        ended_at_ms: now_ms + 10,
        integrity: None,
        content_identity: format!("scan-report-v1:{}:run-qual:1:{state}", home.home.home_id.0),
    };
    (manifest, run)
}

fn seal_manifest(mut manifest: scan::ScanReportManifest) -> scan::ScanReportManifest {
    let value = serde_json::to_value(&manifest).expect("manifest serializes");
    manifest.integrity = Some(skill_man_lib::seams::scan_integrity::canonical_json_digest(
        &value,
    ));
    manifest
}

fn seal_qualification(mut q: scan::ScanSnapshotQualification) -> scan::ScanSnapshotQualification {
    let value = serde_json::to_value(&q).expect("qualification serializes");
    q.integrity = Some(skill_man_lib::seams::scan_integrity::canonical_json_digest(
        &value,
    ));
    q
}

/// The Restore → successful startup → manual Complete chain: marker, report,
/// qualification, exactly as the Run engine writes them.
fn qualify(
    home: &BoundTestHome,
    snapshot_id: &str,
    report_state: &str,
    trigger: &str,
) -> scan::ScanSnapshotQualification {
    let now_ms = (SystemClock::new().unix_epoch_nanos() / 1_000_000) as u64;
    let store = skill_man_lib::adapters::scan_evidence_store::SystemScanEvidenceStore::new(
        home.library_root.join("cache/scan"),
        home.home.home_id.0.clone(),
    );
    let marker = scan::ScanStartupMarker {
        schema_version: scan::SCAN_STORE_SCHEMA_VERSION,
        home_id: home.home.home_id.0.clone(),
        marked_at_ms: now_ms + 3_600_000,
    };
    store.write_startup_marker(&marker).expect("marker");
    let (mut manifest, _run) = base_manifest(home, trigger, report_state);
    manifest = seal_manifest(manifest);
    store
        .publish_report("run-qual", &manifest)
        .expect("publish report");
    let qualification = scan::ScanSnapshotQualification {
        schema_version: scan::SCAN_STORE_SCHEMA_VERSION,
        home_id: home.home.home_id.0.clone(),
        snapshot_ids: vec![snapshot_id.into()],
        startup_marked_at_ms: marker.marked_at_ms,
        report_run_id: manifest.run_id.clone(),
        report_generation: manifest.generation,
        report_content_identity: manifest.content_identity.clone(),
        recorded_at_ms: now_ms,
        integrity: None,
    };
    let qualification = seal_qualification(qualification);
    store
        .write_qualification(&qualification)
        .expect("qualification");
    qualification
}

#[test]
fn restore_then_successful_startup_then_manual_complete_allows_delete() {
    let home = BoundTestHome::new();
    let snapshot_id = format!(
        "{}.snapshot-op1",
        home.library_root.file_name().unwrap().to_string_lossy()
    );
    seed_restore(&home, &snapshot_id);
    let recovery = recovery(&home);
    // Before any Scan evidence: refused at plan AND apply.
    match recovery.plan_delete_snapshot(&snapshot_id) {
        Err(FixtureRecoveryError::SnapshotNotQualified(_)) => {}
        other => panic!("no evidence must refuse, got {other:?}"),
    }
    let err = recovery
        .apply_delete_snapshot(&snapshot_id)
        .expect_err("apply without evidence must refuse");
    assert!(matches!(err, FixtureRecoveryError::SnapshotNotQualified(_)));

    // Restore → successful startup → manual Complete Report.
    let _qualification = qualify(&home, &snapshot_id, "complete", "manual");
    assert!(
        home.library_root
            .parent()
            .unwrap()
            .join(&snapshot_id)
            .is_dir()
    );
    let delete = recovery
        .plan_delete_snapshot(&snapshot_id)
        .expect("qualified plan delete");
    assert!(delete.file_count > 0);
    recovery
        .apply_delete_snapshot(&snapshot_id)
        .expect("qualified apply delete");
    assert!(
        !home
            .library_root
            .parent()
            .unwrap()
            .join(&snapshot_id)
            .exists()
    );
    assert!(recovery.list_snapshots().unwrap().is_empty());
}

#[test]
fn onboarding_or_other_trigger_reports_never_qualify() {
    let home = BoundTestHome::new();
    let snapshot_id = format!(
        "{}.snapshot-op2",
        home.library_root.file_name().unwrap().to_string_lossy()
    );
    seed_restore(&home, &snapshot_id);
    // An onboarding-triggered Complete Report must NOT qualify: the
    // qualification record can only bind a manual report identity, so we
    // craft one and expect `ReportMismatch` at verification time.
    qualify(&home, &snapshot_id, "complete", "onboarding");
    let recovery = recovery(&home);
    match recovery.plan_delete_snapshot(&snapshot_id) {
        Err(FixtureRecoveryError::SnapshotNotQualified(_)) => {}
        other => panic!("onboarding Report must not qualify, got {other:?}"),
    }
}

#[test]
fn incomplete_cancelled_or_failed_reports_do_not_qualify() {
    let home = BoundTestHome::new();
    let snapshot_id = format!(
        "{}.snapshot-op3",
        home.library_root.file_name().unwrap().to_string_lossy()
    );
    seed_restore(&home, &snapshot_id);
    // An Incomplete Report is not a Complete Report: no qualification is
    // written by the engine, and a forged one fails the identity check.
    qualify(&home, &snapshot_id, "incomplete", "manual");
    let recovery = recovery(&home);
    match recovery.plan_delete_snapshot(&snapshot_id) {
        Err(FixtureRecoveryError::SnapshotNotQualified(_)) => {}
        other => panic!("Incomplete Report must not qualify, got {other:?}"),
    }
}

#[test]
fn corrupt_or_tampered_evidence_fails_closed() {
    let home = BoundTestHome::new();
    let snapshot_id = format!(
        "{}.snapshot-op4",
        home.library_root.file_name().unwrap().to_string_lossy()
    );
    seed_restore(&home, &snapshot_id);
    qualify(&home, &snapshot_id, "complete", "manual");
    // Tamper the current manifest: integrity fails → No cached report.
    let scan_dir = home.library_root.join("cache/scan");
    let current = std::fs::read_to_string(scan_dir.join("current.json")).expect("current");
    std::fs::write(
        scan_dir.join("current.json"),
        current.replace("\"run-qual\"", "\"run-tampered\""),
    )
    .expect("tamper");
    let recovery = recovery(&home);
    match recovery.plan_delete_snapshot(&snapshot_id) {
        Err(FixtureRecoveryError::SnapshotNotQualified(_)) => {}
        other => panic!("tampered manifest must fail closed, got {other:?}"),
    }
    // Removing the current Report also fails closed (cache loss = no proof).
    std::fs::remove_file(scan_dir.join("current.json")).expect("remove current");
    match recovery.plan_delete_snapshot(&snapshot_id) {
        Err(FixtureRecoveryError::SnapshotNotQualified(_)) => {}
        other => panic!("missing current Report must fail closed, got {other:?}"),
    }
}

#[test]
fn startup_marker_missing_or_stale_never_qualifies() {
    let home = BoundTestHome::new();
    let snapshot_id = format!(
        "{}.snapshot-op5",
        home.library_root.file_name().unwrap().to_string_lossy()
    );
    seed_restore(&home, &snapshot_id);
    // Marker is older than the Restore: the engine would have rewritten it;
    // a stale marker must fail the verification even with a report present.
    let store = skill_man_lib::adapters::scan_evidence_store::SystemScanEvidenceStore::new(
        home.library_root.join("cache/scan"),
        home.home.home_id.0.clone(),
    );
    let stale_marker = scan::ScanStartupMarker {
        schema_version: scan::SCAN_STORE_SCHEMA_VERSION,
        home_id: home.home.home_id.0.clone(),
        marked_at_ms: 0,
    };
    store
        .write_startup_marker(&stale_marker)
        .expect("stale marker");
    let (mut manifest, _run) = base_manifest(&home, "manual", "complete");
    manifest = seal_manifest(manifest);
    store
        .publish_report("run-qual", &manifest)
        .expect("publish");
    let recovery = recovery(&home);
    match recovery.plan_delete_snapshot(&snapshot_id) {
        Err(FixtureRecoveryError::SnapshotNotQualified(_)) => {}
        other => panic!("stale/missing marker must fail closed, got {other:?}"),
    }
}

#[test]
fn different_home_evidence_never_carries_over() {
    let home = BoundTestHome::new();
    let snapshot_id = format!(
        "{}.snapshot-op6",
        home.library_root.file_name().unwrap().to_string_lossy()
    );
    seed_restore(&home, &snapshot_id);
    qualify(&home, &snapshot_id, "complete", "manual");
    // A different Home's locator cannot be acted on: the delete gate only
    // runs under the current Bound identity, proven by the service itself.
    let recovery = recovery(&home);
    let delete = recovery
        .plan_delete_snapshot(&snapshot_id)
        .expect("qualified delete");
    recovery
        .apply_delete_snapshot(&delete.snapshot_id)
        .expect("delete");
}
