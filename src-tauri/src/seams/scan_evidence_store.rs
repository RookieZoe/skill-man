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

/// v3: the Report carries source classification (§8.2/§8.1, ADR-0017) —
/// Local candidates, aggregated Git Repository Source hints, Local↔Local
/// Conflict Sets, typed attention items, Excluded/already-Managed rows and
/// per-candidate typed operation eligibility, plus the consumer Agents of
/// every Root coverage row. A v2 artifact fails closed as `No cached
/// report`.
pub const SCAN_STORE_SCHEMA_VERSION: u32 = 3;
pub const SCAN_ARTIFACT_VERSION_PREFIX: &str = "scan-artifact-v1";

/// Generation-bound file-system object identity (vol/device + inode,
/// ADR-0017): valid only inside one Scan Report generation — never a
/// persistent Skill identity and never written to the Catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanObjectIdentity {
    pub device: u64,
    pub inode: u64,
}

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
    /// Funnel facts frozen at Run start (spec §4.6/ADR-0017): the number of
    /// configured Agent Configurations that declare roots.
    pub configured_agents: u64,
    /// The number of configured Root declarations (before the canonical
    /// union deduplicates them).
    pub declared_roots: u64,
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
    /// Configured Agent Configurations consuming this Root (coverage rows
    /// list them; the id is the durable pointer, the name is Source
    /// Content).
    pub consumer_agents: Vec<ScanRootAgentRef>,
}

/// One Root consumer Agent (spec §7.6 coverage table).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanRootAgentRef {
    pub agent_id: String,
    pub agent_name: String,
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
    /// Configured Agent Configurations contributing roots (funnel).
    pub configured_agents: u64,
    /// Configured Root declarations before the canonical union (funnel).
    pub declared_roots: u64,
    /// Distinct physical Roots after the canonical union (funnel).
    pub canonical_roots: u64,
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
    /// Generation-bound object identity of the resolved final entity;
    /// `None` when the chain faulted or the identity could not be read.
    pub identity: Option<ScanObjectIdentity>,
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
    /// The declaring strict-parse entry facts (never the lock body):
    /// `source_type`, `source_url`, `requested_ref` and `skill_path`. A
    /// faulted or structurally-faulted hint carries `None`.
    pub source_type: Option<String>,
    pub source_url: Option<String>,
    pub requested_ref: Option<String>,
    pub skill_path: Option<String>,
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
    /// The final object identity re-verified after the tree hash: `None`
    /// when the identity was replaced/unreadable during hashing (ADR-0017
    /// identity replacement — the tree facts are discarded, never a partial
    /// fingerprint). Aggregate key; generation-bound only.
    pub identity: Option<ScanObjectIdentity>,
    pub file_count: u64,
    pub byte_count: u64,
    pub tree_hash: Option<String>,
    pub hash_fault: Option<String>,
    pub elapsed_ms: u64,
}

/// One canonical Skill Entity of a Report generation (ADR-0017): all
/// appearances that resolved to the same file-system object identity are
/// aggregated into exactly one record. The object identity is valid only
/// inside this generation — never a persistent Skill identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanCanonicalEntityRecord {
    /// Stable per-Report canonical entity sequence (1-based).
    pub entity_seq: u64,
    pub identity: ScanObjectIdentity,
    /// The representative resolved final entity path (first appearance).
    pub canonical_path: PathBuf,
    pub file_count: u64,
    pub byte_count: u64,
    pub tree_hash: Option<String>,
    pub hash_fault: Option<String>,
    /// Exact number of appearances aggregated into this entity.
    pub appearances: u64,
    /// The Root that first revealed this entity (healthy Roots only).
    pub first_root_index: u32,
    /// The walk sequence of the first appearance within that Root.
    pub first_entry_seq: u64,
}

/// One aggregated appearance of a healthy Root (ADR-0017 §canonical entity):
/// the directory entry with its full bounded chain, memoized hints and the
/// canonical entity it aggregates into.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanAppearanceRecord {
    pub root_index: u32,
    pub seq: u64,
    pub name: String,
    pub entry_path: PathBuf,
    pub entry_kind: String,
    pub chain: Vec<ScanChainHopRecord>,
    pub chain_fault: Option<ScanChainFaultRecord>,
    pub final_entity: Option<PathBuf>,
    pub identity: Option<ScanObjectIdentity>,
    /// Canonical entity this appearance aggregates into; `None` when the
    /// chain faulted or the tree facts could not be read (never a guess).
    pub entity_seq: Option<u64>,
    pub lock_hint: Option<ScanLockHintRecord>,
    pub worktree_hint: Option<ScanWorktreeHintRecord>,
}

