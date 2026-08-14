use std::path::PathBuf;

use crate::core::adopt::{AdoptError, AdoptPlanRequest, AdoptSelection, AdoptService};
use crate::core::domain::AgentId;
use crate::tauri_adapter::dto::{
    AdoptEvidenceReportDto, AdoptPlanDto, AdoptResultDto, AdoptUndoResultDto, ApplyAdoptRequestDto,
    CancelAdoptRequestDto, CommandFailureDto, DiagnosticDto, FinalizeAdoptRequestDto,
    PlanAdoptRequestDto, PublicErrorDto, UndoAdoptRequestDto,
};

pub struct AdoptApi {
    service: AdoptService,
}

impl AdoptApi {
    pub fn new(service: AdoptService) -> Self {
        Self { service }
    }

    pub fn scan_adopt(&self) -> Result<AdoptEvidenceReportDto, CommandFailureDto> {
        self.service.scan().map(Into::into).map_err(command_error)
    }

    pub fn plan_adopt(
        &self,
        request: PlanAdoptRequestDto,
    ) -> Result<AdoptPlanDto, CommandFailureDto> {
        let selections = request
            .selections
            .into_iter()
            .map(|selection| AdoptSelection {
                canonical_entity: PathBuf::from(selection.canonical_entity),
                agent_ids: selection.agent_ids.into_iter().map(AgentId).collect(),
                modified_branch: selection.modified_branch.map(Into::into),
                target_directory: selection.target_directory.map(PathBuf::from),
            })
            .collect::<Vec<_>>();
        let plan = self
            .service
            .plan(&AdoptPlanRequest {
                evidence_generation: request.evidence_generation,
                selections,
            })
            .map_err(command_error)?;
        Ok(AdoptPlanDto {
            plan_token: plan.plan_token,
            evidence_generation: plan.evidence_generation,
            items: plan.items.into_iter().map(Into::into).collect(),
            can_apply: plan.can_apply,
        })
    }

    pub fn apply_adopt(
        &self,
        request: ApplyAdoptRequestDto,
    ) -> Result<AdoptResultDto, CommandFailureDto> {
        let result = self
            .service
            .apply(&request.plan_token)
            .map_err(command_error)?;
        Ok(AdoptResultDto {
            operation_id: result.operation_id,
            items: result
                .items
                .into_iter()
                .map(|item| crate::tauri_adapter::dto::AdoptSkillResultDto {
                    skill_id: item.skill_id.0,
                    directory_name: item.directory_name,
                    adopted: item.adopted,
                    error: item.error,
                })
                .collect(),
            snapshot_version: result.snapshot_version,
            undo_available: result.undo_available,
        })
    }

    pub fn undo_adopt(
        &self,
        request: UndoAdoptRequestDto,
    ) -> Result<AdoptUndoResultDto, CommandFailureDto> {
        let result = self
            .service
            .undo(&request.operation_id)
            .map_err(command_error)?;
        Ok(AdoptUndoResultDto {
            operation_id: result.operation_id,
            items: result
                .items
                .into_iter()
                .map(|item| crate::tauri_adapter::dto::AdoptUndoItemResultDto {
                    directory_name: item.directory_name,
                    undone: item.undone,
                    error: item.error,
                })
                .collect(),
            snapshot_version: result.snapshot_version,
        })
    }

    pub fn finalize_adopt(
        &self,
        request: FinalizeAdoptRequestDto,
    ) -> Result<(), CommandFailureDto> {
        self.service
            .finalize(&request.operation_id)
            .map_err(command_error)
    }

    pub fn cancel_adopt(&self, request: CancelAdoptRequestDto) -> Result<bool, CommandFailureDto> {
        self.service
            .cancel(&request.plan_token)
            .map_err(command_error)
    }
}

