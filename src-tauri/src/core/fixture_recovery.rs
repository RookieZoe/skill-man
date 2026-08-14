//! Fixture Recovery core (spec §3.5, §4.4, §5.2): the immutable
//! `FixtureFingerprintV1` constants, the read-only pure/mixed/unknown
//! classifier, and the crash-convergent recovery state machine that
//! snapshots a contaminated Home, prepares a clean one and promotes it only
//! after validation. Regular product writes stay closed for the whole flow
//! (`WriteGate::Closed { FixtureRecoveryLocked }`); the only writes are the
//! recovery module's own, driven by the external recovery ledger cursors.
//!
//! Production code keeps only the immutable fingerprint constants — never a
//! composition that could seed or fall back to demo fixture data.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use thiserror::Error;

use crate::core::bootstrap::{BootstrapConfig, BootstrapService, BootstrapSnapshot, CatalogAccess};
use crate::core::home::{HomeId, HomeMarker};
use crate::core::write_gate::ReadOnlyReason;
use crate::seams::catalog_probe::{
    CatalogHomeIdentity, CatalogProbe, CatalogProbeError, FixtureAgentRowEvidence,
    FixtureCatalogEvidence, FixturePreferencesEvidence, FixtureSkillRowEvidence,
};
use crate::seams::filesystem::FileSystem;
use crate::seams::prepared_catalog::PreparedCatalogFactory;

use crate::seams::app_state_store::{
    AppStateStore, AppStateStoreError, RECOVERY_LEDGER_FILE_NAME, RecoveryLedgerFile,
    RecoveryOperationRecord,
};

/// The `fixture-entities` sibling inside a fixture-contaminated Home.
pub const FIXTURE_ENTITIES_DIR: &str = "fixture-entities";
/// The one fixture entity that must never exist as a directory: its Skill row
/// is part of the fixture shape, but the seed never materialized it.
pub const FIXTURE_LEGACY_AUDIT_DIR: &str = "legacy-audit";
/// The exact fixture Catalog schema. Any other schema is a modified fact.
pub const FIXTURE_CATALOG_SCHEMA_VERSION: u32 = 4;

/// Exact tree hashes of the fixture entity trees (spec §3.5). Verified
/// against the real fixture footprint with the production `tree-sha256-v1`
/// algorithm; immutable — a hash change is a modified fact.
pub const FIXTURE_TREE_HASH_SKILL_AUTHORING: &str =
    "tree-sha256-v1:1ae22a3015f6d3f028667af331a3616ca2673d05fb3d7b33ac0802ab626a9061";
pub const FIXTURE_TREE_HASH_MEDIA_XRAY: &str =
    "tree-sha256-v1:146e94fa7177b5c4b034ccafad6fba24175de8a1030fbc0937744961811a2475";
pub const FIXTURE_TREE_HASH_ROOT: &str =
    "tree-sha256-v1:bfbd3ade08b7c05a2f5f806e2a0979dc6b241be253e71d28c73f08651af5cc0a";

pub const FIXTURE_SKILL_IDS: [&str; 3] = ["skill-authoring", "media-xray", "legacy-audit"];
pub const FIXTURE_AGENT_IDS: [&str; 3] = ["claude-code", "codex", "workbench"];

/// Recovery operation kind recorded in the external ledger.
pub const RECOVERY_KIND_FIXTURE: &str = "fixture_recovery";
/// Restore operation kind recorded in the external ledger: the same
/// crash-convergent state machine, entered from a Bound Home whose content
/// failed validation without a fixture footprint (ADR-0012 §6).
pub const RECOVERY_KIND_RESTORE: &str = "restore";

/// Durable cursors of the recovery state machine (spec §5.2). Each cursor is
/// persisted to the external ledger with the tmp → fsync → rename → parent
/// fsync protocol; the ledger write is the commit point of every step, so a
/// crash always resumes from a recorded cursor plus filesystem facts.
pub mod cursors {
    pub const CONFIRMED: &str = "confirmed";
    pub const SNAPSHOTTED: &str = "snapshotted";
    pub const PREPARED: &str = "prepared";
    pub const VALIDATED: &str = "validated";
    pub const PROMOTED: &str = "promoted";
    pub const VERIFIED: &str = "verified";
    pub const AWAITING_COMMIT: &str = "awaiting_commit";
    pub const COMMITTED: &str = "committed";
    pub const ROLLED_BACK: &str = "rolled_back";
}

/// Read-only tree facts of a Home's `fixture-entities` sibling (spec §3.5).
/// `None` hashes mean the tree could not be read — a fact that must classify
/// the Home unknown, never as a partial match.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureTreeEvidence {
    pub fixture_entities_exists: bool,
    pub skill_authoring_exists: bool,
    pub skill_authoring_hash: Option<String>,
    pub media_xray_exists: bool,
    pub media_xray_hash: Option<String>,
    pub root_hash: Option<String>,
    pub legacy_audit_entity_exists: bool,
}

impl FixtureTreeEvidence {
    /// A Home with no fixture footprint at all.
    pub fn clean() -> Self {
        Self {
            fixture_entities_exists: false,
            skill_authoring_exists: false,
            skill_authoring_hash: None,
            media_xray_exists: false,
            media_xray_hash: None,
            root_hash: None,
            legacy_audit_entity_exists: false,
        }
    }
}

/// The closed classification of a Home's fixture footprint (spec §3.5).
///
/// - `Pure`: every checked fact matches the immutable fingerprint exactly;
///   only this class may Preview and recover.
/// - `Mixed`: readable evidence shows extra, missing or modified facts; the
///   whole Home locks, no subset is ever recovered.
/// - `Unknown`: some fact could not be read; same lock, no guessing.
/// - `Clean`: no fixture footprint at all — an ordinary Home.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FixtureClassification {
    Pure,
    Mixed { reasons: Vec<String> },
    Unknown { reasons: Vec<String> },
    Clean,
}

impl FixtureClassification {
    /// Any fixture footprint — pure or not — keeps the recovery lock.
    pub fn is_contaminated(&self) -> bool {
        matches!(
            self,
            FixtureClassification::Pure
                | FixtureClassification::Mixed { .. }
                | FixtureClassification::Unknown { .. }
        )
    }

    /// Only the exact fingerprint may move past Preview.
    pub fn can_preview(&self) -> bool {
        matches!(self, FixtureClassification::Pure)
    }
}

/// Which Home shape the fixture fingerprint is matched against (spec §5.2):
/// the unbound Legacy fixture is schema v4; a Bound fixture footprint is
/// schema v5 under a verified identity (bootstrap verifies identity before
/// classification ever runs in Bound mode).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixtureShapeMode {
    Legacy,
    Bound,
}

/// Read-only fixture classification seam used by the bootstrap authority
/// (§5.1 step 7): any footprint keeps the recovery lock; only a clean Home
/// proceeds to `LegacyDetected` / `Bound`.
pub trait FixtureClassifier: Send + Sync {
    fn classify(&self, home_root: &Path, mode: FixtureShapeMode) -> FixtureClassification;
}

/// System classifier: real probe + filesystem evidence, pure classification.
pub struct SystemFixtureClassifier {
    probe: Arc<dyn CatalogProbe>,
    filesystem: Arc<dyn FileSystem>,
    catalog_file_name: String,
}

impl SystemFixtureClassifier {
    pub fn new(
        probe: Arc<dyn CatalogProbe>,
        filesystem: Arc<dyn FileSystem>,
        catalog_file_name: String,
    ) -> Self {
        Self {
            probe,
            filesystem,
            catalog_file_name,
        }
    }
}

impl FixtureClassifier for SystemFixtureClassifier {
    fn classify(&self, home_root: &Path, mode: FixtureShapeMode) -> FixtureClassification {
        let collector =
            FixtureEvidenceCollector::new(self.probe.as_ref(), self.filesystem.as_ref());
        match collector.collect(home_root, &self.catalog_file_name) {
            Ok((db, tree)) => classify_fixture(&db, &tree, home_root, mode),
            Err(error) => FixtureClassification::Unknown {
                reasons: vec![format!("catalog_unreadable:{error}")],
            },
        }
    }
}

/// Test support: a classifier that always reports a clean Home. Used by
/// compositions that do not exercise fixture classification (unit tests,
/// API tests); integration tests use the system classifier.
pub struct FixedCleanClassifier;

impl FixtureClassifier for FixedCleanClassifier {
    fn classify(&self, _home_root: &Path, _mode: FixtureShapeMode) -> FixtureClassification {
        FixtureClassification::Clean
    }
}

/// Read-only fixture evidence collector: raw Catalog rows via the probe seam
/// plus tree facts via the FileSystem seam. Never writes anything.
pub struct FixtureEvidenceCollector<'a> {
    probe: &'a dyn CatalogProbe,
    filesystem: &'a dyn crate::seams::filesystem::FileSystem,
}

impl<'a> FixtureEvidenceCollector<'a> {
    pub fn new(
        probe: &'a dyn CatalogProbe,
        filesystem: &'a dyn crate::seams::filesystem::FileSystem,
    ) -> Self {
        Self { probe, filesystem }
    }

    pub fn collect(
        &self,
        home_root: &Path,
        catalog_file_name: &str,
    ) -> Result<(FixtureCatalogEvidence, FixtureTreeEvidence), CatalogProbeError> {
        let db = self
            .probe
            .probe_fixture(&home_root.join(catalog_file_name))?;
        let tree = self.collect_tree(home_root);
        Ok((db, tree))
    }

    fn collect_tree(&self, home_root: &Path) -> FixtureTreeEvidence {
        let entities = home_root.join(FIXTURE_ENTITIES_DIR);
        let entities_exists = self
            .filesystem
            .path_is_directory(&entities)
            .unwrap_or(false);
        let skill_authoring = entities.join("skill-authoring");
        let media_xray = entities.join("media-xray");
        let skill_authoring_exists = entities_exists
            && self
                .filesystem
                .path_is_directory(&skill_authoring)
                .unwrap_or(false);
        let media_xray_exists = entities_exists
            && self
                .filesystem
                .path_is_directory(&media_xray)
                .unwrap_or(false);
        FixtureTreeEvidence {
            fixture_entities_exists: entities_exists,
            skill_authoring_exists,
            skill_authoring_hash: if skill_authoring_exists {
                self.filesystem.tree_hash(&skill_authoring).ok()
            } else {
                None
            },
            media_xray_exists,
            media_xray_hash: if media_xray_exists {
                self.filesystem.tree_hash(&media_xray).ok()
            } else {
                None
            },
            root_hash: if entities_exists {
                self.filesystem.tree_hash(&entities).ok()
            } else {
                None
            },
            legacy_audit_entity_exists: entities_exists
                && self
                    .filesystem
                    .path_is_directory(&entities.join(FIXTURE_LEGACY_AUDIT_DIR))
                    .unwrap_or(false),
        }
    }
}

