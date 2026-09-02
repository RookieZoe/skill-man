//! Scan Evidence Store seam (spec §3.6, ADR-0020): the streamed, disk-backed
//! evidence of a Scan Run under `<Home>/cache/scan/`. The store is a derived
//! cache — it never closes the Catalog, never changes the WriteGate and never
//! writes a single Catalog row.
//!
//! Layout (all under the Home `cache/scan/` directory):
//!
//! ```text
//! current.json                 # tiny terminal manifest; the atomic switch point
//! startup.json                 # "successful startup" marker for Snapshot qualification
//! qualification.json           # Snapshot deletion qualification evidence
//! runs/<run_id>/
//!   run.json                   # Run provenance: home_id, run_id, trigger, frozen facts
//!   roots/<root_key>/
//!     root.json                # terminal Root record (atomic)
//!     entries.jsonl            # streamed entry evidence
//!     entities.jsonl           # streamed entity evidence
//!   manifest.json              # terminal Run manifest (atomic write before current.json)
//! ```
//!
//! Every artifact is written with `home_id` + `run_id` provenance. Only a
//! manifest-atomic switch (`current.json`) publishes a Report; a crash or a
//! cancelled/superseded Run leaves at most a temporary `runs/<run_id>`
//! directory that the startup sweep removes only when `run.json` proves the
//! artifact belongs to the current `home_id` (spec §3.6 orphan rule). A
//! corrupt or unreadable current manifest is `Ok(None)` — `No cached report`
//! — never a Catalog or WriteGate change.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::core::home::BoundHome;

pub const SCAN_STORE_SCHEMA_VERSION: u32 = 1;
pub const SCAN_ARTIFACT_VERSION_PREFIX: &str = "scan-artifact-v1";

/// Frozen generation facts a Run binds to (spec §4.10): any change after the
/// Run starts makes it Superseded.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanFrozenFacts {
    pub home_id: String,
    pub write_gate_generation: u64,
    pub agent_configuration_generation: u64,
    pub mutation_generation: u64,
    /// Identity fingerprint of the configured canonical Root snapshot; a
    /// Root set/identity change makes the Run stale.
    pub roots_fingerprint: String,
}

/// One configured Root frozen into a Run: canonical path resolved once, with
/// the directory identity that must not change while the Run walks it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanFrozenRoot {
    pub index: u32,
    pub configured_path: PathBuf,
    pub canonical_path: PathBuf,
    pub device: u64,
    pub inode: u64,
}

/// Every counter a Run carries; `roots`/`failed_roots` are Run-level, the
/// others are also per-Root.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanEvidenceCounts {
    pub roots: u64,
    pub entries: u64,
    pub entities: u64,
    pub files: u64,
    pub bytes: u64,
    pub git_probes: u64,
    pub failed_roots: u64,
}

/// `run.json`: one-time provenance record proving the Run artifact belongs
/// to `(home_id, run_id, generation)`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanRunRecord {
    pub schema_version: u32,
    pub home_id: String,
    pub run_id: String,
    pub generation: u64,
    /// `onboarding` | `manual`; closed by the caller, persisted verbatim.
    pub trigger: String,
    pub frozen: ScanFrozenFacts,
    pub roots: Vec<ScanFrozenRoot>,
    pub started_at_ms: u64,
}

impl ScanRunRecord {
    pub fn provenance(&self) -> String {
        format!(
            "{SCAN_ARTIFACT_VERSION_PREFIX}:{}:{}",
            self.home_id, self.run_id
        )
    }
    pub fn parse(json: &str) -> Option<Self> {
        let record: Self = serde_json::from_str(json).ok()?;
        if record.schema_version != SCAN_STORE_SCHEMA_VERSION {
            return None;
        }
        if record.home_id.is_empty()
            || record.run_id.is_empty()
            || !matches!(record.trigger.as_str(), "onboarding" | "manual")
        {
            return None;
        }
        Some(record)
    }
}

/// Permanently-closed reason a Root did not produce evidence: a failed
/// transaction or the typed 30-second `Unresponsive` watchdog isolation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ScanRootState {
    Completed,
    Failed,
    Unresponsive,
}

/// Terminal `root.json` under the temporary Run (a Root is the minimum
/// evidence commit unit, spec §4.10): written only when the Root fully
/// finished or concretely failed — never a partial candidate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanRootRecord {
    pub schema_version: u32,
    pub home_id: String,
    pub run_id: String,
    pub index: u32,
    pub configured_path: PathBuf,
    pub canonical_path: PathBuf,
    pub state: ScanRootState,
    pub counts: ScanEvidenceCounts,
    pub elapsed_ms: u64,
    pub slow: bool,
    pub diagnostic: Option<String>,
    pub completed_at_ms: u64,
}

