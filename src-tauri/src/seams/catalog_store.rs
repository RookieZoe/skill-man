use thiserror::Error;

use crate::core::domain::{CatalogFilter, SkillDetail, SkillId, SkillSummary};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupAccess {
    ReadWrite,
    ReadOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupDiagnosticCode {
    MigrationFailed,
    /// The Catalog predates Home identity; only the Home Binding/Legacy flow
    /// may migrate it (spec §3.4).
    MigrationRequired,
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
    /// When the onboarding was completed (or explicitly skipped); `None`
    /// means the next launch is still a first run (spec §8.7).
    fn first_run_completed_at(&self) -> Result<Option<String>, CatalogStoreError> {
        Ok(None)
    }

    /// Record that onboarding finished or was skipped, so later launches run
    /// the light scan instead of the first-run full scan.
    fn mark_first_run_completed(&self) -> Result<(), CatalogStoreError> {
        Ok(())
    }

    /// Skills ordered by their most recent enable, for the tray quick view
    /// (spec §9.4). `SkillSummary` carries the tray's display needs.
    fn recently_enabled(&self, limit: u32) -> Result<Vec<SkillSummary>, CatalogStoreError> {
        let _ = limit;
        Ok(Vec::new())
    }
}