/// Classify a Home's fixture footprint (spec §3.5). Pure function: every
/// fact is passed in, nothing is read here, and a single deviation or
/// unreadable fact decides the whole Home.
pub fn classify_fixture(
    db: &FixtureCatalogEvidence,
    tree: &FixtureTreeEvidence,
    home_root: &Path,
    mode: FixtureShapeMode,
) -> FixtureClassification {
    let entities_path = home_root.join(FIXTURE_ENTITIES_DIR);
    let has_fixture_rows = db
        .skills
        .iter()
        .any(|row| is_fixture_skill_row(row, &entities_path));
    let footprint = tree.fixture_entities_exists || has_fixture_rows;
    if !footprint {
        return FixtureClassification::Clean;
    }

    let mut reasons = Vec::new();

    // -- Catalog facts (spec §3.5: exact tuples, no relations) --

    // The Legacy shape is the untouched v4 fixture Catalog; the Bound
    // shape is the fixture after the one-time migration (schema v6, Remote
    // Source Parent tables replace the legacy remote_sources table).
    let mut expected_tables: Vec<&str> = match mode {
        FixtureShapeMode::Legacy => vec![
            "activations",
            "agents",
            "catalog_meta",
            "file_sources",
            "preferences",
            "remote_sources",
            "skills",
        ],
        FixtureShapeMode::Bound => vec![
            "activations",
            "agents",
            "catalog_meta",
            "file_sources",
            "preferences",
            "remote_bindings",
            "remote_source_aliases",
            "remote_source_parents",
            "skills",
        ],
    };
    expected_tables.sort_unstable();
    for table in &expected_tables {
        if !db.tables.iter().any(|t| t == table) {
            reasons.push(format!("missing_table:{table}"));
        }
    }
    for table in &db.tables {
        if !expected_tables.iter().any(|t| t == table) {
            reasons.push(format!("extra_table:{table}"));
        }
    }

    match &db.meta {
        Some(meta) => {
            let expected_schema = match mode {
                FixtureShapeMode::Legacy => FIXTURE_CATALOG_SCHEMA_VERSION,
                FixtureShapeMode::Bound => {
                    crate::seams::catalog_probe::CURRENT_CATALOG_SCHEMA_VERSION
                }
            };
            if meta.schema_version != expected_schema {
                reasons.push(format!("unknown_schema:{}", meta.schema_version));
            }
            if meta.first_run_completed_at.is_some() {
                reasons.push("fixture_first_run_completed".into());
            }
        }
        None => reasons.push("catalog_meta_missing".into()),
    }

    let expected_skills = expected_skill_rows(home_root);
    for expected in &expected_skills {
        match db.skills.iter().find(|row| row.id == expected.id) {
            None => reasons.push(format!("missing_skill_row:{}", expected.id)),
            Some(row) if row != expected => {
                reasons.push(format!("skill_row_modified:{}", expected.id));
            }
            Some(_) => {}
        }
    }
    for row in &db.skills {
        if !expected_skills.iter().any(|expected| expected.id == row.id) {
            reasons.push(format!("extra_skill_row:{}", row.id));
        }
    }

    let expected_agents = expected_agent_rows();
    for expected in &expected_agents {
        match db.agents.iter().find(|row| row.id == expected.id) {
            None => reasons.push(format!("missing_agent_row:{}", expected.id)),
            Some(row) if row != expected => {
                reasons.push(format!("agent_row_modified:{}", expected.id));
            }
            Some(_) => {}
        }
    }
    for row in &db.agents {
        if !expected_agents.iter().any(|expected| expected.id == row.id) {
            reasons.push(format!("extra_agent_row:{}", row.id));
        }
    }

    match &db.preferences {
        Some(preferences) => {
            if preferences != &expected_preferences_row() {
                reasons.push("preferences_row_modified".into());
            }
        }
        None => reasons.push("preferences_row_missing".into()),
    }

    if db.activation_count != 0 {
        reasons.push(format!("activation_rows:{}", db.activation_count));
    }
    if db.file_source_count != 0 {
        reasons.push(format!("file_source_rows:{}", db.file_source_count));
    }
    if db.remote_source_count != 0 {
        reasons.push(format!("remote_source_rows:{}", db.remote_source_count));
    }

    // -- Tree facts (spec §3.5: the three fixed hashes) --

    if !tree.fixture_entities_exists {
        reasons.push("fixture_entities_missing".into());
    } else {
        if !tree.skill_authoring_exists {
            reasons.push("missing_tree:skill-authoring".into());
        } else if tree.skill_authoring_hash.as_deref() != Some(FIXTURE_TREE_HASH_SKILL_AUTHORING) {
            reasons.push("tree_hash_mismatch:skill-authoring".into());
        }
        if !tree.media_xray_exists {
            reasons.push("missing_tree:media-xray".into());
        } else if tree.media_xray_hash.as_deref() != Some(FIXTURE_TREE_HASH_MEDIA_XRAY) {
            reasons.push("tree_hash_mismatch:media-xray".into());
        }
        if tree.root_hash.as_deref() != Some(FIXTURE_TREE_HASH_ROOT) {
            reasons.push("tree_hash_mismatch:fixture-entities-root".into());
        }
        if tree.legacy_audit_entity_exists {
            reasons.push("legacy_audit_entity_present".into());
        }
    }

    if reasons.is_empty() {
        FixtureClassification::Pure
    } else {
        FixtureClassification::Mixed { reasons }
    }
}

fn is_fixture_skill_row(row: &FixtureSkillRowEvidence, entities_path: &Path) -> bool {
    FIXTURE_SKILL_IDS.iter().any(|id| {
        row.id == *id && row.final_entity_path == entities_path.join(id).to_string_lossy()
    })
}

/// The exact fixture `skills` tuples (spec §3.5). `home_root` parameterizes
/// the machine-specific absolute paths into `fixture-entities`; every other
/// column is a byte-exact constant.
fn expected_skill_rows(home_root: &Path) -> Vec<FixtureSkillRowEvidence> {
    let entity = |name: &str| {
        home_root
            .join(FIXTURE_ENTITIES_DIR)
            .join(name)
            .to_string_lossy()
            .into_owned()
    };
    vec![
        FixtureSkillRowEvidence {
            id: "skill-authoring".into(),
            directory_name: "skill-authoring".into(),
            identity_key: "skill-authoring".into(),
            display_name: "Skill authoring".into(),
            description: "A precise workflow for building maintainable Agent Skills.".into(),
            source_kind: "link".into(),
            library_entry_path: None,
            final_entity_path: entity("skill-authoring"),
            recorded_content_hash: None,
            health: "healthy".into(),
            created_at: "2026-07-20T10:42:00Z".into(),
            updated_at: "2026-07-20T10:42:00Z".into(),
        },
        FixtureSkillRowEvidence {
            id: "media-xray".into(),
            directory_name: "media-xray".into(),
            identity_key: "media-xray".into(),
            display_name: "Media X-ray".into(),
            description: "Transcribes and inspects local audio and video.".into(),
            source_kind: "remote_install".into(),
            library_entry_path: Some(entity("media-xray")),
            final_entity_path: entity("media-xray"),
            recorded_content_hash: None,
            health: "healthy".into(),
            created_at: "2026-07-19T14:08:00Z".into(),
            updated_at: "2026-07-19T14:08:00Z".into(),
        },
        FixtureSkillRowEvidence {
            id: "legacy-audit".into(),
            directory_name: "legacy-audit".into(),
            identity_key: "legacy-audit".into(),
            display_name: "Legacy audit".into(),
            description: "Checks an existing skills directory before Adopt.".into(),
            source_kind: "link".into(),
            library_entry_path: None,
            final_entity_path: entity("legacy-audit"),
            recorded_content_hash: None,
            health: "broken".into(),
            created_at: "2026-07-18T03:16:00Z".into(),
            updated_at: "2026-07-18T03:16:00Z".into(),
        },
    ]
}

/// The exact fixture `agents` tuples.
fn expected_agent_rows() -> Vec<FixtureAgentRowEvidence> {
    let preset = |id: &str, name: &str, kind: &str, skills_path: &str| FixtureAgentRowEvidence {
        id: id.into(),
        name: name.into(),
        kind: kind.into(),
        skills_path: skills_path.into(),
        path_identity_key: skills_path.to_lowercase(),
        detected: true,
        compatibility: "verified".into(),
        created_at: "1970-01-01T00:00:00Z".into(),
        updated_at: "1970-01-01T00:00:00Z".into(),
    };
    vec![
        preset(
            "claude-code",
            "Claude Code",
            "claude_preset",
            "~/.claude/skills",
        ),
        preset("codex", "Codex", "codex_preset", "~/.codex/skills"),
        FixtureAgentRowEvidence {
            id: "workbench".into(),
            name: "Workbench".into(),
            kind: "custom".into(),
            skills_path: "~/Library/Application Support/workbench/skills".into(),
            path_identity_key: "~/library/application support/workbench/skills".into(),
            detected: true,
            compatibility: "unknown".into(),
            created_at: "1970-01-01T00:00:00Z".into(),
            updated_at: "1970-01-01T00:00:00Z".into(),
        },
    ]
}

/// The exact fixture `preferences` tuple.
fn expected_preferences_row() -> FixturePreferencesEvidence {
    FixturePreferencesEvidence {
        launch_at_login: false,
        show_in_dock: true,
        check_app_updates: true,
        check_skill_updates: true,
        last_app_update_check_at: None,
        last_skill_update_check_at: None,
    }
}

// ---------------------------------------------------------------------------
// Fixture Recovery service: the crash-convergent state machine (spec §5.2)
// ---------------------------------------------------------------------------

/// Which Home shape the recovery operates on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryMode {
    /// No binding yet: the contaminated Legacy Home recovers to a clean
    /// unbound Home that the binding flow (#45) commits later.
    LegacyUnbound,
    /// A verified Bound Home whose content failed validation: recovery
    /// restores the SAME `home_id` (never a new identity, never Relocate).
    BoundRestore { home_id: HomeId },
}

/// Why a Bound Home needs Restore (ADR-0012 §6): content validation failed
/// under a provable same identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreReason {
    /// SQLite integrity or foreign-key verification failed.
    CatalogIntegrityFailed,
    /// The Bound Home carries a fixture footprint (the recovery lock).
    FixtureContamination,
}

/// Why Restore does not apply right now (closed reasons for the UI; no
/// free-form text crosses the boundary).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreNotApplicableReason {
    NoBinding,
    LegacyUnbound,
    AppStateUnavailable,
    IdentityMismatch,
    HomeUnavailable,
    UnsupportedSchema,
    OpenFailed,
    ActiveOperation,
}

/// Restore eligibility probe (spec §5.5, ADR-0012 §6): only a provable
/// same-identity content failure is `RestoreRequired`. A healthy Bound Home
/// is `NotRequired`; every other state carries a closed reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestoreEligibility {
    RestoreRequired {
        home_id: HomeId,
        path: PathBuf,
        reason: RestoreReason,
    },
    NotRequired,
    NotApplicable {
        reason: RestoreNotApplicableReason,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogEvidenceSummary {
    pub tables: Vec<String>,
    pub schema_version: Option<u32>,
    pub first_run_completed_at: Option<String>,
    pub skill_row_count: u64,
    pub agent_row_count: u64,
    pub activation_row_count: u64,
    pub file_source_row_count: u64,
    pub remote_source_row_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeEvidenceSummary {
    pub fixture_entities_present: bool,
    /// `None` = the tree is missing or unreadable (never a match).
    pub skill_authoring_hash_matches: Option<bool>,
    pub media_xray_hash_matches: Option<bool>,
    pub root_hash_matches: Option<bool>,
    pub legacy_audit_entity_present: bool,
}

/// The recovery route's preview: classification plus the exact evidence the
/// user confirms against. React renders one route; no booleans composed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureRecoveryPreview {
    pub mode: RecoveryMode,
    pub path: PathBuf,
    pub classification: FixtureClassification,
    pub catalog_evidence: CatalogEvidenceSummary,
    pub tree_evidence: TreeEvidenceSummary,
    pub can_preview: bool,
    pub active_operation: Option<RecoveryOperationRecord>,
}

/// User confirmation of the preview. Fixture Recovery has nothing to select:
/// a pure footprint recovers the whole Home or nothing.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FixtureRecoverySelection {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixtureRecoveryPlan {
    pub plan_token: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryResult {
    pub operation_id: String,
    /// `true` when the prepared Home is live and verified, waiting for the
    /// user's explicit commit (`confirm_result`); `false` when the operation
    /// was rolled back and the original Home restored.
    pub awaiting_commit: bool,
    pub rolled_back: bool,
}

/// A Safety Snapshot: the whole old Home isolated at a sibling path. Never
/// auto-deleted; deletion is always an explicit user action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SafetySnapshot {
    /// The sibling directory name, e.g. `skill-man.snapshot-fr-…`.
    pub snapshot_id: String,
    pub path: PathBuf,
    pub manifest_hash: Option<String>,
    pub file_count: u64,
    pub total_bytes: u64,
    pub taken_at: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteSnapshotPreview {
    pub snapshot_id: String,
    pub path: PathBuf,
    pub file_count: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Error)]
