//! Source classification of a terminal Scan Report (spec §8.2, ADR-0017):
//! the pure rule pass that turns canonical-entity evidence — position in a
//! control zone, applicable lock claims, bounded worktree hints — into
//! closed verdicts. It never writes anything and never fetches; Git Source
//! groups are hint aggregates that await #92's explicit Fetch Latest and
//! Manage. #91's Current/Legacy capability is consumed through the managed
//! facts (Catalog paths/names), never by guessing from a schema version or
//! a global UNIQUE.
//!
//! Rules implemented here (all fail closed):
//! - an entity already Managed (Catalog final entity path or Home namespace)
//!   is `already_managed`; fixture footprints are `excluded`;
//! - one entity under multiple Directory Identities is an Identity
//!   Conflict; the entity is not selectable — rename and Rescan;
//! - worktree and lock hints are parallel evidence: identical canonical
//!   repository → one Git group; any contradiction → typed Blocked;
//! - uninterpretable Git metadata inside a control zone → Blocked; outside
//!   → Local with the "Git metadata not used" note;
//! - user-owned development workspaces (outside control zones, no
//!   applicable lock) stay Local even when they are Git repositories;
//! - same canonical repository multiple legacy refs → Repository Ref
//!   Conflict; applicable claims across multiple lock files → Repository
//!   Ownership Split (zero-write reject); a conflicted group is never a
//!   selectable candidate;
//! - Local↔Local entities with the same NFC + casefold Directory Identity
//!   form a Conflict Set with no default winner; Git members never join;
//! - destructive operations (move, fetch/manage that releases external
//!   ownership) require complete coverage — the Core closed reason is
//!   `scan_incomplete`; a keep-in-place Local Link is never blocked.

#[cfg(test)]
use std::collections::{BTreeMap, HashMap};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::domain::skill_identity_key;
use crate::core::fixture_recovery::{
    FIXTURE_ENTITIES_DIR, FIXTURE_TREE_HASH_MEDIA_XRAY, FIXTURE_TREE_HASH_ROOT,
    FIXTURE_TREE_HASH_SKILL_AUTHORING,
};
use crate::core::git_source::parse_git_source_input;
use crate::seams::scan_evidence_store::{
    ScanClassificationRow, ScanClassificationSpool, ScanClassificationSpoolEntity,
    ScanConflictSetRecord, ScanGitSourceGroupRecord, ScanLockClaimRecord, ScanOperationEligibility,
    ScanSourceCounts, ScanSourceVerdictRecord, ScanWorktreeHintRecord,
};

/// Closed verdict values (the verdict vocabulary of §8.2).
pub const VERDICT_LOCAL: &str = "local";
pub const VERDICT_GIT: &str = "git";
pub const VERDICT_CONFLICT_SET: &str = "conflict_set";
pub const VERDICT_IDENTITY_CONFLICT: &str = "identity_conflict";
pub const VERDICT_BLOCKED: &str = "blocked";
pub const VERDICT_DEFERRED: &str = "deferred";
pub const VERDICT_ALREADY_MANAGED: &str = "already_managed";
pub const VERDICT_EXCLUDED: &str = "excluded";

/// Closed reason kinds of attention verdicts and typed Local notes.
pub const REASON_UNINTERPRETABLE: &str = "uninterpretable_metadata";
pub const REASON_PROVENANCE_CONTRADICTION: &str = "provenance_contradiction";
pub const REASON_LOCK_FILE_FAULT: &str = "lock_file_fault";
pub const REASON_REF_CONFLICT: &str = "repository_ref_conflict";
pub const REASON_OWNERSHIP_SPLIT: &str = "ownership_split";
pub const REASON_DEFERRED: &str = "verification_deferred";
pub const REASON_MULTI_IDENTITY: &str = "multiple_directory_identities";
pub const REASON_MANAGED_NAME: &str = "managed_name_collision";

/// Closed info note kinds (never warnings).
pub const NOTE_GIT_METADATA_NOT_USED: &str = "git_metadata_not_used";
pub const NOTE_NON_GIT_LOCK_CLAIM: &str = "non_git_lock_claim";

/// Closed operation names (typed eligibility contract, §8.1).
pub const OP_LOCAL_LINK: &str = "local_link";
pub const OP_LOCAL_LINK_WITH_MOVE: &str = "local_link_with_move";
pub const OP_GIT_FETCH_MANAGE: &str = "git_fetch_and_manage";
pub const OP_CONFLICT_WINNER: &str = "conflict_winner";

/// The closed Core reason destructive operations are refused with.
pub const CLOSED_REASON_SCAN_INCOMPLETE: &str = "scan_incomplete";

/// Facts outside the scanned evidence (spec §8.2): control zones, managed
/// skill facts of the Catalog and the lock discovery of the Run. The
/// classifier is pure; the engine assembles these.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScanClassificationContext {
    pub ignored_entity_paths: Vec<PathBuf>,
    /// Home, Global Skills Roots, installer-managed roots and App state
    /// paths: an entity inside any of them is inside the control zone.
    pub control_zones: Vec<PathBuf>,
    /// The Managed Library namespace (`<home>/skills`).
    pub home_skills_path: Option<PathBuf>,
    /// Catalog final entity paths of every Managed Skill (all source kinds).
    pub managed_entity_paths: Vec<PathBuf>,
    /// Catalog Directory Identities of Managed Skills (Source Content).
    pub managed_directory_names: Vec<String>,
    /// Installer roots whose lock file is structurally faulted
    /// (path, fault detail): every entity inside is Blocked.
    pub faulted_lock_roots: Vec<(PathBuf, String)>,
    /// Whether every configured Root was covered (complete Report): the
    /// eligibility source of destructive operations.
    pub report_complete: bool,
}

#[cfg(test)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScanClassificationOutput {
    pub verdicts: Vec<ScanSourceVerdictRecord>,
    pub git_groups: Vec<ScanGitSourceGroupRecord>,
    pub conflict_sets: Vec<ScanConflictSetRecord>,
    pub counts: ScanSourceCounts,
}

/// Source of one generation's classification rows. Implementations stream
/// from the Evidence Store; the classifier never receives a full `Vec`.
pub trait ClassificationRowSource {
    fn for_each_row(
        &mut self,
        visitor: &mut dyn FnMut(ScanClassificationRow) -> Result<(), String>,
    ) -> Result<(), String>;
}

/// Destination for immutable classification evidence. The Evidence Store
/// owns the durable JSONL files and fsync protocol.
pub trait ClassificationSink {
    fn append_verdict(&mut self, record: &ScanSourceVerdictRecord) -> Result<(), String>;
    fn append_git_group(&mut self, record: &ScanGitSourceGroupRecord) -> Result<(), String>;
    fn append_conflict_set(&mut self, record: &ScanConflictSetRecord) -> Result<(), String>;
}

/// One attributed bounded worktree hint: provider kind, canonical
/// repository, repository root and the raw hint facts.
#[derive(Clone, Debug, Eq, PartialEq)]
struct AttributedWorktree {
    provider: String,
    canonical_repository: String,
    repository_root: PathBuf,
    remote_urls_seen: Vec<String>,
}

/// The Git hint outcome of one entity.
enum GitHintOutcome {
    /// No Git metadata at all.
    None,
    /// User-owned development workspace: Git metadata exists but the entity
    /// stays Local (outside control zone).
    Local,
    /// Typed fail-closed outcome.
    Blocked {
        reason: &'static str,
        detail: String,
    },
    /// Aggregate this entity into a Git source group.
    Git {
        provider: String,
        canonical_repository: String,
    },
}