/// One typed diagnostic row of a Report (root failure, chain fault, hash
/// fault or identity fault). Presentation owns the copy; the kind is closed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanDiagnosticRecord {
    pub root_index: u32,
    /// `root_failed | root_unresponsive | root_record_missing |
    /// chain_dangling | chain_cycle | chain_hop_limit | chain_non_utf8 |
    /// chain_read_failed | chain_not_directory | chain_identity_replaced |
    /// entity_hash_fault | entity_identity_fault`.
    pub kind: String,
    pub at: Option<PathBuf>,
    pub detail: Option<String>,
}

/// Aggregation statistics of a Run's canonical entity index build.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanEntityIndexStats {
    pub entities: u64,
    pub appearances: u64,
    pub diagnostics: u64,
}

/// Report page sections the unique paged contract serves (spec §4.10
/// `report_page`; ADR-0017): Root coverage, canonical entities,
/// appearances and diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanReportSection {
    Roots,
    Entities,
    Appearances,
    Diagnostics,
    GitSources,
    LocalCandidates,
    ConflictSets,
    NeedsAttention,
    Excluded,
}

/// Stable, generation-bound cursor (spec §4.10/ADR-0020): a page read is
/// accepted only while `report_content_identity` is still the current
/// Report identity; any manifest switch, corrupt manifest or missing
/// artifact returns typed stale/not-found and never falls back to another
/// Report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReportCursor {
    pub report_content_identity: String,
    pub run_id: String,
    pub generation: u64,
    pub section: ScanReportSection,
    /// Row index for `Roots` (manifest-backed); byte offset into the
    /// immutable index stream for the other sections.
    pub offset: u64,
}

/// One page row; the section determines the variant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScanReportRow {
    RootCoverage(ScanRootCoverageRecord),
    Entity(ScanCanonicalEntityRecord),
    Appearance(Box<ScanAppearanceRecord>),
    Diagnostic(ScanDiagnosticRecord),
    GitSourceGroup(Box<ScanGitSourceGroupRecord>),
    SourceVerdict(Box<ScanSourceVerdictRecord>),
    ConflictSet(Box<ScanConflictSetRecord>),
}

/// One enriched applicable lock claim: the governing lock file, the exact
/// declaring entry facts and the entry's fingerprint. Claims are the
/// External Ownership Claim evidence of §8.1 — never the lock body.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanLockClaimRecord {
    pub lock_path: PathBuf,
    pub entry_name: String,
    pub fingerprint: String,
    pub source_type: Option<String>,
    pub source_url: Option<String>,
    pub requested_ref: Option<String>,
    pub skill_path: Option<String>,
}

/// Typed operation eligibility of one candidate (spec §8.1): the operation
/// name is closed (`local_link | local_link_with_move |
/// git_fetch_and_manage | conflict_winner`); destructive operations are
/// allowed only with complete coverage, and the Core closed reason
/// (`scan_incomplete`) is what the presentation disables on — never UI copy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanOperationEligibility {
    pub operation: String,
    pub allowed: bool,
    pub closed_reason: Option<String>,
}

/// One classified canonical entity of a Report generation (spec §8.1/§8.2).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanSourceVerdictRecord {
    pub entity_seq: u64,
    /// `local | git | conflict_set | identity_conflict | blocked |
    /// deferred | already_managed | excluded`.
    pub verdict: String,
    pub canonical_path: PathBuf,
    /// Directory Identity spellings of the appearances (Source Content).
    pub directory_names: Vec<String>,
    pub appearances: u64,
    pub file_count: u64,
    pub byte_count: u64,
    pub tree_hash: Option<String>,
    pub lock_claims: Vec<ScanLockClaimRecord>,
    pub worktree_hints: Vec<ScanWorktreeHintRecord>,
    /// Closed reason kind: `uninterpretable_metadata |
    /// provenance_contradiction | lock_file_fault | repository_ref_conflict
    /// | ownership_split | verification_deferred |
    /// multiple_directory_identities | managed_name_collision`.
    pub reason_kind: Option<String>,
    /// Source Content facts of the reason (refs, lock paths, fault detail).
    pub detail: Option<String>,
    /// Distinct legacy refs observed for this entity's Git hints.
    pub git_refs: Vec<String>,
    /// Distinct governing lock files of this entity's claims.
    pub git_lock_paths: Vec<PathBuf>,
    pub git_group_seq: Option<u64>,
    pub conflict_set_seq: Option<u64>,
    /// Closed info notes (`git_metadata_not_used | non_git_lock_claim`).
    pub notes: Vec<String>,
    pub operations: Vec<ScanOperationEligibility>,
}

