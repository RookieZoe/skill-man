//! Adopt evidence ledger (spec §8.1–§8.2, ADR-0013): the read-only scan/plan
//! deepening of the Adopt module. Every candidate carries its full source
//! chain (every Agent appearance, every hop, the final entity), the strict
//! `.skill-lock.json` evidence, the remote/ref/Verification Anchor/tree
//! closed loop and a closed verdict: Local, Verified, Modified, Conflict,
//! Deferred, Blocked or Excluded.
//!
//! Fail-closed rules implemented here:
//! - a chain fault stops at the exact hop and never yields a partial
//!   fingerprint;
//! - a file-level lock fault blocks every candidate the lock's installer
//!   root governs; an entry-level fault blocks only that entry; two valid
//!   declarations for one name are a duplicate-owner Conflict;
//! - a lock declaration for a name whose final entity is not the canonical
//!   installer subdirectory is a Conflict;
//! - remote availability failures are Verification Deferred per normalized
//!   remote group; other groups continue;
//! - plans freeze the evidence generation, entity identity, tree hash, lock
//!   fingerprint and appearance identities; any rescan or external change
//!   makes the plan stale before any write.
//!
//! Nothing in this module writes Catalog, filesystem or lock state; the
//! only writes in the Adopt module happen in `apply`, and this slice
//! restricts them to stable Local Link registrations (the Ownership Handoff
//! for remote intents is the handoff ticket).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use super::*;

/// Closed evidence verdicts (spec §8.2). `selectable` candidates carry an
/// explicit Include control; Blocked/Deferred/Excluded never do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdoptVerdict {
    /// No lock declaration; the entity is owned by the user.
    Local,
    /// Lock closed loop holds and the local tree equals the anchor tree.
    Verified,
    /// Lock closed loop holds but the local tree differs.
    Modified,
    /// Provenance Conflict: lock/identity/source/path/hash/commit/tree
    /// contradictions, duplicate owners, or file/entry-level lock faults.
    Conflict,
    /// Verification Deferred: DNS/TLS/timeout/rate limit/401/403/404.
    Deferred,
    /// The source chain cannot be proven (dangling/cycle/hop-limit/
    /// non-UTF-8/read failure/identity replacement) or the entity is not
    /// readable.
    Blocked,
    /// Fixture footprint; handled by Fixture Recovery, never selectable.
    Excluded,
}

/// Typed reason behind a verdict; rendered as Source Content, never App
/// Copy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdoptVerdictReason {
    /// Local: no lock declaration anywhere.
    NoLock,
    /// Conflict: two lock files validly declare the same skill.
    DuplicateLockOwner { other_lock_path: PathBuf },
    /// Conflict: the governing lock file itself is faulted.
    LockFileFault {
        lock_path: PathBuf,
        fault: crate::seams::installer_lock_store::LockFileFault,
    },
    /// Conflict: the declaring lock entry is structurally faulted.
    LockEntryFault { lock_path: PathBuf, reason: String },
    /// Conflict: the lock declares the name but the final entity is not the
    /// canonical installer subdirectory.
    EntityNotAtInstallerRoot { expected: PathBuf },
    /// Conflict: remote parse or post-fetch verification contradiction.
    RemoteConflict { detail: String },
    /// Conflict: the same final entity appears under different directory
    /// names; rename or remove before rescanning.
    IdentityConflict { names: Vec<String> },
    /// Conflict: a Managed Skill already uses this directory identity.
    LibraryConflict { directory_name: String },
    /// Deferred: availability facts prevented the closed loop.
    RemoteUnavailable { detail: String },
    /// Blocked: the source chain stopped at an exact hop.
    ChainFault { fault: crate::seams::filesystem::ChainFault },
    /// Blocked: the final entity tree cannot be read completely.
    UnreadableEntity { detail: String },
    /// Excluded: fixture footprint.
    FixtureEntity,
}

/// One Agent appearance with its full per-hop chain (spec §8.1).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptAppearanceEvidence {
    pub appearance: AdoptAppearance,
    pub chain: crate::seams::filesystem::EvidenceChain,
}

/// The lock evidence for one candidate (spec §8.1: lock hit path, exact
/// entry and full fingerprint).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptLockEvidence {
    pub lock_path: PathBuf,
    pub lock_fingerprint: String,
    pub entry_name: String,
    /// `None` when the declaring entry is structurally faulted.
    pub entry: Option<crate::seams::installer_lock_store::LockEntry>,
    pub entry_fault: Option<String>,
    pub file_fault: Option<crate::seams::installer_lock_store::LockFileFault>,
}

/// The remote side of a closed loop (spec §8.1: remote/ref/Verification
/// Anchor, skillPath, provider hash, local/remote tree hashes).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptRemoteEvidence {
    pub canonical_url: String,
    pub requested_ref: String,
    pub ref_kind: String,
    pub anchor_commit: String,
    pub original_install_commit_known: bool,
    pub skill_path: String,
    pub provider_hash: String,
    pub provider_hash_matched: bool,
    pub remote_tree_hash: String,
    pub local_tree_hash: String,
    pub trees_match: bool,
    pub default_branch: Option<String>,
}

/// One Adopt candidate in the evidence ledger. Multiple appearances of the
/// same final entity form one candidate and are listed individually.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptEvidenceCandidate {
    pub canonical_entity: PathBuf,
    pub directory_name: String,
    pub directory_names: Vec<String>,
    pub appearances: Vec<AdoptAppearanceEvidence>,
    pub verdict: AdoptVerdict,
    pub reason: Option<AdoptVerdictReason>,
    pub lock: Option<AdoptLockEvidence>,
    pub remote: Option<AdoptRemoteEvidence>,
    /// `tree-sha256-v1` of the final entity; `None` when the chain is
    /// blocked or the entity is a fixture.
    pub local_tree_hash: Option<String>,
    /// True when the entity sits in Home, an Agent skills directory or the
    /// installer/shared root and must be moved to a user-chosen stable
    /// location before a Local Link (spec §8.2).
    pub requires_relocation: bool,
    /// Candidates with an explicit Include control (Local/Verified/
    /// Modified and not identity-conflicted).
    pub selectable: bool,
    /// Stable Local Link candidates applyable by this slice's `apply`.
    pub adoptable: bool,
    pub conflict: Option<LibraryConflict>,
    pub suggested_agent_ids: Vec<AgentId>,
}

