//! Typed Tauri-facing adapter for Skill Man application Updates.

use crate::core::app_update::{AppUpdateError, AppUpdateService};
use crate::seams::app_updater::AppUpdaterError;
use crate::tauri_adapter::dto::{
    AppUpdateCheckDto, CancelAppUpdateRequestDto, CancelledAppUpdateDto, CheckAppUpdateRequestDto,
    CommandErrorDto, DownloadAppUpdateRequestDto, DownloadedAppUpdateDto,
    InstallAppUpdateRequestDto,
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
    ) -> Result<AppUpdateCheckDto, CommandErrorDto> {
        self.service
            .check(request.force)
            .await
            .map(Into::into)
            .map_err(command_error)
    }

    pub async fn download_app_update(
        &self,
        request: DownloadAppUpdateRequestDto,
    ) -> Result<DownloadedAppUpdateDto, CommandErrorDto> {
        self.service
            .download(&request.update_id)
            .await
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn cancel_app_update(
        &self,
        request: CancelAppUpdateRequestDto,
    ) -> Result<CancelledAppUpdateDto, CommandErrorDto> {
        self.service
            .cancel(&request.update_id)
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn install_app_update(
        &self,
        request: InstallAppUpdateRequestDto,
    ) -> Result<(), CommandErrorDto> {
        self.service
            .install_and_restart(&request.update_id)
            .map_err(command_error)
    }
}

fn command_error(error: AppUpdateError) -> CommandErrorDto {
    let code = match &error {
        AppUpdateError::Preferences(_) => "state_unavailable",
        AppUpdateError::Updater(AppUpdaterError::SourceUnavailable(_)) => "source_unavailable",
        AppUpdateError::Updater(AppUpdaterError::NoPendingUpdate)
        | AppUpdateError::InvalidState(_) => "stale_update",
        AppUpdateError::Updater(AppUpdaterError::Cancelled) => "update_cancelled",
        AppUpdateError::Updater(AppUpdaterError::DownloadFailed(_)) => "download_failed",
        AppUpdateError::Updater(AppUpdaterError::InstallFailed(_)) => "install_failed",
        AppUpdateError::Internal(_) => "internal",
    };
    CommandErrorDto {
        code: code.into(),
        message: error.to_string(),
    }
}
