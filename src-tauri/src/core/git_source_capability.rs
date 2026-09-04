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

/// One current or tombstoned member of a Git Repository Source, surfaced
/// for the minimal #93 lifecycle surface (per-member Create Local Source
/// Copy). Labels use `skill_path`, which is unique within the source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitSourceCapabilityMember {
    pub skill_id: String,
    pub skill_path: String,
    /// `false` marks a tombstoned member (no bytes to copy).
    pub presence: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitSourceCapabilitySource {
    pub remote_id: String,
    pub canonical_url: String,
    pub kind: GitSourceCapabilityKind,
    /// Complete only for `GitRepositorySource`; every other kind is empty.
    pub members: Vec<GitSourceCapabilityMember>,
    pub provider: Option<String>,
    pub tracking_mode: Option<String>,
    pub tracking_value: Option<String>,
    pub selected_ref: Option<String>,
    pub resolved_commit: Option<String>,
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
            .map(|source| {
                let kind = classify(&facts.catalog_structure, &source);
                let (
                    members,
                    provider,
                    tracking_mode,
                    tracking_value,
                    selected_ref,
                    resolved_commit,
                ) = match (&kind, source.repository.as_ref()) {
                    (GitSourceCapabilityKind::GitRepositorySource, Some(repository)) => {
                        let members = repository
                            .current_members
                            .iter()
                            .map(|member| GitSourceCapabilityMember {
                                skill_id: member.skill_id.clone(),
                                skill_path: member.skill_path.clone(),
                                presence: member.presence,
                            })
                            .collect();
                        let release = repository.current_release.as_ref();
                        (
                            members,
                            repository.provider.clone(),
                            repository.tracking_mode.clone(),
                            repository.tracking_value.clone(),
                            repository.current_selected_ref.clone(),
                            release.map(|r| r.resolved_commit.clone()),
                        )
                    }
                    _ => (Vec::new(), None, None, None, None, None),
                };
                GitSourceCapabilitySource {
                    remote_id: source.remote_id.clone(),
                    canonical_url: source.canonical_url.clone(),
                    kind,
                    members,
                    provider,
                    tracking_mode,
                    tracking_value,
                    selected_ref,
                    resolved_commit,
                }
            })
            .collect();
        Ok(GitSourceCapabilityReport { sources })
    }
}

fn classify(
    structure: &GitSourceCatalogStructure,
    source: &GitSourceFact,
) -> GitSourceCapabilityKind {
    // A missing, unreadable, or partial manifest is incomplete capability
    // evidence, not evidence that the Catalog and manifest disagree. Keep
    // the source in the safe, readable Legacy state until a later explicit
    // Source Transition. A Conflict requires two complete facts that differ.
    if !manifest_is_complete(&source.manifest) {
        return GitSourceCapabilityKind::LegacyPerSkillGitState;
    }
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

fn manifest_is_complete(manifest: &GitSourceManifestFact) -> bool {
    let GitSourceManifestFact::Present {
        remote_id,
        canonical_url,
        aliases,
        provider,
        tracking_mode,
        tracking_value,
        current_selected_ref,
        current_release_id,
    } = manifest
    else {
        return false;
    };
    !remote_id.is_empty()
        && !canonical_url.is_empty()
        && provider.as_deref().is_some_and(|value| !value.is_empty())
        && tracking_mode
            .as_deref()
            .is_some_and(is_supported_tracking_mode)
        && tracking_value_matches_mode(tracking_mode.as_deref(), tracking_value.as_deref())
        && current_selected_ref
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        && current_release_id
            .as_deref()
            .is_some_and(|value| !value.is_empty())
        && aliases.iter().all(|alias| !alias.is_empty())
        && has_unique_values(aliases)
}

/// The closed Source Tracking Policy vocabulary of ADR-0018. The value is
/// only a vocabulary member; the Catalog CHECK constraint and the policy
/// implementation own the mode families, so the scan never guesses which
/// mode a Legacy manifest used.
const SUPPORTED_TRACKING_MODES: &[&str] = &[
    "auto_release_tag_head",
    "prerelease_channel",
    "fixed_tag",
    "fixed_commit",
    "branch",
    "head",
];

fn is_supported_tracking_mode(mode: &str) -> bool {
    SUPPORTED_TRACKING_MODES.contains(&mode)
}

/// Only the parameterised modes carry a `tracking_value`. A mode that needs
/// one without a value is incomplete capability evidence; a `head` or
/// automatic mode never requires one.
fn tracking_value_matches_mode(mode: Option<&str>, value: Option<&str>) -> bool {
    match mode {
        Some("prerelease_channel" | "fixed_tag" | "fixed_commit" | "branch") => {
            value.is_some_and(|value| !value.is_empty())
        }
        _ => true,
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
        || repository
            .tracking_mode
            .as_deref()
            .is_none_or(|mode| !is_supported_tracking_mode(mode))
        || !tracking_value_matches_mode(
            repository.tracking_mode.as_deref(),
            repository.tracking_value.as_deref(),
        )
        || repository
            .current_selected_ref
            .as_deref()
            .is_none_or(str::is_empty)
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
        || release.selected_ref
            != repository
                .current_selected_ref
                .as_deref()
                .unwrap_or_default()
        || release.selection_kind.is_empty()
        || release.resolved_commit.is_empty()
    {
        return false;
    }
    same_non_empty_member_set(&release.member_paths, &current_member_paths(repository))
        && repository
            .current_members
            .iter()
            .filter(|member| member.presence)
            .all(|member| {
                member.storage_relpath
                    == format!("skills/git/{}/{}", source.remote_id, member.skill_id)
            })
}

fn current_member_paths(repository: &GitRepositorySourceFact) -> Vec<String> {
    let mut paths: Vec<String> = repository
        .current_members
        .iter()
        .filter(|member| member.presence)
        .map(|member| member.skill_path.clone())
        .collect();
    paths.sort();
    paths
}

fn manifest_matches(source: &GitSourceFact, repository_is_complete: bool) -> bool {
    let GitSourceManifestFact::Present {
        remote_id,
        canonical_url,
        aliases,
        provider,
        tracking_mode,
        tracking_value,
        current_selected_ref,
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
        && tracking_mode.as_deref() == repository.tracking_mode.as_deref()
        && tracking_value.as_deref() == repository.tracking_value.as_deref()
        && current_selected_ref.as_deref() == repository.current_selected_ref.as_deref()
        && current_release_id.as_deref() == repository.current_release_id.as_deref()
}

fn same_unique_string_set(left: &[String], right: &[String]) -> bool {
    if left.len() != right.len()
        || left.iter().any(|value| value.is_empty())
        || right.iter().any(|value| value.is_empty())
    {
        return false;
    }
    has_unique_values(left) && has_unique_values(right) && {
        let mut left = left.to_vec();
        let mut right = right.to_vec();
        left.sort();
        right.sort();
        left == right
    }
}

fn has_unique_values(values: &[String]) -> bool {
    let mut deduplicated = values.to_vec();
    deduplicated.sort();
    deduplicated.dedup();
    deduplicated.len() == values.len()
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