/// Internal classification state of one entity; the final record is built
/// after conflict/group membership resolves.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct PerEntity {
    seq: u64,
    canonical_path: PathBuf,
    display_name: String,
    identity_key: String,
    directory_names: Vec<String>,
    appearances: u64,
    file_count: u64,
    byte_count: u64,
    tree_hash: Option<String>,
    lock_claims: Vec<ScanLockClaimRecord>,
    worktree_hints: Vec<ScanWorktreeHintRecord>,
    requires_move: bool,
    notes: Vec<String>,
    reason_kind: Option<String>,
    detail: Option<String>,
    git_refs: Vec<String>,
    git_lock_paths: Vec<PathBuf>,
    git_group_seq: Option<u64>,
    conflict_set_seq: Option<u64>,
    shape: VerdictShape,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
enum VerdictShape {
    Local,
    Git {
        provider: String,
        canonical_repository: String,
    },
    ConflictSetMember,
    Blocked,
    IdentityConflict,
    /// Verification Deferred (§8.2): the vocabulary is closed and the
    /// presentation must render it; a zero-network #84 scan never produces
    /// it (the fetch/discovery availability facts arrive with #92).
    #[allow(dead_code)]
    Deferred(String),
    AlreadyManaged,
    Excluded,
}

impl PerEntity {
    fn to_record(&self, report_complete: bool) -> ScanSourceVerdictRecord {
        let (verdict, operations) = match &self.shape {
            VerdictShape::Local => {
                if self.requires_move {
                    (
                        VERDICT_LOCAL,
                        vec![eligibility(
                            OP_LOCAL_LINK_WITH_MOVE,
                            report_complete,
                            Some(CLOSED_REASON_SCAN_INCOMPLETE),
                        )],
                    )
                } else {
                    (VERDICT_LOCAL, vec![eligibility(OP_LOCAL_LINK, true, None)])
                }
            }
            VerdictShape::Git { .. } => (VERDICT_GIT, Vec::new()),
            VerdictShape::ConflictSetMember => (
                VERDICT_CONFLICT_SET,
                vec![eligibility(OP_CONFLICT_WINNER, true, None)],
            ),
            VerdictShape::Blocked => (VERDICT_BLOCKED, Vec::new()),
            VerdictShape::IdentityConflict => (VERDICT_IDENTITY_CONFLICT, Vec::new()),
            VerdictShape::Deferred(_) => (VERDICT_DEFERRED, Vec::new()),
            VerdictShape::AlreadyManaged => (VERDICT_ALREADY_MANAGED, Vec::new()),
            VerdictShape::Excluded => (VERDICT_EXCLUDED, Vec::new()),
        };
        ScanSourceVerdictRecord {
            entity_seq: self.seq,
            verdict: verdict.to_owned(),
            canonical_path: self.canonical_path.clone(),
            directory_names: self.directory_names.clone(),
            appearances: self.appearances,
            file_count: self.file_count,
            byte_count: self.byte_count,
            tree_hash: self.tree_hash.clone(),
            lock_claims: self.lock_claims.clone(),
            worktree_hints: self.worktree_hints.clone(),
            reason_kind: self.reason_kind.clone(),
            detail: self.detail.clone(),
            git_refs: self.git_refs.clone(),
            git_lock_paths: self.git_lock_paths.clone(),
            git_group_seq: self.git_group_seq,
            conflict_set_seq: self.conflict_set_seq,
            notes: self.notes.clone(),
            operations,
        }
    }
}