pub enum FixtureRecoveryError {
    #[error("Fixture Recovery does not apply to the current bootstrap state")]
    NotLocked,
    #[error("Restore does not apply to the current bootstrap state: {0}")]
    NotRestorable(String),
    #[error("the Home is not a pure fixture; only the exact fingerprint can be recovered")]
    NotPure,
    #[error("no active recovery operation matches the plan token")]
    NoActiveOperation,
    #[error("a recovery operation is already active: {operation_id}")]
    OperationAlreadyActive { operation_id: String },
    #[error("a SQLite writer holds the WAL index; recovery cannot safely proceed: {0}")]
    WriterActive(String),
    #[error("recovery step {cursor} failed: {message}")]
    StepFailed {
        cursor: String,
        message: String,
        rolled_back: bool,
    },
    #[error("recovery state is ambiguous and stays locked: {0}")]
    AmbiguousState(String),
    #[error("the Safety Snapshot is in use by the active recovery operation")]
    SnapshotInUse,
    #[error("recovery could not read or write app state: {0}")]
    StateStore(#[from] AppStateStoreError),
    #[error("recovery failed on the filesystem: {0}")]
    FileSystem(String),
    #[error("recovery failed probing the Catalog: {0}")]
    Probe(String),
}

pub struct FixtureRecoveryService {
    app_state: Arc<dyn AppStateStore>,
    probe: Arc<dyn CatalogProbe>,
    filesystem: Arc<dyn FileSystem>,
    prepared: Arc<dyn PreparedCatalogFactory>,
    bootstrap: Arc<BootstrapService>,
    config: BootstrapConfig,
}

impl FixtureRecoveryService {
    pub fn new(
        app_state: Arc<dyn AppStateStore>,
        probe: Arc<dyn CatalogProbe>,
        filesystem: Arc<dyn FileSystem>,
        prepared: Arc<dyn PreparedCatalogFactory>,
        bootstrap: Arc<BootstrapService>,
        config: BootstrapConfig,
    ) -> Self {
        Self {
            app_state,
            probe,
            filesystem,
            prepared,
            bootstrap,
            config,
        }
    }

    /// The recovery route preview: read-only classification and evidence.
    pub fn inspect(&self) -> Result<FixtureRecoveryPreview, FixtureRecoveryError> {
        let (mode, live) = self.recovery_context()?;
        let collector =
            FixtureEvidenceCollector::new(self.probe.as_ref(), self.filesystem.as_ref());
        let shape_mode = match &mode {
            RecoveryMode::LegacyUnbound => FixtureShapeMode::Legacy,
            RecoveryMode::BoundRestore { .. } => FixtureShapeMode::Bound,
        };
        let (db, tree, unknown) = match collector.collect(&live, &self.config.catalog_file_name) {
            Ok((db, tree)) => (db, tree, None),
            Err(error) => (
                FixtureCatalogEvidence::clean(),
                FixtureTreeEvidence::clean(),
                Some(format!("catalog_unreadable:{error}")),
            ),
        };
        let classification = match unknown {
            Some(reason) => FixtureClassification::Unknown {
                reasons: vec![reason],
            },
            None => classify_fixture(&db, &tree, &live, shape_mode),
        };
        let can_preview = classification.can_preview();
        let ledger = self.app_state.load()?.recovery_ledger;
        Ok(FixtureRecoveryPreview {
            mode,
            path: live,
            classification,
            catalog_evidence: CatalogEvidenceSummary {
                tables: db.tables,
                schema_version: db.meta.as_ref().map(|meta| meta.schema_version),
                first_run_completed_at: db.meta.and_then(|meta| meta.first_run_completed_at),
                skill_row_count: db.skills.len() as u64,
                agent_row_count: db.agents.len() as u64,
                activation_row_count: db.activation_count,
                file_source_row_count: db.file_source_count,
                remote_source_row_count: db.remote_source_count,
            },
            tree_evidence: TreeEvidenceSummary {
                fixture_entities_present: tree.fixture_entities_exists,
                skill_authoring_hash_matches: tree
                    .skill_authoring_hash
                    .as_deref()
                    .map(|hash| hash == FIXTURE_TREE_HASH_SKILL_AUTHORING),
                media_xray_hash_matches: tree
                    .media_xray_hash
                    .as_deref()
                    .map(|hash| hash == FIXTURE_TREE_HASH_MEDIA_XRAY),
                root_hash_matches: tree
                    .root_hash
                    .as_deref()
                    .map(|hash| hash == FIXTURE_TREE_HASH_ROOT),
                legacy_audit_entity_present: tree.legacy_audit_entity_exists,
            },
            can_preview,
            active_operation: ledger.active,
        })
    }

    /// User confirmation of a pure preview: opens the durable operation at
    /// cursor `confirmed`. No Home bytes move until `apply`.
    pub fn plan(
        &self,
        _selection: &FixtureRecoverySelection,
    ) -> Result<FixtureRecoveryPlan, FixtureRecoveryError> {
        let preview = self.inspect()?;
        if !preview.can_preview {
            return Err(FixtureRecoveryError::NotPure);
        }
        let mut ledger = self.app_state.load()?.recovery_ledger;
        if let Some(active) = &ledger.active {
            return Err(FixtureRecoveryError::OperationAlreadyActive {
                operation_id: active.operation_id.clone(),
            });
        }
        let (mode, live) = self.recovery_context()?;
        let op = RecoveryOperationRecord {
            operation_id: new_operation_id(),
            kind: RECOVERY_KIND_FIXTURE.into(),
            home_id: match &mode {
                RecoveryMode::BoundRestore { home_id } => Some(home_id.clone()),
                RecoveryMode::LegacyUnbound => None,
            },
            live_path: Some(live),
            snapshot_path: None,
            prepared_path: None,
            manifest_hash: None,
            external_probe: None,
            cursor: Some(cursors::CONFIRMED.into()),
            commit_point: None,
            created_at: rfc3339_now(),
        };
        ledger.active = Some(op.clone());
        self.app_state.write_recovery_ledger(&ledger)?;
        Ok(FixtureRecoveryPlan {
            plan_token: op.operation_id,
        })
    }

    /// Restore eligibility probe (spec §5.5): Restore applies only when the
    /// binding's identity is provable and the content failed validation —
    /// SQLite integrity/foreign-key failure (`Bound` read-only) or a Bound
    /// Home fixture footprint (the recovery lock). Read-only; never touches
    /// the locator, the Home or the ledger.
    pub fn restore_eligibility(&self) -> Result<RestoreEligibility, FixtureRecoveryError> {
        let ledger = self.app_state.load()?.recovery_ledger;
        if ledger.active.is_some() {
            return Ok(RestoreEligibility::NotApplicable {
                reason: RestoreNotApplicableReason::ActiveOperation,
            });
        }
        match self.bootstrap.inspect() {
            BootstrapSnapshot::Bound {
                home_id,
                catalog_access:
                    CatalogAccess::ReadOnly {
                        reason: ReadOnlyReason::IntegrityFailed,
                    },
                ..
            } => {
                let path = self.resolve_live_path()?;
                Ok(RestoreEligibility::RestoreRequired {
                    home_id,
                    path,
                    reason: RestoreReason::CatalogIntegrityFailed,
                })
            }
            BootstrapSnapshot::FixtureRecoveryLocked {
                home_id: Some(home_id),
                path: Some(path),
            } => Ok(RestoreEligibility::RestoreRequired {
                home_id,
                path,
                reason: RestoreReason::FixtureContamination,
            }),
            BootstrapSnapshot::Bound {
                catalog_access:
                    CatalogAccess::ReadOnly {
                        reason: ReadOnlyReason::UnsupportedSchema,
                    },
                ..
            } => Ok(RestoreEligibility::NotApplicable {
                reason: RestoreNotApplicableReason::UnsupportedSchema,
            }),
            BootstrapSnapshot::Bound {
                catalog_access:
                    CatalogAccess::ReadOnly {
                        reason: ReadOnlyReason::OpenFailed,
                    },
                ..
            } => Ok(RestoreEligibility::NotApplicable {
                reason: RestoreNotApplicableReason::OpenFailed,
            }),
            BootstrapSnapshot::Bound { .. } => Ok(RestoreEligibility::NotRequired),
            BootstrapSnapshot::HomeUnavailable { .. } => Ok(RestoreEligibility::NotApplicable {
                reason: RestoreNotApplicableReason::HomeUnavailable,
            }),
            BootstrapSnapshot::HomeIdentityMismatch { .. } => {
                Ok(RestoreEligibility::NotApplicable {
                    reason: RestoreNotApplicableReason::IdentityMismatch,
                })
            }
            BootstrapSnapshot::FixtureRecoveryLocked { .. } => {
                Ok(RestoreEligibility::NotApplicable {
                    reason: RestoreNotApplicableReason::LegacyUnbound,
                })
            }
            BootstrapSnapshot::LegacyDetected { .. } => Ok(RestoreEligibility::NotApplicable {
                reason: RestoreNotApplicableReason::LegacyUnbound,
            }),
            BootstrapSnapshot::AppStateUnavailable { .. } => {
                Ok(RestoreEligibility::NotApplicable {
                    reason: RestoreNotApplicableReason::AppStateUnavailable,
                })
            }
            BootstrapSnapshot::Unconfigured
            | BootstrapSnapshot::Abandoned { .. }
            | BootstrapSnapshot::HomeCandidatePending { .. } => {
                Ok(RestoreEligibility::NotApplicable {
                    reason: RestoreNotApplicableReason::NoBinding,
                })
            }
        }
    }

    /// User-initiated Restore of a Bound Home whose same-identity content
    /// failed validation: opens the SAME crash-convergent operation at
    /// cursor `confirmed` without the pure-fixture gate. The locator
    /// identity is never modified; the Safety Snapshot is never
    /// auto-deleted (ADR-0012 §6).
    pub fn plan_restore(&self) -> Result<FixtureRecoveryPlan, FixtureRecoveryError> {
        let (home_id, live) = match self.restore_eligibility()? {
            RestoreEligibility::RestoreRequired { home_id, path, .. } => (home_id, path),
            RestoreEligibility::NotRequired => {
                return Err(FixtureRecoveryError::NotRestorable(
                    "the Bound Home content is healthy; nothing to restore".into(),
                ));
            }
            RestoreEligibility::NotApplicable { reason } => {
                return Err(FixtureRecoveryError::NotRestorable(format!(
                    "restore is not applicable: {reason:?}"
                )));
            }
        };
        let mut ledger = self.app_state.load()?.recovery_ledger;
        if let Some(active) = &ledger.active {
            return Err(FixtureRecoveryError::OperationAlreadyActive {
                operation_id: active.operation_id.clone(),
            });
        }
        let op = RecoveryOperationRecord {
            operation_id: new_operation_id(),
            kind: RECOVERY_KIND_RESTORE.into(),
            home_id: Some(home_id),
            live_path: Some(live),
            snapshot_path: None,
            prepared_path: None,
            manifest_hash: None,
            external_probe: None,
            cursor: Some(cursors::CONFIRMED.into()),
            commit_point: None,
            created_at: rfc3339_now(),
        };
        ledger.active = Some(op.clone());
        self.app_state.write_recovery_ledger(&ledger)?;
        Ok(FixtureRecoveryPlan {
            plan_token: op.operation_id,
        })
    }

    /// Run the recovery state machine from the operation's current cursor.
    /// Every durable cursor is persisted before the next mutation, so a
    /// crash at any point resumes deterministically — rollback or
    /// roll-forward, never a guess.
    pub fn apply(&self, plan_token: &str) -> Result<RecoveryResult, FixtureRecoveryError> {
        let mut ledger = self.app_state.load()?.recovery_ledger;
        let Some(op) = ledger.active.clone() else {
            return Err(FixtureRecoveryError::NoActiveOperation);
        };
        if op.operation_id != plan_token
            || !matches!(
                op.kind.as_str(),
                RECOVERY_KIND_FIXTURE | RECOVERY_KIND_RESTORE
            )
        {
            return Err(FixtureRecoveryError::NoActiveOperation);
        }
        let (_, live) = self.recovery_context()?;
        if op.live_path.as_ref() != Some(&live) {
            return Err(FixtureRecoveryError::AmbiguousState(
                "the Home path changed while the recovery operation was active".into(),
            ));
        }
        self.advance(&mut ledger, op)
    }

