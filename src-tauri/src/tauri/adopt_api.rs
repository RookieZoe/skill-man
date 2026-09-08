use crate::core::adopt::{AdoptError, AdoptReportPlanRequest, AdoptService};
use crate::tauri_adapter::dto::{
    ApplyAdoptRequestDto, CancelAdoptRequestDto, CommandFailureDto, DiagnosticDto,
    FinalizeAdoptRequestDto, PlanAdoptRequestDto, PublicErrorDto, UndoAdoptRequestDto,
};

pub struct AdoptApi {
    service: AdoptService,
}

impl AdoptApi {
    pub fn new(service: AdoptService) -> Self {
        Self { service }
    }

    /// Plan from the current terminal Scan Report (spec §4.6): only a
    /// non-Stale Complete/Incomplete Report of the current Open Home is
    /// accepted; anything else is typed `PlanStale` (fail closed).
    pub fn plan_adopt(
        &self,
        request: PlanAdoptRequestDto,
    ) -> Result<crate::tauri_adapter::dto::AdoptPlanDto, CommandFailureDto> {
        self.service
            .plan_report(&AdoptReportPlanRequest {
                report_generation: request.report_generation,
                selections: request
                    .selections
                    .into_iter()
                    .map(|selection| crate::core::adopt::AdoptReportSelection {
                        entity_ref: selection.entity_ref,
                        action: selection.action,
                        destination_parent: selection.destination_parent,
                    })
                    .collect(),
            })
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn apply_adopt(
        &self,
        request: ApplyAdoptRequestDto,
    ) -> Result<crate::tauri_adapter::dto::AdoptResultDto, CommandFailureDto> {
        self.service
            .apply(&request.plan_token)
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn undo_adopt(
        &self,
        request: UndoAdoptRequestDto,
    ) -> Result<crate::tauri_adapter::dto::AdoptUndoResultDto, CommandFailureDto> {
        self.service
            .undo(&request.operation_id)
            .map(Into::into)
            .map_err(command_error)
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
        AdoptError::Eligibility {
            code,
            entity_seq,
            detail,
        } => PublicErrorDto::AdoptEligibility {
            closed_code: code.clone(),
            entity_seq: *entity_seq,
            detail: detail.clone(),
        },
        AdoptError::PlanStale | AdoptError::PlanNotFound => PublicErrorDto::PlanStale,
        AdoptError::RecoveryRequired(_) => PublicErrorDto::RecoveryRequired,
        AdoptError::Store(crate::seams::adopt_store::AdoptStoreError::Conflict(directory_name)) => {
            PublicErrorDto::Conflict {
                directory_name: directory_name.clone(),
            }
        }
        AdoptError::Store(_) => PublicErrorDto::StateUnavailable,
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
    use crate::core::adopt::{AdoptEntityRef, AdoptReportSelection};

    #[test]
    fn plan_request_round_trips_report_generation_and_entity_ref() {
        let entity_ref =
            AdoptEntityRef::new("scan-report-v1:home-1:run-7:1:complete".to_owned(), 7, 12)
                .encode();
        let dto = crate::tauri_adapter::dto::PlanAdoptRequestDto {
            report_generation: 7,
            selections: vec![crate::tauri_adapter::dto::AdoptReportSelectionDto {
                entity_ref: entity_ref.clone(),
                action: "local_link".into(),
                destination_parent: None,
            }],
        };
        let core = AdoptReportPlanRequest {
            report_generation: dto.report_generation,
            selections: dto
                .selections
                .into_iter()
                .map(|selection| AdoptReportSelection {
                    entity_ref: selection.entity_ref,
                    action: selection.action,
                    destination_parent: selection.destination_parent,
                })
                .collect(),
        };
        assert_eq!(core.report_generation, 7);
        assert_eq!(core.selections[0].entity_ref, entity_ref);
        assert_eq!(
            AdoptEntityRef::decode(&core.selections[0].entity_ref).expect("decode"),
            AdoptEntityRef::new("scan-report-v1:home-1:run-7:1:complete".to_owned(), 7, 12)
        );
    }

    #[test]
    fn entity_ref_rejects_malformed_tokens() {
        assert!(AdoptEntityRef::decode("not-a-ref").is_none());
        assert!(AdoptEntityRef::decode("@1@2").is_none());
        // The generation and entity sequence are decoded from the right.
        assert!(AdoptEntityRef::decode("sha256:deadbeef@2@0").is_none());
    }
}