impl ScanRootRecord {
    pub fn parse(json: &str) -> Option<Self> {
        let record: Self = serde_json::from_str(json).ok()?;
        if record.schema_version != SCAN_STORE_SCHEMA_VERSION {
            return None;
        }
        if record.home_id.is_empty() || record.run_id.is_empty() {
            return None;
        }
        Some(record)
    }
}

/// Streamed entry evidence (one JSONL line): the appearance with its full
/// bounded chain, plus the memoized lock/worktree hints of this Run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanEntryRecord {
    pub seq: u64,
    pub name: String,
    pub entry_path: PathBuf,
    /// `directory` | `symlink` — the entry kind as listed.
    pub entry_kind: String,
    pub chain: Vec<ScanChainHopRecord>,
    pub chain_fault: Option<ScanChainFaultRecord>,
    pub final_entity: Option<PathBuf>,
    pub lock_hint: Option<ScanLockHintRecord>,
    pub worktree_hint: Option<ScanWorktreeHintRecord>,
}

/// One bounded chain hop (spec §8.1 evidence chain); `symlink` hops carry
/// the raw target text as stored on disk.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanChainHopRecord {
    pub path: PathBuf,
    pub kind: String,
    pub device: u64,
    pub inode: u64,
    pub target: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanChainFaultRecord {
    pub kind: String,
    pub at: PathBuf,
    pub detail: Option<String>,
}

/// Memoized lock fact for one entry: the governing lock file, declaring
/// entry name and fingerprint. Never the lock body.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanLockHintRecord {
    pub lock_path: PathBuf,
    pub entry_name: String,
    pub fingerprint: String,
    pub faulted: bool,
    pub fault: Option<String>,
}

/// Memoized bounded local Git worktree hint (never network): nearest
/// repository root inside the originating Root, gitdir shape and remote
/// URLs, or a typed `uninterpretable` reason.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanWorktreeHintRecord {
    pub repository_root: PathBuf,
    /// `dir` | `file` gitdir or `uninterpretable`.
    pub gitdir_kind: String,
    pub remote_urls: Vec<String>,
    pub head_ref: Option<String>,
}

/// Streamed entity evidence (one JSONL line): the resolved final entity with
/// its full tree facts (no per-entity byte or size cap).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanEntityRecord {
    pub seq: u64,
    pub name: String,
    pub entry_path: PathBuf,
    pub final_entity: PathBuf,
    pub file_count: u64,
    pub byte_count: u64,
    pub tree_hash: Option<String>,
    pub hash_fault: Option<String>,
    pub elapsed_ms: u64,
}

/// One Root line of the coverage table in the terminal manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanRootCoverageRecord {
    pub index: u32,
    pub configured_path: PathBuf,
    pub canonical_path: PathBuf,
    pub state: ScanRootState,
    pub counts: ScanEvidenceCounts,
    pub elapsed_ms: u64,
    pub slow: bool,
    pub diagnostic: Option<String>,
}

/// Terminal Report manifest: `manifest.json` (identity-bound) plus the tiny
/// `current.json` pointer that atomically publishes it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanReportManifest {
    pub schema_version: u32,
    pub home_id: String,
    pub run_id: String,
    pub generation: u64,
    /// `onboarding` | `manual`.
    pub trigger: String,
    /// `complete` | `incomplete`.
    pub state: String,
    pub counts: ScanEvidenceCounts,
    pub roots: Vec<ScanRootCoverageRecord>,
    pub frozen: ScanFrozenFacts,
    pub started_at_ms: u64,
    pub ended_at_ms: u64,
    /// Integrity digest over the canonical payload (integrity field
    /// excluded): a tampered or torn manifest is `No cached report`.
    pub integrity: Option<String>,
    /// `scan-report-v1:<home_id>:<run_id>:<generation>:<state>`; the
    /// unforgeable Report identity plan tokens and Snapshot qualification
    /// bind to.
    pub content_identity: String,
}

impl ScanReportManifest {
    pub fn parse(json: &str) -> Option<Self> {
        let manifest: Self = serde_json::from_str(json).ok()?;
        if manifest.schema_version != SCAN_STORE_SCHEMA_VERSION {
            return None;
        }
        if manifest.home_id.is_empty()
            || manifest.run_id.is_empty()
            || !matches!(manifest.trigger.as_str(), "onboarding" | "manual")
            || !matches!(manifest.state.as_str(), "complete" | "incomplete")
        {
            return None;
        }
        if manifest.content_identity
            != format!(
                "scan-report-v1:{}:{}:{}:{}",
                manifest.home_id, manifest.run_id, manifest.generation, manifest.state
            )
        {
            return None;
        }
        if !manifest.verify_integrity() {
            return None;
        }
        Some(manifest)
    }