    /// Final user confirmation: re-verifies the live Home, records the
    /// operation as completed and returns the fresh bootstrap snapshot.
    pub fn confirm_result(
        &self,
        operation_id: &str,
    ) -> Result<BootstrapSnapshot, FixtureRecoveryError> {
        let mut ledger = self.app_state.load()?.recovery_ledger;
        let Some(op) = ledger.active.clone() else {
            return Err(FixtureRecoveryError::NoActiveOperation);
        };
        if op.operation_id != operation_id {
            return Err(FixtureRecoveryError::NoActiveOperation);
        }
        if !matches!(
            op.cursor.as_deref(),
            Some(cursors::VERIFIED) | Some(cursors::AWAITING_COMMIT)
        ) {
            return Err(FixtureRecoveryError::AmbiguousState(
                "the recovery operation is not awaiting commit".into(),
            ));
        }
        if let Err(message) = self.verify_live(&op) {
            // The clean Home failed its final verification (e.g. corruption
            // appeared between apply and commit). Deterministic exit:
            // restore the Safety Snapshot so the user is never stuck at
            // AwaitingCommit with a broken live Home.
            return match self.rollback(&mut ledger, &mut op.clone(), true) {
                Ok(()) => Err(FixtureRecoveryError::StepFailed {
                    cursor: cursors::COMMITTED.into(),
                    message,
                    rolled_back: true,
                }),
                Err(rollback_error) => Err(FixtureRecoveryError::AmbiguousState(format!(
                    "final verification failed ({message}); rollback also failed: \
                     {rollback_error}"
                ))),
            };
        }
        let snapshot_path = op.snapshot_path.clone().ok_or_else(|| {
            FixtureRecoveryError::AmbiguousState("the Safety Snapshot path is unknown".into())
        })?;
        if !self.is_dir(&snapshot_path)? {
            return Err(FixtureRecoveryError::AmbiguousState(
                "the Safety Snapshot disappeared before commit".into(),
            ));
        }
        let mut record = op.clone();
        record.cursor = Some(cursors::COMMITTED.into());
        record.commit_point = Some(cursors::COMMITTED.into());
        ledger.active = None;
        ledger.completed.push(record);
        self.app_state.write_recovery_ledger(&ledger)?;
        Ok(self.bootstrap.inspect())
    }

    /// List every Safety Snapshot sibling of the current Home root. Works
    /// whether or not the recovery lock is held, so leftovers stay visible
    /// after a successful commit until the user deletes them.
    pub fn list_snapshots(&self) -> Result<Vec<SafetySnapshot>, FixtureRecoveryError> {
        let live = self.resolve_live_path()?;
        let ledger = self.app_state.load()?.recovery_ledger;
        let Some(home_name) = live
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            return Err(FixtureRecoveryError::AmbiguousState(
                "the Home path has no file name".into(),
            ));
        };
        let prefix = format!("{home_name}.snapshot-");
        let mut snapshots = Vec::new();
        for entry in self
            .filesystem
            .list_directory(live.parent().ok_or_else(|| {
                FixtureRecoveryError::AmbiguousState("the Home path has no parent".into())
            })?)
            .map_err(|error| FixtureRecoveryError::FileSystem(error.to_string()))?
        {
            if !entry.is_directory || !entry.name.starts_with(&prefix) {
                continue;
            }
            let path = live.parent().unwrap().join(&entry.name);
            let (file_count, total_bytes) = self.tree_stats(&path)?;
            let taken_at = ledger
                .active
                .iter()
                .chain(ledger.completed.iter())
                .find(|record| record.snapshot_path.as_deref() == Some(path.as_path()))
                .map(|record| record.created_at.clone());
            let manifest_hash = ledger
                .active
                .iter()
                .chain(ledger.completed.iter())
                .find(|record| record.snapshot_path.as_deref() == Some(path.as_path()))
                .and_then(|record| record.manifest_hash.clone());
            snapshots.push(SafetySnapshot {
                snapshot_id: entry.name,
                path,
                manifest_hash,
                file_count,
                total_bytes,
                taken_at,
            });
        }
        snapshots.sort_by(|left, right| left.snapshot_id.cmp(&right.snapshot_id));
        Ok(snapshots)
    }

    /// Preview deleting a Safety Snapshot. Refused while the active
    /// operation still references it.
    pub fn plan_delete_snapshot(
        &self,
        snapshot_id: &str,
    ) -> Result<DeleteSnapshotPreview, FixtureRecoveryError> {
        let (path, stats) = self.find_snapshot(snapshot_id)?;
        let ledger = self.app_state.load()?.recovery_ledger;
        if let Some(active) = &ledger.active {
            if active.snapshot_path.as_deref() == Some(path.as_path()) {
                return Err(FixtureRecoveryError::SnapshotInUse);
            }
        }
        Ok(DeleteSnapshotPreview {
            snapshot_id: snapshot_id.into(),
            path,
            file_count: stats.0,
            total_bytes: stats.1,
        })
    }

    /// Delete a Safety Snapshot. The plan token is the snapshot id; the
    /// same in-use check runs again at apply time.
    pub fn apply_delete_snapshot(&self, plan_token: &str) -> Result<(), FixtureRecoveryError> {
        let (path, _) = self.find_snapshot(plan_token)?;
        let ledger = self.app_state.load()?.recovery_ledger;
        if let Some(active) = &ledger.active {
            if active.snapshot_path.as_deref() == Some(path.as_path()) {
                return Err(FixtureRecoveryError::SnapshotInUse);
            }
        }
        let live = self.resolve_live_path()?;
        self.filesystem
            .remove_recovery_artifact(&path, &live)
            .map_err(|error| FixtureRecoveryError::FileSystem(error.to_string()))
    }

    // -- internals ---------------------------------------------------------

    fn recovery_context(&self) -> Result<(RecoveryMode, PathBuf), FixtureRecoveryError> {
        match self.bootstrap.inspect() {
            BootstrapSnapshot::FixtureRecoveryLocked {
                home_id: Some(home_id),
                path: Some(path),
            } => Ok((RecoveryMode::BoundRestore { home_id }, path)),
            BootstrapSnapshot::FixtureRecoveryLocked {
                home_id: None,
                path: Some(path),
            } => Ok((RecoveryMode::LegacyUnbound, path)),
            _ => Err(FixtureRecoveryError::NotLocked),
        }
    }

    /// The Home root the recovery may touch: the bound path when a binding
    /// exists, else the default (Legacy) path. Read-only resolution.
    fn resolve_live_path(&self) -> Result<PathBuf, FixtureRecoveryError> {
        let files = self.app_state.load()?;
        Ok(files
            .binding
            .current
            .map(|current| current.path)
            .unwrap_or_else(|| self.config.default_home_path.clone()))
    }

    fn advance(
        &self,
        ledger: &mut RecoveryLedgerFile,
        mut op: RecoveryOperationRecord,
    ) -> Result<RecoveryResult, FixtureRecoveryError> {
        loop {
            match self.converge(&op)? {
                Convergence::Snapshot => {
                    if let Err(error) = self.step_snapshot(ledger, &mut op) {
                        return self.fail_step(error, ledger, &mut op);
                    }
                }
                Convergence::Prepare => {
                    if let Err(error) = self.step_prepare(ledger, &mut op) {
                        return self.fail_step(error, ledger, &mut op);
                    }
                }
                Convergence::Validate => {
                    if let Err(error) = self.step_validate(ledger, &mut op) {
                        return self.fail_step(error, ledger, &mut op);
                    }
                }
                Convergence::Promote => {
                    if let Err(error) = self.step_promote(ledger, &mut op) {
                        return self.fail_step(error, ledger, &mut op);
                    }
                }
                Convergence::Verify => {
                    if let Err(error) = self.step_verify(ledger, &mut op) {
                        return self.fail_step(error, ledger, &mut op);
                    }
                }
                Convergence::Complete => {
                    return Ok(RecoveryResult {
                        operation_id: op.operation_id.clone(),
                        awaiting_commit: true,
                        rolled_back: false,
                    });
                }
                Convergence::Rollback => {
                    self.rollback(ledger, &mut op, false)?;
                    return Ok(RecoveryResult {
                        operation_id: op.operation_id.clone(),
                        awaiting_commit: false,
                        rolled_back: true,
                    });
                }
                Convergence::Ambiguous(message) => {
                    return Err(FixtureRecoveryError::AmbiguousState(message));
                }
            }
        }
    }

    /// A failed step rolls back every mutation it made before the failure
    /// surfaced; only then the failure is reported. An ambiguity keeps the
    /// operation active so the next start converges again.
    fn fail_step(
        &self,
        error: FixtureRecoveryError,
        ledger: &mut RecoveryLedgerFile,
        op: &mut RecoveryOperationRecord,
    ) -> Result<RecoveryResult, FixtureRecoveryError> {
        if let FixtureRecoveryError::StepFailed {
            cursor, message, ..
        } = &error
        {
            match self.rollback(ledger, op, false) {
                Ok(()) => Err(FixtureRecoveryError::StepFailed {
                    cursor: cursor.clone(),
                    message: message.clone(),
                    rolled_back: true,
                }),
                Err(rollback_error) => Err(FixtureRecoveryError::AmbiguousState(format!(
                    "{message}; rollback also failed: {rollback_error}"
                ))),
            }
        } else {
            Err(error)
        }
    }

    /// Deterministic convergence from the recorded cursor plus filesystem
    /// facts. The ledger write is the commit point of each step, so the only
    /// crash window is "mutation done, ledger not yet written" — the facts
    /// below make that window unambiguous.
    fn converge(&self, op: &RecoveryOperationRecord) -> Result<Convergence, FixtureRecoveryError> {
        let live = op.live_path.clone().ok_or_else(|| {
            FixtureRecoveryError::AmbiguousState("the operation has no live path".into())
        })?;
        let snapshot_path = sibling_path(&live, "snapshot", &op.operation_id);
        let prepared_path = sibling_path(&live, "prepared", &op.operation_id);
        let live_exists = self.is_dir(&live)?;
        let snapshot_exists = self.is_dir(&snapshot_path)?;
        let prepared_exists = self.is_dir(&prepared_path)?;
        match op.cursor.as_deref() {
            Some(cursors::CONFIRMED) => {
                if live_exists {
                    Ok(Convergence::Snapshot)
                } else if snapshot_exists {
                    // Snapshot rename done, ledger write lost: roll forward.
                    Ok(Convergence::Prepare)
                } else {
                    Ok(Convergence::Ambiguous(
                        "the live Home is missing at the confirmed cursor".into(),
                    ))
                }
            }
            Some(cursors::SNAPSHOTTED) | Some(cursors::PREPARED) => {
                if !snapshot_exists {
                    Ok(Convergence::Ambiguous(
                        "the Safety Snapshot is missing".into(),
                    ))
                } else if live_exists && prepared_exists {
                    Ok(Convergence::Ambiguous(
                        "both the live and prepared Homes exist".into(),
                    ))
                } else if live_exists {
                    // Promote rename done, ledger write lost: verify live.
                    Ok(Convergence::Verify)
                } else if prepared_exists {
                    Ok(Convergence::Validate)
                } else {
                    Ok(Convergence::Prepare)
                }
            }
            Some(cursors::VALIDATED) | Some(cursors::PROMOTED) => {
                if !snapshot_exists {
                    Ok(Convergence::Ambiguous(
                        "the Safety Snapshot is missing".into(),
                    ))
                } else if live_exists && prepared_exists {
                    Ok(Convergence::Ambiguous(
                        "both the live and prepared Homes exist".into(),
                    ))
                } else if live_exists {
                    Ok(Convergence::Verify)
                } else if prepared_exists {
                    Ok(Convergence::Promote)
                } else {
                    Ok(Convergence::Rollback)
                }
            }
            Some(cursors::VERIFIED) | Some(cursors::AWAITING_COMMIT) => {
                if live_exists {
                    Ok(Convergence::Complete)
                } else if snapshot_exists && !prepared_exists {
                    Ok(Convergence::Rollback)
                } else {
                    Ok(Convergence::Ambiguous(
                        "the verified live Home is missing".into(),
                    ))
                }
            }
            other => Ok(Convergence::Ambiguous(format!(
                "unknown recovery cursor {other:?}"
            ))),
        }
    }

