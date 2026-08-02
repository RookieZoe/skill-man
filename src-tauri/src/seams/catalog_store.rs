use thiserror::Error;

use crate::core::domain::{AgentActivation, CatalogFilter, SkillDetail, SkillId, SkillSummary};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupAccess {
    ReadWrite,
    ReadOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupDiagnosticCode {
    MigrationFailed,
    UnsupportedSchema,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupDiagnostic {
    pub code: StartupDiagnosticCode,
    pub message: String,
    pub backup_path: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupStatus {
    pub access: StartupAccess,
    pub schema_version: u32,
    pub diagnostic: Option<StartupDiagnostic>,
}

#[derive(Debug, Error)]
pub enum CatalogStoreError {
    #[error("the catalog could not be read: {0}")]
    Unavailable(String),
}

pub trait CatalogStore: Send + Sync {
    fn snapshot_version(&self) -> u64;
    fn list(&self, filter: CatalogFilter) -> Result<Vec<SkillSummary>, CatalogStoreError>;
    fn inspect(&self, skill_id: &SkillId) -> Result<Option<SkillDetail>, CatalogStoreError>;
    fn list_agents(
        &self,
        skill_id: &SkillId,
    ) -> Result<Option<Vec<AgentActivation>>, CatalogStoreError>;
}
