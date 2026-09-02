use std::sync::Arc;

use thiserror::Error;

use crate::core::domain::{CatalogFilter, CatalogSnapshot, SkillDetail, SkillId, SkillSummary};
use crate::seams::catalog_store::{CatalogStore, CatalogStoreError};

#[derive(Clone)]
pub struct CatalogService {
    store: Arc<dyn CatalogStore>,
}

impl CatalogService {
    pub fn new(store: Arc<dyn CatalogStore>) -> Self {
        Self { store }
    }

    pub fn list(
        &self,
        filter: CatalogFilter,
    ) -> Result<CatalogSnapshot<SkillSummary>, CatalogError> {
        Ok(CatalogSnapshot {
            snapshot_version: self.store.snapshot_version(),
            items: self.store.list(filter)?,
        })
    }

    pub fn inspect(&self, skill_id: SkillId) -> Result<SkillDetail, CatalogError> {
        self.store
            .inspect(&skill_id)?
            .ok_or(CatalogError::SkillNotFound(skill_id.0))
    }
}

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("Managed Skill '{0}' was not found")]
    SkillNotFound(String),
    #[error(transparent)]
    Store(#[from] CatalogStoreError),
}