    /// Quiesce proof: no other process may hold the Catalog WAL index. Our
    /// own probe connections are short-lived and closed before this runs.
    fn step_snapshot(
        &self,
        ledger: &mut RecoveryLedgerFile,
        op: &mut RecoveryOperationRecord,
    ) -> Result<(), FixtureRecoveryError> {
        let live = op.live_path.clone().ok_or_else(|| {
            FixtureRecoveryError::AmbiguousState("the operation has no live path".into())
        })?;
        let shm_path = PathBuf::from(format!(
            "{}-shm",
            live.join(&self.config.catalog_file_name).display()
        ));
        match self.filesystem.try_lock_wal_index_exclusive(&shm_path) {
            Ok(true) => {}
            Ok(false) => {
                return Err(FixtureRecoveryError::WriterActive(
                    "the SQLite WAL index is locked by another process".into(),
                ));
            }
            Err(error) => {
                return Err(FixtureRecoveryError::WriterActive(error.to_string()));
            }
        }
        let snapshot_path = sibling_path(&live, "snapshot", &op.operation_id);
        self.filesystem
            .rename_directory(&live, &snapshot_path)
            .map_err(|error| FixtureRecoveryError::StepFailed {
                cursor: cursors::SNAPSHOTTED.into(),
                message: format!("could not isolate the live Home: {error}"),
                rolled_back: false,
            })?;
        self.fsync_parent(&live)?;
        op.snapshot_path = Some(snapshot_path);
        op.cursor = Some(cursors::SNAPSHOTTED.into());
        self.persist(ledger, op)
    }

    fn step_prepare(
        &self,
        ledger: &mut RecoveryLedgerFile,
        op: &mut RecoveryOperationRecord,
    ) -> Result<(), FixtureRecoveryError> {
        let live = op.live_path.clone().ok_or_else(|| {
            FixtureRecoveryError::AmbiguousState("the operation has no live path".into())
        })?;
        let prepared_path = sibling_path(&live, "prepared", &op.operation_id);
        if self.is_dir(&prepared_path)? {
            return Err(FixtureRecoveryError::AmbiguousState(
                "the prepared Home already exists".into(),
            ));
        }
        self.filesystem
            .create_directory_all(&prepared_path)
            .map_err(|error| FixtureRecoveryError::StepFailed {
                cursor: cursors::PREPARED.into(),
                message: format!("could not create the prepared Home: {error}"),
                rolled_back: false,
            })?;
        for directory in ["skills", "remotes", "operations", "cache", "staging"] {
            self.filesystem
                .create_directory_all(&prepared_path.join(directory))
                .map_err(|error| FixtureRecoveryError::StepFailed {
                    cursor: cursors::PREPARED.into(),
                    message: format!("could not create {directory}: {error}"),
                    rolled_back: false,
                })?;
        }
        let identity = self.prepared_identity(op)?;
        self.prepared
            .create_prepared(
                &prepared_path.join(&self.config.catalog_file_name),
                identity.as_ref(),
            )
            .map_err(|error| FixtureRecoveryError::StepFailed {
                cursor: cursors::PREPARED.into(),
                message: format!("could not create the prepared Catalog: {error}"),
                rolled_back: false,
            })?;
        if let Some(identity) = &identity {
            let marker = HomeMarker {
                schema_version: HomeMarker::SCHEMA_VERSION,
                home_id: identity.home_id.clone(),
                volume_fsid: identity.volume_fsid.clone(),
                volume_uuid: identity.volume_uuid.clone(),
                created_at: identity.home_bound_at.clone(),
            };
            let json = serde_json::to_string_pretty(&marker).map_err(|error| {
                FixtureRecoveryError::StepFailed {
                    cursor: cursors::PREPARED.into(),
                    message: format!("could not serialize the Home marker: {error}"),
                    rolled_back: false,
                }
            })?;
            self.filesystem
                .write_utf8_file(&prepared_path.join(HomeMarker::FILE_NAME), &json)
                .map_err(|error| FixtureRecoveryError::StepFailed {
                    cursor: cursors::PREPARED.into(),
                    message: format!("could not write the Home marker: {error}"),
                    rolled_back: false,
                })?;
        }
        op.prepared_path = Some(prepared_path);
        op.cursor = Some(cursors::PREPARED.into());
        self.persist(ledger, op)
    }

    fn step_validate(
        &self,
        ledger: &mut RecoveryLedgerFile,
        op: &mut RecoveryOperationRecord,
    ) -> Result<(), FixtureRecoveryError> {
        if let Err(error) = self.validate_prepared(op) {
            return Err(FixtureRecoveryError::StepFailed {
                cursor: cursors::VALIDATED.into(),
                message: error,
                rolled_back: false,
            });
        }
        op.cursor = Some(cursors::VALIDATED.into());
        self.persist(ledger, op)
    }

    fn step_promote(
        &self,
        ledger: &mut RecoveryLedgerFile,
        op: &mut RecoveryOperationRecord,
    ) -> Result<(), FixtureRecoveryError> {
        let live = op.live_path.clone().ok_or_else(|| {
            FixtureRecoveryError::AmbiguousState("the operation has no live path".into())
        })?;
        let prepared_path = op.prepared_path.clone().ok_or_else(|| {
            FixtureRecoveryError::AmbiguousState("the prepared Home path is unknown".into())
        })?;
        if let Err(message) = self.validate_prepared(op) {
            return Err(FixtureRecoveryError::StepFailed {
                cursor: cursors::PROMOTED.into(),
                message,
                rolled_back: false,
            });
        }
        self.filesystem
            .rename_directory(&prepared_path, &live)
            .map_err(|error| FixtureRecoveryError::StepFailed {
                cursor: cursors::PROMOTED.into(),
                message: format!("could not promote the prepared Home: {error}"),
                rolled_back: false,
            })?;
        self.fsync_parent(&live)?;
        op.cursor = Some(cursors::PROMOTED.into());
        op.commit_point = Some(cursors::PROMOTED.into());
        self.persist(ledger, op)
    }

    fn step_verify(
        &self,
        ledger: &mut RecoveryLedgerFile,
        op: &mut RecoveryOperationRecord,
    ) -> Result<(), FixtureRecoveryError> {
        if let Err(message) = self.verify_live(op) {
            return Err(FixtureRecoveryError::StepFailed {
                cursor: cursors::VERIFIED.into(),
                message,
                rolled_back: false,
            });
        }
        op.cursor = Some(cursors::VERIFIED.into());
        self.persist(ledger, op)
    }

    /// Validate the prepared Home offline: Catalog integrity/foreign keys,
    /// identity per mode, fixture absence, manifest and external probes.
    /// Records the manifest on first validation; later runs must match it.
    fn validate_prepared(&self, op: &mut RecoveryOperationRecord) -> Result<(), String> {
        let prepared = op
            .prepared_path
            .as_ref()
            .ok_or_else(|| "the prepared Home path is unknown".to_string())?;
        self.validate_prepared_shape(prepared, op.home_id.is_some())?;
        self.verify_home_content(prepared, op)?;
        if let Some(recorded) = &op.manifest_hash {
            let manifest = self.manifest_of(prepared)?;
            if recorded != &manifest {
                return Err("the prepared tree changed since it was validated".into());
            }
        } else {
            op.manifest_hash = Some(self.manifest_of(prepared)?);
        }
        if op.external_probe.is_none() {
            // First validation records the "before" probe; every later run
            // must see the same external state. The ledger write that
            // persists it may have been lost in a crash — recording at the
            // first validation the operation reaches is the earliest
            // provable point (the snapshot only touched the live sibling).
            op.external_probe = Some(
                self.external_probe_json()
                    .map_err(|error| error.to_string())?,
            );
        }
        self.check_external_probe(op)?;
        Ok(())
    }

    /// The prepared Home must have exactly the fresh shape the recovery
    /// builds: the five layout directories (each empty), the Catalog with
    /// its derived SQLite sidecars and — in Bound mode — the Home marker.
    /// Any extra entry means the prepared tree was tampered with or the
    /// build was interrupted by foreign writes; never promote it.
    fn validate_prepared_shape(&self, prepared: &Path, bound: bool) -> Result<(), String> {
        let catalog = &self.config.catalog_file_name;
        let mut allowed_files = vec![
            catalog.clone(),
            format!("{catalog}-wal"),
            format!("{catalog}-shm"),
        ];
        if bound {
            allowed_files.push(HomeMarker::FILE_NAME.into());
        }
        let layout = ["skills", "remotes", "operations", "cache", "staging"];
        let allowed_dirs: Vec<&str> = layout.to_vec();
        let mut seen_dirs = Vec::new();
        for entry in self
            .filesystem
            .list_directory(prepared)
            .map_err(|error| format!("could not inspect the prepared Home: {error}"))?
        {
            if entry.is_directory {
                seen_dirs.push(entry.name.clone());
                if !allowed_dirs.contains(&entry.name.as_str()) {
                    return Err(format!(
                        "the prepared Home contains an unexpected directory: {}",
                        entry.name
                    ));
                }
                let children = self
                    .filesystem
                    .list_directory(&prepared.join(&entry.name))
                    .map_err(|error| {
                        format!("could not inspect prepared {}/: {error}", entry.name)
                    })?;
                if !children.is_empty() {
                    return Err(format!(
                        "the prepared Home's {} directory is not empty",
                        entry.name
                    ));
                }
            } else if !allowed_files.iter().any(|name| name == &entry.name) {
                return Err(format!(
                    "the prepared Home contains an unexpected file: {}",
                    entry.name
                ));
            }
        }
        for directory in layout {
            if !seen_dirs.contains(&directory.to_string()) {
                return Err(format!(
                    "the prepared Home is missing the {directory} directory"
                ));
            }
        }
        Ok(())
    }

    /// Verify the live Home after promote: the same checks plus manifest
    /// equality against the recorded one.
    fn verify_live(&self, op: &RecoveryOperationRecord) -> Result<(), String> {
        let live = op
            .live_path
            .as_ref()
            .ok_or_else(|| "the live Home path is unknown".to_string())?;
        self.verify_home_content(live, op)?;
        let recorded = op.manifest_hash.as_ref().ok_or_else(|| {
            "the operation has no recorded manifest; refusing to accept the live Home".to_string()
        })?;
        let manifest = self.manifest_of(live)?;
        if recorded != &manifest {
            return Err("the live tree does not match the validated manifest".into());
        }
        self.check_external_probe(op)?;
        Ok(())
    }

    /// The validation both the prepared and the promoted live Home must
    /// pass: Catalog present with the current schema and clean
    /// integrity/foreign keys, identity per mode, and zero fixture
    /// footprint. Read-only throughout.
    fn verify_home_content(&self, root: &Path, op: &RecoveryOperationRecord) -> Result<(), String> {
        let report = self
            .probe
            .probe(&root.join(&self.config.catalog_file_name))
            .map_err(|error| format!("Catalog unreadable: {error}"))?;
        if !report.exists
            || report.schema_version
                != Some(crate::seams::catalog_probe::CURRENT_CATALOG_SCHEMA_VERSION)
        {
            return Err("the Catalog is missing or has an unknown schema".into());
        }
        if !report.integrity_ok || !report.foreign_keys_ok {
            return Err("the Catalog failed integrity or foreign-key checks".into());
        }
        let expected = self
            .prepared_identity(op)
            .map_err(|error| error.to_string())?;
        match (&expected, &report.home_identity) {
            (Some(expected), Some(actual))
                if actual.home_id == expected.home_id
                    && actual.volume_fsid == expected.volume_fsid
                    && actual.volume_uuid == expected.volume_uuid => {}
            (Some(_), _) => {
                return Err("the Catalog identity does not match the binding".into());
            }
            (None, None) => {}
            (None, Some(_)) => {
                return Err("the Catalog unexpectedly carries identity".into());
            }
        }
        let shape_mode = match op.home_id {
            Some(_) => FixtureShapeMode::Bound,
            None => FixtureShapeMode::Legacy,
        };
        let collector =
            FixtureEvidenceCollector::new(self.probe.as_ref(), self.filesystem.as_ref());
        let (db, tree) = collector
            .collect(root, &self.config.catalog_file_name)
            .map_err(|error| format!("Home evidence unreadable: {error}"))?;
        if !matches!(
            classify_fixture(&db, &tree, root, shape_mode),
            FixtureClassification::Clean
        ) {
            return Err("the Home still carries a fixture footprint".into());
        }
        Ok(())
    }

