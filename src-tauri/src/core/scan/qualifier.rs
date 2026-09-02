//! Safety Snapshot deletion qualification (spec §2.1 invariant 6,
//! ADR-0020): a Snapshot may only be planned/deleted when the same
//! `home_id` had a subsequent successful startup and a manual Complete
//! Scan Report after the Restore. Startup Probe, onboarding Reports,
//! Stale/cache-carried Reports and Incomplete/Failed/Cancelled/Superseded
//! Runs never qualify.
//!
//! The evidence is unforgeable in the sense that only Core writes the
//! qualification record, every record binds `home_id + snapshot_ids +
//! startup marker identity + Report content identity`, carries an
//! integrity digest, and `plan`/`apply` both re-verify every fact — a
//! corrupt or lost artifact fails closed (unqualified).

use std::path::Path;

use crate::core::home::BoundHome;
use crate::seams::app_state_store::RecoveryOperationRecord;
use crate::seams::scan_evidence_store::{
    CurrentManifestRead, ScanEvidenceStore, ScanEvidenceStoreFactory,
};

/// Injectable verification seam: Fixture Recovery receives the production
/// impl and never interprets the Evidence Store itself.
pub trait SnapshotDeleteEvidence: Send + Sync {
    fn verify_snapshot_delete(
        &self,
        home: &BoundHome,
        snapshot_id: &str,
        restore_record: &RecoveryOperationRecord,
    ) -> Result<(), String>;
}

#[derive(Debug, thiserror::Error, Clone, Eq, PartialEq)]
pub enum QualificationError {
    #[error("the Snapshot is not qualified for deletion: no manual Complete Scan Report evidence")]
    NoQualification,
    #[error("the qualification evidence belongs to a different Home")]
    HomeMismatch,
    #[error("the Safety Snapshot is not listed in the qualification evidence")]
    SnapshotNotListed,
    #[error("the qualification evidence is missing the authoritative successful startup marker")]
    StartupMarkerMissing,
    #[error("the startup marker predates the Restore that created this Snapshot")]
    StartupPredatesRestore,
    #[error("the current Scan Report does not match the qualification evidence")]
    ReportMismatch,
    #[error("the Scan Evidence Store is unreadable: {0}")]
    StoreUnavailable(String),
}

pub struct SnapshotDeleteQualifier {
    factory: Arc<dyn ScanEvidenceStoreFactory>,
}

use std::sync::Arc;

impl SnapshotDeleteQualifier {
    pub fn new(factory: Arc<dyn ScanEvidenceStoreFactory>) -> Self {
        Self { factory }
    }

    fn verify_against(
        &self,
        store: &dyn ScanEvidenceStore,
        home: &BoundHome,
        snapshot_id: &str,
        restore_record: &RecoveryOperationRecord,
    ) -> Result<(), QualificationError> {
        let qualification = store
            .qualification()
            .map_err(|error| QualificationError::StoreUnavailable(error.to_string()))?
            .filter(|record| record.home_id == home.home_id.0)
            .ok_or(QualificationError::HomeMismatch)?;
        if qualification.home_id != home.home_id.0 {
            return Err(QualificationError::HomeMismatch);
        }
        if !qualification
            .snapshot_ids
            .iter()
            .any(|id| id == snapshot_id)
        {
            return Err(QualificationError::SnapshotNotListed);
        }
        // The authoritative startup mark must still exist for this Home and
        // be at least as recent as the recorded qualification fact
        // (monotonic forward: later startups never revoke).
        let marker = store
            .startup_marker()
            .map_err(|error| QualificationError::StoreUnavailable(error.to_string()))?
            .filter(|marker| marker.home_id == home.home_id.0)
            .ok_or(QualificationError::StartupMarkerMissing)?;
        let restore_ms =
            super::rfc3339_to_epoch_millis(&restore_record.created_at).unwrap_or(u64::MAX);
        if marker.marked_at_ms < restore_ms
            || marker.marked_at_ms < qualification.startup_marked_at_ms
        {
            return Err(QualificationError::StartupPredatesRestore);
        }
        // The current Report must still be the qualifying manual Complete
        // Report, exactly by content identity (never a Stale or partial one).
        let manifest = match store
            .current_manifest()
            .map_err(|error| QualificationError::StoreUnavailable(error.to_string()))?
        {
            CurrentManifestRead::Report(manifest) => manifest,
            CurrentManifestRead::Absent | CurrentManifestRead::Corrupt => {
                return Err(QualificationError::ReportMismatch);
            }
        };
        if manifest.run_id != qualification.report_run_id
            || manifest.generation != qualification.report_generation
            || manifest.content_identity != qualification.report_content_identity
            || manifest.state != "complete"
            || manifest.trigger != "manual"
            || manifest.home_id != home.home_id.0
        {
            return Err(QualificationError::ReportMismatch);
        }
        Ok(())
    }
}

impl SnapshotDeleteEvidence for SnapshotDeleteQualifier {
    fn verify_snapshot_delete(
        &self,
        home: &BoundHome,
        snapshot_id: &str,
        restore_record: &RecoveryOperationRecord,
    ) -> Result<(), String> {
        let store = self
            .factory
            .store_for(home)
            .map_err(|error| QualificationError::StoreUnavailable(error.to_string()).to_string())?;
        self.verify_against(&*store, home, snapshot_id, restore_record)
            .map_err(|error| error.to_string())
    }
}

/// Test composition: no evidence — every delete stays unqualified
/// (fail-closed default).
pub struct UnqualifiedSnapshotDeleteEvidence;

impl SnapshotDeleteEvidence for UnqualifiedSnapshotDeleteEvidence {
    fn verify_snapshot_delete(
        &self,
        _home: &BoundHome,
        _snapshot_id: &str,
        _restore_record: &RecoveryOperationRecord,
    ) -> Result<(), String> {
        Err(QualificationError::NoQualification.to_string())
    }
}

#[allow(dead_code)]
fn _snapshot_identity_check(_snapshot_path: &Path) {}

#[cfg(test)]
mod tests {
    use crate::seams::scan_evidence_store::ScanStartupMarker;

    #[test]
    fn rfc3339_ordering_round_trip() {
        // 2026-08-01T00:00:00Z = epoch seconds 1785542400? Verified against
        // the fixture recovery timestamp helper semantics.
        let millis = super::super::rfc3339_to_epoch_millis("2026-08-01T00:00:00Z").unwrap();
        assert_eq!(millis, 1_785_542_400_000);
        // Ordering: later timestamp parses to a larger value.
        let later = super::super::rfc3339_to_epoch_millis("2026-08-02T00:00:00Z").unwrap();
        assert!(later > millis);
        assert!(super::super::rfc3339_to_epoch_millis("not-a-date").is_none());
    }

    #[test]
    fn startup_marker_round_trip() {
        let marker = ScanStartupMarker {
            schema_version: crate::seams::scan_evidence_store::SCAN_STORE_SCHEMA_VERSION,
            home_id: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into(),
            marked_at_ms: 42,
        };
        let json = serde_json::to_string(&marker).unwrap();
        assert_eq!(ScanStartupMarker::parse(&json), Some(marker));
    }
}
