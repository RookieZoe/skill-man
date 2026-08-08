use std::path::PathBuf;

use crate::core::domain::SkillId;
use crate::core::maintenance::{MaintenanceError, StartupMaintenance};
use crate::seams::filesystem::FileSystemError;
use crate::tauri_adapter::dto::{
    ActivationHealthReportDto, ApplyRelocateLinkRequestDto, CancelRelocateLinkRequestDto,
    CommandErrorDto, RelocateLinkPreviewDto, RelocateLinkRequestDto, RelocateLinkResultDto,
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
    ) -> Result<ActivationHealthReportDto, CommandErrorDto> {
        self.maintenance
            .run_activation_health_check()
            .map(ActivationHealthReportDto::from)
            .map_err(maintenance_error)
    }

    pub fn relocate_link(
        &self,
        request: RelocateLinkRequestDto,
    ) -> Result<RelocateLinkPreviewDto, CommandErrorDto> {
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
    ) -> Result<RelocateLinkResultDto, CommandErrorDto> {
        self.maintenance
            .apply_relocate(&request.plan_token)
            .map(RelocateLinkResultDto::from)
            .map_err(maintenance_error)
    }

    pub fn cancel_relocate_link(
        &self,
        request: CancelRelocateLinkRequestDto,
    ) -> Result<bool, CommandErrorDto> {
        self.maintenance
            .cancel_relocate(&request.plan_token)
            .map_err(maintenance_error)
    }
}

fn maintenance_error(error: MaintenanceError) -> CommandErrorDto {
    let code = match &error {
        MaintenanceError::FileSystem(FileSystemError::RecoveryRequired { .. })
        | MaintenanceError::RecoveryRequired { .. } => "recovery_required",
        MaintenanceError::FileSystem(FileSystemError::PlanStale { .. })
        | MaintenanceError::PlanStale => "plan_stale",
        MaintenanceError::FileSystem(FileSystemError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            "permission_denied"
        }
        MaintenanceError::NotLink(_) | MaintenanceError::Validation(_) => "validation",
        MaintenanceError::RecoveryInProgress => "recovery_required",
        MaintenanceError::PlanNotFound => "plan_stale",
        MaintenanceError::Store(_) | MaintenanceError::MaintenanceStore(_) => "state_unavailable",
        MaintenanceError::FileSystem(_) | MaintenanceError::Internal(_) => "internal",
    };
    CommandErrorDto {
        code: code.into(),
        message: error.to_string(),
    }
}
