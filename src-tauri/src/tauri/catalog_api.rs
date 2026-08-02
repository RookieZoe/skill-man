use crate::core::catalog::{CatalogError, CatalogService};
use crate::core::domain::SkillId;
use crate::tauri_adapter::dto::{
    AgentActivationDto, CatalogListDto, CommandErrorDto, ListSkillsRequestDto, SkillDetailDto,
    SkillSummaryDto,
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
    ) -> Result<CatalogListDto, CommandErrorDto> {
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

    pub fn inspect_skill(&self, skill_id: String) -> Result<SkillDetailDto, CommandErrorDto> {
        self.catalog
            .inspect(SkillId(skill_id))
            .map(SkillDetailDto::from)
            .map_err(command_error)
    }

    pub fn list_agents(
        &self,
        skill_id: String,
    ) -> Result<Vec<AgentActivationDto>, CommandErrorDto> {
        self.catalog
            .list_agents(SkillId(skill_id))
            .map(|agents| agents.into_iter().map(AgentActivationDto::from).collect())
            .map_err(command_error)
    }
}

fn command_error(error: CatalogError) -> CommandErrorDto {
    let code = match error {
        CatalogError::SkillNotFound(_) => "not_found",
        CatalogError::Store(_) => "catalog_unavailable",
    };
    CommandErrorDto {
        code: code.into(),
        message: error.to_string(),
    }
}
