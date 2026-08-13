use std::path::PathBuf;

use crate::core::import::{ImportError, ImportService};
use crate::seams::filesystem::FileSystemError;
use crate::seams::import_store::ImportStoreError;
use crate::tauri_adapter::dto::{
    ApplyFileImportRequestDto, ApplyFileImportSelectionRequestDto,
    ApplyGitImportSelectionRequestDto, ApplyLinkImportRequestDto, CancelFileImportRequestDto,
    CancelGitImportSelectionRequestDto, CancelLinkImportRequestDto, CommandFailureDto,
    DiagnosticDto, DiscoverFileImportCollectionRequestDto, DiscoverFileImportRequestDto,
    DiscoverGitImportRequestDto, DiscoverLinkImportRequestDto, FileImportCandidateDto,
    FileImportDiscoveryDto, FileImportPreviewDto, FileImportResultDto,
    FileImportSelectionPreviewDto, FileImportSelectionResultDto, GitImportDiscoveryDto,
    GitImportSelectionPreviewDto, GitImportSelectionResultDto, LinkImportCandidateDto,
    LinkImportPreviewDto, LinkImportResultDto, PlanFileImportRequestDto,
    PlanFileImportSelectionRequestDto, PlanFileReinstallRequestDto,
    PlanGitImportSelectionRequestDto, PlanLinkImportRequestDto, PublicErrorDto,
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
    ) -> Result<LinkImportCandidateDto, CommandFailureDto> {
        self.service
            .discover_link(&PathBuf::from(request.source_path))
            .map(LinkImportCandidateDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn plan_link_import(
        &self,
        request: PlanLinkImportRequestDto,
    ) -> Result<LinkImportPreviewDto, CommandFailureDto> {
        self.service
            .plan_link(&PathBuf::from(request.source_path))
            .map(LinkImportPreviewDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn apply_link_import(
        &self,
        request: ApplyLinkImportRequestDto,
    ) -> Result<LinkImportResultDto, CommandFailureDto> {
        self.service
            .apply_link(&request.plan_token)
            .map(LinkImportResultDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn cancel_link_import(
        &self,
        request: CancelLinkImportRequestDto,
    ) -> Result<bool, CommandFailureDto> {
        self.service
            .cancel_link(&request.plan_token)
            .map_err(|error| import_command_error(&error))
    }

    pub fn discover_file_import(
        &self,
        request: DiscoverFileImportRequestDto,
    ) -> Result<FileImportCandidateDto, CommandFailureDto> {
        self.service
            .discover_file(&PathBuf::from(request.source_path))
            .map(FileImportCandidateDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn discover_file_import_collection(
        &self,
        request: DiscoverFileImportCollectionRequestDto,
    ) -> Result<FileImportDiscoveryDto, CommandFailureDto> {
        self.service
            .discover_file_collection(&PathBuf::from(request.source_path))
            .map(FileImportDiscoveryDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn plan_file_import(
        &self,
        request: PlanFileImportRequestDto,
    ) -> Result<FileImportPreviewDto, CommandFailureDto> {
        self.service
            .plan_file(&PathBuf::from(request.source_path))
            .map(FileImportPreviewDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn plan_file_reinstall(
        &self,
        request: PlanFileReinstallRequestDto,
    ) -> Result<FileImportPreviewDto, CommandFailureDto> {
        self.service
            .plan_file_reinstall(&PathBuf::from(request.source_path))
            .map(FileImportPreviewDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn plan_file_import_selection(
        &self,
        request: PlanFileImportSelectionRequestDto,
    ) -> Result<FileImportSelectionPreviewDto, CommandFailureDto> {
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
    ) -> Result<FileImportResultDto, CommandFailureDto> {
        self.service
            .apply_file(&request.plan_token)
            .map(FileImportResultDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn apply_file_import_selection(
        &self,
        request: ApplyFileImportSelectionRequestDto,
    ) -> Result<FileImportSelectionResultDto, CommandFailureDto> {
        self.service
            .apply_file_selection(&request.plan_token)
            .map(FileImportSelectionResultDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn cancel_file_import(
        &self,
        request: CancelFileImportRequestDto,
    ) -> Result<bool, CommandFailureDto> {
        self.service
            .cancel_file(&request.plan_token)
            .map_err(|error| import_command_error(&error))
    }

    pub fn discover_git_import(
        &self,
        request: DiscoverGitImportRequestDto,
    ) -> Result<GitImportDiscoveryDto, CommandFailureDto> {
        self.service
            .discover_git(&request.source, request.force_full_depth)
            .map(GitImportDiscoveryDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn plan_git_import_selection(
        &self,
        request: PlanGitImportSelectionRequestDto,
    ) -> Result<GitImportSelectionPreviewDto, CommandFailureDto> {
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
    ) -> Result<GitImportSelectionResultDto, CommandFailureDto> {
        self.service
            .apply_git_selection(&request.plan_token)
            .map(GitImportSelectionResultDto::from)
            .map_err(|error| import_command_error(&error))
    }

    pub fn cancel_git_import_selection(
        &self,
        request: CancelGitImportSelectionRequestDto,
    ) -> Result<bool, CommandFailureDto> {
        self.service
            .cancel_file(&request.plan_token)
            .map_err(|error| import_command_error(&error))
    }
}

pub fn import_command_error(error: &ImportError) -> CommandFailureDto {
    let public_error = match &error {
        ImportError::Validation(_) => PublicErrorDto::Validation,
        ImportError::Conflict(directory_name)
        | ImportError::Store(ImportStoreError::Conflict(directory_name)) => {
            PublicErrorDto::Conflict {
                directory_name: directory_name.clone(),
            }
        }
        ImportError::SourceUnavailable(_) => PublicErrorDto::SourceUnavailable,
        ImportError::PlanStale | ImportError::PlanNotFound => PublicErrorDto::PlanStale,
        ImportError::DiskFull {
            required_bytes,
            available_bytes,
        } => PublicErrorDto::DiskFull {
            required_bytes: *required_bytes,
            available_bytes: *available_bytes,
        },
        ImportError::Modified => PublicErrorDto::Modified,
        ImportError::RecoveryRequired(_) => PublicErrorDto::RecoveryRequired,
        ImportError::FileSystem(FileSystemError::RecoveryRequired { .. }) => {
            PublicErrorDto::RecoveryRequired
        }
        ImportError::FileSystem(FileSystemError::PlanStale { .. }) => PublicErrorDto::PlanStale,
        ImportError::FileSystem(FileSystemError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            PublicErrorDto::PermissionDenied
        }
        ImportError::FileSystem(FileSystemError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::StorageFull =>
        {
            PublicErrorDto::DiskFull {
                required_bytes: 0,
                available_bytes: 0,
            }
        }
        ImportError::Store(_) => PublicErrorDto::StateUnavailable,
        ImportError::Source(crate::seams::source::SourceError::Validation(_)) => {
            PublicErrorDto::Validation
        }
        ImportError::Source(crate::seams::source::SourceError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            PublicErrorDto::PermissionDenied
        }
        ImportError::Source(crate::seams::source::SourceError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::StorageFull =>
        {
            PublicErrorDto::DiskFull {
                required_bytes: 0,
                available_bytes: 0,
            }
        }
        ImportError::Source(crate::seams::source::SourceError::Io { .. }) => {
            PublicErrorDto::SourceUnavailable
        }
        ImportError::Source(crate::seams::source::SourceError::Git(_)) => {
            PublicErrorDto::SourceUnavailable
        }
        ImportError::FileSystem(_) | ImportError::Internal(_) => PublicErrorDto::Internal,
    };
    CommandFailureDto {
        error: public_error,
        diagnostic: Some(DiagnosticDto {
            code: "command_error".into(),
            message: error.to_string(),
        }),
    }
}
