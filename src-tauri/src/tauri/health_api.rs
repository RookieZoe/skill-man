use crate::core::maintenance::{MaintenanceError, StartupMaintenance};
use crate::seams::filesystem::FileSystemError;
use crate::tauri_adapter::dto::{ActivationHealthReportDto, CommandErrorDto};

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
}

fn maintenance_error(error: MaintenanceError) -> CommandErrorDto {
    let code = match &error {
        MaintenanceError::FileSystem(FileSystemError::RecoveryRequired { .. }) => {
            "recovery_required"
        }
        MaintenanceError::FileSystem(FileSystemError::PlanStale { .. }) => "recovery_required",
        MaintenanceError::FileSystem(FileSystemError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            "permission_denied"
        }
        MaintenanceError::Store(_) => "state_unavailable",
        MaintenanceError::MaintenanceStore(_) => "state_unavailable",
        MaintenanceError::FileSystem(_) | MaintenanceError::Internal(_) => "internal",
    };
    CommandErrorDto {
        code: code.into(),
        message: error.to_string(),
    }
}