/// One aggregated Git Repository Source hint group of a Report generation
/// (spec §8.2: aggregate by provider + canonical repository; hints never
/// constitute a Source Release). A group is a single row; members allow no
/// per-member Include.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanGitSourceGroupRecord {
    pub group_seq: u64,
    /// `github | gitlab | git` (provider kind; the URL host agrees).
    pub provider: String,
    /// Normalized canonical repository (spec §8.3); Source Content.
    pub canonical_repository: String,
    /// The bounded worktree repository root inside the originating Root.
    pub repository_root: Option<PathBuf>,
    pub remote_urls_seen: Vec<String>,
    pub member_entity_seqs: Vec<u64>,
    pub member_paths: Vec<PathBuf>,
    pub member_names: Vec<String>,
    pub lock_claims: Vec<ScanLockClaimRecord>,
    /// Distinct legacy refs across the group's claims (a >1 set is a
    /// Repository Ref Conflict, spec §8.2).
    pub refs: Vec<String>,
    /// Distinct governing lock files (a >1 set is a Repository Ownership
    /// Split, spec §8.2 — zero-write reject).
    pub lock_paths: Vec<PathBuf>,
    /// `candidate | repository_ref_conflict | ownership_split`.
    pub status: String,
    pub operations: Vec<ScanOperationEligibility>,
    /// Fact detail of a conflicted status (refs, lock paths).
    pub detail: Option<String>,
}

/// One Local↔Local Conflict Set (ADR-0017): same NFC + Unicode casefold
/// Directory Identity, different canonical entities. No winner until the
/// user explicitly chooses one.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanConflictSetRecord {
    pub set_seq: u64,
    pub directory_identity_key: String,
    /// Representative display name (Source Content).
    pub directory_name: String,
    pub member_entity_seqs: Vec<u64>,
    pub member_paths: Vec<PathBuf>,
    /// `None` until the user explicitly picks a winner (default: none).
    pub winner_entity_seq: Option<u64>,
}

/// Compact classification input of one canonical entity (built by the store
/// from the entity/appearance indexes; the Core rules classify it).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanClassificationRow {
    pub entity_seq: u64,
    pub identity: ScanObjectIdentity,
    pub canonical_path: PathBuf,
    pub file_count: u64,
    pub byte_count: u64,
    pub tree_hash: Option<String>,
    pub hash_fault: Option<String>,
    pub directory_names: Vec<String>,
    /// Number of appearances aggregated into this entity.
    pub appearances: u64,
    pub first_root_index: u32,
    /// Enriched lock claims of the entity's appearances (deduplicated by
    /// `(lock_path, entry_name)`).
    pub lock_claims: Vec<ScanLockClaimRecord>,
    pub worktree_hints: Vec<ScanWorktreeHintRecord>,
}

/// Bounded source classification counts of a terminal Report (spec §8.1):
/// cards + attention tallies; the candidate detail is paged, never
/// summarized away here.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanSourceCounts {
    /// Git Repository Source candidates (groups with status `candidate`).
    pub git_groups: u64,
    /// Git groups whose aggregated hints conflict (ref conflict / split).
    pub git_groups_conflicted: u64,
    /// Local candidates (entities; includes in-place and move-required).
    pub local_candidates: u64,
    /// Local↔Local Conflict Sets (no winner by default).
    pub conflict_sets: u64,
    /// Entities inside a Conflict Set.
    pub conflict_members: u64,
    /// Typed blocked entities (uninterpretable/contradictory metadata).
    pub blocked: u64,
    /// Verification Deferred entities.
    pub deferred: u64,
    /// Same entity under multiple Directory Identities.
    pub identity_conflicts: u64,
    /// Entities already Managed (Catalog path / Home namespace).
    pub already_managed: u64,
    /// Fixture entities.
    pub excluded: u64,
    /// Total immediate-attention rows (blocked + deferred + identity
    /// conflicts + conflicted groups).
    pub needs_attention: u64,
}

/// A bounded page of the current Report (spec §4.10): at most `limit`
/// rows; `next_offset` continues the same section, `None` means exhausted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanReportPageRead {
    pub rows: Vec<ScanReportRow>,
    pub next_offset: Option<u64>,
}

