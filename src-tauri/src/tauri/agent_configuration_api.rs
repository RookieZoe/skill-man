use std::sync::Arc;

use crate::core::agent_configuration::{AgentConfigurationError, AgentConfigurationService};
use crate::tauri_adapter::dto::{
    AgentConfigurationApplyResultDto, AgentConfigurationPlanDto, AgentManagementSnapshotDto,
    ApplyAgentConfigurationPlanRequestDto, CommandFailureDto, CreateAgentConfigurationRequestDto,
    DeleteAgentConfigurationRequestDto, DiagnosticDto, EditAgentConfigurationRequestDto,
    PublicErrorDto,
};

pub struct AgentConfigurationApi {
    service: Arc<AgentConfigurationService>,
}

impl AgentConfigurationApi {
    pub fn new(service: Arc<AgentConfigurationService>) -> Self {
        Self { service }
    }

    pub fn snapshot(&self) -> Result<AgentManagementSnapshotDto, CommandFailureDto> {
        self.service
            .snapshot()
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn plan_create(
        &self,
        request: CreateAgentConfigurationRequestDto,
    ) -> Result<AgentConfigurationPlanDto, CommandFailureDto> {
        self.service
            .plan_create(request.into())
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn plan_edit(
        &self,
        request: EditAgentConfigurationRequestDto,
    ) -> Result<AgentConfigurationPlanDto, CommandFailureDto> {
        let (agent_id, draft) = request.into_parts();
        self.service
            .plan_edit(&agent_id, draft)
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn plan_delete(
        &self,
        request: DeleteAgentConfigurationRequestDto,
    ) -> Result<AgentConfigurationPlanDto, CommandFailureDto> {
        self.service
            .plan_delete(&request.agent_id)
            .map(Into::into)
            .map_err(command_error)
    }

    pub fn apply(
        &self,
        request: ApplyAgentConfigurationPlanRequestDto,
    ) -> Result<AgentConfigurationApplyResultDto, CommandFailureDto> {
        self.service
            .apply(&request.plan_token)
            .map(Into::into)
            .map_err(command_error)
    }
}

fn command_error(error: AgentConfigurationError) -> CommandFailureDto {
    let (public_error, code) = match &error {
        AgentConfigurationError::WriteGateClosed => {
            (PublicErrorDto::RecoveryRequired, "agent_write_gate_closed")
        }
        AgentConfigurationError::InvalidName => (
            PublicErrorDto::AgentConfigurationNameInvalid,
            "agent_name_invalid",
        ),
        AgentConfigurationError::NameConflict { name } => (
            PublicErrorDto::AgentConfigurationNameConflict { name: name.clone() },
            "agent_name_conflict",
        ),
        AgentConfigurationError::PresetNotFound { preset_key } => (
            PublicErrorDto::AgentPresetNotFound {
                preset_key: preset_key.clone(),
            },
            "agent_preset_not_found",
        ),
        AgentConfigurationError::RootRequired => {
            (PublicErrorDto::AgentRootRequired, "agent_root_required")
        }
        AgentConfigurationError::ActivationTargetRequired => (
            PublicErrorDto::AgentActivationTargetRequired,
            "agent_target_required",
        ),
        AgentConfigurationError::DuplicateRoot { path } => (
            PublicErrorDto::AgentRootDuplicate { path: path.clone() },
            "agent_root_duplicate",
        ),
        AgentConfigurationError::RootOverlap {
            path,
            conflicting_path,
        } => (
            PublicErrorDto::AgentRootOverlap {
                path: path.clone(),
                conflicting_path: conflicting_path.clone(),
            },
            "agent_root_overlap",
        ),
        AgentConfigurationError::HomeOverlap { path } => (
            PublicErrorDto::AgentRootHomeOverlap { path: path.clone() },
            "agent_root_home_overlap",
        ),
        AgentConfigurationError::InvalidRoot { path } => (
            PublicErrorDto::AgentRootInvalid { path: path.clone() },
            "agent_root_invalid",
        ),
        AgentConfigurationError::TargetNotWritable { path } => (
            PublicErrorDto::AgentTargetNotWritable { path: path.clone() },
            "agent_target_not_writable",
        ),
        AgentConfigurationError::TargetNotAllowed { path } => (
            PublicErrorDto::AgentTargetNotAllowed { path: path.clone() },
            "agent_target_not_allowed",
        ),
        AgentConfigurationError::InvalidProjectSkillsDir => (
            PublicErrorDto::AgentProjectSkillsDirInvalid,
            "agent_project_skills_dir_invalid",
        ),
        AgentConfigurationError::NotFound { agent_id } => (
            PublicErrorDto::AgentConfigurationNotFound {
                agent_id: agent_id.clone(),
            },
            "agent_configuration_not_found",
        ),
        AgentConfigurationError::PlanStale => (PublicErrorDto::PlanStale, "agent_plan_stale"),
        AgentConfigurationError::TargetInUse { skill_ids } => (
            PublicErrorDto::AgentTargetInUse {
                skill_ids: skill_ids.clone(),
            },
            "agent_target_in_use",
        ),
        AgentConfigurationError::Store(_) => (
            PublicErrorDto::CatalogUnavailable,
            "agent_configuration_store_failed",
        ),
        AgentConfigurationError::FileSystem(_) => (
            PublicErrorDto::StateUnavailable,
            "agent_configuration_filesystem_failed",
        ),
        AgentConfigurationError::RecoveryRequired(_) => (
            PublicErrorDto::RecoveryRequired,
            "agent_configuration_recovery_required",
        ),
    };
    CommandFailureDto {
        error: public_error,
        diagnostic: Some(DiagnosticDto {
            code: code.into(),
            message: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_configuration_errors_keep_app_copy_out_of_the_error_union() {
        let failure = command_error(AgentConfigurationError::RootOverlap {
            path: "/Users/me/.agents/skills".into(),
            conflicting_path: "/Users/me/.agents".into(),
        });
        let json = serde_json::to_value(failure).expect("serialize failure");
        assert_eq!(json["error"]["code"], "agent_root_overlap");
        assert_eq!(json["error"]["path"], "/Users/me/.agents/skills");
        assert!(json["error"].get("message").is_none());
        assert_eq!(
            json["diagnostic"]["code"], "agent_root_overlap",
            "raw detail stays separately labeled"
        );
    }
}
