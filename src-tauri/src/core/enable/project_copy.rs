//! Project-owned copy actions within the Enable Module (ADR-0025, #101).
use super::*;
use crate::seams::filesystem::{EvidenceChainHopKind, ProjectCopyJournal};

impl EnableService {
    pub fn plan_project_enable(
        &self,
        skill_ids: &[SkillId],
        project_folder: &Path,
        agent_ids: &[String],
        _resolutions: &[(String, CellResolution)],
    ) -> Result<EnablePlan, EnableError> {
        let context = self.capture_write_context()?;
        if skill_ids.len() != 1 || !agent_ids.is_empty() {
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
        self.build_project_plan(root, vec![], vec![planned], context)
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
        if outcome == CellOutcome::Succeeded {
            let root = batch
                .project_root_identity
                .as_ref()
                .ok_or(EnableError::PlanStale)?;
            let parent = self
                .filesystem
                .prepare_project_copy_parent(root, &planned.frozen_project_targets[0].resolution)?;
            let mut copy = ProjectCopyJournal {
                cleanup_authorized: false,
                target_resolution: self
                    .filesystem
                    .resolve_project_target(&root.canonical_path, Path::new(".agents/skills"))?,
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
                copy.content_hash = Some(self.filesystem.project_copy_hash(&copy.staging_path)?);
                journal.project_copy = Some(copy.clone());
                self.filesystem.write_enable_journal(library, &journal)?;
                self.filesystem.publish_project_copy(&copy)?;
                copy.phase = ActivationReplacePhase::Committed;
                journal.project_copy = Some(copy.clone());
                journal.phase = ActivationReplacePhase::Committed;
                self.filesystem
                    .write_enable_journal(library, &journal)
                    .map_err(|e| self.block_for_recovery("persist committed project copy", e))?;
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
                    .insert(batch.operation_id.clone(), copy);
                let path = root.canonical_path.clone();
                if let Err(error) =
                    self.agent_store
                        .record_recent_project_folder(RecentProjectFolder {
                            canonical_path_key: crate::core::domain::configured_path_identity_key(
                                &path.to_string_lossy(),
                            ),
                            canonical_path: path,
                            last_used_at: crate::seams::clock::iso_timestamp(
                                self.clock.unix_epoch_nanos(),
                            ),
                        })
                {
                    diagnostic = Some(error.to_string());
                }
            }
        }
        let result = EnableResult {
            operation_id: batch.operation_id.clone(),
            cells: vec![self.result_for(planned, outcome, diagnostic)],
            snapshot_version: version,
        };
        self.applied
            .lock()
            .map_err(|_| EnableError::Internal("Enable applied lock".into()))?
            .insert(batch.operation_id.clone(), batch);
        Ok(result)
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
        let mut copies = self
            .copy_journals
            .lock()
            .map_err(|_| EnableError::Internal("copy journal lock".into()))?;
        let mut cells = vec![];
        if let Some(mut copy) = copies.remove(&batch.operation_id) {
            let unchanged = self.filesystem.project_copy_unchanged(&copy);
            if unchanged {
                copy.phase = ActivationReplacePhase::Undoing;
                let mut journal = self.build_journal(batch, library);
                journal.cells.clear();
                journal.project_copy = Some(copy.clone());
                self.filesystem
                    .write_enable_journal(library, &journal)
                    .map_err(|e| self.block_for_recovery("persist project copy Undo", e))?;
                self.recover_copy_artifacts(&copy, library, &batch.operation_id)
                    .map_err(|e| self.block_for_recovery("finish project copy Undo", e))?;
            }
            cells.push(EnableUndoCellResult {
                cell_key: batch.cells[0].cell.cell_key.clone(),
                undone: unchanged,
                diagnostic: if unchanged {
                    None
                } else {
                    Some("project copy changed; preserved".into())
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
