use crate::core::domain::SkillId;
use crate::core::update::{UpdateApplyRequest, UpdateError, UpdateSelection, UpdateService};
use crate::tauri_adapter::dto::{
    ApplySkillUpdatesRequestDto, CheckSkillUpdatesRequestDto, CommandFailureDto, DiagnosticDto,
    PinSkillUpdatesRequestDto, PlanSkillUpdatesRequestDto, PublicErrorDto, UpdateCheckReportDto,
    UpdatePlanDto, UpdateResultDto,
};

pub struct UpdateApi {
    service: UpdateService,
}

impl UpdateApi {
    pub fn new(service: UpdateService) -> Self {
        Self { service }
    }

    pub fn check_skill_updates(
        &self,
        request: CheckSkillUpdatesRequestDto,
    ) -> Result<UpdateCheckReportDto, CommandFailureDto> {
        self.service
            .check_updates(request.force)
            .map(UpdateCheckReportDto::from)
            .map_err(command_error)
    }

    pub fn plan_skill_updates(
        &self,
        request: PlanSkillUpdatesRequestDto,
    ) -> Result<UpdatePlanDto, CommandFailureDto> {
        let selections = request
            .selections
            .into_iter()
            .map(|selection| UpdateSelection {
                skill_id: SkillId(selection.skill_id),
                new_skill_path: selection.new_skill_path,
            })
            .collect::<Vec<_>>();
        self.service
            .plan_updates(&selections)
            .map(UpdatePlanDto::from)
            .map_err(command_error)
    }

    pub fn apply_skill_updates(
        &self,
        request: ApplySkillUpdatesRequestDto,
    ) -> Result<UpdateResultDto, CommandFailureDto> {
        let requests = request
            .requests
            .into_iter()
            .map(|item| UpdateApplyRequest {
                plan_token: item.plan_token,
                skill_id: SkillId(item.skill_id),
                directory_name: item.directory_name,
            })
            .collect::<Vec<_>>();
        self.service
            .apply_updates(&requests, request.abandon_changes)
            .map(UpdateResultDto::from)
            .map_err(command_error)
    }

    pub fn pin_skill_updates(
        &self,
        request: PinSkillUpdatesRequestDto,
    ) -> Result<(), CommandFailureDto> {
        let skill_ids = request
            .skill_ids
            .into_iter()
            .map(SkillId)
            .collect::<Vec<_>>();
        self.service.pin_updates(&skill_ids).map_err(command_error)
    }
}

fn command_error(error: UpdateError) -> CommandFailureDto {
    let public_error = match &error {
        UpdateError::Import(import_error) => {
            return crate::tauri_adapter::import_api::import_command_error(import_error);
        }
        UpdateError::Validation(_) => PublicErrorDto::Validation,
        UpdateError::Store(_) => PublicErrorDto::StateUnavailable,
        UpdateError::Source(crate::seams::source::SourceError::Git(_)) => {
            PublicErrorDto::SourceUnavailable
        }
        UpdateError::Source(crate::seams::source::SourceError::Validation(_)) => {
            PublicErrorDto::Validation
        }
        UpdateError::Source(crate::seams::source::SourceError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            PublicErrorDto::PermissionDenied
        }
        UpdateError::Source(_) => PublicErrorDto::SourceUnavailable,
        UpdateError::Internal(_) => PublicErrorDto::Internal,
    };
    CommandFailureDto {
        error: public_error,
        diagnostic: Some(DiagnosticDto {
            code: "command_error".into(),
            message: error.to_string(),
        }),
    }
}