    pub fn verify_integrity(&self) -> bool {
        let Some(expected) = &self.integrity else {
            return false;
        };
        let mut payload = self.clone();
        payload.integrity = None;
        let Ok(value) = serde_json::to_value(&payload) else {
            return false;
        };
        crate::seams::scan_integrity::canonical_json_digest(&value) == *expected
    }
}

/// Read outcome of the current terminal manifest: a live Report, a genuine
/// never-scanned absence, or a corrupt/torn/tampered artifact (spec §3.6:
/// `No cached report` — never an error that changes Bootstrap or WriteGate
/// behavior).
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum CurrentManifestRead {
    Report(ScanReportManifest),
    Absent,
    Corrupt,
}

/// Durable "successful startup" mark (Snapshot deletion qualification): the
/// Bound WriteGate was reached for this `home_id` after the restore.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanStartupMarker {
    pub schema_version: u32,
    pub home_id: String,
    pub marked_at_ms: u64,
}

impl ScanStartupMarker {
    pub fn parse(json: &str) -> Option<Self> {
        let marker: Self = serde_json::from_str(json).ok()?;
        if marker.schema_version != SCAN_STORE_SCHEMA_VERSION || marker.home_id.is_empty() {
            return None;
        }
        Some(marker)
    }
}

/// Snapshot deletion qualification evidence (spec §2.1 invariant 6,
/// ADR-0020): the unattachable recorded facts that a Safety Snapshot of the
/// same `home_id` may be planned/delete. Only Core writes it; only an exact
/// identity match plus a verified current-manifest identity can qualify.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanSnapshotQualification {
    pub schema_version: u32,
    pub home_id: String,
    /// Safety Snapshot directory names recorded by the restore operations of
    /// this Home; only these are eligible.
    pub snapshot_ids: Vec<String>,
    /// The startup marker identity this qualification builds on.
    pub startup_marked_at_ms: u64,
    /// The qualifying Report identity: `run_id`, `generation` and content
    /// identity of the terminal manifest that was current when the
    /// qualification was written.
    pub report_run_id: String,
    pub report_generation: u64,
    pub report_content_identity: String,
    pub recorded_at_ms: u64,
    pub integrity: Option<String>,
}

impl ScanSnapshotQualification {
    pub fn parse(json: &str) -> Option<Self> {
        let record: Self = serde_json::from_str(json).ok()?;
        if record.schema_version != SCAN_STORE_SCHEMA_VERSION || record.home_id.is_empty() {
            return None;
        }
        if record.report_content_identity
            != format!(
                "scan-report-v1:{}:{}:{}:{}",
                record.home_id, record.report_run_id, record.report_generation, "complete"
            )
        {
            return None;
        }
        if !record.verify_integrity() {
            return None;
        }
        Some(record)
    }

    pub fn verify_integrity(&self) -> bool {
        let Some(expected) = &self.integrity else {
            return false;
        };
        let mut payload = self.clone();
        payload.integrity = None;
        let Ok(value) = serde_json::to_value(&payload) else {
            return false;
        };
        crate::seams::scan_integrity::canonical_json_digest(&value) == *expected
    }
}

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum ScanEvidenceStoreError {
    #[error("the Scan Evidence Store write failed: {operation}: {detail}")]
    Write {
        operation: &'static str,
        detail: String,
    },
    #[error("the Scan Evidence Store is unavailable: {detail}")]
    Unavailable { detail: String },
    #[error("the Run artifact provenance does not match this Home: {detail}")]
    ProvenanceMismatch { detail: String },
}

impl ScanEvidenceStoreError {
    pub fn io(operation: &'static str, path: &std::path::Path, source: &std::io::Error) -> Self {
        Self::Write {
            operation,
            detail: format!("{}: {source}", path.display()),
        }
    }
}

/// Seam: the atomic, provenance-checked Evidence Store. Operations are
/// either identity-checked writes or identity-checked removals; nothing
/// outside `scan_dir` is ever touched.
pub trait ScanEvidenceStore: Send + Sync {
    fn create_run(&self, run: &ScanRunRecord) -> Result<(), ScanEvidenceStoreError>;

    fn append_entry(
        &self,
        run_id: &str,
        root_key: &str,
        record: &ScanEntryRecord,
    ) -> Result<(), ScanEvidenceStoreError>;

    fn append_entity(
        &self,
        run_id: &str,
        root_key: &str,
        record: &ScanEntityRecord,
    ) -> Result<(), ScanEvidenceStoreError>;

    /// Terminal per-Root record: `root.json` written atomically; never
    /// called with a partial Root.
    fn write_root(&self, run_id: &str, root: &ScanRootRecord)
    -> Result<(), ScanEvidenceStoreError>;

    /// Read the provenance record of a temporary Run (identity check before
    /// any removal).
    fn read_run(&self, run_id: &str) -> Result<Option<ScanRunRecord>, ScanEvidenceStoreError>;

