use std::path::PathBuf;

use crate::core::import::{ImportError, ImportService};
use crate::seams::filesystem::FileSystemError;
use crate::seams::import_store::ImportStoreError;
use crate::tauri_adapter::dto::{
    ApplyFileImportRequestDto, ApplyFileImportSelectionRequestDto,
    ApplyGitImportSelectionRequestDto, ApplyLinkImportRequestDto, CancelFileImportRequestDto,
    CancelGitImportSelectionRequestDto, CancelLinkImportRequestDto, CommandErrorDto,
    DiscoverFileImportCollectionRequestDto, DiscoverFileImportRequestDto,
    DiscoverGitImportRequestDto, DiscoverLinkImportRequestDto, FileImportCandidateDto,
    FileImportDiscoveryDto, FileImportPreviewDto, FileImportResultDto,
    FileImportSelectionPreviewDto, FileImportSelectionResultDto, GitImportDiscoveryDto,
    GitImportSelectionPreviewDto, GitImportSelectionResultDto, LinkImportCandidateDto,
    LinkImportPreviewDto, LinkImportResultDto, PlanFileImportRequestDto,
    PlanFileImportSelectionRequestDto, PlanFileReinstallRequestDto,
    PlanGitImportSelectionRequestDto, PlanLinkImportRequestDto,
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
            .map_err(|error| import_command_error(&error))
    }

    pub fn plan_link_import(
        &self,
        request: PlanLinkImportRequestDto,
    ) -> Result<LinkImportPreviewDto, CommandErrorDto> {
        self.service
            .plan_link(&PathBuf::from(request.source_path))
            .map(LinkImportPreviewDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn apply_link_import(
        &self,
        request: ApplyLinkImportRequestDto,
    ) -> Result<LinkImportResultDto, CommandErrorDto> {
        self.service
            .apply_link(&request.plan_token)
            .map(LinkImportResultDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn cancel_link_import(
        &self,
        request: CancelLinkImportRequestDto,
    ) -> Result<bool, CommandErrorDto> {
        self.service
            .cancel_link(&request.plan_token)
            .map_err(|error| import_command_error(&error))
    }

    pub fn discover_file_import(
        &self,
        request: DiscoverFileImportRequestDto,
    ) -> Result<FileImportCandidateDto, CommandErrorDto> {
        self.service
            .discover_file(&PathBuf::from(request.source_path))
            .map(FileImportCandidateDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn discover_file_import_collection(
        &self,
        request: DiscoverFileImportCollectionRequestDto,
    ) -> Result<FileImportDiscoveryDto, CommandErrorDto> {
        self.service
            .discover_file_collection(&PathBuf::from(request.source_path))
            .map(FileImportDiscoveryDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn plan_file_import(
        &self,
        request: PlanFileImportRequestDto,
    ) -> Result<FileImportPreviewDto, CommandErrorDto> {
        self.service
            .plan_file(&PathBuf::from(request.source_path))
            .map(FileImportPreviewDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn plan_file_reinstall(
        &self,
        request: PlanFileReinstallRequestDto,
    ) -> Result<FileImportPreviewDto, CommandErrorDto> {
        self.service
            .plan_file_reinstall(&PathBuf::from(request.source_path))
            .map(FileImportPreviewDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn plan_file_import_selection(
        &self,
        request: PlanFileImportSelectionRequestDto,
    ) -> Result<FileImportSelectionPreviewDto, CommandErrorDto> {
        self.service
            .plan_file_selection(
                &PathBuf::from(request.source_path),
                &request.selected_directory_names,
            )
            .map(FileImportSelectionPreviewDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn apply_file_import(
        &self,
        request: ApplyFileImportRequestDto,
    ) -> Result<FileImportResultDto, CommandErrorDto> {
        self.service
            .apply_file(&request.plan_token)
            .map(FileImportResultDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn apply_file_import_selection(
        &self,
        request: ApplyFileImportSelectionRequestDto,
    ) -> Result<FileImportSelectionResultDto, CommandErrorDto> {
        self.service
            .apply_file_selection(&request.plan_token)
            .map(FileImportSelectionResultDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn cancel_file_import(
        &self,
        request: CancelFileImportRequestDto,
    ) -> Result<bool, CommandErrorDto> {
        self.service
            .cancel_file(&request.plan_token)
            .map_err(|error| import_command_error(&error))
    }

    pub fn discover_git_import(
        &self,
        request: DiscoverGitImportRequestDto,
    ) -> Result<GitImportDiscoveryDto, CommandErrorDto> {
        self.service
            .discover_git(&request.source, request.force_full_depth)
            .map(GitImportDiscoveryDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn plan_git_import_selection(
        &self,
        request: PlanGitImportSelectionRequestDto,
    ) -> Result<GitImportSelectionPreviewDto, CommandErrorDto> {
        self.service
            .plan_git_selection(
                &request.source,
                request.force_full_depth,
                &request.selected_directory_names,
            )
            .map(GitImportSelectionPreviewDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn apply_git_import_selection(
        &self,
        request: ApplyGitImportSelectionRequestDto,
    ) -> Result<GitImportSelectionResultDto, CommandErrorDto> {
        self.service
            .apply_git_selection(&request.plan_token)
            .map(GitImportSelectionResultDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn cancel_git_import_selection(
        &self,
        request: CancelGitImportSelectionRequestDto,
    ) -> Result<bool, CommandErrorDto> {
        self.service
            .cancel_file(&request.plan_token)
            .map_err(|error| import_command_error(&error))
    }
}

pub fn import_command_error(error: &ImportError) -> CommandErrorDto {
    let code = match &error {
        ImportError::Validation(_) => "validation",
        ImportError::Conflict(_) | ImportError::Store(ImportStoreError::Conflict(_)) => "conflict",
        ImportError::SourceUnavailable(_) => "source_unavailable",
        ImportError::PlanStale | ImportError::PlanNotFound => "plan_stale",
        ImportError::DiskFull { .. } => "disk_full",
        ImportError::Modified => "modified",
        ImportError::RecoveryRequired(_) => "recovery_required",
        ImportError::FileSystem(FileSystemError::RecoveryRequired { .. }) => "recovery_required",
        ImportError::FileSystem(FileSystemError::PlanStale { .. }) => "plan_stale",
        ImportError::FileSystem(FileSystemError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            "permission_denied"
        }
        ImportError::FileSystem(FileSystemError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::StorageFull =>
        {
            "disk_full"
        }
        ImportError::Store(_) => "state_unavailable",
        ImportError::Source(crate::seams::source::SourceError::Validation(_)) => "validation",
        ImportError::Source(crate::seams::source::SourceError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            "permission_denied"
        }
        ImportError::Source(crate::seams::source::SourceError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::StorageFull =>
        {
            "disk_full"
        }
        ImportError::Source(crate::seams::source::SourceError::Io { .. }) => "source_unavailable",
        ImportError::Source(crate::seams::source::SourceError::Git(_)) => "source_unavailable",
        ImportError::FileSystem(_) | ImportError::Internal(_) => "internal",
    };
    CommandErrorDto {
        code: code.into(),
        message: error.to_string(),
    }
}