/// Small in-memory helper retained for pure classifier unit tests.
///
/// The production Run uses `classify_stream`, which keeps cross-entity joins
/// in a temporary disk-backed spool.
#[cfg(test)]
pub fn classify(
    rows: Vec<ScanClassificationRow>,
    context: &ScanClassificationContext,
) -> ScanClassificationOutput {
    let managed_paths: HashSet<&Path> = context
        .managed_entity_paths
        .iter()
        .map(|path| path.as_path())
        .collect();
    let managed_name_keys: HashSet<String> = context
        .managed_directory_names
        .iter()
        .map(|name| skill_identity_key(name))
        .collect();
    let faulted_roots: Vec<(&Path, &str)> = context
        .faulted_lock_roots
        .iter()
        .map(|(path, fault)| (path.as_path(), fault.as_str()))
        .collect();

    let mut per_entity: Vec<PerEntity> = Vec::new();
    for row in rows {
        per_entity.push(classify_entity(
            row,
            context,
            &managed_paths,
            &managed_name_keys,
            &faulted_roots,
        ));
    }

    // Git group aggregation (spec §8.2: by provider + canonical repository;
    // hints parallel, contradictions fail closed).
    let mut group_accs: BTreeMap<(String, String), GroupAcc> = BTreeMap::new();
    for entity in &per_entity {
        if let VerdictShape::Git {
            provider,
            canonical_repository,
        } = &entity.shape
        {
            let acc = group_accs
                .entry((provider.clone(), canonical_repository.clone()))
                .or_default();
            acc.provider = provider.clone();
            acc.canonical_repository = canonical_repository.clone();
            acc.members.push(entity.seq);
            acc.member_paths.push(entity.canonical_path.clone());
            acc.member_names.push(entity.display_name.clone());
            acc.claims.extend(entity.lock_claims.iter().cloned());
            for hint in &entity.worktree_hints {
                if hint.gitdir_kind != "uninterpretable" {
                    acc.repository_root = Some(hint.repository_root.clone());
                    acc.remote_urls_seen
                        .extend(hint.remote_urls.iter().cloned());
                }
            }
        }
    }
    let mut groups: Vec<ScanGitSourceGroupRecord> = Vec::new();
    let mut group_status: HashMap<u64, (String, String)> = HashMap::new();
    for (seq, (_key, mut acc)) in group_accs.into_iter().enumerate() {
        let group_seq = seq as u64 + 1;
        acc.dedupe();
        let (status, detail): (String, Option<String>) = if acc.lock_paths.len() > 1 {
            (
                "ownership_split".to_owned(),
                Some(format!(
                    "lock files: {}",
                    acc.lock_paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            )
        } else if acc.refs.len() > 1 {
            (
                "repository_ref_conflict".to_owned(),
                Some(format!(
                    "refs: {}",
                    acc.refs.iter().cloned().collect::<Vec<_>>().join(", ")
                )),
            )
        } else {
            ("candidate".to_owned(), None)
        };
        if status != "candidate" {
            group_status.insert(
                group_seq,
                (status.clone(), detail.clone().unwrap_or_default()),
            );
        }
        let is_candidate = status == "candidate";
        groups.push(ScanGitSourceGroupRecord {
            group_seq,
            provider: acc.provider,
            canonical_repository: acc.canonical_repository,
            repository_root: acc.repository_root,
            remote_urls_seen: acc.remote_urls_seen,
            member_entity_seqs: acc.members.clone(),
            member_paths: acc.member_paths.clone(),
            member_names: acc.member_names.clone(),
            lock_claims: acc.claims,
            refs: acc.refs.into_iter().collect(),
            lock_paths: acc.lock_paths.into_iter().collect(),
            status: status.clone(),
            operations: if is_candidate {
                vec![eligibility(
                    OP_GIT_FETCH_MANAGE,
                    context.report_complete,
                    Some(CLOSED_REASON_SCAN_INCOMPLETE),
                )]
            } else {
                Vec::new()
            },
            detail,
        });
        // A conflicted group fails closed: members become typed attention
        // rows and lose every per-member control (spec §8.2).
        if let Some((kind, detail)) = group_status.get(&group_seq).cloned() {
            for entity in per_entity
                .iter_mut()
                .filter(|entity| acc.members.contains(&entity.seq))
            {
                entity.git_group_seq = Some(group_seq);
                entity.shape = VerdictShape::Blocked;
                entity.reason_kind = Some(
                    if kind == "ownership_split" {
                        REASON_OWNERSHIP_SPLIT
                    } else {
                        REASON_REF_CONFLICT
                    }
                    .to_owned(),
                );
                entity.detail = Some(detail.clone());
            }
        } else {
            for entity in per_entity
                .iter_mut()
                .filter(|entity| acc.members.contains(&entity.seq))
            {
                entity.git_group_seq = Some(group_seq);
            }
        }
    }

    // Local↔Local Conflict Sets (ADR-0017: NFC + casefold Directory
    // Identity; no default winner; Git members never join).
    let mut local_by_key: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, entity) in per_entity.iter().enumerate() {
        if matches!(entity.shape, VerdictShape::Local) {
            local_by_key
                .entry(entity.identity_key.clone())
                .or_default()
                .push(index);
        }
    }
    let mut conflict_sets: Vec<ScanConflictSetRecord> = Vec::new();
    let mut set_for_entity: HashMap<usize, u64> = HashMap::new();
    for (seq, (key, members)) in local_by_key.into_iter().enumerate() {
        if members.len() < 2 {
            continue;
        }
        let set_seq = seq as u64 + 1;
        let representative = &per_entity[members[0]];
        conflict_sets.push(ScanConflictSetRecord {
            set_seq,
            directory_identity_key: key.clone(),
            directory_name: representative.display_name.clone(),
            member_entity_seqs: members.iter().map(|index| per_entity[*index].seq).collect(),
            member_paths: members
                .iter()
                .map(|index| per_entity[*index].canonical_path.clone())
                .collect(),
            winner_entity_seq: None,
        });
        for index in members {
            set_for_entity.insert(index, set_seq);
        }
    }

    // Final verdict records in deterministic entity order.
    let mut verdicts = Vec::with_capacity(per_entity.len());
    for (index, entity) in per_entity.iter_mut().enumerate() {
        if matches!(entity.shape, VerdictShape::Local) {
            if let Some(set_seq) = set_for_entity.get(&index).copied() {
                entity.shape = VerdictShape::ConflictSetMember;
                entity.conflict_set_seq = Some(set_seq);
            }
        }
        verdicts.push(entity.to_record(context.report_complete));
    }

    let counts = count(&verdicts, &groups, &conflict_sets);
    ScanClassificationOutput {
        verdicts,
        git_groups: groups,
        conflict_sets,
        counts,
    }
}

/// Classify one generation through a disk-backed spool.
///
/// Git group and Local Conflict Set membership require cross-entity joins.
/// The spool keeps those joins on disk, then emits each immutable output row
/// through `sink`; only one entity or one group is resident at a time.
pub fn classify_stream(
    source: &mut dyn ClassificationRowSource,
    context: &ScanClassificationContext,
    spool: &mut dyn ScanClassificationSpool,
    sink: &mut dyn ClassificationSink,
) -> Result<ScanSourceCounts, String> {
    let managed_paths: HashSet<&Path> = context
        .managed_entity_paths
        .iter()
        .map(|path| path.as_path())
        .collect();
    let managed_name_keys: HashSet<String> = context
        .managed_directory_names
        .iter()
        .map(|name| skill_identity_key(name))
        .collect();
    let faulted_roots: Vec<(&Path, &str)> = context
        .faulted_lock_roots
        .iter()
        .map(|(path, fault)| (path.as_path(), fault.as_str()))
        .collect();
    {
        let mut insert_row = |row: ScanClassificationRow| -> Result<(), String> {
            let entity = classify_entity(
                row,
                context,
                &managed_paths,
                &managed_name_keys,
                &faulted_roots,
            );
            let (shape, provider, repository) = match &entity.shape {
                VerdictShape::Git {
                    provider,
                    canonical_repository,
                } => (
                    "git",
                    Some(provider.as_str()),
                    Some(canonical_repository.as_str()),
                ),
                VerdictShape::Local => ("local", None, None),
                _ => ("other", None, None),
            };
            let payload = serde_json::to_vec(&entity)
                .map_err(|error| format!("serialize classification row: {error}"))?;
            spool.insert_entity(
                entity.seq,
                &payload,
                shape,
                provider,
                repository,
                &entity.identity_key,
            )?;
            Ok(())
        };
        source.for_each_row(&mut insert_row)?;
    }

    let mut next_group_seq = 1_u64;
    let mut git_groups = 0_u64;
    let mut git_groups_conflicted = 0_u64;
    while let Some((provider, repository)) = spool.next_git_group()? {
        let mut acc = GroupAcc {
            provider: provider.clone(),
            canonical_repository: repository.clone(),
            ..GroupAcc::default()
        };
        let mut collect_member = |payload: Vec<u8>| -> Result<(), String> {
            let entity = serde_json::from_slice::<PerEntity>(&payload)
                .map_err(|error| format!("parse classification Git member: {error}"))?;
            acc.members.push(entity.seq);
            acc.member_paths.push(entity.canonical_path);
            acc.member_names.push(entity.display_name);
            acc.claims.extend(entity.lock_claims);
            for hint in entity.worktree_hints {
                if hint.gitdir_kind != "uninterpretable" {
                    acc.repository_root = Some(hint.repository_root);
                    acc.remote_urls_seen.extend(hint.remote_urls);
                }
            }
            Ok(())
        };
        spool.for_each_git_member(&provider, &repository, &mut collect_member)?;
        acc.dedupe();
        let (status, detail) = group_status(&acc);
        let is_candidate = status == "candidate";
        if is_candidate {
            git_groups += 1;
        } else {
            git_groups_conflicted += 1;
        }
        sink.append_git_group(&ScanGitSourceGroupRecord {
            group_seq: next_group_seq,
            provider: acc.provider,
            canonical_repository: acc.canonical_repository,
            repository_root: acc.repository_root,
            remote_urls_seen: acc.remote_urls_seen,
            member_entity_seqs: acc.members,
            member_paths: acc.member_paths,
            member_names: acc.member_names,
            lock_claims: acc.claims,
            refs: acc.refs.into_iter().collect(),
            lock_paths: acc.lock_paths.into_iter().collect(),
            status: status.clone(),
            operations: if is_candidate {
                vec![eligibility(
                    OP_GIT_FETCH_MANAGE,
                    context.report_complete,
                    Some(CLOSED_REASON_SCAN_INCOMPLETE),
                )]
            } else {
                Vec::new()
            },
            detail: detail.clone(),
        })?;
        spool.mark_git_group(
            &provider,
            &repository,
            next_group_seq,
            &status,
            detail.as_deref(),
        )?;
        next_group_seq = next_group_seq.saturating_add(1);
    }

    let mut next_conflict_seq = 1_u64;
    while let Some(identity_key) = spool.next_local_conflict_key()? {
        let mut member_entity_seqs = Vec::new();
        let mut member_paths = Vec::new();
        let mut representative_name = String::new();
        let mut collect_member = |payload: Vec<u8>| -> Result<(), String> {
            let entity = serde_json::from_slice::<PerEntity>(&payload)
                .map_err(|error| format!("parse classification Conflict member: {error}"))?;
            if representative_name.is_empty() {
                representative_name = entity.display_name;
            }
            member_entity_seqs.push(entity.seq);
            member_paths.push(entity.canonical_path);
            Ok(())
        };
        spool.for_each_local_member(&identity_key, &mut collect_member)?;
        sink.append_conflict_set(&ScanConflictSetRecord {
            set_seq: next_conflict_seq,
            directory_identity_key: identity_key.clone(),
            directory_name: representative_name,
            member_entity_seqs: member_entity_seqs.clone(),
            member_paths,
            winner_entity_seq: None,
        })?;
        spool.mark_local_conflict(&identity_key, next_conflict_seq)?;
        next_conflict_seq = next_conflict_seq.saturating_add(1);
    }

    let mut counts = ScanSourceCounts {
        git_groups,
        git_groups_conflicted,
        conflict_sets: next_conflict_seq.saturating_sub(1),
        ..ScanSourceCounts::default()
    };
    let mut emit_verdict = |row: ScanClassificationSpoolEntity| -> Result<(), String> {
        let mut entity = serde_json::from_slice::<PerEntity>(&row.payload)
            .map_err(|error| format!("parse classification verdict: {error}"))?;
        entity.git_group_seq = row.group_seq;
        if let Some(status) = row.group_status {
            if status != "candidate" {
                entity.shape = VerdictShape::Blocked;
                entity.reason_kind = Some(
                    if status == "ownership_split" {
                        REASON_OWNERSHIP_SPLIT
                    } else {
                        REASON_REF_CONFLICT
                    }
                    .to_owned(),
                );
                entity.detail = row.group_detail;
            }
        }
        if let Some(conflict_seq) = row.conflict_seq {
            entity.shape = VerdictShape::ConflictSetMember;
            entity.conflict_set_seq = Some(conflict_seq);
        }
        let verdict = entity.to_record(context.report_complete);
        add_verdict_counts(&mut counts, &verdict);
        sink.append_verdict(&verdict)?;
        Ok(())
    };
    spool.for_each_entity(&mut emit_verdict)?;
    counts.needs_attention = counts.blocked + counts.deferred + counts.identity_conflicts;
    Ok(counts)
}

fn classify_entity(
    row: ScanClassificationRow,
    context: &ScanClassificationContext,
    managed_paths: &HashSet<&Path>,
    managed_name_keys: &HashSet<String>,
    faulted_roots: &[(&Path, &str)],
) -> PerEntity {
    let mut entity = PerEntity {
        seq: row.entity_seq,
        canonical_path: row.canonical_path.clone(),
        display_name: row.directory_names.first().cloned().unwrap_or_default(),
        identity_key: row
            .directory_names
            .first()
            .map(|name| skill_identity_key(name))
            .unwrap_or_default(),
        directory_names: row.directory_names.clone(),
        appearances: row.appearances,
        file_count: row.file_count,
        byte_count: row.byte_count,
        tree_hash: row.tree_hash.clone(),
        lock_claims: row.lock_claims,
        worktree_hints: row.worktree_hints,
        requires_move: false,
        notes: Vec::new(),
        reason_kind: None,
        detail: None,
        git_refs: Vec::new(),
        git_lock_paths: Vec::new(),
        git_group_seq: None,
        conflict_set_seq: None,
        shape: VerdictShape::Local,
    };

    // 1. Already Managed (Catalog final entity path or the Managed Library
    // namespace): Excluded·already Managed, never a replacement candidate.
    let in_home_skills = context
        .home_skills_path
        .as_deref()
        .map(|root| entity.canonical_path.starts_with(root))
        .unwrap_or(false);
    if in_home_skills || managed_paths.contains(entity.canonical_path.as_path()) {
        entity.shape = VerdictShape::AlreadyManaged;
        return entity;
    }

    // 2. Fixture footprint: Fixture Recovery owns it (spec §3.5).
    if is_fixture(&entity) {
        entity.shape = VerdictShape::Excluded;
        return entity;
    }

    if context
        .ignored_entity_paths
        .contains(&entity.canonical_path)
    {
        entity.shape = VerdictShape::Excluded;
        entity.reason_kind = Some("ignored".to_owned());
        return entity;
    }

    // 3. A structurally faulted governing lock file blocks every entity
    // inside the installer root (ADR-0013 §2.1).
    if let Some((_, fault)) = faulted_roots
        .iter()
        .find(|(root, _)| entity.canonical_path.starts_with(root))
    {
        entity.shape = VerdictShape::Blocked;
        entity.reason_kind = Some(REASON_LOCK_FILE_FAULT.to_owned());
        entity.detail = Some((*fault).to_owned());
        return entity;
    }

    // 4. One final entity under several Directory Identities is an Identity
    // Conflict: rename and Rescan (ADR-0017); no Include anywhere.
    let distinct_names: BTreeSet<&String> = entity.directory_names.iter().collect();
    if distinct_names.len() > 1 {
        entity.shape = VerdictShape::IdentityConflict;
        entity.reason_kind = Some(REASON_MULTI_IDENTITY.to_owned());
        entity.detail = Some(format!(
            "directory identities: {}",
            entity.directory_names.join(", ")
        ));
        return entity;
    }

    // 5. Git evidence: parallel worktree + lock hints, fail closed.
    let inside = inside_control_zone(&entity.canonical_path, context);
    match git_hint_outcome(&mut entity, inside) {
        GitHintOutcome::None => {
            entity.requires_move = inside;
            entity.shape = VerdictShape::Local;
        }
        GitHintOutcome::Local => {
            entity.requires_move = inside;
            entity.notes.push(NOTE_GIT_METADATA_NOT_USED.to_owned());
            entity.shape = VerdictShape::Local;
        }
        GitHintOutcome::Blocked { reason, detail } => {
            entity.shape = VerdictShape::Blocked;
            entity.reason_kind = Some(reason.to_owned());
            entity.detail = Some(detail);
        }
        GitHintOutcome::Git {
            provider,
            canonical_repository,
            ..
        } => {
            entity.git_refs = collect_refs(&entity.lock_claims);
            entity.git_lock_paths = collect_lock_paths(&entity.lock_claims);
            entity.shape = VerdictShape::Git {
                provider,
                canonical_repository,
            };
        }
    }

    // 6. A Local candidate whose Directory Identity collides with a Managed
    // Skill is never a replacement (既有非 Git Managed Skill 不被替换); the
    // fact is typed, the candidate stays selectable.
    if matches!(entity.shape, VerdictShape::Local)
        && managed_name_keys.contains(&entity.identity_key)
    {
        entity.reason_kind = Some(REASON_MANAGED_NAME.to_owned());
        entity.detail = Some(entity.display_name.clone());
    }
    entity
}

/// Git hint classification of one entity (step 5 of the verdict rules).
fn git_hint_outcome(entity: &mut PerEntity, inside: bool) -> GitHintOutcome {
    // Worktree attribution: a valid bounded hint whose supported-provider
    // remote URLs normalize to exactly one canonical repository.
    let mut attributed: Option<AttributedWorktree> = None;
    let mut ambiguous = false;
    let mut has_valid = false;
    let mut has_uninterpretable = false;
    for hint in &entity.worktree_hints {
        if hint.gitdir_kind == "uninterpretable" {
            has_uninterpretable = true;
            continue;
        }
        has_valid = true;
        let mut repos = BTreeSet::new();
        for url in &hint.remote_urls {
            if let Some((provider, canonical)) = canonical_git_url(url) {
                repos.insert((provider, canonical));
            }
        }
        if repos.len() == 1 && attributed.is_none() {
            let (provider, canonical_repository) = repos.into_iter().next().expect("one repo");
            attributed = Some(AttributedWorktree {
                provider,
                canonical_repository,
                repository_root: hint.repository_root.clone(),
                remote_urls_seen: hint.remote_urls.clone(),
            });
        } else if repos.len() != 1 {
            // Multiple supported repositories (or none at all): the
            // metadata is not uniquely interpretable (spec §8.2).
            ambiguous = true;
        } else {
            // A second distinct hint with one repo is itself ambiguity.
            ambiguous = true;
        }
    }
    let worktree_attributed = if ambiguous { None } else { attributed };

    // Lock claims: git-family source types parse into canonical repos.
    let mut claim_repos: BTreeSet<(String, String)> = BTreeSet::new();
    let mut valid_claims: Vec<&ScanLockClaimRecord> = Vec::new();
    let mut faulty_claim: Option<(&'static str, String)> = None;
    let mut non_git_claim = false;
    for claim in &entity.lock_claims {
        let Some(claimed_provider) = provider_of_source_type(claim.source_type.as_deref()) else {
            non_git_claim = true;
            entity.notes.push(NOTE_NON_GIT_LOCK_CLAIM.to_owned());
            continue;
        };
        let Some(source_url) = claim.source_url.as_deref() else {
            faulty_claim = Some((
                REASON_UNINTERPRETABLE,
                format!("{}: claim without source_url", claim.entry_name),
            ));
            continue;
        };
        let Some((url_provider, canonical)) = canonical_git_url(source_url) else {
            faulty_claim = Some((
                REASON_UNINTERPRETABLE,
                format!("{}: unparseable source_url {source_url}", claim.entry_name),
            ));
            continue;
        };
        if url_provider != claimed_provider {
            faulty_claim = Some((
                REASON_PROVENANCE_CONTRADICTION,
                format!(
                    "{}: source_type '{claimed_provider}' contradicts URL '{source_url}'",
                    claim.entry_name
                ),
            ));
            continue;
        }
        claim_repos.insert((claimed_provider.to_owned(), canonical));
        valid_claims.push(claim);
    }

    // Entity-level Ownership Split: one entity claimed by several lock
    // files is zero-write blocked immediately (spec §8.2).
    if valid_claims
        .iter()
        .map(|claim| &claim.lock_path)
        .collect::<BTreeSet<_>>()
        .len()
        > 1
    {
        return GitHintOutcome::Blocked {
            reason: REASON_OWNERSHIP_SPLIT,
            detail: format!(
                "lock files: {}",
                valid_claims
                    .iter()
                    .map(|claim| claim.lock_path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
    }

    if claim_repos.len() > 1 {
        return GitHintOutcome::Blocked {
            reason: REASON_PROVENANCE_CONTRADICTION,
            detail: format!(
                "lock repositories: {}",
                claim_repos
                    .iter()
                    .map(|(_, repository)| repository.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
    }
    if ambiguous && !claim_repos.is_empty() {
        return GitHintOutcome::Blocked {
            reason: REASON_PROVENANCE_CONTRADICTION,
            detail: "lock claim contradicts ambiguous worktree repositories".to_owned(),
        };
    }
    if let Some((reason, detail)) = faulty_claim {
        return GitHintOutcome::Blocked { reason, detail };
    }

    match (claim_repos.len(), worktree_attributed) {
        (1, Some(worktree)) => {
            let (provider, canonical_repository) =
                claim_repos.into_iter().next().expect("one claim repo");
            if provider != worktree.provider
                || canonical_repository != worktree.canonical_repository
            {
                return GitHintOutcome::Blocked {
                    reason: REASON_PROVENANCE_CONTRADICTION,
                    detail: format!(
                        "worktree repository '{}' contradicts lock repository '{}'",
                        worktree.canonical_repository, canonical_repository
                    ),
                };
            }
            GitHintOutcome::Git {
                provider,
                canonical_repository,
            }
        }
        (1, None) => {
            let (provider, canonical_repository) =
                claim_repos.into_iter().next().expect("one claim repo");
            GitHintOutcome::Git {
                provider,
                canonical_repository,
            }
        }
        (0, Some(worktree)) => {
            if inside {
                GitHintOutcome::Git {
                    provider: worktree.provider,
                    canonical_repository: worktree.canonical_repository,
                }
            } else {
                // User-owned development workspace: stays Local (spec §8.2
                // row 1); the Git metadata is a hint that is not adopted.
                GitHintOutcome::Local
            }
        }
        (0, None) => {
            if has_uninterpretable || has_valid {
                // Uninterpretable or un-attributable Git metadata.
                if inside {
                    GitHintOutcome::Blocked {
                        reason: REASON_UNINTERPRETABLE,
                        detail: if has_uninterpretable {
                            "the Git worktree metadata is not interpretable".to_owned()
                        } else {
                            "no supported Git provider can be attributed from the worktree metadata"
                                .to_owned()
                        },
                    }
                } else {
                    GitHintOutcome::Local
                }
            } else {
                // No Git metadata (or only a non-Git lock claim).
                let _ = non_git_claim;
                GitHintOutcome::None
            }
        }
        (_, _) => GitHintOutcome::Blocked {
            reason: REASON_PROVENANCE_CONTRADICTION,
            detail: "Git claims could not be attributed to one repository".to_owned(),
        },
    }
}

#[derive(Default)]
struct GroupAcc {
    provider: String,
    canonical_repository: String,
    repository_root: Option<PathBuf>,
    remote_urls_seen: Vec<String>,
    members: Vec<u64>,
    member_paths: Vec<PathBuf>,
    member_names: Vec<String>,
    claims: Vec<ScanLockClaimRecord>,
    refs: BTreeSet<String>,
    lock_paths: BTreeSet<PathBuf>,
}

impl GroupAcc {
    fn dedupe(&mut self) {
        self.members.sort_unstable();
        self.members.dedup();
        let mut seen = BTreeSet::new();
        self.claims
            .retain(|claim| seen.insert((claim.lock_path.clone(), claim.entry_name.clone())));
        self.refs.extend(
            self.claims
                .iter()
                .filter_map(|claim| claim.requested_ref.clone()),
        );
        for claim in &self.claims {
            self.lock_paths.insert(claim.lock_path.clone());
        }
        self.remote_urls_seen.sort();
        self.remote_urls_seen.dedup();
    }
}

fn group_status(acc: &GroupAcc) -> (String, Option<String>) {
    if acc.lock_paths.len() > 1 {
        (
            "ownership_split".to_owned(),
            Some(format!(
                "lock files: {}",
                acc.lock_paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        )
    } else if acc.refs.len() > 1 {
        (
            "repository_ref_conflict".to_owned(),
            Some(format!(
                "refs: {}",
                acc.refs.iter().cloned().collect::<Vec<_>>().join(", ")
            )),
        )
    } else {
        ("candidate".to_owned(), None)
    }
}

fn add_verdict_counts(counts: &mut ScanSourceCounts, verdict: &ScanSourceVerdictRecord) {
    match verdict.verdict.as_str() {
        VERDICT_LOCAL => counts.local_candidates += 1,
        VERDICT_CONFLICT_SET => counts.conflict_members += 1,
        VERDICT_IDENTITY_CONFLICT => counts.identity_conflicts += 1,
        VERDICT_BLOCKED => counts.blocked += 1,
        VERDICT_DEFERRED => counts.deferred += 1,
        VERDICT_ALREADY_MANAGED => counts.already_managed += 1,
        VERDICT_EXCLUDED => counts.excluded += 1,
        _ => {}
    }
}

#[cfg(test)]
fn count(
    verdicts: &[ScanSourceVerdictRecord],
    groups: &[ScanGitSourceGroupRecord],
    conflict_sets: &[ScanConflictSetRecord],
) -> ScanSourceCounts {
    let mut counts = ScanSourceCounts::default();
    for verdict in verdicts {
        match verdict.verdict.as_str() {
            VERDICT_LOCAL => counts.local_candidates += 1,
            VERDICT_CONFLICT_SET => counts.conflict_members += 1,
            VERDICT_IDENTITY_CONFLICT => counts.identity_conflicts += 1,
            VERDICT_BLOCKED => counts.blocked += 1,
            VERDICT_DEFERRED => counts.deferred += 1,
            VERDICT_ALREADY_MANAGED => counts.already_managed += 1,
            VERDICT_EXCLUDED => counts.excluded += 1,
            _ => {}
        }
    }
    counts.conflict_sets = conflict_sets.len() as u64;
    for group in groups {
        if group.status == "candidate" {
            counts.git_groups += 1;
        } else {
            counts.git_groups_conflicted += 1;
        }
    }
    counts.needs_attention = counts.blocked + counts.deferred + counts.identity_conflicts;
    counts
}

fn inside_control_zone(path: &Path, context: &ScanClassificationContext) -> bool {
    context
        .control_zones
        .iter()
        .any(|zone| path.starts_with(zone))
}

fn is_fixture(entity: &PerEntity) -> bool {
    entity
        .canonical_path
        .components()
        .any(|component| component.as_os_str() == FIXTURE_ENTITIES_DIR)
        || matches!(
            entity.tree_hash.as_deref(),
            Some(
                FIXTURE_TREE_HASH_SKILL_AUTHORING
                    | FIXTURE_TREE_HASH_MEDIA_XRAY
                    | FIXTURE_TREE_HASH_ROOT
            )
        )
}

/// Normalize one HTTPS Git URL to `(provider, canonical repository)`.
/// `parse_git_source_input` proves the syntax equivalences (host lowercase,
/// default port, `.git`, path); ssh/file inputs yield `None` — supported
/// HTTPS Git providers only (ADR-0013 §2.2.4).
fn canonical_git_url(raw: &str) -> Option<(String, String)> {
    if !raw.starts_with("https://") && !raw.starts_with("http://") {
        return None;
    }
    let spec = parse_git_source_input(raw).ok()?;
    let provider = provider_of_host(&spec.url)?;
    Some((provider.to_owned(), spec.url))
}

/// The audited provider kind of a canonical URL host.
fn provider_of_host(canonical_url: &str) -> Option<&'static str> {
    let host = canonical_url
        .strip_prefix("https://")
        .or_else(|| canonical_url.strip_prefix("http://"))?
        .split('/')
        .next()?;
    match host {
        "github.com" => Some("github"),
        "gitlab.com" => Some("gitlab"),
        _ => Some("git"),
    }
}

/// The provider kind a lock entry's `source_type` claims. `None` is a
/// non-Git source type (kept out of Git hints, §8.2 last row).
fn provider_of_source_type(source_type: Option<&str>) -> Option<&'static str> {
    match source_type? {
        "github" => Some("github"),
        "gitlab" => Some("gitlab"),
        "git" | "https" => Some("git"),
        _ => None,
    }
}

fn collect_refs(claims: &[ScanLockClaimRecord]) -> Vec<String> {
    let mut refs = claims
        .iter()
        .filter_map(|claim| claim.requested_ref.clone())
        .collect::<Vec<_>>();
    refs.sort();
    refs.dedup();
    refs
}

fn collect_lock_paths(claims: &[ScanLockClaimRecord]) -> Vec<PathBuf> {
    let mut paths = claims
        .iter()
        .map(|claim| claim.lock_path.clone())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

fn eligibility(
    operation: &str,
    allowed: bool,
    closed_reason: Option<&str>,
) -> ScanOperationEligibility {
    ScanOperationEligibility {
        operation: operation.to_owned(),
        allowed,
        closed_reason: if allowed {
            None
        } else {
            closed_reason.map(str::to_owned)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seams::scan_evidence_store::ScanObjectIdentity;

    const IDENTITY: ScanObjectIdentity = ScanObjectIdentity {
        device: 1,
        inode: 1,
    };

    fn row(seq: u64, path: &str, name: &str) -> ScanClassificationRow {
        row_with_names(seq, path, vec![name.to_owned()])
    }

    fn row_with_names(seq: u64, path: &str, names: Vec<String>) -> ScanClassificationRow {
        ScanClassificationRow {
            entity_seq: seq,
            identity: IDENTITY,
            canonical_path: PathBuf::from(path),
            file_count: 1,
            byte_count: 10,
            tree_hash: Some("tree-sha256-v1:abc".to_owned()),
            hash_fault: None,
            directory_names: names,
            appearances: 1,
            first_root_index: 0,
            lock_claims: Vec::new(),
            worktree_hints: Vec::new(),
        }
    }

    fn worktree(
        repository_root: &str,
        urls: &[&str],
        head: Option<&str>,
        kind: &str,
    ) -> ScanWorktreeHintRecord {
        ScanWorktreeHintRecord {
            repository_root: PathBuf::from(repository_root),
            gitdir_kind: kind.to_owned(),
            remote_urls: urls.iter().map(|url| (*url).to_owned()).collect(),
            head_ref: head.map(str::to_owned),
        }
    }

    fn claim(
        lock: &str,
        entry: &str,
        source_type: &str,
        url: &str,
        reference: Option<&str>,
    ) -> ScanLockClaimRecord {
        ScanLockClaimRecord {
            lock_path: PathBuf::from(lock),
            entry_name: entry.to_owned(),
            fingerprint: "fp1".to_owned(),
            source_type: Some(source_type.to_owned()),
            source_url: Some(url.to_owned()),
            requested_ref: reference.map(str::to_owned),
            skill_path: Some(format!("skills/{entry}")),
        }
    }

    fn context() -> ScanClassificationContext {
        ScanClassificationContext {
            ignored_entity_paths: Vec::new(),
            control_zones: vec![PathBuf::from("/root/agent-skills")],
            home_skills_path: Some(PathBuf::from("/home/skills")),
            managed_entity_paths: Vec::new(),
            managed_directory_names: Vec::new(),
            faulted_lock_roots: Vec::new(),
            report_complete: true,
        }
    }

    #[test]
    fn user_git_workspace_outside_control_zone_is_local() {
        // Acceptance: 控制区外且无 applicable lock 的用户 Git 开发工作区归
        // Local；不 move/copy/rewrite，无内容 baseline。
        let ctx = context();
        let mut row = row(1, "/workspace/my-skill", "my-skill");
        row.worktree_hints = vec![worktree(
            "/workspace",
            &["https://github.com/owner/repo.git"],
            Some("ref: refs/heads/main"),
            "dir",
        )];
        let output = classify(vec![row], &ctx);
        assert_eq!(output.counts.git_groups, 0);
        assert_eq!(output.counts.local_candidates, 1);
        let verdict = &output.verdicts[0];
        assert_eq!(verdict.verdict, VERDICT_LOCAL);
        assert!(
            verdict
                .operations
                .iter()
                .any(|op| op.operation == OP_LOCAL_LINK && op.allowed)
        );
        assert!(
            verdict
                .operations
                .iter()
                .all(|op| op.operation != OP_LOCAL_LINK_WITH_MOVE)
        );
        assert!(
            verdict
                .notes
                .contains(&NOTE_GIT_METADATA_NOT_USED.to_owned())
        );
    }

    #[test]
    fn inside_control_zone_without_git_signal_requires_relocation() {
        // §8.2: 实体位于 Home/Global Skills Root/installer root 且无 Git/lock
        // 信号 → 先迁移到稳定外部位置再 Local Link。
        let ctx = context();
        let output = classify(vec![row(1, "/root/agent-skills/plain", "plain")], &ctx);
        let verdict = &output.verdicts[0];
        assert_eq!(verdict.verdict, VERDICT_LOCAL);
        let move_op = verdict
            .operations
            .iter()
            .find(|op| op.operation == OP_LOCAL_LINK_WITH_MOVE)
            .expect("move operation");
        assert!(move_op.allowed);
        assert!(move_op.closed_reason.is_none());
    }

    #[test]
    fn move_and_fetch_are_blocked_until_complete_coverage() {
        // Acceptance: 迁移/删除/替换/释放 ownership 需完整 coverage；Core
        // closed reason 与破坏性主按钮一致禁用；非破坏 Local Link 不受影响。
        let context = ScanClassificationContext {
            control_zones: vec![PathBuf::from("/root/agent-skills")],
            report_complete: false,
            ..ScanClassificationContext::default()
        };
        let output = classify(vec![row(1, "/root/agent-skills/plain", "plain")], &context);
        let move_op = output.verdicts[0]
            .operations
            .iter()
            .find(|op| op.operation == OP_LOCAL_LINK_WITH_MOVE)
            .expect("move operation");
        assert!(!move_op.allowed);
        assert_eq!(
            move_op.closed_reason.as_deref(),
            Some(CLOSED_REASON_SCAN_INCOMPLETE)
        );

        // Git fetch/manage eligibility follows the same rule.
        let mut row = row(1, "/root/agent-skills/repo/skill", "skill");
        row.worktree_hints = vec![worktree(
            "/root/agent-skills/repo",
            &["https://github.com/owner/repo"],
            None,
            "dir",
        )];
        let output = classify(vec![row], &context);
        let fetch_op = output.git_groups[0]
            .operations
            .iter()
            .find(|op| op.operation == OP_GIT_FETCH_MANAGE)
            .expect("fetch operation");
        assert!(!fetch_op.allowed);
        assert_eq!(
            fetch_op.closed_reason.as_deref(),
            Some(CLOSED_REASON_SCAN_INCOMPLETE)
        );
    }

    #[test]
    fn supported_worktree_inside_control_zone_is_git_source_candidate() {
        let ctx = context();
        let mut row = row(1, "/root/agent-skills/repo/skill", "skill");
        row.worktree_hints = vec![worktree(
            "/root/agent-skills/repo",
            &["https://GitHub.com/owner/repo.git"],
            Some("ref: refs/heads/main"),
            "dir",
        )];
        let output = classify(vec![row], &ctx);
        assert_eq!(output.counts.git_groups, 1);
        let group = &output.git_groups[0];
        assert_eq!(group.provider, "github");
        assert_eq!(group.canonical_repository, "https://github.com/owner/repo");
        assert_eq!(group.status, "candidate");
        let verdict = &output.verdicts[0];
        assert_eq!(verdict.verdict, VERDICT_GIT);
        assert_eq!(verdict.git_group_seq, Some(1));
    }

    #[test]
    fn multiple_supported_remotes_are_not_uniquely_interpretable() {
        // §8.2 row 5: 多 remote 无法唯一解释，且实体内在控制区 → Blocked。
        let ctx = context();
        let mut row = row(1, "/root/agent-skills/repo/skill", "skill");
        row.worktree_hints = vec![worktree(
            "/root/agent-skills/repo",
            &[
                "https://github.com/owner/repo.git",
                "https://gitlab.com/other/tree",
            ],
            None,
            "dir",
        )];
        let output = classify(vec![row], &ctx);
        assert_eq!(output.verdicts[0].verdict, VERDICT_BLOCKED);
        assert_eq!(
            output.verdicts[0].reason_kind.as_deref(),
            Some(REASON_UNINTERPRETABLE)
        );
    }

    #[test]
    fn worktree_and_lock_agree_and_aggregate_one_group() {
        let ctx = context();
        let mut row = row(1, "/root/agent-skills/repo/skill", "skill");
        row.worktree_hints = vec![worktree(
            "/root/agent-skills/repo",
            &["https://github.com/owner/repo"],
            Some("ref: refs/heads/main"),
            "dir",
        )];
        row.lock_claims = vec![claim(
            "/installer/.skill-lock.json",
            "skill",
            "github",
            "https://github.com/owner/repo",
            Some("v1.2.3"),
        )];
        let mut row2 = row.clone();
        row2.entity_seq = 2;
        row2.canonical_path = PathBuf::from("/root/agent-skills/repo/other");
        let output = classify(vec![row, row2], &ctx);
        assert_eq!(output.counts.git_groups, 1);
        let group = &output.git_groups[0];
        assert_eq!(group.member_entity_seqs, vec![1, 2]);
        assert_eq!(
            group.repository_root,
            Some(PathBuf::from("/root/agent-skills/repo"))
        );
        assert_eq!(group.refs, vec!["v1.2.3".to_owned()]);
        assert_eq!(
            group.lock_paths,
            vec![PathBuf::from("/installer/.skill-lock.json")]
        );
    }

    #[test]
    fn worktree_lock_contradiction_fails_closed() {
        let ctx = context();
        let mut row = row(1, "/root/agent-skills/repo/skill", "skill");
        row.worktree_hints = vec![worktree(
            "/root/agent-skills/repo",
            &["https://github.com/owner/repo"],
            None,
            "dir",
        )];
        row.lock_claims = vec![claim(
            "/installer/.skill-lock.json",
            "skill",
            "github",
            "https://github.com/other/repo",
            None,
        )];
        let output = classify(vec![row], &ctx);
        assert_eq!(output.counts.git_groups, 0);
        let verdict = &output.verdicts[0];
        assert_eq!(verdict.verdict, VERDICT_BLOCKED);
        assert_eq!(
            verdict.reason_kind.as_deref(),
            Some(REASON_PROVENANCE_CONTRADICTION)
        );
        assert!(verdict.operations.is_empty());
        assert_eq!(output.counts.needs_attention, 1);
    }

    #[test]
    fn claims_for_multiple_repositories_are_provenance_conflict() {
        let ctx = context();
        let mut row = row(1, "/root/installer/skill", "skill");
        row.lock_claims = vec![
            claim(
                "/root/installer/.skill-lock.json",
                "skill",
                "github",
                "https://github.com/owner/one",
                Some("v1"),
            ),
            claim(
                "/root/installer/.skill-lock.json",
                "skill",
                "github",
                "https://github.com/owner/two",
                Some("v1"),
            ),
        ];
        let output = classify(vec![row], &ctx);
        let verdict = &output.verdicts[0];
        assert_eq!(verdict.verdict, VERDICT_BLOCKED);
        assert_eq!(
            verdict.reason_kind.as_deref(),
            Some(REASON_PROVENANCE_CONTRADICTION)
        );
        assert!(verdict.operations.is_empty());
    }

    #[test]
    fn multiple_legacy_refs_are_repository_ref_conflict() {
        // Acceptance: 同 repository 多 legacy ref → Repository Ref Conflict，
        // 不拆分来源。
        let ctx = context();
        let mut row = row(1, "/installer/a/skill", "skill");
        row.lock_claims = vec![claim(
            "/installer/a/.skill-lock.json",
            "skill",
            "github",
            "https://github.com/owner/repo",
            Some("v1.0.0"),
        )];
        let mut row2 = row.clone();
        row2.entity_seq = 2;
        row2.canonical_path = PathBuf::from("/installer/a/skill2");
        row2.lock_claims = vec![claim(
            "/installer/a/.skill-lock.json",
            "skill2",
            "github",
            "https://github.com/owner/repo",
            Some("v2.0.0"),
        )];
        let output = classify(vec![row, row2], &ctx);
        assert_eq!(output.counts.git_groups, 0);
        assert_eq!(output.counts.git_groups_conflicted, 1);
        assert_eq!(output.counts.needs_attention, 2);
        let group = &output.git_groups[0];
        assert_eq!(group.status, "repository_ref_conflict");
        assert_eq!(group.refs, vec!["v1.0.0".to_owned(), "v2.0.0".to_owned()]);
        for verdict in &output.verdicts {
            assert_eq!(verdict.verdict, VERDICT_BLOCKED);
            assert_eq!(verdict.reason_kind.as_deref(), Some(REASON_REF_CONFLICT));
            assert!(verdict.operations.is_empty());
        }
    }

    #[test]
    fn claims_across_multiple_lock_files_are_ownership_split() {
        // Acceptance: 多 lock 文件 → Repository Ownership Split 零写拒绝。
        let ctx = context();
        let mut row = row(1, "/installer/a/skill", "skill");
        row.lock_claims = vec![
            claim(
                "/installer/a/.skill-lock.json",
                "skill",
                "github",
                "https://github.com/owner/repo",
                Some("v1.0.0"),
            ),
            claim(
                "/installer/b/.skill-lock.json",
                "skill",
                "github",
                "https://github.com/owner/repo",
                Some("v1.0.0"),
            ),
        ];
        let output = classify(vec![row], &ctx);
        let verdict = &output.verdicts[0];
        assert_eq!(verdict.verdict, VERDICT_BLOCKED);
        assert_eq!(verdict.reason_kind.as_deref(), Some(REASON_OWNERSHIP_SPLIT));
        assert!(verdict.operations.is_empty());
        assert_eq!(output.git_groups.len(), 0);
    }

    #[test]
    fn local_conflict_set_formed_by_nfc_casefold_name() {
        // Acceptance: Local↔Local 同 NFC + casefold Directory Identity 形成
        // Conflict Set，默认无 winner。
        let mut ctx = context();
        ctx.control_zones.clear();
        let mut row1 = row(1, "/workspace/Café", "Café");
        row1.worktree_hints = vec![worktree(
            "/workspace",
            &["https://github.com/owner/cafe"],
            None,
            "dir",
        )];
        let mut row2 = row(2, "/workspace/café", "café");
        row2.worktree_hints = vec![worktree(
            "/workspace",
            &["https://github.com/owner/cafe"],
            None,
            "dir",
        )];
        let output = classify(vec![row1, row2], &ctx);
        assert_eq!(output.counts.conflict_sets, 1);
        assert_eq!(output.counts.conflict_members, 2);
        assert_eq!(output.counts.local_candidates, 0);
        let set = &output.conflict_sets[0];
        assert_eq!(set.member_entity_seqs, vec![1, 2]);
        assert_eq!(set.winner_entity_seq, None);
        for verdict in &output.verdicts {
            assert_eq!(verdict.verdict, VERDICT_CONFLICT_SET);
            assert_eq!(verdict.conflict_set_seq, Some(1));
            assert!(
                verdict
                    .operations
                    .iter()
                    .any(|op| op.operation == OP_CONFLICT_WINNER && op.allowed)
            );
        }
    }

    #[test]
    fn same_entity_multiple_identities_is_identity_conflict() {
        let ctx = context();
        let row = row_with_names(
            1,
            "/workspace/skill",
            vec!["alpha".to_owned(), "beta".to_owned()],
        );
        let output = classify(vec![row], &ctx);
        let verdict = &output.verdicts[0];
        assert_eq!(verdict.verdict, VERDICT_IDENTITY_CONFLICT);
        assert_eq!(verdict.reason_kind.as_deref(), Some(REASON_MULTI_IDENTITY));
        assert!(verdict.operations.is_empty());
    }

    #[test]
    fn already_managed_and_fixture_are_excluded() {
        let ctx = ScanClassificationContext {
            managed_entity_paths: vec![PathBuf::from("/workspace/managed")],
            ..context()
        };
        let mut row1 = row(1, "/workspace/managed", "managed");
        row1.worktree_hints = vec![worktree(
            "/workspace",
            &["https://github.com/owner/repo"],
            None,
            "dir",
        )];
        let row2 = row(2, "/fixture-entities/skill", "skill");
        let output = classify(vec![row1, row2], &ctx);
        assert_eq!(output.counts.already_managed, 1);
        assert_eq!(output.counts.excluded, 1);
        assert_eq!(output.verdicts[0].verdict, VERDICT_ALREADY_MANAGED);
        assert_eq!(output.verdicts[1].verdict, VERDICT_EXCLUDED);
        assert!(
            output
                .verdicts
                .iter()
                .all(|verdict| verdict.operations.is_empty())
        );
    }

    #[test]
    fn uninterpretable_metadata_inside_is_blocked_outside_is_local_with_note() {
        // 控制区内无法解释 → Blocked；控制区外 → Local + 未采用 Git metadata。
        let mut row = row(1, "/root/agent-skills/broken/skill", "skill");
        row.worktree_hints = vec![worktree(
            "/root/agent-skills/broken",
            &["git@github.com:owner/repo.git"],
            None,
            "dir",
        )];
        let output = classify(vec![row.clone()], &context());
        assert_eq!(output.verdicts[0].verdict, VERDICT_BLOCKED);
        assert_eq!(
            output.verdicts[0].reason_kind.as_deref(),
            Some(REASON_UNINTERPRETABLE)
        );

        let mut outside = row;
        outside.canonical_path = PathBuf::from("/workspace/skill");
        let output = classify(vec![outside], &context());
        assert_eq!(output.verdicts[0].verdict, VERDICT_LOCAL);
        assert!(
            output.verdicts[0]
                .notes
                .contains(&NOTE_GIT_METADATA_NOT_USED.to_owned())
        );
    }

    #[test]
    fn git_member_names_never_join_local_conflict_set() {
        // Acceptance: Git 同名成员不进入 Local Conflict Set。
        let ctx = context();
        let mut git_row = row(1, "/root/agent-skills/repo/skill", "same-name");
        git_row.worktree_hints = vec![worktree(
            "/root/agent-skills/repo",
            &["https://github.com/owner/repo"],
            None,
            "dir",
        )];
        let mut local_row = row(2, "/workspace/same-name", "same-name");
        local_row.worktree_hints = vec![worktree(
            "/workspace",
            &["https://github.com/owner/other"],
            None,
            "dir",
        )];
        let output = classify(vec![git_row, local_row], &ctx);
        assert_eq!(output.counts.conflict_sets, 0);
        assert_eq!(output.verdicts[0].verdict, VERDICT_GIT);
        assert_eq!(output.verdicts[1].verdict, VERDICT_LOCAL);
    }

    #[test]
    fn managed_name_collision_is_typed_and_not_a_replacement() {
        let ctx = ScanClassificationContext {
            managed_directory_names: vec!["Taken".to_owned()],
            ..context()
        };
        let mut row = row(1, "/workspace/taken", "taken");
        row.worktree_hints = vec![worktree(
            "/workspace",
            &["https://github.com/owner/taken"],
            None,
            "dir",
        )];
        let output = classify(vec![row], &ctx);
        let verdict = &output.verdicts[0];
        assert_eq!(verdict.verdict, VERDICT_LOCAL);
        assert_eq!(verdict.reason_kind.as_deref(), Some(REASON_MANAGED_NAME));
        // 非破坏 Local Link 仍然可用。
        assert!(
            verdict
                .operations
                .iter()
                .any(|op| op.operation == OP_LOCAL_LINK && op.allowed)
        );
    }

    #[test]
    fn lock_file_fault_blocks_entities_inside_installer_root() {
        let ctx = ScanClassificationContext {
            faulted_lock_roots: vec![(PathBuf::from("/installer"), "invalid json".to_owned())],
            ..context()
        };
        let output = classify(vec![row(1, "/installer/root/skill", "skill")], &ctx);
        let verdict = &output.verdicts[0];
        assert_eq!(verdict.verdict, VERDICT_BLOCKED);
        assert_eq!(verdict.reason_kind.as_deref(), Some(REASON_LOCK_FILE_FAULT));
    }
}
