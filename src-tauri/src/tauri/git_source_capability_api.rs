use std::sync::Arc;

use crate::core::git_source_capability::{GitSourceCapabilityError, GitSourceCapabilityScan};
use crate::tauri_adapter::dto::{
    CommandFailureDto, DiagnosticDto, GitSourceCapabilityReportDto, PublicErrorDto,
};

pub struct GitSourceCapabilityApi {
    scan: Arc<GitSourceCapabilityScan>,
}

impl GitSourceCapabilityApi {
    pub fn new(scan: Arc<GitSourceCapabilityScan>) -> Self {
        Self { scan }
    }

    pub fn get_git_source_capability(
        &self,
    ) -> Result<GitSourceCapabilityReportDto, CommandFailureDto> {
        self.scan.scan().map(Into::into).map_err(command_error)
    }
}

fn command_error(error: GitSourceCapabilityError) -> CommandFailureDto {
    CommandFailureDto {
        error: PublicErrorDto::StateUnavailable,
        diagnostic: Some(DiagnosticDto {
            code: "git_source_capability_scan_failed".into(),
            message: error.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_failure_maps_to_the_closed_state_error() {
        let failure = command_error(GitSourceCapabilityError::Read("unreadable".into()));
        assert_eq!(failure.error, PublicErrorDto::StateUnavailable);
        assert_eq!(
            failure.diagnostic.expect("diagnostic").code,
            "git_source_capability_scan_failed"
        );
    }
}
