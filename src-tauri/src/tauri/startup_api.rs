use crate::core::domain::AgentId;
use crate::core::preferences::{PreferencesError, PreferencesService};
use crate::core::startup::{StartupError, StartupService};
use crate::seams::filesystem::FileSystemError;
use crate::seams::preferences_store::PreferenceUpdates;
use crate::tauri_adapter::dto::{
    AppPreferencesDto, CommandFailureDto, CreateAgentDirectoryRequestDto, DiagnosticDto,
    PreferenceUpdatesDto, PublicErrorDto, StartupInfoDto, UpdatePreferencesResultDto,
};

pub struct StartupApi {
    preferences: PreferencesService,
    startup: StartupService,
}

impl StartupApi {
    pub fn new(preferences: PreferencesService, startup: StartupService) -> Self {
        Self {
            preferences,
            startup,
        }
    }

    pub fn load_preferences(&self) -> Result<AppPreferencesDto, CommandFailureDto> {
        self.preferences
            .load()
            .map(AppPreferencesDto::from)
            .map_err(preferences_command_error)
    }

    pub fn update_preferences(
        &self,
        request: PreferenceUpdatesDto,
    ) -> Result<UpdatePreferencesResultDto, CommandFailureDto> {
        let updates: PreferenceUpdates = request.into();
        self.preferences
            .update(updates)
            .map(|preferences| UpdatePreferencesResultDto {
                preferences: preferences.into(),
                // Runtime side effects are applied by the command glue; the
                // API layer stays UI-independent and returns no warning.
                warning: None,
            })
            .map_err(preferences_command_error)
    }

    pub fn startup_info(&self) -> Result<StartupInfoDto, CommandFailureDto> {
        self.startup
            .startup_info()
            .map(StartupInfoDto::from)
            .map_err(startup_command_error)
    }

    pub fn complete_onboarding(&self) -> Result<(), CommandFailureDto> {
        self.startup
            .complete_onboarding()
            .map_err(startup_command_error)
    }

    pub fn create_agent_directory(
        &self,
        request: CreateAgentDirectoryRequestDto,
    ) -> Result<StartupInfoDto, CommandFailureDto> {
        self.startup
            .create_agent_directory(&AgentId(request.agent_id))
            .and_then(|()| self.startup.startup_info())
            .map(StartupInfoDto::from)
            .map_err(startup_command_error)
    }
}

fn preferences_command_error(error: PreferencesError) -> CommandFailureDto {
    let public_error = match &error {
        PreferencesError::WriteGateClosed => PublicErrorDto::RecoveryRequired,
        PreferencesError::Store(_) => PublicErrorDto::StateUnavailable,
    };
    CommandFailureDto {
        error: public_error,
        diagnostic: Some(DiagnosticDto {
            code: "command_error".into(),
            message: error.to_string(),
        }),
    }
}

fn startup_command_error(error: StartupError) -> CommandFailureDto {
    let public_error = match &error {
        StartupError::Validation(_) => PublicErrorDto::Validation,
        StartupError::Store(_) | StartupError::Agents(_) => PublicErrorDto::StateUnavailable,
        StartupError::WriteGateClosed => PublicErrorDto::RecoveryRequired,
        StartupError::FileSystem(FileSystemError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            PublicErrorDto::PermissionDenied
        }
        StartupError::FileSystem(_) | StartupError::Internal(_) => PublicErrorDto::Internal,
    };
    CommandFailureDto {
        error: public_error,
        diagnostic: Some(DiagnosticDto {
            code: "command_error".into(),
            message: error.to_string(),
        }),
    }
}