    /// Roll back every mutation the operation made and restore the original
    /// Home from the Safety Snapshot. The snapshot is never deleted here;
    /// only the app-created prepared artifact is removed.
    fn rollback(
        &self,
        ledger: &mut RecoveryLedgerFile,
        op: &mut RecoveryOperationRecord,
        trust_live_as_prepared: bool,
    ) -> Result<(), FixtureRecoveryError> {
        let live = op.live_path.clone().ok_or_else(|| {
            FixtureRecoveryError::AmbiguousState("the operation has no live path".into())
        })?;
        let snapshot = op
            .snapshot_path
            .clone()
            .unwrap_or_else(|| sibling_path(&live, "snapshot", &op.operation_id));
        let prepared = op
            .prepared_path
            .clone()
            .unwrap_or_else(|| sibling_path(&live, "prepared", &op.operation_id));
        let live_exists = self.is_dir(&live)?;
        let prepared_exists = self.is_dir(&prepared)?;
        let snapshot_exists = self.is_dir(&snapshot)?;

        if op.cursor.as_deref() == Some(cursors::CONFIRMED)
            && !live_exists
            && !prepared_exists
            && !snapshot_exists
        {
            // Nothing was mutated (e.g. quiesce refused): just clear.
            self.finish_rollback(ledger, op)
        } else if op.cursor.as_deref() == Some(cursors::CONFIRMED) {
            if live_exists {
                // Nothing was mutated: quiesce or the snapshot rename failed
                // before any change. Just clear the operation.
                self.finish_rollback(ledger, op)
            } else {
                self.remove_prepared_if_ours(&prepared, &live)?;
                if snapshot_exists {
                    self.filesystem
                        .rename_directory(&snapshot, &live)
                        .map_err(|error| {
                            FixtureRecoveryError::AmbiguousState(format!(
                                "could not restore the Safety Snapshot: {error}"
                            ))
                        })?;
                    self.fsync_parent(&live)?;
                    self.finish_rollback(ledger, op)
                } else {
                    Err(FixtureRecoveryError::AmbiguousState(
                        "the live Home is missing and no Safety Snapshot exists; cannot restore"
                            .into(),
                    ))
                }
            }
        } else {
            // A later cursor: the live Home (if present) must be provably our
            // prepared Home before it is moved back.
            if live_exists {
                if !trust_live_as_prepared {
                    // Crash-resume rollback: only move content that is
                    // provably our validated prepared Home. A user-replaced
                    // directory must never be deleted.
                    let recorded = op.manifest_hash.clone().ok_or_else(|| {
                        FixtureRecoveryError::AmbiguousState(
                            "live content predates validation; refusing rollback over it".into(),
                        )
                    })?;
                    let manifest = self
                        .manifest_of(&live)
                        .map_err(|error| FixtureRecoveryError::AmbiguousState(error.to_string()))?;
                    if recorded != manifest {
                        return Err(FixtureRecoveryError::AmbiguousState(
                            "live content is not the validated prepared Home; refusing rollback"
                                .into(),
                        ));
                    }
                }
                self.filesystem
                    .rename_directory(&live, &prepared)
                    .map_err(|error| {
                        FixtureRecoveryError::AmbiguousState(format!(
                            "could not move the live Home back: {error}"
                        ))
                    })?;
            }
            self.remove_prepared_if_ours(&prepared, &live)?;
            if !snapshot_exists {
                return Err(FixtureRecoveryError::AmbiguousState(
                    "the Safety Snapshot is missing; cannot restore the original Home".into(),
                ));
            }
            self.filesystem
                .rename_directory(&snapshot, &live)
                .map_err(|error| {
                    FixtureRecoveryError::AmbiguousState(format!(
                        "could not restore the Safety Snapshot: {error}"
                    ))
                })?;
            self.fsync_parent(&live)?;
            self.finish_rollback(ledger, op)
        }
    }

    fn remove_prepared_if_ours(
        &self,
        prepared: &Path,
        live: &Path,
    ) -> Result<(), FixtureRecoveryError> {
        if self.is_dir(prepared)? {
            self.filesystem
                .remove_recovery_artifact(prepared, live)
                .map_err(|error| {
                    FixtureRecoveryError::AmbiguousState(format!(
                        "could not remove the prepared artifact: {error}"
                    ))
                })?;
        }
        Ok(())
    }

    fn finish_rollback(
        &self,
        ledger: &mut RecoveryLedgerFile,
        op: &mut RecoveryOperationRecord,
    ) -> Result<(), FixtureRecoveryError> {
        op.cursor = Some(cursors::ROLLED_BACK.into());
        op.commit_point = Some(cursors::ROLLED_BACK.into());
        ledger.active = None;
        ledger.completed.push(op.clone());
        self.app_state.write_recovery_ledger(ledger)?;
        Ok(())
    }

    fn persist(
        &self,
        ledger: &mut RecoveryLedgerFile,
        op: &RecoveryOperationRecord,
    ) -> Result<(), FixtureRecoveryError> {
        ledger.active = Some(op.clone());
        self.app_state.write_recovery_ledger(ledger)?;
        Ok(())
    }

    /// The identity the prepared Home must carry: the binding's identity in
    /// Bound Restore mode, `None` in Legacy mode.
    fn prepared_identity(
        &self,
        op: &RecoveryOperationRecord,
    ) -> Result<Option<CatalogHomeIdentity>, FixtureRecoveryError> {
        if op.home_id.is_none() {
            return Ok(None);
        }
        let files = self.app_state.load()?;
        let current = files.binding.current.ok_or_else(|| {
            FixtureRecoveryError::AmbiguousState(
                "the bound operation has no current binding".into(),
            )
        })?;
        if current.home_id != *op.home_id.as_ref().unwrap() {
            return Err(FixtureRecoveryError::AmbiguousState(
                "the binding identity changed during recovery".into(),
            ));
        }
        Ok(Some(CatalogHomeIdentity {
            home_id: current.home_id,
            volume_fsid: current.volume_fsid,
            volume_uuid: current.volume_uuid,
            home_bound_at: current.bound_at,
        }))
    }

    fn manifest_of(&self, root: &Path) -> Result<String, String> {
        let catalog = &self.config.catalog_file_name;
        self.filesystem
            .tree_hash_excluding(root, &[format!("{catalog}-wal"), format!("{catalog}-shm")])
            .map_err(|error| format!("could not hash {root:?}: {error}"))
    }

    fn external_probe_json(&self) -> Result<String, FixtureRecoveryError> {
        let entries = self.external_probe()?;
        serde_json::to_string(&entries)
            .map_err(|error| FixtureRecoveryError::FileSystem(error.to_string()))
    }

    /// Targeted lstat probe of the app state directory (external to any
    /// Home): every entry except the recovery ledger itself and its tmp
    /// file, as (name, length) pairs.
    fn external_probe(&self) -> Result<Vec<(String, u64)>, FixtureRecoveryError> {
        let mut entries: Vec<(String, u64)> = self
            .filesystem
            .list_directory(&self.config.state_dir)
            .map_err(|error| FixtureRecoveryError::FileSystem(error.to_string()))?
            .into_iter()
            .filter(|entry| {
                entry.name != RECOVERY_LEDGER_FILE_NAME
                    && entry.name != format!("{RECOVERY_LEDGER_FILE_NAME}.tmp")
            })
            .map(|entry| (entry.name, entry.len))
            .collect();
        entries.sort();
        Ok(entries)
    }

    fn check_external_probe(&self, op: &RecoveryOperationRecord) -> Result<(), String> {
        let recorded: Vec<(String, u64)> = op
            .external_probe
            .as_ref()
            .and_then(|json| serde_json::from_str(json).ok())
            .ok_or_else(|| "the operation has no recorded external probe".to_string())?;
        let current = self.external_probe().map_err(|error| error.to_string())?;
        if recorded != current {
            return Err("the external app state changed during recovery".into());
        }
        Ok(())
    }

    fn find_snapshot(
        &self,
        snapshot_id: &str,
    ) -> Result<(PathBuf, (u64, u64)), FixtureRecoveryError> {
        let live = self.resolve_live_path()?;
        let parent = live.parent().ok_or_else(|| {
            FixtureRecoveryError::AmbiguousState("the Home path has no parent".into())
        })?;
        let home_name = live
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| {
                FixtureRecoveryError::AmbiguousState("the Home path has no file name".into())
            })?;
        let expected_prefix = format!("{home_name}.snapshot-");
        let op_id = snapshot_id.strip_prefix(&expected_prefix).ok_or_else(|| {
            FixtureRecoveryError::AmbiguousState(format!(
                "{snapshot_id} is not a Safety Snapshot of {home_name}"
            ))
        })?;
        if op_id.is_empty() || op_id.contains('/') {
            return Err(FixtureRecoveryError::AmbiguousState(
                "invalid Safety Snapshot id".into(),
            ));
        }
        let path = parent.join(snapshot_id);
        if !self.is_dir(&path)? {
            return Err(FixtureRecoveryError::AmbiguousState(format!(
                "the Safety Snapshot {snapshot_id} does not exist"
            )));
        }
        let stats = self.tree_stats(&path)?;
        Ok((path, stats))
    }

    fn tree_stats(&self, root: &Path) -> Result<(u64, u64), FixtureRecoveryError> {
        let mut file_count = 0_u64;
        let mut total_bytes = 0_u64;
        let mut stack = vec![root.to_path_buf()];
        while let Some(directory) = stack.pop() {
            for entry in self
                .filesystem
                .list_directory(&directory)
                .map_err(|error| FixtureRecoveryError::FileSystem(error.to_string()))?
            {
                if entry.is_directory {
                    stack.push(directory.join(&entry.name));
                } else {
                    file_count += 1;
                    total_bytes = total_bytes.saturating_add(entry.len);
                }
            }
        }
        Ok((file_count, total_bytes))
    }

    fn is_dir(&self, path: &Path) -> Result<bool, FixtureRecoveryError> {
        self.filesystem
            .path_is_directory(path)
            .map_err(|error| FixtureRecoveryError::FileSystem(error.to_string()))
    }

    fn fsync_parent(&self, live: &Path) -> Result<(), FixtureRecoveryError> {
        let parent = live.parent().ok_or_else(|| {
            FixtureRecoveryError::AmbiguousState("the Home path has no parent".into())
        })?;
        self.filesystem.fsync_directory(parent).map_err(|error| {
            FixtureRecoveryError::AmbiguousState(format!("parent fsync failed: {error}"))
        })
    }
}

enum Convergence {
    Snapshot,
    Prepare,
    Validate,
    Promote,
    Verify,
    Complete,
    Rollback,
    Ambiguous(String),
}

fn sibling_path(live: &Path, kind: &str, operation_id: &str) -> PathBuf {
    let name = live
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    live.parent()
        .unwrap_or_else(|| Path::new("/"))
        .join(format!("{name}.{kind}-{operation_id}"))
}

fn new_operation_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("fr-{nanos}-{}", std::process::id())
}

fn rfc3339_now() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    epoch_seconds_to_rfc3339(seconds)
}

