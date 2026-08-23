//! Source Capability Scan classification (ADR-0014, spec §8.3).
//!
//! The public module has one operation: `scan`.  It classifies only facts
//! read by its seam; it never infers a Source Release from legacy bindings
//! and has no write capability.

use std::sync::Arc;

use thiserror::Error;

pub use crate::seams::git_source_capability::{
    GitRepositorySourceFact, GitSourceCapabilityFacts, GitSourceCapabilityReader,
    GitSourceCatalogStructure, GitSourceFact, GitSourceManifestFact, GitSourceReleaseFact,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitSourceCapabilityKind {
    GitRepositorySource,
    LegacyPerSkillGitState,
    RemoteSourceIdentityConflict,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitSourceCapabilitySource {
    pub remote_id: String,
    pub canonical_url: String,
    pub kind: GitSourceCapabilityKind,
}

impl GitSourceCapabilitySource {
    /// Legacy and conflicted sources retain their existing per-Skill read,
    /// Disable and Remove behaviour.  This is deliberately separate from
    /// the narrower source-level write gate.
    pub fn allows_read_and_maintenance(&self) -> bool {
        true
    }

    pub fn allows_source_writes(&self) -> bool {
        self.kind == GitSourceCapabilityKind::GitRepositorySource
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitSourceCapabilityReport {
    pub sources: Vec<GitSourceCapabilitySource>,
}

#[derive(Debug, Error)]
pub enum GitSourceCapabilityError {
    #[error("the Source Capability Scan could not read the Catalog: {0}")]
    Read(String),
}

pub struct GitSourceCapabilityScan {
    reader: Arc<dyn GitSourceCapabilityReader>,
}

impl GitSourceCapabilityScan {
    pub fn new(reader: Arc<dyn GitSourceCapabilityReader>) -> Self {
        Self { reader }
    }

    pub fn scan(&self) -> Result<GitSourceCapabilityReport, GitSourceCapabilityError> {
        let facts = self.reader.read().map_err(GitSourceCapabilityError::Read)?;
        let sources = facts
            .sources
            .into_iter()
            .map(|source| GitSourceCapabilitySource {
                remote_id: source.remote_id.clone(),
                canonical_url: source.canonical_url.clone(),
                kind: classify(&facts.catalog_structure, &source),
            })
            .collect();
        Ok(GitSourceCapabilityReport { sources })
    }
}

fn classify(
    structure: &GitSourceCatalogStructure,
    source: &GitSourceFact,
) -> GitSourceCapabilityKind {
    let repository_is_complete = source
        .repository
        .as_ref()
        .is_some_and(|repository| repository_is_complete(structure, source, repository));

    if !manifest_matches(source, repository_is_complete) {
        return GitSourceCapabilityKind::RemoteSourceIdentityConflict;
    }

    if repository_is_complete {
        GitSourceCapabilityKind::GitRepositorySource
    } else {
        // A parent without a complete current release is still Legacy.  This
        // covers v6 per-Skill bindings and every partial/missing future
        // shape; scan never writes a "repair" or promotes it automatically.
        GitSourceCapabilityKind::LegacyPerSkillGitState
    }
}

fn repository_is_complete(
    structure: &GitSourceCatalogStructure,
    source: &GitSourceFact,
    repository: &GitRepositorySourceFact,
) -> bool {
    if !structure.supports_repository_sources()
        || repository.canonical_url != source.canonical_url
        || repository.provider.as_deref().is_none_or(str::is_empty)
        || repository.tracking_ref.as_deref().is_none_or(str::is_empty)
        || repository
            .current_release_id
            .as_deref()
            .is_none_or(str::is_empty)
    {
        return false;
    }
    let Some(release) = repository.current_release.as_ref() else {
        return false;
    };
    if release.release_id != repository.current_release_id.as_deref().unwrap_or_default()
        || release.remote_id != source.remote_id
        || release.tracking_ref != repository.tracking_ref.as_deref().unwrap_or_default()
        || release.resolved_commit.is_empty()
    {
        return false;
    }
    same_non_empty_member_set(&release.member_paths, &repository.current_member_paths)
}

fn manifest_matches(source: &GitSourceFact, repository_is_complete: bool) -> bool {
    let GitSourceManifestFact::Present {
        remote_id,
        canonical_url,
        aliases,
        provider,
        tracking_ref,
        current_release_id,
    } = &source.manifest
    else {
        return false;
    };
    if remote_id != &source.remote_id
        || canonical_url != &source.canonical_url
        || !same_unique_string_set(aliases, &source.catalog_aliases)
    {
        return false;
    }
    if !repository_is_complete {
        return true;
    }
    let repository = source
        .repository
        .as_ref()
        .expect("complete repository fact");
    provider.as_deref() == repository.provider.as_deref()
        && tracking_ref.as_deref() == repository.tracking_ref.as_deref()
        && current_release_id.as_deref() == repository.current_release_id.as_deref()
}

fn same_unique_string_set(left: &[String], right: &[String]) -> bool {
    if left.len() != right.len()
        || left.iter().any(|value| value.is_empty())
        || right.iter().any(|value| value.is_empty())
    {
        return false;
    }
    let left_len = left.len();
    let right_len = right.len();
    let mut left = left.to_vec();
    let mut right = right.to_vec();
    left.sort();
    right.sort();
    left.dedup();
    right.dedup();
    left.len() == left_len && right.len() == right_len && left == right
}

fn same_non_empty_member_set(left: &[String], right: &[String]) -> bool {
    if left.is_empty() || right.is_empty() || left.len() != right.len() {
        return false;
    }
    if left.iter().any(|path| path.is_empty()) || right.iter().any(|path| path.is_empty()) {
        return false;
    }
    let left_len = left.len();
    let right_len = right.len();
    let mut left = left.to_vec();
    let mut right = right.to_vec();
    left.sort();
    right.sort();
    left.dedup();
    right.dedup();
    left.len() == left_len && right.len() == right_len && left == right
}
