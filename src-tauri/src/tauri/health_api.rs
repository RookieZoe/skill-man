use std::path::PathBuf;

use crate::core::domain::SkillId;
use crate::core::maintenance::{MaintenanceError, StartupMaintenance};
use crate::seams::filesystem::FileSystemError;
use crate::tauri_adapter::dto::{
    ActivationHealthReportDto, ApplyRelocateLinkRequestDto, ApplyRemoveSkillRequestDto,
    CancelRelocateLinkRequestDto, CancelRemoveSkillRequestDto, CommandFailureDto, DiagnosticDto,
    PlanRemoveSkillRequestDto, PublicErrorDto, RelocateLinkPreviewDto, RelocateLinkRequestDto,
    RelocateLinkResultDto, RemoveSkillPreviewDto, RemoveSkillResultDto,
};

pub struct HealthApi {
    maintenance: StartupMaintenance,
}

impl HealthApi {
    pub fn new(maintenance: StartupMaintenance) -> Self {
        Self { maintenance }
    }

    pub fn run_activation_health_check(
        &self,
    ) -> Result<ActivationHealthReportDto, CommandFailureDto> {
        self.maintenance
            .run_activation_health_check()
            .map(ActivationHealthReportDto::from)
            .map_err(maintenance_error)
    }

    pub fn relocate_link(
        &self,
        request: RelocateLinkRequestDto,
    ) -> Result<RelocateLinkPreviewDto, CommandFailureDto> {
        self.maintenance
            .relocate(
                &SkillId(request.skill_id),
                &PathBuf::from(request.source_path),
            )
            .map(RelocateLinkPreviewDto::from)
            .map_err(maintenance_error)
    }

    pub fn apply_relocate_link(
        &self,
        request: ApplyRelocateLinkRequestDto,
    ) -> Result<RelocateLinkResultDto, CommandFailureDto> {
        self.maintenance
            .apply_relocate(&request.plan_token)
            .map(RelocateLinkResultDto::from)
            .map_err(maintenance_error)
    }

    pub fn cancel_relocate_link(
        &self,
        request: CancelRelocateLinkRequestDto,
    ) -> Result<bool, CommandFailureDto> {
        self.maintenance
            .cancel_relocate(&request.plan_token)
            .map_err(maintenance_error)
    }

    pub fn plan_remove_skill(
        &self,
        request: PlanRemoveSkillRequestDto,
    ) -> Result<RemoveSkillPreviewDto, CommandFailureDto> {
        self.maintenance
            .plan_remove(&SkillId(request.skill_id))
            .map(RemoveSkillPreviewDto::from)
            .map_err(maintenance_error)
    }

    pub fn apply_remove_skill(
        &self,
        request: ApplyRemoveSkillRequestDto,
    ) -> Result<RemoveSkillResultDto, CommandFailureDto> {
        self.maintenance
            .apply_remove(&request.plan_token)
            .map(RemoveSkillResultDto::from)
            .map_err(maintenance_error)
    }

    pub fn cancel_remove_skill(
        &self,
        request: CancelRemoveSkillRequestDto,
    ) -> Result<bool, CommandFailureDto> {
        self.maintenance
            .cancel_remove(&request.plan_token)
            .map_err(maintenance_error)
    }
}

fn maintenance_error(error: MaintenanceError) -> CommandFailureDto {
    let public_error = match &error {
        MaintenanceError::FileSystem(FileSystemError::RecoveryRequired { .. })
        | MaintenanceError::RecoveryRequired { .. } => PublicErrorDto::RecoveryRequired,
        MaintenanceError::FileSystem(FileSystemError::PlanStale { .. })
        | MaintenanceError::PlanStale => PublicErrorDto::PlanStale,
        MaintenanceError::FileSystem(FileSystemError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            PublicErrorDto::PermissionDenied
        }
        MaintenanceError::NotLink(_) | MaintenanceError::Validation(_) => {
            PublicErrorDto::Validation
        }
        MaintenanceError::SkillNotFound(_) => PublicErrorDto::NotFound,
        MaintenanceError::RecoveryInProgress => PublicErrorDto::RecoveryRequired,
        MaintenanceError::PlanNotFound => PublicErrorDto::PlanStale,
        MaintenanceError::Store(_) | MaintenanceError::MaintenanceStore(_) => {
            PublicErrorDto::StateUnavailable
        }
        MaintenanceError::SourceTransition(_) => PublicErrorDto::RecoveryRequired,
        MaintenanceError::SourceLifecycle(_) => PublicErrorDto::RecoveryRequired,
        MaintenanceError::SourceUpdate(_) => PublicErrorDto::RecoveryRequired,
        MaintenanceError::FileSystem(_) | MaintenanceError::Internal(_) => PublicErrorDto::Internal,
    };
    CommandFailureDto {
        error: public_error,
        diagnostic: Some(DiagnosticDto {
            code: "command_error".into(),
            message: error.to_string(),
        }),
    }
}
