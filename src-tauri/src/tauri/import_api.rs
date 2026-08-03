use std::path::PathBuf;

use crate::core::import::{ImportError, ImportService};
use crate::seams::filesystem::FileSystemError;
use crate::seams::import_store::ImportStoreError;
use crate::tauri_adapter::dto::{
    ApplyLinkImportRequestDto, CancelLinkImportRequestDto, CommandErrorDto,
    DiscoverLinkImportRequestDto, LinkImportCandidateDto, LinkImportPreviewDto,
    LinkImportResultDto, PlanLinkImportRequestDto,
};

pub struct ImportApi {
    service: ImportService,
}

impl ImportApi {
    pub fn new(service: ImportService) -> Self {
        Self { service }
    }

    pub fn discover_link_import(
        &self,
        request: DiscoverLinkImportRequestDto,
    ) -> Result<LinkImportCandidateDto, CommandErrorDto> {
        self.service
            .discover_link(&PathBuf::from(request.source_path))
            .map(LinkImportCandidateDto::from)
            .map_err(command_error)
    }

    pub fn plan_link_import(
        &self,
        request: PlanLinkImportRequestDto,
    ) -> Result<LinkImportPreviewDto, CommandErrorDto> {
        self.service
            .plan_link(&PathBuf::from(request.source_path))
            .map(LinkImportPreviewDto::from)
            .map_err(command_error)
    }

    pub fn apply_link_import(
        &self,
        request: ApplyLinkImportRequestDto,
    ) -> Result<LinkImportResultDto, CommandErrorDto> {
        self.service
            .apply_link(&request.plan_token)
            .map(LinkImportResultDto::from)
            .map_err(command_error)
    }

    pub fn cancel_link_import(
        &self,
        request: CancelLinkImportRequestDto,
    ) -> Result<bool, CommandErrorDto> {
        self.service
            .cancel_link(&request.plan_token)
            .map_err(command_error)
    }
}

fn command_error(error: ImportError) -> CommandErrorDto {
    let code = match &error {
        ImportError::Validation(_) => "validation",
        ImportError::Conflict(_) | ImportError::Store(ImportStoreError::Conflict(_)) => "conflict",
        ImportError::SourceUnavailable(_) => "source_unavailable",
        ImportError::PlanStale | ImportError::PlanNotFound => "plan_stale",
        ImportError::FileSystem(FileSystemError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            "permission_denied"
        }
        ImportError::Store(_) => "state_unavailable",
        ImportError::FileSystem(_) | ImportError::Internal(_) => "internal",
    };
    CommandErrorDto {
        code: code.into(),
        message: error.to_string(),
    }
}