/// The read-only evidence report (spec §8.1): candidate context, the full
/// source chain, lock files and per-candidate verdicts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptEvidenceReport {
    pub generation: u64,
    pub candidates: Vec<AdoptEvidenceCandidate>,
    pub lock_files: Vec<crate::seams::installer_lock_store::LockFileReport>,
    pub truncated: bool,
}

/// The three explicit Modified branches (spec §8.2, ADR-0013 §3): all three
/// are visible simultaneously; the default recommendation (keep current
/// bytes) never replaces the user's Include action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModifiedBranch {
    /// Keep the current bytes: Remote Install with a Modified baseline.
    KeepCurrent,
    /// Discard local modifications and reinstall the Verification Anchor.
    DiscardToAnchor,
    /// Keep the current bytes and convert to a Local Link (no Remote
    /// Binding).
    ConvertToLocalLink,
}

/// An explicit selection for one candidate (viewing is never selecting).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptSelection {
    pub canonical_entity: PathBuf,
    /// Overrides the suggested Agents for shared candidates.
    pub agent_ids: Vec<AgentId>,
    /// Required exactly when the candidate verdict is Modified.
    pub modified_branch: Option<ModifiedBranch>,
}

/// The planned operation for one included candidate. This slice only
/// freezes evidence and the handoff intent; acquiring the external lock or
/// Home entity ownership is the handoff ticket's work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdoptPlanIntent {
    /// Register a Link; the source tree stays in place, never copied or
    /// rewritten. The only intent this slice can apply.
    LocalLink,
    /// Local Link after a journaled move to a user-chosen stable location;
    /// the location choice and move are apply-side work of the handoff
    /// ticket.
    LocalLinkWithMove,
    /// Remote Install keeping the current bytes (Modified branch one).
    RemoteInstallKeepCurrent,
    /// Remote Install re-materializing the Verification Anchor (Modified
    /// branch two).
    RemoteInstallDiscardModified,
    /// Convert a Modified candidate to a Local Link (branch three).
    RemoteInstallConvertToLink,
}

