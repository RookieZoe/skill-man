//! Typed Tauri-facing adapter for Skill Man application Updates.

use crate::core::app_update::{AppUpdateError, AppUpdateService};
use crate::seams::app_updater::AppUpdaterError;
use crate::tauri_adapter::dto::{
    AppUpdateCheckDto, CancelAppUpdateRequestDto, CancelledAppUpdateDto, CheckAppUpdateRequestDto,
    CommandFailureDto, DiagnosticDto, DownloadAppUpdateRequestDto, DownloadedAppUpdateDto,
    InstallAppUpdateRequestDto, PublicErrorDto,
};

pub struct AppUpdateApi {
    service: AppUpdateService,
}

impl AppUpdateApi {
    pub fn new(service: AppUpdateService) -> Self {
        Self { service }
    }

    pub async fn check_app_update(
        &self,
        request: CheckAppUpdateRequestDto,
    ) -> Result<AppUpdateCheckDto, CommandFailureDto> {
        self.service
            .check(request.force)
            .await
            .map(Into::into)
            .map_err(command_error)
    }

    pub async fn download_app_update(
        &self,
        request: DownloadAppUpdateRequestDto,
    ) -> Result<DownloadedAppUpdateDto, CommandFailureDto> {
        self.service
            .download(&request.update_id)
            .await
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn cancel_app_update(
        &self,
        request: CancelAppUpdateRequestDto,
    ) -> Result<CancelledAppUpdateDto, CommandFailureDto> {
        self.service
            .cancel(&request.update_id)
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn install_app_update(
        &self,
        request: InstallAppUpdateRequestDto,
    ) -> Result<(), CommandFailureDto> {
        self.service
            .install_and_restart(&request.update_id)
            .map_err(command_error)
    }
}

fn command_error(error: AppUpdateError) -> CommandFailureDto {
    let public_error = match &error {
        AppUpdateError::Preferences(_) => PublicErrorDto::StateUnavailable,
        AppUpdateError::Updater(AppUpdaterError::SourceUnavailable(_)) => {
            PublicErrorDto::SourceUnavailable
        }
        AppUpdateError::Updater(AppUpdaterError::NoPendingUpdate)
        | AppUpdateError::InvalidState(_) => PublicErrorDto::StaleUpdate,
        AppUpdateError::WriteGateClosed => PublicErrorDto::RecoveryRequired,
        AppUpdateError::Updater(AppUpdaterError::Cancelled) => PublicErrorDto::UpdateCancelled,
        AppUpdateError::Updater(AppUpdaterError::DownloadFailed(_)) => {
            PublicErrorDto::DownloadFailed
        }
        AppUpdateError::Updater(AppUpdaterError::InstallFailed(_)) => PublicErrorDto::InstallFailed,
        AppUpdateError::Internal(_) => PublicErrorDto::Internal,
    };
    CommandFailureDto {
        error: public_error,
        diagnostic: Some(DiagnosticDto {
            code: "command_error".into(),
            message: error.to_string(),
        }),
    }
}