/// Spec §5.3: persisted times may be integer epoch; the ledger records
/// RFC 3339. Shared with the DTO layer (single source of truth).
pub(crate) fn epoch_seconds_to_rfc3339(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant's civil-from-days algorithm (proleptic Gregorian calendar).
pub(crate) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    use crate::core::home::VolumeIdentity;
    use crate::seams::catalog_probe::{CatalogProbeError, CatalogProbeReport};
    use crate::seams::filesystem::FileSystem;

    fn home_root() -> &'static Path {
        Path::new("/Users/demo/Library/Application Support/skill-man")
    }

    struct FixedClassifier(FixtureClassification);

    impl FixtureClassifier for FixedClassifier {
        fn classify(&self, _home_root: &Path, _mode: FixtureShapeMode) -> FixtureClassification {
            self.0.clone()
        }
    }

    /// The exact fixture Catalog evidence, as the real production fixture
    /// reads back (verified against the machine's contaminated Legacy Home).
    fn fixture_db() -> FixtureCatalogEvidence {
        let entity = |name: &str| {
            home_root()
                .join(FIXTURE_ENTITIES_DIR)
                .join(name)
                .to_string_lossy()
                .into_owned()
        };
        FixtureCatalogEvidence {
            tables: vec![
                "activations".into(),
                "agents".into(),
                "catalog_meta".into(),
                "file_sources".into(),
                "preferences".into(),
                "remote_sources".into(),
                "skills".into(),
            ],
            meta: Some(crate::seams::catalog_probe::FixtureCatalogMetaEvidence {
                schema_version: 4,
                first_run_completed_at: None,
            }),
            skills: vec![
                FixtureSkillRowEvidence {
                    id: "skill-authoring".into(),
                    directory_name: "skill-authoring".into(),
                    identity_key: "skill-authoring".into(),
                    display_name: "Skill authoring".into(),
                    description: "A precise workflow for building maintainable Agent Skills."
                        .into(),
                    source_kind: "link".into(),
                    library_entry_path: None,
                    final_entity_path: entity("skill-authoring"),
                    recorded_content_hash: None,
                    health: "healthy".into(),
                    created_at: "2026-07-20T10:42:00Z".into(),
                    updated_at: "2026-07-20T10:42:00Z".into(),
                },
                FixtureSkillRowEvidence {
                    id: "media-xray".into(),
                    directory_name: "media-xray".into(),
                    identity_key: "media-xray".into(),
                    display_name: "Media X-ray".into(),
                    description: "Transcribes and inspects local audio and video.".into(),
                    source_kind: "remote_install".into(),
                    library_entry_path: Some(entity("media-xray")),
                    final_entity_path: entity("media-xray"),
                    recorded_content_hash: None,
                    health: "healthy".into(),
                    created_at: "2026-07-19T14:08:00Z".into(),
                    updated_at: "2026-07-19T14:08:00Z".into(),
                },
                FixtureSkillRowEvidence {
                    id: "legacy-audit".into(),
                    directory_name: "legacy-audit".into(),
                    identity_key: "legacy-audit".into(),
                    display_name: "Legacy audit".into(),
                    description: "Checks an existing skills directory before Adopt.".into(),
                    source_kind: "link".into(),
                    library_entry_path: None,
                    final_entity_path: entity("legacy-audit"),
                    recorded_content_hash: None,
                    health: "broken".into(),
                    created_at: "2026-07-18T03:16:00Z".into(),
                    updated_at: "2026-07-18T03:16:00Z".into(),
                },
            ],
            agents: vec![
                FixtureAgentRowEvidence {
                    id: "claude-code".into(),
                    name: "Claude Code".into(),
                    kind: "claude_preset".into(),
                    skills_path: "~/.claude/skills".into(),
                    path_identity_key: "~/.claude/skills".into(),
                    detected: true,
                    compatibility: "verified".into(),
                    created_at: "1970-01-01T00:00:00Z".into(),
                    updated_at: "1970-01-01T00:00:00Z".into(),
                },
                FixtureAgentRowEvidence {
                    id: "codex".into(),
                    name: "Codex".into(),
                    kind: "codex_preset".into(),
                    skills_path: "~/.codex/skills".into(),
                    path_identity_key: "~/.codex/skills".into(),
                    detected: true,
                    compatibility: "verified".into(),
                    created_at: "1970-01-01T00:00:00Z".into(),
                    updated_at: "1970-01-01T00:00:00Z".into(),
                },
                FixtureAgentRowEvidence {
                    id: "workbench".into(),
                    name: "Workbench".into(),
                    kind: "custom".into(),
                    skills_path: "~/Library/Application Support/workbench/skills".into(),
                    path_identity_key: "~/library/application support/workbench/skills".into(),
                    detected: true,
                    compatibility: "unknown".into(),
                    created_at: "1970-01-01T00:00:00Z".into(),
                    updated_at: "1970-01-01T00:00:00Z".into(),
                },
            ],
            preferences: Some(FixturePreferencesEvidence {
                launch_at_login: false,
                show_in_dock: true,
                check_app_updates: true,
                check_skill_updates: true,
                last_app_update_check_at: None,
                last_skill_update_check_at: None,
            }),
            activation_count: 0,
            file_source_count: 0,
            remote_source_count: 0,
        }
    }

    fn fixture_tree() -> FixtureTreeEvidence {
        FixtureTreeEvidence {
            fixture_entities_exists: true,
            skill_authoring_exists: true,
            skill_authoring_hash: Some(FIXTURE_TREE_HASH_SKILL_AUTHORING.into()),
            media_xray_exists: true,
            media_xray_hash: Some(FIXTURE_TREE_HASH_MEDIA_XRAY.into()),
            root_hash: Some(FIXTURE_TREE_HASH_ROOT.into()),
            legacy_audit_entity_exists: false,
        }
    }

    #[test]
    fn exact_fixture_footprint_is_pure() {
        assert_eq!(
            classify_fixture(
                &fixture_db(),
                &fixture_tree(),
                home_root(),
                FixtureShapeMode::Legacy
            ),
            FixtureClassification::Pure
        );
    }

    #[test]
    fn bound_mode_accepts_the_current_schema_fixture_shape() {
        let mut db = fixture_db();
        db.meta = Some(crate::seams::catalog_probe::FixtureCatalogMetaEvidence {
            schema_version: crate::seams::catalog_probe::CURRENT_CATALOG_SCHEMA_VERSION,
            first_run_completed_at: None,
        });
        db.tables = vec![
            "activations".into(),
            "agents".into(),
            "catalog_meta".into(),
            "file_sources".into(),
            "preferences".into(),
            "remote_bindings".into(),
            "remote_source_aliases".into(),
            "remote_source_parents".into(),
            "skills".into(),
        ];
        assert_eq!(
            classify_fixture(&db, &fixture_tree(), home_root(), FixtureShapeMode::Bound),
            FixtureClassification::Pure
        );
        // The same Catalog is a modified fact for an unbound Legacy shape.
        match classify_fixture(&db, &fixture_tree(), home_root(), FixtureShapeMode::Legacy) {
            FixtureClassification::Mixed { reasons } => {
                assert!(reasons.contains(&format!(
                    "unknown_schema:{}",
                    crate::seams::catalog_probe::CURRENT_CATALOG_SCHEMA_VERSION
                )));
            }
            other => panic!("expected Mixed for the current schema in Legacy mode, got {other:?}"),
        }
    }

    #[test]
    fn no_footprint_is_clean() {
        let clean_db = FixtureCatalogEvidence {
            tables: vec!["skills".into()],
            skills: vec![FixtureSkillRowEvidence {
                id: "real-skill".into(),
                directory_name: "real-skill".into(),
                identity_key: "real-skill".into(),
                display_name: "Real skill".into(),
                description: String::new(),
                source_kind: "remote_install".into(),
                library_entry_path: Some("/home/skills/real-skill".into()),
                final_entity_path: "/home/skills/real-skill".into(),
                recorded_content_hash: None,
                health: "healthy".into(),
                created_at: "2026-08-01T00:00:00Z".into(),
                updated_at: "2026-08-01T00:00:00Z".into(),
            }],
            ..FixtureCatalogEvidence::clean()
        };
        assert_eq!(
            classify_fixture(
                &clean_db,
                &FixtureTreeEvidence::clean(),
                home_root(),
                FixtureShapeMode::Legacy
            ),
            FixtureClassification::Clean
        );
    }

    #[test]
    fn a_real_skill_named_like_a_fixture_entity_is_not_a_footprint() {
        // A genuine user skill at `skills/skill-authoring` is not fixture
        // lineage: its final entity path is outside `fixture-entities`.
        let db = FixtureCatalogEvidence {
            tables: vec!["skills".into()],
            skills: vec![FixtureSkillRowEvidence {
                id: "skill-authoring".into(),
                directory_name: "skill-authoring".into(),
                identity_key: "skill-authoring".into(),
                display_name: "Skill authoring".into(),
                description: String::new(),
                source_kind: "link".into(),
                library_entry_path: None,
                final_entity_path: "/home/skills/skill-authoring".into(),
                recorded_content_hash: None,
                health: "healthy".into(),
                created_at: "2026-08-01T00:00:00Z".into(),
                updated_at: "2026-08-01T00:00:00Z".into(),
            }],
            ..FixtureCatalogEvidence::clean()
        };
        assert_eq!(
            classify_fixture(
                &db,
                &FixtureTreeEvidence::clean(),
                home_root(),
                FixtureShapeMode::Legacy
            ),
            FixtureClassification::Clean
        );
    }

    #[test]
    fn modified_fixture_row_is_mixed() {
        let mut db = fixture_db();
        db.skills[1].updated_at = "2026-08-13T10:07:02.781Z".into();
        let classification =
            classify_fixture(&db, &fixture_tree(), home_root(), FixtureShapeMode::Legacy);
        match classification {
            FixtureClassification::Mixed { reasons } => {
                assert!(reasons.contains(&"skill_row_modified:media-xray".into()));
                assert_eq!(reasons.len(), 1, "only the modified fact deviates");
            }
            other => panic!("expected Mixed, got {other:?}"),
        }
    }

    #[test]
    fn runtime_meta_fields_do_not_disqualify() {
        // snapshot_version / last_startup_check_at are rewritten by the old
        // app on every launch; they are meta fields, never evidence.
        let mut db = fixture_db();
        db.meta = Some(crate::seams::catalog_probe::FixtureCatalogMetaEvidence {
            schema_version: 4,
            first_run_completed_at: None,
        });
        assert_eq!(
            classify_fixture(&db, &fixture_tree(), home_root(), FixtureShapeMode::Legacy),
            FixtureClassification::Pure
        );
    }

    #[test]
    fn unknown_schema_is_mixed() {
        let mut db = fixture_db();
        db.meta = Some(crate::seams::catalog_probe::FixtureCatalogMetaEvidence {
            schema_version: 5,
            first_run_completed_at: None,
        });
        let classification =
            classify_fixture(&db, &fixture_tree(), home_root(), FixtureShapeMode::Legacy);
        match classification {
            FixtureClassification::Mixed { reasons } => {
                assert!(reasons.contains(&"unknown_schema:5".into()));
            }
            other => panic!("expected Mixed, got {other:?}"),
        }
    }

    #[test]
    fn activation_rows_are_mixed() {
        let mut db = fixture_db();
        db.activation_count = 2;
        let classification =
            classify_fixture(&db, &fixture_tree(), home_root(), FixtureShapeMode::Legacy);
        match classification {
            FixtureClassification::Mixed { reasons } => {
                assert!(reasons.contains(&"activation_rows:2".into()));
            }
            other => panic!("expected Mixed, got {other:?}"),
        }
    }

    #[test]
    fn extra_skill_row_is_mixed() {
        let mut db = fixture_db();
        let mut row = db.skills[0].clone();
        row.id = "user-skill".into();
        row.identity_key = "user-skill".into();
        row.directory_name = "user-skill".into();
        row.display_name = "User skill".into();
        row.final_entity_path = "/home/skills/user-skill".into();
        db.skills.push(row);
        let classification =
            classify_fixture(&db, &fixture_tree(), home_root(), FixtureShapeMode::Legacy);
        match classification {
            FixtureClassification::Mixed { reasons } => {
                assert!(reasons.contains(&"extra_skill_row:user-skill".into()));
            }
            other => panic!("expected Mixed, got {other:?}"),
        }
    }

    #[test]
    fn missing_tree_hash_is_mixed() {
        let mut tree = fixture_tree();
        tree.skill_authoring_hash = Some("tree-sha256-v1:deadbeef".into());
        let classification =
            classify_fixture(&fixture_db(), &tree, home_root(), FixtureShapeMode::Legacy);
        match classification {
            FixtureClassification::Mixed { reasons } => {
                assert!(reasons.contains(&"tree_hash_mismatch:skill-authoring".into()));
            }
            other => panic!("expected Mixed, got {other:?}"),
        }
    }

    #[test]
    fn missing_entity_tree_is_mixed() {
        let mut tree = fixture_tree();
        tree.skill_authoring_exists = false;
        tree.skill_authoring_hash = None;
        let classification =
            classify_fixture(&fixture_db(), &tree, home_root(), FixtureShapeMode::Legacy);
        match classification {
            FixtureClassification::Mixed { reasons } => {
                assert!(reasons.contains(&"missing_tree:skill-authoring".into()));
            }
            other => panic!("expected Mixed, got {other:?}"),
        }
    }

    #[test]
    fn legacy_audit_entity_directory_is_mixed() {
        let mut tree = fixture_tree();
        tree.legacy_audit_entity_exists = true;
        let classification =
            classify_fixture(&fixture_db(), &tree, home_root(), FixtureShapeMode::Legacy);
        match classification {
            FixtureClassification::Mixed { reasons } => {
                assert!(reasons.contains(&"legacy_audit_entity_present".into()));
            }
            other => panic!("expected Mixed, got {other:?}"),
        }
    }

    #[test]
    fn fixture_rows_without_trees_are_mixed_not_pure() {
        let mut tree = fixture_tree();
        tree.fixture_entities_exists = false;
        tree.skill_authoring_exists = false;
        tree.skill_authoring_hash = None;
        tree.media_xray_exists = false;
        tree.media_xray_hash = None;
        tree.root_hash = None;
        let classification =
            classify_fixture(&fixture_db(), &tree, home_root(), FixtureShapeMode::Legacy);
        match classification {
            FixtureClassification::Mixed { reasons } => {
                assert!(reasons.contains(&"fixture_entities_missing".into()));
            }
            other => panic!("expected Mixed, got {other:?}"),
        }
    }

    // -- Restore eligibility & plan (spec §5.5, ADR-0012 §6) ----------------

    const RESTORE_HOME_ID: &str = "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab";

    use crate::seams::app_state_store::{AppStateFiles, HomeBindingFile, HomeBindingRecord};
    use crate::seams::volume_identity::{VolumeIdentityError, VolumeIdentitySource};

    struct MemoryStateStore(Mutex<AppStateFiles>);

    impl AppStateStore for MemoryStateStore {
        fn load(&self) -> Result<AppStateFiles, AppStateStoreError> {
            Ok(self.0.lock().unwrap().clone())
        }

        fn write_locator(&self, binding: &HomeBindingFile) -> Result<(), AppStateStoreError> {
            self.0.lock().unwrap().binding = binding.clone();
            Ok(())
        }

        fn write_recovery_ledger(
            &self,
            ledger: &RecoveryLedgerFile,
        ) -> Result<(), AppStateStoreError> {
            self.0.lock().unwrap().recovery_ledger = ledger.clone();
            Ok(())
        }
    }

    struct FixedVolume(Option<VolumeIdentity>);

    impl VolumeIdentitySource for FixedVolume {
        fn volume_identity(
            &self,
            _path: &Path,
        ) -> Result<Option<VolumeIdentity>, VolumeIdentityError> {
            Ok(self.0.clone())
        }
    }

    struct MemoryProbe(Mutex<CatalogProbeReport>);

    impl CatalogProbe for MemoryProbe {
        fn probe(&self, _path: &Path) -> Result<CatalogProbeReport, CatalogProbeError> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    fn restore_files(home_path: &Path) -> AppStateFiles {
        AppStateFiles {
            binding: HomeBindingFile {
                schema_version: 1,
                current: Some(HomeBindingRecord {
                    home_id: HomeId(RESTORE_HOME_ID.into()),
                    path: home_path.to_path_buf(),
                    volume_fsid: "fsid-1".into(),
                    volume_uuid: "uuid-1".into(),
                    bound_at: "2026-08-01T00:00:00Z".into(),
                }),
                abandoned: vec![],
            },
            recovery_ledger: RecoveryLedgerFile::empty(),
        }
    }

    fn restore_probe_report(integrity_ok: bool) -> CatalogProbeReport {
        CatalogProbeReport {
            exists: true,
            schema_version: Some(crate::seams::catalog_probe::CURRENT_CATALOG_SCHEMA_VERSION),
            integrity_ok,
            foreign_keys_ok: integrity_ok,
            home_identity: Some(CatalogHomeIdentity {
                home_id: HomeId(RESTORE_HOME_ID.into()),
                volume_fsid: "fsid-1".into(),
                volume_uuid: "uuid-1".into(),
                home_bound_at: "2026-08-01T00:00:00Z".into(),
            }),
            snapshot_version: Some(3),
        }
    }

    fn restore_service(
        dir: &std::path::Path,
        app_state: Arc<MemoryStateStore>,
        probe: CatalogProbeReport,
        volume: Option<VolumeIdentity>,
        classifier: FixtureClassification,
    ) -> FixtureRecoveryService {
        let home = dir.join("home");
        let filesystem = crate::adapters::macos_fs::MacOsFileSystem::new(dir.to_path_buf());
        filesystem.ensure_directory(&home).expect("home dir");
        filesystem
            .write_utf8_file(
                &home.join(HomeMarker::FILE_NAME),
                &serde_json::to_string_pretty(&HomeMarker {
                    schema_version: HomeMarker::SCHEMA_VERSION,
                    home_id: HomeId(RESTORE_HOME_ID.into()),
                    volume_fsid: "fsid-1".into(),
                    volume_uuid: "uuid-1".into(),
                    created_at: "2026-08-01T00:00:00Z".into(),
                })
                .expect("marker JSON"),
            )
            .expect("marker");
        let filesystem = crate::adapters::macos_fs::MacOsFileSystem::new(dir.to_path_buf());
        let bootstrap = BootstrapService::new(
            app_state.clone(),
            Arc::new(FixedVolume(volume)),
            Arc::new(MemoryProbe(Mutex::new(probe))),
            Arc::new(filesystem),
            Arc::new(FixedClassifier(classifier)),
            BootstrapConfig {
                state_dir: dir.join("state"),
                default_home_path: dir.join("default-home"),
                catalog_file_name: "skill-man.sqlite3".into(),
            },
        );
        FixtureRecoveryService::new(
            app_state,
            Arc::new(MemoryProbe(Mutex::new(CatalogProbeReport::absent()))),
            Arc::new(crate::adapters::macos_fs::MacOsFileSystem::new(
                dir.to_path_buf(),
            )),
            Arc::new(crate::adapters::sqlite::SqlitePreparedCatalogFactory),
            Arc::new(bootstrap),
            BootstrapConfig {
                state_dir: dir.join("state"),
                default_home_path: dir.join("default-home"),
                catalog_file_name: "skill-man.sqlite3".into(),
            },
        )
    }

    #[test]
    fn restore_eligibility_requires_same_identity_content_failure() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let volume = Some(VolumeIdentity {
            fsid: "fsid-1".into(),
            uuid: "uuid-1".into(),
        });

        // Healthy Bound Home: nothing to restore.
        let service = restore_service(
            dir.path(),
            Arc::new(MemoryStateStore(Mutex::new(restore_files(&home)))),
            restore_probe_report(true),
            volume.clone(),
            FixtureClassification::Clean,
        );
        assert_eq!(
            service.restore_eligibility().expect("probe"),
            RestoreEligibility::NotRequired
        );

        // Integrity failure under a provable identity: Restore required.
        let service = restore_service(
            dir.path(),
            Arc::new(MemoryStateStore(Mutex::new(restore_files(&home)))),
            restore_probe_report(false),
            volume.clone(),
            FixtureClassification::Clean,
        );
        match service.restore_eligibility().expect("probe") {
            RestoreEligibility::RestoreRequired {
                home_id,
                path,
                reason,
            } => {
                assert_eq!(home_id.0, RESTORE_HOME_ID);
                assert_eq!(path, home);
                assert_eq!(reason, RestoreReason::CatalogIntegrityFailed);
            }
            other => panic!("expected RestoreRequired, got {other:?}"),
        }

        // Bound Home with a fixture footprint (recovery lock): Restore required.
        let service = restore_service(
            dir.path(),
            Arc::new(MemoryStateStore(Mutex::new(restore_files(&home)))),
            restore_probe_report(true),
            volume,
            FixtureClassification::Pure,
        );
        match service.restore_eligibility().expect("probe") {
            RestoreEligibility::RestoreRequired { reason, .. } => {
                assert_eq!(reason, RestoreReason::FixtureContamination);
            }
            other => panic!("expected RestoreRequired, got {other:?}"),
        }
    }

    #[test]
    fn restore_eligibility_is_closed_outside_a_provable_bound_home() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let mut files = restore_files(&home);
        files.binding.current = None;
        let service = restore_service(
            dir.path(),
            Arc::new(MemoryStateStore(Mutex::new(files))),
            CatalogProbeReport::absent(),
            None,
            FixtureClassification::Clean,
        );
        match service.restore_eligibility().expect("probe") {
            RestoreEligibility::NotApplicable { reason } => {
                assert_eq!(reason, RestoreNotApplicableReason::NoBinding);
            }
            other => panic!("expected NotApplicable, got {other:?}"),
        }

        // Identity mismatch (volume changed): Restore never applies.
        let mut files = restore_files(&home);
        files.binding.current.as_mut().unwrap().volume_fsid = "other-fsid".into();
        let service = restore_service(
            dir.path(),
            Arc::new(MemoryStateStore(Mutex::new(files))),
            restore_probe_report(true),
            Some(VolumeIdentity {
                fsid: "fsid-1".into(),
                uuid: "uuid-1".into(),
            }),
            FixtureClassification::Clean,
        );
        match service.restore_eligibility().expect("probe") {
            RestoreEligibility::NotApplicable { reason } => {
                assert_eq!(reason, RestoreNotApplicableReason::IdentityMismatch);
            }
            other => panic!("expected NotApplicable, got {other:?}"),
        }

        // An active operation owns the lock: the probe stays closed.
        let mut files = restore_files(&home);
        files.recovery_ledger.active = Some(RecoveryOperationRecord {
            operation_id: "op-1".into(),
            kind: RECOVERY_KIND_FIXTURE.into(),
            home_id: Some(HomeId(RESTORE_HOME_ID.into())),
            live_path: Some(home.clone()),
            snapshot_path: None,
            prepared_path: None,
            manifest_hash: None,
            external_probe: None,
            cursor: Some(cursors::CONFIRMED.into()),
            commit_point: None,
            created_at: "2026-08-01T00:00:00Z".into(),
        });
        let service = restore_service(
            dir.path(),
            Arc::new(MemoryStateStore(Mutex::new(files))),
            restore_probe_report(false),
            Some(VolumeIdentity {
                fsid: "fsid-1".into(),
                uuid: "uuid-1".into(),
            }),
            FixtureClassification::Clean,
        );
        match service.restore_eligibility().expect("probe") {
            RestoreEligibility::NotApplicable { reason } => {
                assert_eq!(reason, RestoreNotApplicableReason::ActiveOperation);
            }
            other => panic!("expected NotApplicable, got {other:?}"),
        }
    }

    #[test]
    fn plan_restore_opens_a_restore_operation_only_when_eligible() {
        let dir = tempfile::tempdir().expect("temp dir");
        let home = dir.path().join("home");
        let volume = Some(VolumeIdentity {
            fsid: "fsid-1".into(),
            uuid: "uuid-1".into(),
        });

        // Healthy: refused with the closed NotRestorable error.
        let service = restore_service(
            dir.path(),
            Arc::new(MemoryStateStore(Mutex::new(restore_files(&home)))),
            restore_probe_report(true),
            volume.clone(),
            FixtureClassification::Clean,
        );
        assert!(matches!(
            service.plan_restore(),
            Err(FixtureRecoveryError::NotRestorable(_))
        ));

        // Integrity failed: the operation opens with the same identity and
        // cursor, kind `restore`, no Home bytes touched.
        let store = Arc::new(MemoryStateStore(Mutex::new(restore_files(&home))));
        let service = restore_service(
            dir.path(),
            store.clone(),
            restore_probe_report(false),
            volume,
            FixtureClassification::Clean,
        );
        let plan = service.plan_restore().expect("plan restore");
        let files = store.load().expect("app state");
        let active = files.recovery_ledger.active.expect("active operation");
        assert_eq!(active.kind, RECOVERY_KIND_RESTORE);
        assert_eq!(
            active.home_id.as_ref().map(|id| id.0.as_str()),
            Some(RESTORE_HOME_ID)
        );
        assert_eq!(active.live_path.as_deref(), Some(home.as_path()));
        assert_eq!(active.cursor.as_deref(), Some(cursors::CONFIRMED));
        assert_eq!(plan.plan_token, active.operation_id);
        // The ledger write is the only mutation.
        assert!(
            home.join(HomeMarker::FILE_NAME).is_file(),
            "the Home marker is untouched"
        );
    }
}