fn command_error(error: AdoptError) -> CommandFailureDto {
    let public_error = match &error {
        AdoptError::Validation(_) => PublicErrorDto::Validation,
        AdoptError::PlanStale | AdoptError::PlanNotFound => PublicErrorDto::PlanStale,
        AdoptError::RecoveryRequired(_) => PublicErrorDto::RecoveryRequired,
        AdoptError::Store(crate::seams::adopt_store::AdoptStoreError::Conflict(directory_name)) => {
            PublicErrorDto::Conflict {
                directory_name: directory_name.clone(),
            }
        }
        AdoptError::Store(_) => PublicErrorDto::StateUnavailable,
        AdoptError::LockConcurrentChange(_) => PublicErrorDto::PlanStale,
        AdoptError::LockRelease(_) => PublicErrorDto::StateUnavailable,
        AdoptError::RemoteProvider(_) => PublicErrorDto::SourceUnavailable,
        AdoptError::FileSystem(crate::seams::filesystem::FileSystemError::RecoveryRequired {
            ..
        }) => PublicErrorDto::RecoveryRequired,
        AdoptError::FileSystem(crate::seams::filesystem::FileSystemError::PlanStale { .. }) => {
            PublicErrorDto::PlanStale
        }
        AdoptError::FileSystem(crate::seams::filesystem::FileSystemError::Io {
            source, ..
        }) if source.kind() == std::io::ErrorKind::PermissionDenied => {
            PublicErrorDto::PermissionDenied
        }
        AdoptError::Lock(_) => PublicErrorDto::StateUnavailable,
        AdoptError::FileSystem(_) | AdoptError::Internal(_) => PublicErrorDto::Internal,
    };
    CommandFailureDto {
        error: public_error,
        diagnostic: Some(DiagnosticDto {
            code: "command_error".into(),
            message: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::adopt::{AdoptSelection as CoreSelection, ModifiedBranch};
    use crate::tauri_adapter::dto::{AdoptSelectionDto, AdoptVerdictReasonDto, ModifiedBranchDto};

    #[test]
    fn adopt_selection_round_trips_target_directory_and_branch() {
        let dto = AdoptSelectionDto {
            canonical_entity: "/tmp/entity/networking".into(),
            agent_ids: vec!["claude-code".into()],
            modified_branch: Some(ModifiedBranchDto::DiscardToAnchor),
            target_directory: Some("/tmp/stable/networking".into()),
        };
        let json = serde_json::to_string(&dto).expect("serialize");
        assert!(
            json.contains(r#""targetDirectory":"/tmp/stable/networking""#),
            "{json}"
        );
        assert!(
            json.contains(r#""modifiedBranch":"discard_to_anchor""#),
            "{json}"
        );
        let parsed: AdoptSelectionDto = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, dto);

        // The API mapping carries the target directory into the Core plan.
        let core: CoreSelection = CoreSelection {
            canonical_entity: "/tmp/entity/networking".into(),
            agent_ids: vec![AgentId("claude-code".into())],
            modified_branch: Some(ModifiedBranch::DiscardToAnchor),
            target_directory: Some("/tmp/stable/networking".into()),
        };
        assert_eq!(
            core.target_directory.as_deref(),
            Some(std::path::Path::new("/tmp/stable/networking"))
        );
    }

    #[test]
    fn ownership_conflict_reason_serializes_as_a_closed_kind() {
        let reason = crate::core::adopt::AdoptVerdictReason::OwnershipConflict {
            managed_directory_name: "networking".into(),
        };
        let dto: AdoptVerdictReasonDto = reason.into();
        let json = serde_json::to_string(&dto).expect("serialize");
        assert_eq!(
            json,
            r#"{"kind":"ownership_conflict","managedDirectoryName":"networking"}"#
        );
    }

    #[test]
    fn lock_concurrent_change_maps_to_plan_stale() {
        let failure = command_error(AdoptError::LockConcurrentChange("changed".into()));
        assert_eq!(failure.error, PublicErrorDto::PlanStale);
    }
}
