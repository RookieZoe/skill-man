//! Read-only Fetch Latest and Manage source-group discovery.
//!
//! This is deliberately not the legacy per-Skill Git import flow. A Git
//! repository is one source: discovery always includes every supported member
//! and this module produces no plan token or persistent draft.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use thiserror::Error;

use crate::core::domain::parse_skill_metadata;
use crate::core::git_source::{
    DiscoveryMode, GitResolveError, discover_skills_from_paths, git_mirror_path,
    parse_git_source_input, repo_name_from_url, resolve_git_ref, skill_document_path,
    validate_skill_path,
};
use crate::seams::installer_lock_store::{InstallerLockError, InstallerLockStore};
use crate::seams::source::{GitSource, GitTreeEntryKind, SourceError};

const MAX_SKILL_DOCUMENT_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FetchLatestAndManageRequest {
    /// `github`, `gitlab`, or generic `git` over HTTPS.
    pub source_type: String,
    pub source_url: String,
    /// The explicitly selected branch, tag, or commit. `None` allows this
    /// service to surface a legacy Ref Conflict before transport begins.
    pub tracking_ref: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceGroupPreviewOutcome {
    Preview(SourceGroupPreview),
    RepositoryRefConflict(RepositoryRefConflict),
    RepositoryOwnershipSplit(RepositoryOwnershipSplit),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceGroupPreview {
    pub provider: String,
    pub source_url: String,
    pub tracking_ref: String,
    pub resolved_commit: String,
    pub members: Vec<SourceGroupMember>,
    /// Legacy locks can only make an external ownership claim. They never
    /// establish remote provenance or contribute a tree summary.
    pub external_ownership_claims: Vec<ExternalOwnershipClaim>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceGroupMember {
    pub directory_name: String,
    pub display_name: String,
    pub description: String,
    pub skill_path: String,
    pub tree_summary: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ExternalOwnershipClaim {
    pub lock_path: PathBuf,
    pub entry_name: String,
    pub requested_ref: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryRefConflict {
    pub provider: String,
    pub source_url: String,
    pub available_refs: Vec<String>,
    pub external_ownership_claims: Vec<ExternalOwnershipClaim>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryOwnershipSplit {
    pub provider: String,
    pub source_url: String,
    pub tracking_ref: String,
    pub lock_paths: Vec<PathBuf>,
    pub external_ownership_claims: Vec<ExternalOwnershipClaim>,
}

#[derive(Debug, Error)]
pub enum SourceGroupPreviewError {
    #[error("{0}")]
    Validation(String),
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    Locks(#[from] InstallerLockError),
    #[error(transparent)]
    Resolve(#[from] GitResolveError),
}

/// Core service for the read-only Source Group Draft. Git transport receives
/// an ephemeral temporary mirror, which is cleaned on return; this service
/// never receives a Home, Catalog, staging, journal, or lock-writing seam.
pub struct SourceGroupPreviewService {
    git_source: Arc<dyn GitSource>,
    lock_store: Arc<dyn InstallerLockStore>,
}

impl SourceGroupPreviewService {
    pub fn new(git_source: Arc<dyn GitSource>, lock_store: Arc<dyn InstallerLockStore>) -> Self {
        Self {
            git_source,
            lock_store,
        }
    }

    pub fn fetch_latest_and_manage(
        &self,
        request: FetchLatestAndManageRequest,
    ) -> Result<SourceGroupPreviewOutcome, SourceGroupPreviewError> {
        let provider = validate_provider(&request.source_type)?;
        let mut spec = parse_git_source_input(&request.source_url)
            .map_err(|error| SourceGroupPreviewError::Validation(error.to_string()))?;
        if !spec.url.starts_with("https://") {
            return Err(SourceGroupPreviewError::Validation(
                "Git Repository Sources must use HTTPS".into(),
            ));
        }
        validate_provider_url(&provider, &spec.url)?;
        if spec.path_prefix.is_some() {
            return Err(SourceGroupPreviewError::Validation(
                "Fetch Latest and Manage always reviews the complete repository source".into(),
            ));
        }

        let requested_tracking_ref = normalize_optional_ref(request.tracking_ref)?;
        if let (Some(url_ref), Some(selected_ref)) = (&spec.requested_ref, &requested_tracking_ref)
            && url_ref != selected_ref
        {
            return Err(SourceGroupPreviewError::Validation(format!(
                "the selected ref '{selected_ref}' conflicts with the source URL ref '{url_ref}'"
            )));
        }
        if requested_tracking_ref.is_some() {
            spec.requested_ref = requested_tracking_ref.clone();
        }

        let claims = self.external_ownership_claims(&spec.url)?;
        if requested_tracking_ref.is_none() && spec.requested_ref.is_none() {
            let available_refs: Vec<_> = claims
                .iter()
                .map(|claim| claim.requested_ref.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            if available_refs.len() > 1 {
                return Ok(SourceGroupPreviewOutcome::RepositoryRefConflict(
                    RepositoryRefConflict {
                        provider,
                        source_url: spec.url,
                        available_refs,
                        external_ownership_claims: claims,
                    },
                ));
            }
        }

        let selected_ref = spec
            .requested_ref
            .clone()
            .or_else(|| claims.first().map(|claim| claim.requested_ref.clone()));
        if spec.requested_ref.is_none() {
            spec.requested_ref = selected_ref;
        }
        let recorded_ref = spec.requested_ref.clone().unwrap_or_else(|| "HEAD".into());
        let lock_paths: Vec<_> = claims
            .iter()
            .map(|claim| claim.lock_path.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if lock_paths.len() > 1 {
            return Ok(SourceGroupPreviewOutcome::RepositoryOwnershipSplit(
                RepositoryOwnershipSplit {
                    provider,
                    source_url: spec.url,
                    tracking_ref: recorded_ref,
                    lock_paths,
                    external_ownership_claims: claims,
                },
            ));
        }

        let temporary_mirror_root = tempfile::Builder::new()
            .prefix("skill-man-source-preview-")
            .tempdir()
            .map_err(|source| {
                SourceGroupPreviewError::Source(SourceError::Io {
                    operation: "create temporary Git preview mirror",
                    path: std::env::temp_dir(),
                    source,
                })
            })?;
        let mirror = git_mirror_path(temporary_mirror_root.path(), &spec.url);
        let report = self.git_source.fetch_mirror(&spec.url, &mirror)?;
        let resolved = resolve_git_ref(self.git_source.as_ref(), &mirror, &spec, &report)?;
        let tree_entries = self.git_source.list_tree(&mirror, &resolved.commit)?;
        if tree_entries.iter().any(|entry| {
            entry.kind == GitTreeEntryKind::Symlink
                && entry
                    .path
                    .file_name()
                    .is_some_and(|name| name == "SKILL.md")
        }) {
            return Err(SourceGroupPreviewError::Validation(
                "the source repository contains a symlinked SKILL.md".into(),
            ));
        }
        let tree_files = tree_entries
            .into_iter()
            .filter_map(|entry| (entry.kind == GitTreeEntryKind::Blob).then_some(entry.path))
            .collect::<Vec<_>>();
        let discovered = discover_skills_from_paths(
            &repo_name_from_url(&spec.url),
            &tree_files,
            None,
            DiscoveryMode::ForceFullDepth,
        );
        if discovered.is_empty() {
            return Err(SourceGroupPreviewError::Validation(
                "the source repository contains no discoverable Skills".into(),
            ));
        }

        let mut members = Vec::with_capacity(discovered.len());
        for skill in discovered {
            validate_skill_path(&skill.skill_path)
                .map_err(|error| SourceGroupPreviewError::Validation(error.to_string()))?;
            let document_path = skill_document_path(&skill.skill_path);
            let bytes = self
                .git_source
                .read_blob(
                    &mirror,
                    &resolved.commit,
                    &document_path,
                    MAX_SKILL_DOCUMENT_BYTES,
                )?
                .ok_or_else(|| {
                    SourceGroupPreviewError::Validation(format!(
                        "discovered source member '{}' has no readable SKILL.md",
                        skill.skill_path
                    ))
                })?;
            let document = String::from_utf8(bytes).map_err(|_| {
                SourceGroupPreviewError::Validation(format!(
                    "discovered source member '{}' has a non-UTF-8 SKILL.md",
                    skill.skill_path
                ))
            })?;
            let metadata = parse_skill_metadata(&document);
            let display_name = metadata
                .name
                .unwrap_or_else(|| skill.directory_name.clone());
            members.push(SourceGroupMember {
                directory_name: skill.directory_name,
                display_name,
                description: metadata.description.unwrap_or_default(),
                tree_summary: self.git_source.tree_summary(
                    &mirror,
                    &resolved.commit,
                    &skill.skill_path,
                )?,
                skill_path: skill.skill_path,
            });
        }
        members.sort_by(|left, right| left.skill_path.cmp(&right.skill_path));

        Ok(SourceGroupPreviewOutcome::Preview(SourceGroupPreview {
            provider,
            source_url: spec.url,
            tracking_ref: resolved.recorded_ref,
            resolved_commit: resolved.commit,
            members,
            external_ownership_claims: claims,
        }))
    }

    fn external_ownership_claims(
        &self,
        source_url: &str,
    ) -> Result<Vec<ExternalOwnershipClaim>, SourceGroupPreviewError> {
        let mut claims = Vec::new();
        for report in self.lock_store.discover()? {
            if report.fault.is_some() {
                continue;
            }
            for entry in report.entries {
                let Ok(entry_spec) = parse_git_source_input(&entry.source_url) else {
                    continue;
                };
                if entry_spec.url != source_url {
                    continue;
                }
                claims.push(ExternalOwnershipClaim {
                    lock_path: report.path.clone(),
                    entry_name: entry.name,
                    requested_ref: entry.requested_ref.unwrap_or_else(|| "HEAD".into()),
                });
            }
        }
        claims.sort();
        claims.dedup();
        Ok(claims)
    }
}

fn validate_provider(input: &str) -> Result<String, SourceGroupPreviewError> {
    match input.trim().to_ascii_lowercase().as_str() {
        "github" => Ok("github".into()),
        "gitlab" => Ok("gitlab".into()),
        "git" | "https" => Ok("git".into()),
        value => Err(SourceGroupPreviewError::Validation(format!(
            "unsupported Git repository source type '{value}'"
        ))),
    }
}

fn validate_provider_url(provider: &str, source_url: &str) -> Result<(), SourceGroupPreviewError> {
    let expected = match provider {
        "github" => Some("https://github.com/"),
        "gitlab" => Some("https://gitlab.com/"),
        _ => None,
    };
    if expected.is_some_and(|prefix| !source_url.starts_with(prefix)) {
        return Err(SourceGroupPreviewError::Validation(format!(
            "source type '{provider}' does not match '{source_url}'"
        )));
    }
    Ok(())
}

fn normalize_optional_ref(
    value: Option<String>,
) -> Result<Option<String>, SourceGroupPreviewError> {
    let value = value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if let Some(value) = &value
        && value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(SourceGroupPreviewError::Validation(
            "the selected Git ref contains whitespace or control characters".into(),
        ));
    }
    Ok(value)
}
