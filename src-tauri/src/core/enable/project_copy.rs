//! Project-owned copy actions within the Enable Module (ADR-0025, #101).
use super::*;
use crate::seams::filesystem::{EvidenceChainHopKind, ProjectCopyJournal, ProjectLinkJournal};

impl EnableService {
    pub fn plan_project_enable(
        &self,
        skill_ids: &[SkillId],
        project_folder: &Path,
        agent_ids: &[String],
        _resolutions: &[(String, CellResolution)],
    ) -> Result<EnablePlan, EnableError> {
        let context = self.capture_write_context()?;
        if skill_ids.len() != 1 {
            return Err(EnableError::Validation(
                "project_copy_scope_not_ready".into(),
            ));
        }
        let catalog_generation = self.store.catalog_generation()?;
        let agent_generation = self
            .agent_store
            .agent_configuration_snapshot()?
            .snapshot_version;
        let root = self.filesystem.canonical_directory(project_folder)?;
        let root_before = self.filesystem.directory_fingerprint(&root)?;
        let skill =
            self.catalog
                .inspect(&skill_ids[0])?
                .ok_or_else(|| EnableError::SkillNotFound {
                    skill_id: skill_ids[0].0.clone(),
                })?;
        let source = PathBuf::from(&skill.final_entity_path);
        let configured = PathBuf::from(".agents/skills");
        let resolution = self.filesystem.resolve_project_target(&root, &configured)?;
        let target = resolution.resolved_container.clone();
        let mut entry = target.join(&skill.summary.directory_name);
        let mut reason = None;
        let mut detail = None;
        if resolution.fault.is_some() {
            reason = Some(CellBlockedReason::TargetUnavailable);
            detail = Some(format!("{:?}", resolution.fault));
        }
        if resolution.hops.iter().any(|hop| matches!(&hop.kind, EvidenceChainHopKind::Symlink { target } if target.is_absolute())) {
            reason = Some(CellBlockedReason::NonPortableProjectAlias);
            detail = Some("absolute project directory alias is not portable".into());
        }
        // Honor Directory Identity while retaining the existing physical spelling.
        if reason.is_none() && resolution.create_steps.is_empty() {
            for item in self.filesystem.list_directory(&target)? {
                if crate::core::domain::skill_identity_key(&item.name)
                    == crate::core::domain::skill_identity_key(&skill.summary.directory_name)
                {
                    entry = target.join(item.name);
                    break;
                }
            }
        }
        let mut frozen_entry = None;
        let mut payload = None;
        let mut reuse = false;
        let mut copy_alias = None;
        if reason.is_none() {
            reuse =
                self.filesystem.activation_snapshot(&entry)? != ActivationEntrySnapshot::Missing;
            let observed_path = if reuse {
                frozen_entry = Some(self.filesystem.occupant_snapshot(&entry)?);
                let relative = entry
                    .strip_prefix(&root)
                    .map_err(|_| EnableError::PlanStale)?;
                let existing = self.filesystem.resolve_project_target(&root, relative)?;
                if existing.fault.is_some() || !existing.create_steps.is_empty() || existing.hops.iter().any(|hop| matches!(&hop.kind, EvidenceChainHopKind::Symlink { target } if target.is_absolute())) {
                    reason = Some(CellBlockedReason::InvalidProjectCopy);
                    detail = Some("existing copy is not a portable project directory".into());
                }
                copy_alias = Some((relative.to_path_buf(), existing.clone()));
                existing.resolved_container
            } else {
                source.clone()
            };
            if reason.is_none() {
                match self.filesystem.project_copy_payload(&observed_path, reuse) {
                    Ok(value) => payload = Some(value),
                    Err(error) => {
                        reason = Some(if reuse {
                            CellBlockedReason::InvalidProjectCopy
                        } else {
                            CellBlockedReason::InvalidCopyPayload
                        });
                        detail = Some(error.to_string());
                    }
                }
            }
        }
        if skill.summary.health != Health::Healthy {
            reason = Some(if skill.summary.health == Health::SourceSnapshotMismatch {
                CellBlockedReason::SourceSnapshotMismatch
            } else {
                CellBlockedReason::EntityBroken
            });
        }
        if let Some(service) = &self.source_update {
            if let Err(error) = service.ensure_new_enable_allowed(&skill_ids[0]) {
                reason = Some(CellBlockedReason::SourceSnapshotMismatch);
                detail = Some(error.to_string());
            }
        }
        let eligibility = if reason.is_some() {
            CellEligibility::Blocked
        } else if reuse {
            CellEligibility::NoOp
        } else {
            CellEligibility::Ready
        };
        let target_id = format!("project-copy:{}", target.display());
        let cell = EnableCell {
            depends_on_copy: None,
            project_copy: Some(if reuse {
                ProjectCopyAction::Reuse
            } else {
                ProjectCopyAction::Create
            }),
            cell_key: format!("{}|{target_id}", skill_ids[0].0),
            skill_id: skill_ids[0].clone(),
            skill_name: skill.summary.display_name,
            directory_name: skill.summary.directory_name.clone(),
            directory_identity_key: crate::core::domain::skill_identity_key(
                &skill.summary.directory_name,
            ),
            target_root_id: target_id,
            target_path: target.clone(),
            entry_path: entry,
            final_entity_path: source.clone(),
            action: EnableAction::Enable,
            affected_agent_ids: vec![],
            affected_agent_names: vec![],
            occupancy: if reuse {
                Occupier::Untracked {
                    kind: UntrackedOccupierKind::RealDirectory,
                }
            } else {
                Occupier::Empty
            },
            occ_exact_direct: false,
            destructive: None,
            eligibility,
            blocked_reason: reason,
            resolution: CellResolution::Skip,
            detail,
            create_steps: resolution.create_steps.clone(),
            hop_evidence: vec![],
        };
        let planned = PlannedCell {
            cell,
            frozen_entry,
            project_copy_payload: payload,
            copy_alias,
            frozen_health: skill.summary.health,
            frozen_source_kind: skill.summary.source_kind,
            frozen_final_entity: source,
            frozen_availability: TargetGroupAvailability::Available,
            frozen_target: if resolution.create_steps.is_empty() && resolution.fault.is_none() {
                Some(self.filesystem.directory_fingerprint(&target)?)
            } else {
                None
            },
            frozen_project_targets: vec![FrozenProjectTarget {
                configured_relative_path: configured,
                resolution,
            }],
            before_desired: false,
        };
        if self.store.catalog_generation()? != catalog_generation
            || self
                .agent_store
                .agent_configuration_snapshot()?
                .snapshot_version
                != agent_generation
            || self.filesystem.directory_fingerprint(&root)? != root_before
        {
            return Err(EnableError::PlanStale);
        }
        let mut cells = vec![planned];
        self.plan_copy_links(&root, agent_ids, &mut cells)?;
        let hops = cells
            .iter()
            .flat_map(|p| p.cell.hop_evidence.clone())
            .collect();
        self.build_project_plan(root, hops, cells, context)
    }