/// The frozen evidence a plan binds to (spec §8.1, §11): a plan is stale
/// when the generation, entity identity, tree, lock bytes or any appearance
/// identity changes before apply.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptFrozenEvidence {
    pub evidence_generation: u64,
    pub canonical_entity: PathBuf,
    pub entity_device: u64,
    pub entity_inode: u64,
    pub tree_hash: String,
    pub lock_path: Option<PathBuf>,
    pub lock_fingerprint: Option<String>,
    pub lock_entry_name: Option<String>,
    pub appearance_identities: Vec<AppearanceIdentity>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppearanceIdentity {
    pub entry_path: PathBuf,
    pub device: u64,
    pub inode: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptPlanItem {
    pub directory_name: String,
    pub canonical_entity: PathBuf,
    pub intent: AdoptPlanIntent,
    /// The Link target: the current entity for Local Link intents; empty
    /// for Remote Install intents (the Home path is the handoff ticket's).
    pub final_entity_path: PathBuf,
    pub appearances: Vec<AdoptAppearanceEvidence>,
    pub target_agents: Vec<AdoptAgent>,
    pub frozen: AdoptFrozenEvidence,
    /// Whether this slice's `apply` can execute the intent.
    pub applyable: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptPlan {
    pub plan_token: String,
    pub evidence_generation: u64,
    pub items: Vec<AdoptPlanItem>,
    pub can_apply: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdoptPlanRequest {
    pub evidence_generation: u64,
    pub selections: Vec<AdoptSelection>,
}

/// The stored plan: frozen evidence plus the selections, bound to one
/// evidence generation (the token itself is the map key; staleness is
/// enforced by the frozen evidence re-check and the durable batch TTL).
#[derive(Clone)]
pub(super) struct EvidencePlannedBatch {
    pub items: Vec<PlannedEvidenceItem>,
}

#[derive(Clone)]
pub(super) struct PlannedEvidenceItem {
    pub candidate: AdoptEvidenceCandidate,
    pub intent: AdoptPlanIntent,
    pub frozen: AdoptFrozenEvidence,
    pub target_agent_ids: Vec<AgentId>,
}

impl AdoptService {
    /// Scan every detected Agent skills directory plus the shared/installer
    /// root (`~/.agents/skills`) and assemble the read-only evidence ledger
    /// (spec §8.1). The report is generation-bound: any later scan bumps the
    /// generation and invalidates older plans. No Catalog, filesystem or
    /// lock write happens here.
    pub fn scan(&self) -> Result<AdoptEvidenceReport, AdoptError> {
        let (report, workspaces) = self.scan_with_workspaces()?;
        for workspace in workspaces {
            let _ = self.filesystem.discard_temp_workspace(&workspace);
        }
        Ok(report)
    }

    /// Internal scan that also returns the remote-verification workspaces
    /// (one per normalized remote, shared across the group's candidates,
    /// ADR-0013 §2.3) so callers can discard them after assembly.
    fn scan_with_workspaces(
        &self,
    ) -> Result<(AdoptEvidenceReport, Vec<PathBuf>), AdoptError> {
        let agents = self.store.list_agents()?;
        let lock_files = self
            .lock_store
            .discover()
            .map_err(AdoptError::Lock)?;
        let shared_dir = self.home_directory.join(".agents").join("skills");
        // Canonical spellings for identity comparisons: the evidence walk
        // resolves system symlinks (macOS `/var -> /private/var`), so every
        // raw root must be canonicalized before `starts_with`/equality
        // checks against canonical entities.
        let installer_root = self.seam_canonical_root(&shared_dir);
        let library_root = self.seam_canonical_root(&self.library_root);
        let agent_roots = agents
            .iter()
            .filter(|agent| agent.detected)
            .map(|agent| {
                self.filesystem
                    .normalize_configured_path(&agent.skills_path)
                    .map(|path| self.seam_canonical_root(&path))
                    .unwrap_or_else(|_| agent.skills_path.clone())
            })
            .collect::<Vec<_>>();

        let mut grouped: BTreeMap<PathBuf, GroupedEvidence> = BTreeMap::new();
        for agent in &agents {
            if !agent.detected {
                continue;
            }
            for entry in self.filesystem.scan_skills_evidence(&agent.skills_path)? {
                self.accumulate_evidence(entry, Some(agent), false, &mut grouped);
            }
        }
        for entry in self.filesystem.scan_skills_evidence(&shared_dir)? {
            self.accumulate_evidence(entry, None, true, &mut grouped);
        }

        let suggested = agents
            .iter()
            .filter(|agent| {
                agent.detected
                    && matches!(agent.kind, AgentKind::ClaudePreset | AgentKind::CodexPreset)
            })
            .map(|agent| agent.agent_id.clone())
            .collect::<Vec<_>>();
        let mut candidates = Vec::new();
        // One temp workspace per normalized remote, shared by every
        // candidate of that remote and discarded after the whole scan
        // (ADR-0013 §2.3: 按 normalized remote 共享一次 fetch).
        let mut workspaces: HashMap<String, PathBuf> = HashMap::new();
        for (key, group) in grouped {
            if let Some(blocked) = group.blocked {
                candidates.push(blocked);
                continue;
            }
            if let Some(excluded) = group.excluded {
                candidates.push(excluded);
                continue;
            }
            let entity = key;
            let directory_names = group.directory_names.iter().cloned().collect::<Vec<_>>();
            let directory_name = directory_names[0].clone();
            let mut candidate = self.assemble_candidate(
                &entity,
                &directory_name,
                &directory_names,
                group.appearances,
                &lock_files,
                &installer_root,
                &library_root,
                &agent_roots,
                &suggested,
                &mut workspaces,
            )?;
            candidate.appearances.sort_by(|left, right| {
                left.appearance
                    .entry_path
                    .cmp(&right.appearance.entry_path)
            });
            candidates.push(candidate);
        }
        candidates.sort_by(|left, right| left.directory_name.cmp(&right.directory_name));
        let truncated = candidates.len() > MAX_ADOPT_SKILLS;
        candidates.truncate(MAX_ADOPT_SKILLS);
        let generation = self.evidence_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let report = AdoptEvidenceReport {
            generation,
            candidates,
            lock_files,
            truncated,
        };
        if let Ok(mut last) = self.last_report.lock() {
            *last = Some(report.clone());
        }
        Ok((report, workspaces.into_values().collect()))
    }

    fn accumulate_evidence(
        &self,
        entry: crate::seams::filesystem::ScannedSkillEvidence,
        agent: Option<&AdoptAgent>,
        shared: bool,
        grouped: &mut BTreeMap<PathBuf, GroupedEvidence>,
    ) {
        if entry.name.starts_with('.') {
            return;
        }
        if let Some(agent) = agent {
            if agent.kind == AgentKind::CodexPreset && entry.name == ".system" {
                return;
            }
        }
        let appearance = AdoptAppearance {
            entry_path: entry.entry_path.clone(),
            kind: match entry.chain.hops.first() {
                Some(crate::seams::filesystem::EvidenceChainHop {
                    kind: crate::seams::filesystem::EvidenceChainHopKind::Symlink { .. },
                    ..
                }) => AdoptAppearanceKind::Symlink {
                    original_target: match entry.chain.hops.first() {
                        Some(crate::seams::filesystem::EvidenceChainHop {
                            kind: crate::seams::filesystem::EvidenceChainHopKind::Symlink {
                                target,
                            },
                            ..
                        }) => target.clone(),
                        _ => PathBuf::new(),
                    },
                },
                _ => AdoptAppearanceKind::RealDirectory,
            },
            agent_id: agent.map(|agent| agent.agent_id.clone()),
            shared,
        };
        let Some(final_entity) = entry.chain.final_entity.clone() else {
            let fault = entry
                .chain
                .fault
                .clone()
                .unwrap_or(crate::seams::filesystem::ChainFault::ReadFailed {
                    at: entry.entry_path.clone(),
                    detail: "the chain stopped without a fault".into(),
                });
            grouped
                .entry(entry.entry_path)
                .or_default()
                .blocked = Some(self.blocked_candidate(&entry, &fault));
            return;
        };
        // Fixture footprints are visible in the ledger as Excluded (spec
        // §8.2): Fixture Recovery owns them, they never enter a selection.
        if is_fixture_entity(&final_entity) {
            grouped
                .entry(entry.entry_path)
                .or_default()
                .excluded = Some(self.excluded_candidate(&entry, &final_entity));
            return;
        }
        if final_entity.starts_with(&self.seam_canonical_root(&self.library_root)) {
            return;
        }
        if !self.filesystem.skill_directory_is_readable(&final_entity).unwrap_or(false) {
            grouped
                .entry(entry.entry_path)
                .or_default()
                .blocked = Some(self.blocked_candidate(
                &entry,
                &crate::seams::filesystem::ChainFault::ReadFailed {
                    at: final_entity.clone(),
                    detail: "the final entity is not a readable directory".into(),
                },
            ));
            return;
        }
        let group = grouped.entry(final_entity).or_default();
        group.directory_names.insert(entry.name.clone());
        group.appearances.push(AdoptAppearanceEvidence {
            appearance,
            chain: entry.chain,
        });
    }

    fn blocked_candidate(
        &self,
        entry: &crate::seams::filesystem::ScannedSkillEvidence,
        fault: &crate::seams::filesystem::ChainFault,
    ) -> AdoptEvidenceCandidate {
        AdoptEvidenceCandidate {
            canonical_entity: entry.entry_path.clone(),
            directory_name: entry.name.clone(),
            directory_names: vec![entry.name.clone()],
            appearances: vec![AdoptAppearanceEvidence {
                appearance: AdoptAppearance {
                    entry_path: entry.entry_path.clone(),
                    kind: match entry.chain.hops.first() {
                        Some(crate::seams::filesystem::EvidenceChainHop {
                            kind: crate::seams::filesystem::EvidenceChainHopKind::Symlink {
                                target,
                            },
                            ..
                        }) => AdoptAppearanceKind::Symlink {
                            original_target: target.clone(),
                        },
                        _ => AdoptAppearanceKind::RealDirectory,
                    },
                    agent_id: None,
                    shared: false,
                },
                chain: entry.chain.clone(),
            }],
            verdict: AdoptVerdict::Blocked,
            reason: Some(AdoptVerdictReason::ChainFault {
                fault: fault.clone(),
            }),
            lock: None,
            remote: None,
            local_tree_hash: None,
            requires_relocation: false,
            selectable: false,
            adoptable: false,
            conflict: None,
            suggested_agent_ids: Vec::new(),
        }
    }

    fn excluded_candidate(
        &self,
        entry: &crate::seams::filesystem::ScannedSkillEvidence,
        final_entity: &Path,
    ) -> AdoptEvidenceCandidate {
        AdoptEvidenceCandidate {
            canonical_entity: final_entity.to_path_buf(),
            directory_name: entry.name.clone(),
            directory_names: vec![entry.name.clone()],
            appearances: vec![AdoptAppearanceEvidence {
                appearance: AdoptAppearance {
                    entry_path: entry.entry_path.clone(),
                    kind: match entry.chain.hops.first() {
                        Some(crate::seams::filesystem::EvidenceChainHop {
                            kind: crate::seams::filesystem::EvidenceChainHopKind::Symlink {
                                target,
                            },
                            ..
                        }) => AdoptAppearanceKind::Symlink {
                            original_target: target.clone(),
                        },
                        _ => AdoptAppearanceKind::RealDirectory,
                    },
                    agent_id: None,
                    shared: false,
                },
                chain: entry.chain.clone(),
            }],
            verdict: AdoptVerdict::Excluded,
            reason: Some(AdoptVerdictReason::FixtureEntity),
            lock: None,
            remote: None,
            local_tree_hash: None,
            requires_relocation: false,
            selectable: false,
            adoptable: false,
            conflict: None,
            suggested_agent_ids: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble_candidate(
        &self,
        entity: &Path,
        directory_name: &str,
        directory_names: &[String],
        appearances: Vec<AdoptAppearanceEvidence>,
        lock_files: &[crate::seams::installer_lock_store::LockFileReport],
        installer_root: &Path,
        library_root: &Path,
        agent_roots: &[PathBuf],
        suggested: &[AgentId],
        workspaces: &mut HashMap<String, PathBuf>,
    ) -> Result<AdoptEvidenceCandidate, AdoptError> {
        // 1. Fixture footprint is Excluded before anything else (spec §3.5,
        // §8.2): Fixture Recovery owns it.
        if is_fixture_entity(entity) {
            return Ok(AdoptEvidenceCandidate {
                canonical_entity: entity.to_path_buf(),
                directory_name: directory_name.to_owned(),
                directory_names: directory_names.to_vec(),
                appearances,
                verdict: AdoptVerdict::Excluded,
                reason: Some(AdoptVerdictReason::FixtureEntity),
                lock: None,
                remote: None,
                local_tree_hash: None,
                requires_relocation: false,
                selectable: false,
                adoptable: false,
                conflict: None,
                suggested_agent_ids: Vec::new(),
            });
        }

        // 2. The whole tree must hash or the candidate is Blocked (no
        // partial fingerprint, spec §8.1).
        let local_tree_hash = match self.filesystem.tree_hash(entity) {
            Ok(hash) => hash,
            Err(error) => {
                return Ok(AdoptEvidenceCandidate {
                    canonical_entity: entity.to_path_buf(),
                    directory_name: directory_name.to_owned(),
                    directory_names: directory_names.to_vec(),
                    appearances,
                    verdict: AdoptVerdict::Blocked,
                    reason: Some(AdoptVerdictReason::UnreadableEntity {
                        detail: error.to_string(),
                    }),
                    lock: None,
                    remote: None,
                    local_tree_hash: None,
                    requires_relocation: false,
                    selectable: false,
                    adoptable: false,
                    conflict: None,
                    suggested_agent_ids: Vec::new(),
                });
            }
        };
        if is_fixture_tree_hash(&local_tree_hash) {
            return Ok(AdoptEvidenceCandidate {
                canonical_entity: entity.to_path_buf(),
                directory_name: directory_name.to_owned(),
                directory_names: directory_names.to_vec(),
                appearances,
                verdict: AdoptVerdict::Excluded,
                reason: Some(AdoptVerdictReason::FixtureEntity),
                lock: None,
                remote: None,
                local_tree_hash: None,
                requires_relocation: false,
                selectable: false,
                adoptable: false,
                conflict: None,
                suggested_agent_ids: Vec::new(),
            });
        }

        // 3. One final entity under different directory names is an
        // Identity Conflict (ADR-0013 §3).
        if directory_names.len() > 1 {
            return Ok(AdoptEvidenceCandidate {
                canonical_entity: entity.to_path_buf(),
                directory_name: directory_name.to_owned(),
                directory_names: directory_names.to_vec(),
                appearances,
                verdict: AdoptVerdict::Conflict,
                reason: Some(AdoptVerdictReason::IdentityConflict {
                    names: directory_names.to_vec(),
                }),
                lock: None,
                remote: None,
                local_tree_hash: Some(local_tree_hash),
                requires_relocation: false,
                selectable: false,
                adoptable: false,
                conflict: None,
                suggested_agent_ids: Vec::new(),
            });
        }

        let conflict = self
            .store
            .find_library_conflict(&crate::core::import::normalize_identity(directory_name).unwrap_or_default())?
            .filter(|found| found.final_entity_path != entity)
            .map(|found| LibraryConflict {
                existing_skill_id: found.skill_id,
                directory_name: found.directory_name,
            });
        if conflict.is_some() {
            return Ok(AdoptEvidenceCandidate {
                canonical_entity: entity.to_path_buf(),
                directory_name: directory_name.to_owned(),
                directory_names: directory_names.to_vec(),
                appearances,
                verdict: AdoptVerdict::Conflict,
                reason: Some(AdoptVerdictReason::LibraryConflict {
                    directory_name: conflict
                        .as_ref()
                        .map(|found| found.directory_name.clone())
                        .unwrap_or_default(),
                }),
                lock: None,
                remote: None,
                local_tree_hash: Some(local_tree_hash),
                requires_relocation: false,
                selectable: false,
                adoptable: false,
                conflict,
                suggested_agent_ids: Vec::new(),
            });
        }

        // 4. Lock discovery and the closed-loop verdict (ADR-0013 §2).
        let shared = appearances
            .iter()
            .any(|appearance| appearance.appearance.shared);
        let suggested_agent_ids = if shared {
            suggested.to_vec()
        } else {
            Vec::new()
        };
        let (verdict, reason, lock_evidence, remote_evidence) = self.classify_with_lock(
            entity,
            directory_name,
            lock_files,
            installer_root,
            &local_tree_hash,
            workspaces,
        )?;
        let requires_relocation = entity.starts_with(library_root)
            || entity.starts_with(installer_root)
            || agent_roots.iter().any(|root| entity.starts_with(root));
        let selectable = matches!(
            verdict,
            AdoptVerdict::Local | AdoptVerdict::Verified | AdoptVerdict::Modified
        );
        Ok(AdoptEvidenceCandidate {
            canonical_entity: entity.to_path_buf(),
            directory_name: directory_name.to_owned(),
            directory_names: directory_names.to_vec(),
            appearances,
            verdict,
            reason,
            lock: lock_evidence,
            remote: remote_evidence,
            local_tree_hash: Some(local_tree_hash),
            requires_relocation,
            selectable,
            adoptable: selectable && !requires_relocation && conflict.is_none(),
            conflict,
            suggested_agent_ids,
        })
    }

    /// Lock discovery and remote verification for one candidate. Network
    /// Deferred is grouped by normalized remote: one failed group never
    /// blocks other groups (ADR-0013 §2.3).
    fn classify_with_lock(
        &self,
        entity: &Path,
        directory_name: &str,
        lock_files: &[crate::seams::installer_lock_store::LockFileReport],
        installer_root: &Path,
        local_tree_hash: &str,
        workspaces: &mut HashMap<String, PathBuf>,
    ) -> Result<
        (
            AdoptVerdict,
            Option<AdoptVerdictReason>,
            Option<AdoptLockEvidence>,
            Option<AdoptRemoteEvidence>,
        ),
        AdoptError,
    > {
        // File-level faults block every candidate the lock's installer root
        // governs (ADR-0013 §2.1). The default lock at
        // `~/.agents/.skill-lock.json` is the only one governing the
        // `~/.agents/skills` root Adopt scans; the XDG variant governs a
        // different root that is not an Adopt scan source, so a fault there
        // must not block candidates here.
        if entity.starts_with(installer_root) {
            let default_lock = self.home_directory.join(".agents/.skill-lock.json");
            if let Some(faulted) = lock_files
                .iter()
                .find(|report| report.fault.is_some() && report.path == default_lock)
            {
                let fault = faulted.fault.clone().expect("file fault present");
                return Ok((
                    AdoptVerdict::Conflict,
                    Some(AdoptVerdictReason::LockFileFault {
                        lock_path: faulted.path.clone(),
                        fault: fault.clone(),
                    }),
                    Some(AdoptLockEvidence {
                        lock_path: faulted.path.clone(),
                        lock_fingerprint: faulted.fingerprint.clone(),
                        entry_name: directory_name.to_owned(),
                        entry: None,
                        entry_fault: None,
                        file_fault: Some(fault),
                    }),
                    None,
                ));
            }
        }
        let valid_declarations = lock_files
            .iter()
            .filter(|report| report.fault.is_none())
            .flat_map(|report| {
                report
                    .entries
                    .iter()
                    .filter(move |entry| entry.name == directory_name)
                    .map(move |entry| (report, entry))
            })
            .collect::<Vec<_>>();
        if valid_declarations.len() > 1 {
            let other_path = valid_declarations[1].0.path.clone();
            return Ok((
                AdoptVerdict::Conflict,
                Some(AdoptVerdictReason::DuplicateLockOwner { other_lock_path: other_path }),
                Some(AdoptLockEvidence {
                    lock_path: valid_declarations[0].0.path.clone(),
                    lock_fingerprint: valid_declarations[0].0.fingerprint.clone(),
                    entry_name: directory_name.to_owned(),
                    entry: None,
                    entry_fault: None,
                    file_fault: None,
                }),
                None,
            ));
        }
        if valid_declarations.is_empty() {
            // An entry-level fault for this name blocks only this entry.
            let entry_fault = lock_files
                .iter()
                .flat_map(|report| report.entry_faults.iter())
                .find(|fault| fault.name == directory_name);
            if let Some(fault) = entry_fault {
                return Ok((
                    AdoptVerdict::Conflict,
                    Some(AdoptVerdictReason::LockEntryFault {
                        lock_path: lock_files
                            .iter()
                            .find(|report| {
                                report
                                    .entry_faults
                                    .iter()
                                    .any(|entry| entry.name == directory_name)
                            })
                            .map(|report| report.path.clone())
                            .unwrap_or_default(),
                        reason: fault.reason.clone(),
                    }),
                    Some(AdoptLockEvidence {
                        lock_path: lock_files
                            .iter()
                            .find(|report| {
                                report
                                    .entry_faults
                                    .iter()
                                    .any(|entry| entry.name == directory_name)
                            })
                            .map(|report| report.path.clone())
                            .unwrap_or_default(),
                        lock_fingerprint: String::new(),
                        entry_name: directory_name.to_owned(),
                        entry: None,
                        entry_fault: Some(fault.reason.clone()),
                        file_fault: None,
                    }),
                    None,
                ));
            }
            return Ok((AdoptVerdict::Local, Some(AdoptVerdictReason::NoLock), None, None));
        }
        let (declaration_report, declaration) = valid_declarations[0];
        let canonical_subdirectory = installer_root.join(directory_name);
        if entity != canonical_subdirectory {
            return Ok((
                AdoptVerdict::Conflict,
                Some(AdoptVerdictReason::EntityNotAtInstallerRoot {
                    expected: canonical_subdirectory,
                }),
                Some(AdoptLockEvidence {
                    lock_path: declaration_report.path.clone(),
                    lock_fingerprint: declaration_report.fingerprint.clone(),
                    entry_name: directory_name.to_owned(),
                    entry: None,
                    entry_fault: None,
                    file_fault: None,
                }),
                None,
            ));
        }
        // The lock governs this exact canonical entity: remote verification.
        let lock_path = declaration_report.path.clone();
        let lock_evidence = AdoptLockEvidence {
            lock_path: lock_path.clone(),
            lock_fingerprint: declaration_report.fingerprint.clone(),
            entry_name: directory_name.to_owned(),
            entry: Some(declaration.clone()),
            entry_fault: None,
            file_fault: None,
        };
        let request = match self.remote_provider.parse_request(declaration) {
            Ok(request) => request,
            Err(error) => {
                return Ok((
                    AdoptVerdict::Conflict,
                    Some(AdoptVerdictReason::RemoteConflict {
                        detail: error.to_string(),
                    }),
                    Some(lock_evidence),
                    None,
                ));
            }
        };
        // Verification Deferred grouping: one failed group marks every
        // candidate of the same canonical remote; other groups continue.
        // The workspace is shared by the whole remote group and discarded
        // by the caller after the scan (ADR-0013 §2.3).
        let workspace = match workspaces.get(&request.canonical_url) {
            Some(workspace) => workspace.clone(),
            None => {
                let workspace = self
                    .filesystem
                    .create_temp_workspace("adopt-evidence")
                    .map_err(|error| {
                        AdoptError::Internal(format!("temp workspace: {error}"))
                    })?;
                workspaces.insert(request.canonical_url.clone(), workspace.clone());
                workspace
            }
        };
        let outcome = self.remote_provider.verify(&request, &workspace);
        let facts = match outcome {
            Ok(facts) => facts,
            Err(RemoteProviderError::Deferred(detail)) => {
                return Ok((
                    AdoptVerdict::Deferred,
                    Some(AdoptVerdictReason::RemoteUnavailable { detail }),
                    Some(lock_evidence),
                    None,
                ));
            }
            Err(RemoteProviderError::Conflict(detail)) => {
                return Ok((
                    AdoptVerdict::Conflict,
                    Some(AdoptVerdictReason::RemoteConflict { detail }),
                    Some(lock_evidence),
                    None,
                ));
            }
        };
        let remote_tree_hash = match self.filesystem.tree_hash(&facts.materialized_root) {
            Ok(hash) => hash,
            Err(error) => {
                return Ok((
                    AdoptVerdict::Conflict,
                    Some(AdoptVerdictReason::RemoteConflict {
                        detail: format!("the remote tree could not be hashed: {error}"),
                    }),
                    Some(lock_evidence),
                    None,
                ));
            }
        };
        let trees_match = remote_tree_hash == local_tree_hash;
        let verdict = if trees_match {
            AdoptVerdict::Verified
        } else {
            AdoptVerdict::Modified
        };
        let remote = AdoptRemoteEvidence {
            canonical_url: request.canonical_url,
            requested_ref: request.requested_ref,
            ref_kind: match facts.disposition {
                crate::seams::remote_provider::RefDisposition::Head => "head".into(),
                crate::seams::remote_provider::RefDisposition::Branch => "branch".into(),
                crate::seams::remote_provider::RefDisposition::Tag => "tag".into(),
                crate::seams::remote_provider::RefDisposition::Commit => "commit".into(),
            },
            anchor_commit: facts.anchor.anchor_commit,
            original_install_commit_known: facts.anchor.original_install_commit_known,
            skill_path: request.skill_path,
            provider_hash: request.provider_hash,
            provider_hash_matched: facts.provider_hash_matched,
            remote_tree_hash,
            local_tree_hash: local_tree_hash.to_owned(),
            trees_match,
            default_branch: facts.default_branch,
        };
        Ok((
            verdict,
            None,
            Some(lock_evidence),
            Some(remote),
        ))
    }

    /// Generation-bound plan: freezes the scan evidence plus the explicit
    /// selections into handoff intents. Read-only: no Catalog, filesystem
    /// or lock write. A rescan or any external change makes the plan stale
    /// before any write.
    pub fn plan(&self, request: &AdoptPlanRequest) -> Result<AdoptPlan, AdoptError> {
        let report = {
            let last = self
                .last_report
                .lock()
                .map_err(|_| AdoptError::Internal("Adopt report lock poisoned".into()))?;
            last.clone()
                .ok_or(AdoptError::PlanStale)?
        };
        if request.evidence_generation != report.generation {
            return Err(AdoptError::PlanStale);
        }
        let agents = self.store.list_agents()?;
        let mut items = Vec::with_capacity(request.selections.len());
        let mut can_apply = true;
        for selection in &request.selections {
            let Some(candidate) = report
                .candidates
                .iter()
                .find(|candidate| candidate.canonical_entity == selection.canonical_entity)
            else {
                return Err(AdoptError::PlanStale);
            };
            if !candidate.selectable {
                return Err(AdoptError::Validation(
                    "this candidate cannot be included; its verdict is closed".into(),
                ));
            }
            if candidate.verdict == AdoptVerdict::Modified
                && selection.modified_branch.is_none()
            {
                return Err(AdoptError::Validation(
                    "a Modified candidate requires an explicit three-way branch choice".into(),
                ));
            }
            // Re-verify the frozen world read-only: any change since the
            // scan is a stale plan (spec §8.1, §11).
            self.verify_frozen_evidence(candidate)?;
            let intent = match candidate.verdict {
                AdoptVerdict::Local => {
                    if candidate.requires_relocation {
                        AdoptPlanIntent::LocalLinkWithMove
                    } else {
                        AdoptPlanIntent::LocalLink
                    }
                }
                AdoptVerdict::Verified => AdoptPlanIntent::RemoteInstallKeepCurrent,
                AdoptVerdict::Modified => match selection.modified_branch {
                    Some(ModifiedBranch::KeepCurrent) => {
                        AdoptPlanIntent::RemoteInstallKeepCurrent
                    }
                    Some(ModifiedBranch::DiscardToAnchor) => {
                        AdoptPlanIntent::RemoteInstallDiscardModified
                    }
                    Some(ModifiedBranch::ConvertToLocalLink) => {
                        AdoptPlanIntent::RemoteInstallConvertToLink
                    }
                    None => unreachable!("Modified branch enforced above"),
                },
                _ => {
                    return Err(AdoptError::Validation(
                        "this candidate cannot be planned; its verdict is closed".into(),
                    ));
                }
            };
            let applyable = matches!(intent, AdoptPlanIntent::LocalLink);
            can_apply = can_apply && applyable;
            let target_agent_ids = if candidate
                .appearances
                .iter()
                .any(|appearance| appearance.appearance.shared)
            {
                if selection.agent_ids.is_empty() {
                    candidate.suggested_agent_ids.clone()
                } else {
                    selection.agent_ids.clone()
                }
            } else {
                Vec::new()
            };
            items.push(PlannedEvidenceItem {
                candidate: candidate.clone(),
                intent,
                frozen: self.freeze_evidence(candidate, report.generation)?,
                target_agent_ids,
            });
        }
        let plan_number = self.next_plan_id.fetch_add(1, Ordering::Relaxed);
        let plan_token = format!("adopt-plan-{plan_number}");
        let plan_items = items
            .iter()
            .map(|item| AdoptPlanItem {
                directory_name: item.candidate.directory_name.clone(),
                canonical_entity: item.candidate.canonical_entity.clone(),
                intent: item.intent,
                final_entity_path: if matches!(item.intent, AdoptPlanIntent::LocalLink) {
                    item.candidate.canonical_entity.clone()
                } else {
                    PathBuf::new()
                },
                appearances: item.candidate.appearances.clone(),
                target_agents: agents
                    .iter()
                    .filter(|agent| item.target_agent_ids.contains(&agent.agent_id))
                    .cloned()
                    .collect(),
                frozen: item.frozen.clone(),
                applyable: matches!(item.intent, AdoptPlanIntent::LocalLink),
                error: None,
            })
            .collect::<Vec<_>>();
        let batch = EvidencePlannedBatch { items };
        let mut evidence_plans = self
            .evidence_plans
            .lock()
            .map_err(|_| AdoptError::Internal("Adopt evidence plan lock poisoned".into()))?;
        evidence_plans.insert(plan_token.clone(), batch.clone());
        // When every included candidate is a stable Local Link, register the
        // durable per-Skill batch too, so `apply` runs the journaled
        // machinery with per-item rollback and conditional Undo. Remote
        // intents keep their frozen handoff intent only.
        let mut batch_registered = false;
        if can_apply {
            let inputs = items_for_durable_batch(
                &request.selections,
                &agents,
                &batch,
            )?;
            if let Some(inputs) = inputs {
                let durable = self.build_planned_batch(&plan_token, inputs, &agents)?;
                batch_registered = true;
                let _ = durable;
            }
        }
        Ok(AdoptPlan {
            plan_token,
            evidence_generation: report.generation,
            items: plan_items,
            can_apply: can_apply && batch_registered,
        })
    }

    /// Read-only re-verification that the scan-time facts still hold.
    fn verify_frozen_evidence(
        &self,
        candidate: &AdoptEvidenceCandidate,
    ) -> Result<(), AdoptError> {
        for appearance in &candidate.appearances {
            let chain = self
                .filesystem
                .inspect_evidence_chain(&appearance.appearance.entry_path)
                .map_err(|_| AdoptError::PlanStale)?;
            if chain.fault.is_some()
                || chain.final_entity != Some(candidate.canonical_entity.clone())
                || chain.entry_device != appearance.chain.entry_device
                || chain.entry_inode != appearance.chain.entry_inode
            {
                return Err(AdoptError::PlanStale);
            }
        }
        let current_hash = self
            .filesystem
            .tree_hash(&candidate.canonical_entity)
            .map_err(|_| AdoptError::PlanStale)?;
        if current_hash != candidate.local_tree_hash.as_deref().unwrap_or_default() {
            return Err(AdoptError::PlanStale);
        }
        if let Some(lock) = &candidate.lock {
            if !lock.lock_fingerprint.is_empty() {
                let reports = self
                    .lock_store
                    .discover()
                    .map_err(AdoptError::Lock)?;
                let still_matching = reports
                    .iter()
                    .any(|report| report.path == lock.lock_path && report.fingerprint == lock.lock_fingerprint);
                if !still_matching {
                    return Err(AdoptError::PlanStale);
                }
            }
        }
        Ok(())
    }

    fn freeze_evidence(
        &self,
        candidate: &AdoptEvidenceCandidate,
        generation: u64,
    ) -> Result<AdoptFrozenEvidence, AdoptError> {
        let fingerprint = self
            .filesystem
            .directory_fingerprint(&candidate.canonical_entity)
            .map_err(|_| AdoptError::PlanStale)?;
        let mut appearance_identities = Vec::new();
        for appearance in &candidate.appearances {
            let chain = self
                .filesystem
                .inspect_evidence_chain(&appearance.appearance.entry_path)
                .map_err(|_| AdoptError::PlanStale)?;
            appearance_identities.push(AppearanceIdentity {
                entry_path: appearance.appearance.entry_path.clone(),
                device: chain.entry_device,
                inode: chain.entry_inode,
            });
        }
        Ok(AdoptFrozenEvidence {
            evidence_generation: generation,
            canonical_entity: candidate.canonical_entity.clone(),
            entity_device: fingerprint.device,
            entity_inode: fingerprint.inode,
            tree_hash: candidate
                .local_tree_hash
                .clone()
                .unwrap_or_default(),
            lock_path: candidate
                .lock
                .as_ref()
                .map(|lock| lock.lock_path.clone()),
            lock_fingerprint: candidate
                .lock
                .as_ref()
                .map(|lock| lock.lock_fingerprint.clone()),
            lock_entry_name: candidate
                .lock
                .as_ref()
                .map(|lock| lock.entry_name.clone()),
            appearance_identities,
        })
    }

    /// Re-check the frozen evidence of an evidence plan item before any
    /// write: the generation, the entity identity, the tree, the lock bytes
    /// and every appearance identity (spec §8.1 TOCTOU row).
    pub(super) fn recheck_frozen_evidence(&self, frozen: &AdoptFrozenEvidence) -> Result<(), AdoptError> {
        if self.evidence_generation.load(Ordering::Relaxed) != frozen.evidence_generation {
            return Err(AdoptError::PlanStale);
        }
        let fingerprint = self
            .filesystem
            .directory_fingerprint(&frozen.canonical_entity)
            .map_err(|_| AdoptError::PlanStale)?;
        if fingerprint.device != frozen.entity_device || fingerprint.inode != frozen.entity_inode {
            return Err(AdoptError::PlanStale);
        }
        let current_hash = self
            .filesystem
            .tree_hash(&frozen.canonical_entity)
            .map_err(|_| AdoptError::PlanStale)?;
        if current_hash != frozen.tree_hash {
            return Err(AdoptError::PlanStale);
        }
        for identity in &frozen.appearance_identities {
            let chain = self
                .filesystem
                .inspect_evidence_chain(&identity.entry_path)
                .map_err(|_| AdoptError::PlanStale)?;
            if chain.fault.is_some()
                || chain.entry_device != identity.device
                || chain.entry_inode != identity.inode
                || chain.final_entity != Some(frozen.canonical_entity.clone())
            {
                return Err(AdoptError::PlanStale);
            }
        }
        if let (Some(lock_path), Some(expected)) =
            (&frozen.lock_path, &frozen.lock_fingerprint)
        {
            if !expected.is_empty() {
                let reports = self.lock_store.discover().map_err(AdoptError::Lock)?;
                let still_matching = reports.iter().any(|report| {
                    report.path == *lock_path && report.fingerprint == *expected
                });
                if !still_matching {
                    return Err(AdoptError::PlanStale);
                }
            }
        }
        Ok(())
    }

    /// Discard a read-only plan. An applyable plan registers BOTH the
    /// evidence batch and the durable per-Skill batch under the same token;
    /// cancel must remove both or `apply` could still execute the durable
    /// batch without its frozen-evidence recheck (spec §11).
    pub fn cancel(&self, plan_token: &str) -> Result<bool, AdoptError> {
        let evidence_removed = self
            .evidence_plans
            .lock()
            .map_err(|_| AdoptError::Internal("Adopt evidence plan lock poisoned".into()))?
            .remove(plan_token)
            .is_some();
        let durable_removed = self
            .plans
            .lock()
            .map_err(|_| AdoptError::Internal("Adopt plan lock poisoned".into()))?
            .remove(plan_token)
            .is_some();
        Ok(evidence_removed || durable_removed)
    }

}

fn items_for_durable_batch(
    selections: &[AdoptSelection],
    agents: &[AdoptAgent],
    batch: &EvidencePlannedBatch,
) -> Result<Option<Vec<super::PlanInputItem>>, AdoptError> {
    let mut inputs = Vec::with_capacity(batch.items.len());
    for item in &batch.items {
        if !matches!(item.intent, AdoptPlanIntent::LocalLink) {
            return Ok(None);
        }
        let candidate = &item.candidate;
        let selection = selections
            .iter()
            .find(|selection| selection.canonical_entity == candidate.canonical_entity)
            .ok_or(AdoptError::PlanStale)?;
        let target_agents = if candidate
            .appearances
            .iter()
            .any(|appearance| appearance.appearance.shared)
        {
            let ids = if selection.agent_ids.is_empty() {
                candidate.suggested_agent_ids.clone()
            } else {
                selection.agent_ids.clone()
            };
            agents
                .iter()
                .filter(|agent| ids.contains(&agent.agent_id))
                .cloned()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        inputs.push(super::PlanInputItem {
            directory_name: candidate.directory_name.clone(),
            kind: super::AdoptPlanKind::Link,
            canonical_entity: candidate.canonical_entity.clone(),
            final_entity_path: candidate.canonical_entity.clone(),
            appearances: candidate
                .appearances
                .iter()
                .map(|appearance| appearance.appearance.clone())
                .collect(),
            target_agents,
        });
    }
    Ok(Some(inputs))
}

#[derive(Default)]
struct GroupedEvidence {
    directory_names: BTreeSet<String>,
    appearances: Vec<AdoptAppearanceEvidence>,
    blocked: Option<AdoptEvidenceCandidate>,
    excluded: Option<AdoptEvidenceCandidate>,
}

/// A fixture footprint is present when the canonical entity lives under a
/// `fixture-entities` root or its exact tree hash equals an immutable
/// fixture fingerprint (spec §3.5).
fn is_fixture_entity(entity: &Path) -> bool {
    entity.components().any(|component| {
        component.as_os_str() == "fixture-entities"
    })
}

fn is_fixture_tree_hash(hash: &str) -> bool {
    hash == crate::core::fixture_recovery::FIXTURE_TREE_HASH_SKILL_AUTHORING
        || hash == crate::core::fixture_recovery::FIXTURE_TREE_HASH_MEDIA_XRAY
}
impl AdoptService {
    /// Canonical spelling through the FileSystem seam (spec §4.1: Core
    /// never touches the filesystem directly); falls back to the raw path
    /// when the canonical form is unavailable.
    fn seam_canonical_root(&self, path: &Path) -> PathBuf {
        self.filesystem
            .canonical_directory(path)
            .unwrap_or_else(|_| path.to_path_buf())
    }
}