    /// Remove one temporary Run artifact, keeping `current.json` untouched.
    /// Only the Run whose `run.json` matches `(home_id, run_id)` is removed;
    /// anything unproven is left behind (`ProvenanceMismatch`).
    fn remove_run(&self, run_id: &str) -> Result<(), ScanEvidenceStoreError>;

    /// Startup sweep: remove every temporary Run not referenced by the
    /// current manifest, verifying `home_id + run_id + artifact identity`
    /// per directory. Corrupt current manifest counts as absent (spec §3.6:
    /// `No cached report`), so all proven temporary Runs are swept.
    fn cleanup_temporary_runs(&self) -> Result<usize, ScanEvidenceStoreError>;

    /// The current terminal Report manifest; corrupt/missing/invalid
    /// identity is `Corrupt`/`Absent` — never an error that changes Bootstrap
    /// or WriteGate behavior.
    fn current_manifest(&self) -> Result<CurrentManifestRead, ScanEvidenceStoreError>;

    /// Atomic publish: write `runs/<run_id>/manifest.json`, then switch
    /// `current.json` (tmp → fsync → rename → parent fsync). After the
    /// switch the previously current Run artifact is deleted; a crash before
    /// the switch leaves the old current Report untouched.
    fn publish_report(
        &self,
        run_id: &str,
        manifest: &ScanReportManifest,
    ) -> Result<(), ScanEvidenceStoreError>;

    fn write_startup_marker(
        &self,
        marker: &ScanStartupMarker,
    ) -> Result<(), ScanEvidenceStoreError>;

    fn startup_marker(&self) -> Result<Option<ScanStartupMarker>, ScanEvidenceStoreError>;

    fn write_qualification(
        &self,
        qualification: &ScanSnapshotQualification,
    ) -> Result<(), ScanEvidenceStoreError>;

    fn qualification(&self) -> Result<Option<ScanSnapshotQualification>, ScanEvidenceStoreError>;
}

/// Home-scoped store construction: the Run needs a `BoundHome`; the store
/// binds the Home path once, so a Run can never write into a different
/// Home than the one its frozen identity says.
pub trait ScanEvidenceStoreFactory: Send + Sync {
    fn store_for(
        &self,
        home: &BoundHome,
    ) -> Result<Arc<dyn ScanEvidenceStore>, ScanEvidenceStoreError>;
}

use std::sync::Arc;

/// Stable fault-injection point names (spec §11): implementations pair each
/// name with a single, exercised failure mode; tests document the matrix.
pub mod fault_points {
    pub const WRITE_ENTRY: &str = "scan.evidence_store.write_entry";
    pub const WRITE_ROOT: &str = "scan.evidence_store.write_root";
    pub const PUBLISH_MANIFEST: &str = "scan.evidence_store.publish_manifest";
    pub const ORPHAN_CLEANUP: &str = "scan.evidence_store.orphan_cleanup";
    pub const DISK_FULL: &str = "scan.evidence_store.disk_full";
    pub const QUALIFICATION_WRITE: &str = "scan.qualification.write";
    pub const RUN_CANCEL: &str = "scan.run.cancel_cleanup";
    pub const RUN_SUPERSEDE: &str = "scan.run.supersede_cleanup";
    pub const ROOT_WATCHDOG: &str = "scan.root.watchdog";
}

/// Matrix registry used by tests and documentation: every `(name, failure
/// mode, observed contract)` row.
pub const FAULT_POINT_MATRIX: &[(&str, &str)] = &[
    (
        fault_points::WRITE_ENTRY,
        "streamed entry write fails → Run Failed, old Report kept",
    ),
    (
        fault_points::WRITE_ROOT,
        "Root terminal record write fails → Root failed, Incomplete Report",
    ),
    (
        fault_points::PUBLISH_MANIFEST,
        "manifest switch fails → Run Failed, old current Report untouched",
    ),
    (
        fault_points::ORPHAN_CLEANUP,
        "orphan sweep fails → leftover temporary Run stays, unqualified reads unchanged",
    ),
    (
        fault_points::DISK_FULL,
        "store-wide write refuses (ENOSPC) → whole Run Failed, old Report kept",
    ),
    (
        fault_points::QUALIFICATION_WRITE,
        "qualification write fails → Snapshot stays unqualified",
    ),
    (
        fault_points::RUN_CANCEL,
        "cancel cleanup fails → Report untouched, temporary Run removed best-effort",
    ),
    (
        fault_points::RUN_SUPERSEDE,
        "supersede cleanup fails → Report untouched, temporary Run removed best-effort",
    ),
    (
        fault_points::ROOT_WATCHDOG,
        "30s zero-progress Root → typed Unresponsive + isolated",
    ),
];