    fn plan_copy_links(
        &self,
        root: &Path,
        agent_ids: &[String],
        cells: &mut Vec<PlannedCell>,
    ) -> Result<(), EnableError> {
        let snapshot = self.agent_store.agent_configuration_snapshot()?;
        let copy = cells[0].clone();
        let copy_path = copy
            .copy_alias
            .as_ref()
            .map(|(_, r)| r.resolved_container.clone())
            .unwrap_or_else(|| copy.cell.entry_path.clone());
        let mut targets = vec![copy.cell.target_path.clone()];
        for id in agent_ids {
            let agent = snapshot
                .configurations
                .iter()
                .find(|a| &a.agent_id == id)
                .ok_or_else(|| EnableError::Validation("unknown project Agent".into()))?;
            let configured = agent.project_skills_dir.as_ref().ok_or_else(|| {
                EnableError::Validation("Agent has no project skills directory".into())
            })?;
            let resolution = self.filesystem.resolve_project_target(root, configured)?;
            if !targets.contains(&resolution.resolved_container) {
                targets.push(resolution.resolved_container);
            }
        }
        // Include every configured consumer of each selected physical container.
        for agent in &snapshot.configurations {
            let Some(configured) = &agent.project_skills_dir else {
                continue;
            };
            let resolution = self.filesystem.resolve_project_target(root, configured)?;
            if !targets.contains(&resolution.resolved_container) {
                continue;
            }
            let portable = resolution.fault.is_none() && !resolution.hops.iter().any(|h| matches!(&h.kind, EvidenceChainHopKind::Symlink { target } if target.is_absolute()));
            let hop = ProjectHopEvidence {
                agent_id: agent.agent_id.clone(),
                agent_name: agent.name.clone(),
                configured_relative_path: configured.clone(),
                resolved_container: resolution.resolved_container.clone(),
                hops: resolution.hops.clone(),
            };
            // Invalid aliases remain separate blocked cells; they cannot bless a physical group.
            let index = if portable {
                cells.iter().position(|p| {
                    p.cell.target_path == resolution.resolved_container
                        && p.cell.blocked_reason != Some(CellBlockedReason::NonPortableProjectAlias)
                })
            } else {
                None
            };
            if let Some(index) = index {
                cells[index]
                    .cell
                    .affected_agent_ids
                    .push(agent.agent_id.clone());
                cells[index]
                    .cell
                    .affected_agent_names
                    .push(agent.name.clone());
                cells[index].cell.hop_evidence.push(hop);
                cells[index]
                    .frozen_project_targets
                    .push(FrozenProjectTarget {
                        configured_relative_path: configured.clone(),
                        resolution,
                    });
                continue;
            }
            let mut link = copy.clone();
            link.cell.project_copy = None;
            link.cell.depends_on_copy = Some(copy.cell.cell_key.clone());
            link.cell.target_root_id = format!(
                "project-link:{}:{}",
                resolution.resolved_container.display(),
                if portable { "shared" } else { &agent.agent_id }
            );
            link.cell.cell_key = format!("{}|{}", copy.cell.skill_id.0, link.cell.target_root_id);
            link.cell.target_path = resolution.resolved_container.clone();
            link.cell.entry_path = resolution
                .resolved_container
                .join(&copy.cell.directory_name);
            link.cell.final_entity_path = copy_path.clone();
            link.cell.affected_agent_ids = vec![agent.agent_id.clone()];
            link.cell.affected_agent_names = vec![agent.name.clone()];
            link.cell.hop_evidence = vec![hop];
            link.cell.create_steps = resolution.create_steps.clone();
            link.cell.eligibility = CellEligibility::Ready;
            link.cell.blocked_reason = None;
            link.cell.detail = None;
            link.cell.occupancy = Occupier::Empty;
            link.project_copy_payload = None;
            link.copy_alias = None;
            link.frozen_entry = None;
            link.frozen_target = None;
            if !portable {
                link.cell.eligibility = CellEligibility::Blocked;
                link.cell.blocked_reason = Some(match &resolution.fault {
                    Some(crate::seams::filesystem::ProjectTargetFault::OutsideProjectRoot) => {
                        CellBlockedReason::OutsideProjectRoot
                    }
                    Some(crate::seams::filesystem::ProjectTargetFault::SymlinkCycle) => {
                        CellBlockedReason::SymlinkCycle
                    }
                    Some(crate::seams::filesystem::ProjectTargetFault::HopLimitExceeded) => {
                        CellBlockedReason::HopLimitExceeded
                    }
                    Some(crate::seams::filesystem::ProjectTargetFault::TargetNotDirectory) => {
                        CellBlockedReason::TargetNotDirectory
                    }
                    Some(crate::seams::filesystem::ProjectTargetFault::TargetUnavailable {
                        ..
                    }) => CellBlockedReason::TargetUnavailable,
                    None => CellBlockedReason::NonPortableProjectAlias,
                });
            } else {
                if resolution.create_steps.is_empty() {
                    link.frozen_target = Some(
                        self.filesystem
                            .directory_fingerprint(&resolution.resolved_container)?,
                    );
                    for item in self
                        .filesystem
                        .list_directory(&resolution.resolved_container)?
                    {
                        if crate::core::domain::skill_identity_key(&item.name)
                            == copy.cell.directory_identity_key
                        {
                            link.cell.entry_path = resolution.resolved_container.join(item.name);
                            break;
                        }
                    }
                }
                if self.filesystem.activation_snapshot(&link.cell.entry_path)?
                    != ActivationEntrySnapshot::Missing
                {
                    let occupant = self.filesystem.occupant_snapshot(&link.cell.entry_path)?;
                    let correct = if let OccupantKind::Symlink { target } = &occupant.kind {
                        if target.is_relative() {
                            let relative = link
                                .cell
                                .entry_path
                                .strip_prefix(root)
                                .map_err(|_| EnableError::PlanStale)?;
                            let observed =
                                self.filesystem.resolve_project_target(root, relative)?;
                            observed.fault.is_none() && observed.create_steps.is_empty() && observed.resolved_container == copy_path && !observed.hops.iter().any(|h| matches!(&h.kind, EvidenceChainHopKind::Symlink { target } if target.is_absolute()))
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    link.cell.eligibility = if correct {
                        CellEligibility::NoOp
                    } else {
                        CellEligibility::Conflict
                    };
                    link.cell.blocked_reason = if correct {
                        None
                    } else {
                        Some(CellBlockedReason::EntryOccupied)
                    };
                    link.cell.occupancy = Occupier::Untracked {
                        kind: match &occupant.kind {
                            OccupantKind::Symlink { target } => UntrackedOccupierKind::Symlink {
                                target: target.clone(),
                            },
                            OccupantKind::RealDirectory => UntrackedOccupierKind::RealDirectory,
                            OccupantKind::File { length } => {
                                UntrackedOccupierKind::File { length: *length }
                            }
                        },
                    };
                    link.frozen_entry = Some(occupant);
                }
            }
            link.frozen_project_targets = vec![FrozenProjectTarget {
                configured_relative_path: configured.clone(),
                resolution,
            }];
            cells.push(link);
        }
        // Refuse ancestors and descendants of any planned artifact, including a reused physical copy.
        for i in 1..cells.len() {
            let entry = &cells[i].cell.entry_path;
            let overlap = entry.starts_with(&copy_path)
                || copy_path.starts_with(entry)
                || cells.iter().enumerate().any(|(j, p)| {
                    j != i
                        && (entry.starts_with(&p.cell.entry_path)
                            || p.cell.entry_path.starts_with(entry))
                });
            if overlap {
                cells[i].cell.eligibility = CellEligibility::Blocked;
                cells[i].cell.blocked_reason = Some(CellBlockedReason::EntryOccupied);
            }
        }
        Ok(())
    }

    pub(super) fn apply_project_copy(
        &self,
        batch: PlannedBatch,
        library: &Path,
    ) -> Result<EnableResult, EnableError> {
        let planned = &batch.cells[0];
        let version = self.store.catalog_generation()?;
        let mut outcome = match planned.cell.eligibility {
            CellEligibility::NoOp => CellOutcome::NoOp,
            CellEligibility::Ready => CellOutcome::Succeeded,
            _ => CellOutcome::Skipped,
        };
        let mut diagnostic = None;
        let mut operation = self.build_journal(&batch, library);
        operation.cells.clear();
        if outcome == CellOutcome::Succeeded {
            'copy: {
                let root = batch
                    .project_root_identity
                    .as_ref()
                    .ok_or(EnableError::PlanStale)?;
                let parent = match self.filesystem.prepare_project_copy_parent(
                    root,
                    &planned.frozen_project_targets[0].resolution,
                ) {
                    Ok(parent) => parent,
                    Err(error) => {
                        outcome = CellOutcome::Failed;
                        diagnostic = Some(error.to_string());
                        break 'copy;
                    }
                };
                let mut copy = ProjectCopyJournal {
                    cleanup_authorized: false,
                    target_resolution: self.filesystem.resolve_project_target(
                        &root.canonical_path,
                        Path::new(".agents/skills"),
                    )?,
                    project_root: root.clone(),
                    parent,
                    entry_path: planned.cell.entry_path.clone(),
                    staging_path: planned
                        .cell
                        .target_path
                        .join(format!(".skillman-{}", batch.operation_id)),
                    staged_identity: None,
                    content_hash: None,
                    phase: ActivationReplacePhase::Applying,
                };
                let mut journal = self.build_journal(&batch, library);
                journal.cells.clear();
                journal.project_copy = Some(copy.clone());
                self.filesystem.write_enable_journal(library, &journal)?;
                let applied = (|| -> Result<(), EnableError> {
                    copy.staged_identity = Some(self.filesystem.reserve_project_copy(&copy)?);
                    journal.project_copy = Some(copy.clone());
                    self.filesystem.write_enable_journal(library, &journal)?;
                    self.filesystem.stage_project_copy(
                        &copy,
                        planned
                            .project_copy_payload
                            .as_ref()
                            .ok_or(EnableError::PlanStale)?,
                    )?;
                    copy.content_hash =
                        Some(self.filesystem.project_copy_hash(&copy.staging_path)?);
                    journal.project_copy = Some(copy.clone());
                    self.filesystem.write_enable_journal(library, &journal)?;
                    self.filesystem.publish_project_copy(&copy)?;
                    copy.phase = ActivationReplacePhase::Committed;
                    journal.project_copy = Some(copy.clone());
                    journal.phase = ActivationReplacePhase::Committed;
                    self.filesystem
                        .write_enable_journal(library, &journal)
                        .map_err(|e| {
                            self.block_for_recovery("persist committed project copy", e)
                        })?;
                    Ok(())
                })();
                if let Err(error) = applied {
                    if matches!(error, EnableError::RecoveryRequired(_)) {
                        return Err(error);
                    }
                    // Recovery consumes only the journal's frozen artifact evidence.
                    self.recover_copy_artifacts(&copy, library, &batch.operation_id)
                        .map_err(|e| self.block_for_recovery("recover failed project copy", e))?;
                    self.filesystem
                        .finish_enable_journal(library, &batch.operation_id)?;
                    outcome = CellOutcome::Failed;
                    diagnostic = Some(error.to_string());
                } else {
                    self.copy_journals
                        .lock()
                        .map_err(|_| EnableError::Internal("copy journal lock".into()))?
                        .insert(batch.operation_id.clone(), journal.clone());
                    operation = journal;
                }
            }
        }
        let ready = matches!(outcome, CellOutcome::Succeeded | CellOutcome::NoOp);
        let mut results = vec![self.result_for(planned, outcome, diagnostic)];
        let mut gate_closed = false;
        for link in batch.cells.iter().skip(1) {
            let copy_still_ready = ready
                && !gate_closed
                && match &operation.project_copy {
                    Some(copy) => self.filesystem.project_copy_unchanged(copy),
                    None => planned
                        .project_copy_payload
                        .as_ref()
                        .is_some_and(|payload| {
                            self.filesystem
                                .project_copy_payload(&payload.root.canonical_path, true)
                                .as_ref()
                                .ok()
                                == Some(payload)
                        }),
                };
            let (outcome, diagnostic) = if !copy_still_ready {
                (
                    CellOutcome::NotAttempted,
                    Some("project_copy_not_ready".into()),
                )
            } else if link.cell.eligibility == CellEligibility::NoOp {
                (CellOutcome::NoOp, None)
            } else if link.cell.eligibility != CellEligibility::Ready {
                (CellOutcome::Skipped, None)
            } else {
                match self.apply_copy_link(&batch, link, &mut operation, library) {
                    Ok(()) => (CellOutcome::Succeeded, None),
                    Err(error @ EnableError::RecoveryRequired(_)) => return Err(error),
                    Err(EnableError::WriteGateClosed) => {
                        gate_closed = true;
                        (CellOutcome::NotAttempted, Some("write_gate_closed".into()))
                    }
                    Err(error) => (CellOutcome::Failed, Some(error.to_string())),
                }
            };
            results.push(self.result_for(link, outcome, diagnostic));
        }
        if results
            .iter()
            .any(|cell| cell.outcome == CellOutcome::Succeeded)
        {
            let path = batch
                .project_root_identity
                .as_ref()
                .ok_or(EnableError::PlanStale)?
                .canonical_path
                .clone();
            if let Err(error) = self
                .agent_store
                .record_recent_project_folder(RecentProjectFolder {
                    canonical_path_key: crate::core::domain::configured_path_identity_key(
                        &path.to_string_lossy(),
                    ),
                    canonical_path: path,
                    last_used_at: crate::seams::clock::iso_timestamp(self.clock.unix_epoch_nanos()),
                })
            {
                results[0].diagnostic = Some(error.to_string());
            }
        }
        self.copy_journals
            .lock()
            .map_err(|_| EnableError::Internal("copy journal lock".into()))?
            .insert(batch.operation_id.clone(), operation);
        let result = EnableResult {
            operation_id: batch.operation_id.clone(),
            cells: results,
            snapshot_version: version,
        };
        self.applied
            .lock()
            .map_err(|_| EnableError::Internal("Enable applied lock".into()))?
            .insert(batch.operation_id.clone(), batch);
        Ok(result)
    }

    fn apply_copy_link(
        &self,
        batch: &PlannedBatch,
        planned: &PlannedCell,
        journal: &mut EnableJournal,
        library: &Path,
    ) -> Result<(), EnableError> {
        self.write_gate
            .validate_open_context(&batch.write_context)
            .map_err(|_| EnableError::WriteGateClosed)?;
        let root = batch
            .project_root_identity
            .as_ref()
            .ok_or(EnableError::PlanStale)?;
        // A parent can have been created by an earlier action. Retain every existing hop identity.
        for frozen in &planned.frozen_project_targets {
            let current = self
                .filesystem
                .resolve_project_target(&root.canonical_path, &frozen.configured_relative_path)?;
            if current.fault.is_some()
                || current.resolved_container != planned.cell.target_path
                || !frozen
                    .resolution
                    .hops
                    .iter()
                    .all(|h| current.hops.contains(h))
            {
                return Err(EnableError::PlanStale);
            }
        }
        let relative = planned
            .cell
            .target_path
            .strip_prefix(&root.canonical_path)
            .map_err(|_| EnableError::PlanStale)?;
        let resolution = self
            .filesystem
            .resolve_project_target(&root.canonical_path, relative)?;
        let parent = self
            .filesystem
            .prepare_project_link_parent(root, relative, &resolution)?;
        let frozen = &planned.frozen_project_targets[0];
        let resolution = self
            .filesystem
            .resolve_project_target(&root.canonical_path, &frozen.configured_relative_path)?;
        let link_text = relative_path(&parent.canonical_path, &planned.cell.final_entity_path);
        let mut link = ProjectLinkJournal {
            cell_key: planned.cell.cell_key.clone(),
            project_root: root.clone(),
            configured_relative_path: frozen.configured_relative_path.clone(),
            resolution,
            parent,
            entry_path: planned.cell.entry_path.clone(),
            link_text,
            occupant: None,
            phase: ActivationReplacePhase::Applying,
            undone: false,
        };
        journal.project_links.push(link.clone());
        let index = journal.project_links.len() - 1;
        self.filesystem
            .write_enable_journal(library, journal)
            .map_err(|e| self.block_for_recovery("persist project link intent", e))?;
        if let Err(error) = self.filesystem.create_activation_nofollow(
            &link.link_text,
            &link.entry_path,
            &link.parent,
        ) {
            // No operation-owned identity is known; never remove a possible external occupant.
            let occupied = self.filesystem.activation_snapshot(&link.entry_path)?
                != ActivationEntrySnapshot::Missing;
            let exclusive_conflict = matches!(&error, FileSystemError::Io { source, .. } if source.kind() == std::io::ErrorKind::AlreadyExists);
            if occupied && !exclusive_conflict {
                return Err(
                    self.block_for_recovery("project link write outcome is uncertain", error)
                );
            }
            journal.project_links[index].undone = !occupied;
            journal.project_links[index].phase = ActivationReplacePhase::Committed;
            self.filesystem
                .write_enable_journal(library, journal)
                .map_err(|e| self.block_for_recovery("persist failed project link", e))?;
            return Err(error.into());
        }
        link.occupant = Some(
            self.filesystem
                .occupant_snapshot(&link.entry_path)
                .map_err(|e| self.block_for_recovery("capture project link", e))?,
        );
        link.phase = ActivationReplacePhase::Committed;
        journal.project_links[index] = link;
        self.filesystem
            .write_enable_journal(library, journal)
            .map_err(|e| self.block_for_recovery("commit project link", e))?;
        Ok(())
    }

    fn recover_copy_artifacts(
        &self,
        copy: &ProjectCopyJournal,
        library: &Path,
        operation_id: &str,
    ) -> Result<(), FileSystemError> {
        self.filesystem.recover_project_copy(copy, &mut |updated| {
            self.filesystem.write_enable_journal(
                library,
                &EnableJournal {
                    project_links: vec![],
                    version: JOURNAL_VERSION,
                    operation_id: operation_id.into(),
                    phase: updated.phase,
                    project_copy: Some(updated.clone()),
                    cells: vec![],
                },
            )
        })
    }

    pub(super) fn undo_project_copy(
        &self,
        batch: &PlannedBatch,
        library: &Path,
    ) -> Result<EnableUndoResult, EnableError> {
        let mut journals = self
            .copy_journals
            .lock()
            .map_err(|_| EnableError::Internal("copy journal lock".into()))?;
        let Some(journal) = journals.get_mut(&batch.operation_id) else {
            return Err(EnableError::PlanNotFound);
        };
        let mut cells = vec![];
        let mut safe = true;
        // Persist the dependency phase before touching any entry or copy.
        journal.phase = ActivationReplacePhase::Undoing;
        for link in &mut journal.project_links {
            if !link.undone {
                link.phase = ActivationReplacePhase::Undoing;
            }
        }
        self.filesystem
            .write_enable_journal(library, journal)
            .map_err(|e| self.block_for_recovery("persist dependency Undo", e))?;
        for index in 0..journal.project_links.len() {
            let link = &journal.project_links[index];
            if link.undone {
                continue;
            }
            let result = self.filesystem.undo_project_link(link);
            let undone = result.is_ok();
            safe &= undone;
            cells.push(EnableUndoCellResult {
                cell_key: link.cell_key.clone(),
                undone,
                diagnostic: result.err().map(|e| e.to_string()),
            });
            journal.project_links[index].undone = undone;
            self.filesystem
                .write_enable_journal(library, journal)
                .map_err(|e| self.block_for_recovery("persist dependency Undo result", e))?;
        }
        if let Some(mut copy) = journal.project_copy.clone() {
            let unchanged = safe && self.filesystem.project_copy_unchanged(&copy);
            if unchanged {
                copy.phase = ActivationReplacePhase::Undoing;
                journal.project_copy = Some(copy.clone());
                self.filesystem
                    .write_enable_journal(library, journal)
                    .map_err(|e| self.block_for_recovery("persist project copy Undo", e))?;
                self.filesystem
                    .recover_project_copy(&copy, &mut |updated| {
                        journal.project_copy = Some(updated.clone());
                        self.filesystem.write_enable_journal(library, journal)
                    })
                    .map_err(|e| self.block_for_recovery("finish project copy Undo", e))?;
            }
            cells.push(EnableUndoCellResult {
                cell_key: batch.cells[0].cell.cell_key.clone(),
                undone: unchanged,
                diagnostic: if unchanged {
                    None
                } else {
                    Some("project_copy_preserved".into())
                },
            });
        }
        self.filesystem
            .finish_enable_journal(library, &batch.operation_id)?;
        self.closed_copy_operations
            .lock()
            .map_err(|_| EnableError::Internal("closed copy lock".into()))?
            .insert(batch.operation_id.clone());
        Ok(EnableUndoResult {
            operation_id: batch.operation_id.clone(),
            cells,
            snapshot_version: self.store.catalog_generation()?,
        })
    }
}

fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let a: Vec<_> = from.components().collect();
    let b: Vec<_> = to.components().collect();
    let shared = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut result = PathBuf::new();
    for _ in shared..a.len() {
        result.push("..");
    }
    for item in &b[shared..] {
        result.push(item.as_os_str());
    }
    result
}
