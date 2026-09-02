use crate::core::catalog::{CatalogError, CatalogService};
use crate::core::domain::SkillId;
use crate::tauri_adapter::dto::{
    CatalogListDto, CommandFailureDto, DiagnosticDto, ListSkillsRequestDto, PublicErrorDto,
    SkillDetailDto, SkillSummaryDto,
};

#[derive(Clone)]
pub struct CatalogApi {
    catalog: CatalogService,
}

impl CatalogApi {
    pub fn new(catalog: CatalogService) -> Self {
        Self { catalog }
    }

    pub fn list_skills(
        &self,
        request: ListSkillsRequestDto,
    ) -> Result<CatalogListDto, CommandFailureDto> {
        let snapshot = self
            .catalog
            .list(request.filter.into())
            .map_err(command_error)?;

        Ok(CatalogListDto {
            snapshot_version: snapshot.snapshot_version,
            items: snapshot
                .items
                .into_iter()
                .map(SkillSummaryDto::from)
                .collect(),
        })
    }

    pub fn inspect_skill(&self, skill_id: String) -> Result<SkillDetailDto, CommandFailureDto> {
        self.catalog
            .inspect(SkillId(skill_id))
            .map(SkillDetailDto::from)
            .map_err(command_error)
    }
}

fn command_error(error: CatalogError) -> CommandFailureDto {
    let public_error = match error {
        CatalogError::SkillNotFound(_) => PublicErrorDto::NotFound,
        CatalogError::Store(_) => PublicErrorDto::CatalogUnavailable,
    };
    CommandFailureDto {
        error: public_error,
        diagnostic: Some(DiagnosticDto {
            code: "command_error".into(),
            message: error.to_string(),
        }),
    }
}
