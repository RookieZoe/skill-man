//! Read-only Fetch Latest and Manage source-group discovery (ADR-0018).
//!
//! This is deliberately not the legacy per-Skill Git import flow. A Git
//! repository is one source: discovery always follows the Source Tracking
//! Policy (or an explicit override), resolves one selected ref to one
//! immutable Source Release, includes every supported member and produces
//! no plan token or persistent draft.

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
use crate::core::source_tracking_policy::{
    PolicySelection, TrackingPolicyError, evaluate, is_supported_tracking_mode,
};
use crate::seams::installer_lock_store::{InstallerLockError, InstallerLockStore};
use crate::seams::remote_provider::{ProviderReleaseFact, RemoteProvider};
use crate::seams::source::{GitSource, GitTreeEntryKind, SourceError};

const MAX_SKILL_DOCUMENT_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceTrackingOverride {
    /// One of the closed modes (`auto_release_tag_head`, `prerelease_channel`,
    /// `fixed_tag`, `fixed_commit`, `branch`, `head`).
    pub mode: String,
    /// Required for the parameterised modes (`prerelease_channel`,
    /// `fixed_tag`, `fixed_commit`, `branch`).
    pub value: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FetchLatestAndManageRequest {
    /// `github`, `gitlab`, or generic `git` over HTTPS.
    pub source_type: String,
    pub source_url: String,
    /// The user-confirmed Source Tracking Policy or explicit override.
    /// `None` applies the default `auto_release_tag_head` policy and may
    /// surface a legacy Repository Ref Conflict before transport begins.
    pub tracking_policy: Option<SourceTrackingOverride>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceGroupPreviewOutcome {
    Preview(SourceGroupPreview),
    RepositoryRefConflict(RepositoryRefConflict),
    RepositoryOwnershipSplit(RepositoryOwnershipSplit),
}

/// The frozen policy facts of one preview: the effective mode, the explicit
/// override value, the deterministic selection and the resolved commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceGroupPolicyFacts {
    pub mode: String,
    pub value: Option<String>,
    pub selection_kind: String,
    pub selected_ref: String,
    pub resolved_commit: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceGroupPreview {
    pub provider: String,
    pub source_url: String,
    /// Confirmed repository URL aliases. A fresh Source has none; a
    /// repository rename confirmation adds one without moving storage.
    pub aliases: Vec<String>,
    pub policy: SourceGroupPolicyFacts,
    pub members: Vec<SourceGroupMember>,
    /// Legacy locks can only make an external ownership claim. They never
    /// establish remote provenance or contribute a tree summary.
    pub external_ownership_claims: Vec<ExternalOwnershipClaim>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceGroupMemberAction {
    /// Enters the Library with this release.
    Added,
    /// Already present in the current Source Release (immutable continue).
    Current,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceGroupMember {
    pub directory_name: String,
    pub directory_identity_key: String,
    pub display_name: String,
    pub description: String,
    pub skill_path: String,
    pub tree_summary: String,
    pub action: SourceGroupMemberAction,
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
    /// The distinct refs the legacy lock claims declared. One must be
    /// confirmed as an explicit policy/override before re-discovery.
    pub available_refs: Vec<String>,
    pub external_ownership_claims: Vec<ExternalOwnershipClaim>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryOwnershipSplit {
    pub provider: String,
    pub source_url: String,
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
    #[error("the remote provider reporting failed: {0}")]
    RemoteProvider(String),
    #[error("the Source Tracking Policy could not be evaluated: {0}")]
    TrackingPolicy(#[from] TrackingPolicyError),
}

/// Core service for the read-only Source Group Draft. Git transport receives
/// an ephemeral temporary mirror, which is cleaned on return; this service
/// never receives a Home, Catalog, staging, journal, or lock-writing seam.
pub struct SourceGroupPreviewService {
    git_source: Arc<dyn GitSource>,
    lock_store: Arc<dyn InstallerLockStore>,
    remote_provider: Option<Arc<dyn RemoteProvider>>,
}

impl SourceGroupPreviewService {
    pub fn new(git_source: Arc<dyn GitSource>, lock_store: Arc<dyn InstallerLockStore>) -> Self {
        Self {
            git_source,
            lock_store,
            remote_provider: None,
        }
    }

    /// Wire the provider Release fact seam (GitHub/GitLab). Without it the
    /// default policy reads release facts as absent and falls back to tags.
    pub fn with_remote_provider(mut self, provider: Arc<dyn RemoteProvider>) -> Self {
        self.remote_provider = Some(provider);
        self
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

        let override_mode = normalize_override(request.tracking_policy.clone())?;
        let claims = self.external_ownership_claims(&spec.url)?;
        if request.tracking_policy.is_none() {
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
        let releases = self.provider_releases(&spec.url)?;
        let tags = self.git_source.list_tags(&mirror)?;
        let reachable = self.reachable_tags(&mirror, &report.default_branch, &tags)?;
        let selection = self.select_ref(override_mode, &releases, &tags, &reachable)?;
        spec.requested_ref = Some(selection.selection.selected_ref.clone());
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
                directory_identity_key: crate::core::domain::skill_identity_key(
                    &skill.directory_name,
                ),
                display_name,
                description: metadata.description.unwrap_or_default(),
                tree_summary: self.git_source.tree_summary(
                    &mirror,
                    &resolved.commit,
                    &skill.skill_path,
                )?,
                skill_path: skill.skill_path,
                action: SourceGroupMemberAction::Added,
                directory_name: skill.directory_name,
            });
        }
        members.sort_by(|left, right| left.skill_path.cmp(&right.skill_path));

        Ok(SourceGroupPreviewOutcome::Preview(SourceGroupPreview {
            provider,
            source_url: spec.url,
            aliases: Vec::new(),
            policy: SourceGroupPolicyFacts {
                mode: selection.mode.clone(),
                value: selection.value.clone(),
                selection_kind: selection.selection.selection_kind.into(),
                selected_ref: resolved.recorded_ref,
                resolved_commit: resolved.commit,
            },
            members,
            external_ownership_claims: claims,
        }))
    }

    /// Evaluate the effective policy. The default policy applies when the
    /// request carries no override; an explicit `auto_release_tag_head` is
    /// the user-confirmed same selection.
    fn select_ref(
        &self,
        override_mode: Option<SourceTrackingOverride>,
        releases: &[ProviderReleaseFact],
        tags: &[crate::seams::source::GitTagFact],
        reachable_tags: &[crate::seams::source::GitTagFact],
    ) -> Result<EffectiveSelection, SourceGroupPreviewError> {
        let (mode, value) = match &override_mode {
            Some(override_value) => (override_value.mode.clone(), override_value.value.clone()),
            None => ("auto_release_tag_head".into(), None),
        };
        let selection = evaluate(&mode, value.as_deref(), releases, tags, reachable_tags)?;
        Ok(EffectiveSelection {
            mode,
            value,
            selection,
        })
    }

    fn provider_releases(
        &self,
        source_url: &str,
    ) -> Result<Vec<ProviderReleaseFact>, SourceGroupPreviewError> {
        match &self.remote_provider {
            Some(provider) => provider
                .list_releases(source_url)
                .map_err(|error| SourceGroupPreviewError::RemoteProvider(error.to_string())),
            None => Ok(Vec::new()),
        }
    }

    /// The subset of tags whose commit is an ancestor of the remote default
    /// branch (the "default branch 可达" ordinary-tag step).
    fn reachable_tags(
        &self,
        mirror: &std::path::Path,
        default_branch: &Option<String>,
        tags: &[crate::seams::source::GitTagFact],
    ) -> Result<Vec<crate::seams::source::GitTagFact>, SourceGroupPreviewError> {
        if tags.is_empty() {
            return Ok(Vec::new());
        }
        let default = default_branch.as_deref().unwrap_or("HEAD");
        let Some(default_commit) = self.git_source.resolve_commit(mirror, default)? else {
            return Ok(Vec::new());
        };
        let mut reachable = Vec::new();
        for tag in tags {
            if self
                .git_source
                .is_ancestor(mirror, &tag.commit, &default_commit)?
            {
                reachable.push(tag.clone());
            }
        }
        Ok(reachable)
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

/// The effective policy of one preview request: mode/value for the manifest
/// plus the deterministic selection.
#[derive(Clone, Debug, Eq, PartialEq)]
struct EffectiveSelection {
    mode: String,
    value: Option<String>,
    selection: PolicySelection,
}

fn normalize_override(
    override_value: Option<SourceTrackingOverride>,
) -> Result<Option<SourceTrackingOverride>, SourceGroupPreviewError> {
    let Some(override_value) = override_value else {
        return Ok(None);
    };
    let mode = override_value.mode.trim().to_owned();
    if !is_supported_tracking_mode(&mode) {
        return Err(SourceGroupPreviewError::Validation(format!(
            "unsupported Source Tracking Policy mode '{mode}'"
        )));
    }
    let value = override_value
        .value
        .as_ref()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    Ok(Some(SourceTrackingOverride { mode, value }))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_normalization_rejects_unknown_modes() {
        assert!(matches!(
            normalize_override(Some(SourceTrackingOverride {
                mode: "fancy".into(),
                value: None,
            })),
            Err(SourceGroupPreviewError::Validation(_))
        ));
        assert_eq!(
            normalize_override(Some(SourceTrackingOverride {
                mode: " branch ".into(),
                value: Some(" main ".into()),
            }))
            .expect("normalized"),
            Some(SourceTrackingOverride {
                mode: "branch".into(),
                value: Some("main".into()),
            })
        );
        assert_eq!(normalize_override(None).expect("no override"), None);
    }
}