/// Typed page read failures: a generation-bound cursor never silently falls
/// back to another Report (spec §4.10).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScanReportPageError {
    /// The current Report identity differs from the cursor's.
    Stale { current_generation: u64 },
    /// No current Report / corrupt manifest / missing run artifact.
    NotFound,
}

/// One Root line of the coverage table in the terminal manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScanRootCoverageRecord {
    pub index: u32,
    pub configured_path: PathBuf,
    pub canonical_path: PathBuf,
    pub state: ScanRootState,
    pub consumer_agents: Vec<ScanRootAgentRef>,
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
    /// Bounded source classification counts of this Report.
    pub source_counts: ScanSourceCounts,
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

    /// Build the canonical entity index of a Run from its Root evidence
    /// (spec §4.10 Root atomicity, ADR-0017 canonical entity): only healthy
    /// Roots contribute appearances/entities; a failed or unresponsive Root
    /// contributes a typed diagnostic and no half candidate. The run-level
    /// `entities.jsonl`, `appearances.jsonl` and `diagnostics.jsonl`
    /// streams are written before the manifest switch, so a torn index is
    /// never published.
    fn build_entity_index(
        &self,
        run_id: &str,
    ) -> Result<ScanEntityIndexStats, ScanEvidenceStoreError>;

    /// The unique paged Report read contract (spec §4.10/§8.1): rows of the
    /// current Report generation only. The cursor is re-verified against
    /// the current manifest on every read.
    fn report_page(
        &self,
        cursor: &ScanReportCursor,
        limit: usize,
    ) -> Result<ScanReportPageRead, ScanReportPageError>;

    /// One canonical entity record of a Run by its generation-bound
    /// entity sequence. The immutable index streams never change after the
    /// manifest switch, so a read is stable for the lifetime of the
    /// Report. `None` means the sequence does not exist in this Run (never
    /// a guess).
    fn read_entity(
        &self,
        run_id: &str,
        entity_seq: u64,
    ) -> Result<Option<ScanCanonicalEntityRecord>, ScanEvidenceStoreError>;

    /// One classified source verdict of a Run by entity sequence. `None`
    /// means the entity has no verdict (failed classification must never
    /// be read as a Local candidate).
    fn read_verdict(
        &self,
        run_id: &str,
        entity_seq: u64,
    ) -> Result<Option<ScanSourceVerdictRecord>, ScanEvidenceStoreError>;

    /// Every appearance of a Run aggregated into one canonical entity
    /// (deterministic first-seen order), with the full bounded chain, the
    /// memoized hints and the frozen object identity.
    fn read_appearances(
        &self,
        run_id: &str,
        entity_seq: u64,
    ) -> Result<Vec<ScanAppearanceRecord>, ScanEvidenceStoreError>;

    /// Remove one temporary Run artifact, keeping `current.json` untouched.
    /// Only the Run whose `run.json` matches `(home_id, run_id)` is removed;
    /// anything unproven is left behind (`ProvenanceMismatch`).
    fn remove_run(&self, run_id: &str) -> Result<(), ScanEvidenceStoreError>;

    /// Compact classification input of every canonical entity (spec §8.1):
    /// entity facts plus the aggregated appearance evidence (names, lock
    /// claims, worktree hints). Fail-closed: no partial rows.
    fn classification_rows(
        &self,
        run_id: &str,
    ) -> Result<Vec<ScanClassificationRow>, ScanEvidenceStoreError>;

    /// Persist the classification pass of a Run (spec §8.2): verdicts,
    /// Git groups and Conflict Sets are written as immutable run-level
    /// index streams before the manifest switch, so a torn classification
    /// is never published.
    fn write_classification(
        &self,
        run_id: &str,
        verdicts: &[ScanSourceVerdictRecord],
        git_groups: &[ScanGitSourceGroupRecord],
        conflict_sets: &[ScanConflictSetRecord],
    ) -> Result<(), ScanEvidenceStoreError>;

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
    pub const ENTITY_INDEX: &str = "scan.evidence_store.entity_index";
    pub const CLASSIFICATION: &str = "scan.evidence_store.classification";
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
    (
        fault_points::ENTITY_INDEX,
        "entity index build fails → Run Failed, old Report kept",
    ),
    (
        fault_points::CLASSIFICATION,
        "classification write fails → Run Failed, old Report kept",
    ),
];
