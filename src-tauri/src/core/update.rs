//! Skill Update module (ADR-0004 §更新).
//!
//! Checks trackable remote Installs (default branch and branch refs; tags and
//! commits are pinned and never checked), merges fetches per repository,
//! groups results for the UI, and applies Updates through the Import
//! module's stable replacement machinery. Modified entities are never
//! silently replaced: Apply requires the user to abandon local changes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use crate::core::domain::{Health, SkillId};
use crate::core::git_source::{
    DEFAULT_BRANCH_REF, git_mirror_path, is_trackable_ref, skill_document_path,
};
use crate::core::git_source_capability::{GitSourceCapabilityError, GitSourceCapabilityScan};
use crate::core::import::{ImportError, ImportService};
use crate::core::write_gate::WriteGate;
use crate::seams::clock::Clock;
use crate::seams::filesystem::FileSystem;
use crate::seams::filesystem::RemoteParentManifest;
use crate::seams::import_store::{ImportStore, ImportStoreError, RemoteInstallRecord};
use crate::seams::source::{GitSource, GitTreeEntry, SourceError};

const UPDATE_COOLDOWN_SECONDS: i64 = 24 * 60 * 60;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateCheckItem {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub source_url: String,
    pub requested_ref: String,
    pub current_commit: String,
    pub resolved_commit: String,
    pub has_update: bool,
    pub modified: bool,
    pub upstream_path_gone: bool,
    pub last_checked_at: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateCheckGroup {
    pub repo_url: String,
    pub items: Vec<UpdateCheckItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateCheckReport {
    pub groups: Vec<UpdateCheckGroup>,
    /// Repo-scoped failures (offline, resolution, listing); the UI presents
    /// them as inline notices without blocking successful groups.
    pub errors: Vec<String>,
    /// Closed Remote Source Identity Conflicts (ADR-0013 §4.3): one entry
    /// per parent whose manifest disagrees with its Catalog row. Update,
    /// new Bindings and alias changes are closed for that parent only.
    pub parent_conflicts: Vec<RemoteSourceParentConflict>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteSourceParentConflict {
    pub remote_id: String,
    pub canonical_url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateSelection {
    pub skill_id: SkillId,
    /// Reselect the repo-relative Skill path (upstream path-gone flow).
    pub new_skill_path: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdatePlanItem {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub plan_token: String,
    pub current_commit: String,
    pub new_commit: String,
    pub modified: bool,
    pub path_changed: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdatePlan {
    pub items: Vec<UpdatePlanItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateApplyRequest {
    pub plan_token: String,
    pub skill_id: SkillId,
    pub directory_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateItemResult {
    pub skill_id: SkillId,
    pub directory_name: String,
    pub updated: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateResult {
    pub items: Vec<UpdateItemResult>,
}

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("{0}")]
    Validation(String),
    #[error(transparent)]
    Store(#[from] ImportStoreError),
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error(transparent)]
    FileSystem(#[from] crate::seams::filesystem::FileSystemError),
    #[error(transparent)]
    Import(#[from] ImportError),
    #[error(transparent)]
    SourceCapability(#[from] GitSourceCapabilityError),
    #[error(
        "Per-Skill Update is closed; Git Repository Source Update is not available in this release"
    )]
    SourceCapabilityClosed,
    #[error("internal Update error: {0}")]
    Internal(String),
}

pub struct UpdateService {
    store: Arc<dyn ImportStore>,
    filesystem: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
    git: Arc<dyn GitSource>,
    configured_library_root: PathBuf,
    configured_git_cache_root: PathBuf,
    configured_remotes_root: PathBuf,
    home_context: Option<Arc<WriteGate>>,
    source_capability_scan: Option<Arc<GitSourceCapabilityScan>>,
    import: ImportService,
}

impl UpdateService {
    pub fn new(
        store: Arc<dyn ImportStore>,
        filesystem: Arc<dyn FileSystem>,
        clock: Arc<dyn Clock>,
        git: Arc<dyn GitSource>,
        library_root: PathBuf,
        git_cache_root: PathBuf,
        write_gate: Arc<WriteGate>,
    ) -> Self {
        let import = ImportService::new(
            store.clone(),
            filesystem.clone(),
            clock.clone(),
            Arc::new(crate::adapters::local_file_source::LocalFileSource::new()),
            library_root.clone(),
        )
        .with_write_gate(write_gate)
        .with_git_source(git.clone())
        .with_git_cache_root(git_cache_root.clone());
        Self {
            store,
            filesystem,
            clock,
            git,
            configured_library_root: library_root.clone(),
            configured_git_cache_root: git_cache_root,
            configured_remotes_root: library_root.join("remotes"),
            home_context: None,
            source_capability_scan: None,
            import,
        }
    }

    /// Make remote mirrors, manifests and nested Import operations follow
    /// the current bootstrap-verified Home after a first-time bind.
    pub fn with_home_context(mut self, home_context: Arc<WriteGate>) -> Self {
        self.import = self.import.with_home_context(home_context.clone());
        self.home_context = Some(home_context);
        self
    }

    /// Production composition supplies the same read-only Source Capability
    /// Production composition supplies this scan to close the historical
    /// per-Skill Update API. Git Repository Source Update is a later,
    /// source-level transition and cannot reuse these per-Skill commands.
    pub fn with_git_source_capability_scan(
        mut self,
        source_capability_scan: Arc<GitSourceCapabilityScan>,
    ) -> Self {
        self.source_capability_scan = Some(source_capability_scan);
        self
    }

    fn active_library_root(&self) -> Result<PathBuf, UpdateError> {
        match &self.home_context {
            Some(context) => context.bound_home().map(|home| home.path).map_err(|error| {
                UpdateError::Internal(format!("active Home unavailable: {error}"))
            }),
            None => Ok(self.configured_library_root.clone()),
        }
    }

    fn active_git_cache_root(&self) -> Result<PathBuf, UpdateError> {
        if self.home_context.is_some() {
            return Ok(self.active_library_root()?.join("cache"));
        }
        Ok(self.configured_git_cache_root.clone())
    }

    fn active_remotes_root(&self) -> Result<PathBuf, UpdateError> {
        if self.home_context.is_some() {
            return Ok(self.active_library_root()?.join("remotes"));
        }
        Ok(self.configured_remotes_root.clone())
    }

    /// Check trackable remote Installs for updates. Each repository is
    /// fetched once; results are grouped per repository. Successful checks
    /// record `last_checked_at` (the 24h cooldown), failures degrade into
    /// report errors without disturbing other repositories.
    pub fn check_updates(&self, force: bool) -> Result<UpdateCheckReport, UpdateError> {
        let records = self.store.load_remote_installs()?;
        let updateable_remote_ids = self.updateable_remote_ids()?;
        let trackable = records
            .iter()
            .filter(|record| {
                is_trackable_ref(&record.requested_ref)
                    && self.source_updates_are_allowed(
                        updateable_remote_ids.as_ref(),
                        &record.remote_id,
                    )
            })
            .collect::<Vec<_>>();
        let mut by_repo: HashMap<&str, Vec<&RemoteInstallRecord>> = HashMap::new();
        for record in &trackable {
            by_repo
                .entry(record.source_url.as_str())
                .or_default()
                .push(record);
        }
        let now_seconds =
            i64::try_from(self.clock.unix_epoch_nanos() / 1_000_000_000).unwrap_or(i64::MAX);
        let mut groups = Vec::new();
        let mut errors = Vec::new();
        let mut parent_conflicts = Vec::new();
        for (repo_url, items) in by_repo {
            // Parent integrity gate (ADR-0013 §4.3): a manifest/row
            // mismatch closes only this parent's Update; other parents and
            // sources continue. A missing manifest is the migrated-legacy
            // state and is backfilled on the first successful write.
            let parent_ok = items.iter().all(|item| self.parent_integrity_ok(item));
            if !parent_ok {
                parent_conflicts.push(RemoteSourceParentConflict {
                    remote_id: items[0].remote_id.clone(),
                    canonical_url: repo_url.to_owned(),
                });
                continue;
            }
            if !force
                && items.iter().all(|item| {
                    item.last_checked_at.is_some_and(|checked| {
                        now_seconds.saturating_sub(checked) < UPDATE_COOLDOWN_SECONDS
                    })
                })
            {
                // Within the cooldown: silent skip, as ADR-0004 requires.
                continue;
            }
            let mirror = self.git_mirror_for(repo_url)?;
            let report = match self.git.fetch_mirror(repo_url, &mirror) {
                Ok(report) => report,
                Err(error) => {
                    errors.push(format!("{repo_url}: {error}"));
                    continue;
                }
            };
            let default_branch = report.default_branch.as_deref();
            let default_resolved = match default_branch {
                Some(branch) => self
                    .git
                    .resolve_commit(&mirror, &format!("refs/heads/{branch}"))?,
                None => self.git.resolve_commit(&mirror, "HEAD")?,
            };
            let mut listed: Option<(String, Vec<GitTreeEntry>)> = None;
            let mut group_items = Vec::with_capacity(items.len());
            for item in items {
                let resolved = match self.resolve_tracked_ref(&mirror, &default_resolved, item)? {
                    Some(commit) => commit,
                    None => {
                        errors.push(format!(
                            "{repo_url}: the tracked branch '{}' could not be resolved",
                            item.requested_ref
                        ));
                        continue;
                    }
                };
                let has_update = resolved != item.verification_anchor_commit;
                let mut upstream_path_gone = false;
                if has_update {
                    let entries = match &listed {
                        Some((commit, entries)) if *commit == resolved => entries,
                        _ => {
                            let entries = self.git.list_tree(&mirror, &resolved)?;
                            listed = Some((resolved.clone(), entries));
                            listed
                                .as_ref()
                                .map(|(_, entries)| entries)
                                .expect("listed entries were stored")
                        }
                    };
                    upstream_path_gone = !entries.iter().any(|entry| {
                        entry.path.to_string_lossy() == skill_document_path(&item.skill_path)
                    });
                }
                if let Err(error) = self.store.record_remote_check(&item.skill_id) {
                    errors.push(format!("{}: {error}", item.directory_name));
                }
                group_items.push(UpdateCheckItem {
                    skill_id: item.skill_id.clone(),
                    directory_name: item.directory_name.clone(),
                    source_url: item.source_url.clone(),
                    requested_ref: item.requested_ref.clone(),
                    current_commit: item.verification_anchor_commit.clone(),
                    resolved_commit: resolved,
                    has_update,
                    modified: item.health == Health::Modified,
                    upstream_path_gone,
                    last_checked_at: item.last_checked_at,
                });
            }
            groups.push(UpdateCheckGroup {
                repo_url: repo_url.to_owned(),
                items: group_items,
            });
        }
        Ok(UpdateCheckReport {
            groups,
            errors,
            parent_conflicts,
        })
    }

    /// Plan Updates for the selected remote Installs. Fetches each involved
    /// repository once, resolves the tracked ref, verifies the Skill path
    /// still exists at the new commit, and delegates each replacement to the
    /// Import module (staging, journal, stable path replacement). Per-item
    /// errors (already up to date, upstream path gone, offline) are reported
    /// on the item so the UI can present them next to the Skill.
    pub fn plan_updates(&self, selections: &[UpdateSelection]) -> Result<UpdatePlan, UpdateError> {
        let records = self.store.load_remote_installs()?;
        let updateable_remote_ids = self.updateable_remote_ids()?;
        let mut items = Vec::new();
        let mut by_repo: HashMap<String, Vec<(&UpdateSelection, &RemoteInstallRecord)>> =
            HashMap::new();
        for selection in selections {
            let Some(record) = records
                .iter()
                .find(|record| record.skill_id == selection.skill_id)
            else {
                items.push(UpdatePlanItem {
                    skill_id: selection.skill_id.clone(),
                    directory_name: String::new(),
                    plan_token: String::new(),
                    current_commit: String::new(),
                    new_commit: String::new(),
                    modified: false,
                    path_changed: false,
                    error: Some("the Skill is not a remote Install".into()),
                });
                continue;
            };
            if !self.source_updates_are_allowed(updateable_remote_ids.as_ref(), &record.remote_id) {
                items.push(update_item_error(
                    selection,
                    record,
                    UpdateError::SourceCapabilityClosed.to_string(),
                ));
                continue;
            }
            by_repo
                .entry(record.source_url.clone())
                .or_default()
                .push((selection, record));
        }
        for (repo_url, selections) in by_repo {
            if selections
                .iter()
                .any(|(_, record)| !self.parent_integrity_ok(record))
            {
                for (selection, record) in &selections {
                    items.push(update_item_error(
                        selection,
                        record,
                        "Remote Source Identity Conflict — the parent manifest does not match \
                         the Catalog row; Update is closed for this parent"
                            .into(),
                    ));
                }
                continue;
            }
            let mirror = self.git_mirror_for(&repo_url)?;
            let fetch = self.git.fetch_mirror(&repo_url, &mirror);
            let default_resolved = match fetch {
                Ok(report) => match report.default_branch.as_deref() {
                    Some(branch) => self
                        .git
                        .resolve_commit(&mirror, &format!("refs/heads/{branch}"))?,
                    None => self.git.resolve_commit(&mirror, "HEAD")?,
                },
                Err(error) => {
                    for (selection, record) in selections {
                        items.push(update_item_error(selection, record, error.to_string()));
                    }
                    continue;
                }
            };
            let mut listed: Option<(String, Vec<GitTreeEntry>)> = None;
            for (selection, record) in selections {
                let resolved = match self.resolve_tracked_ref(&mirror, &default_resolved, record)? {
                    Some(commit) => commit,
                    None => {
                        items.push(update_item_error(
                            selection,
                            record,
                            format!(
                                "the tracked branch '{}' could not be resolved",
                                record.requested_ref
                            ),
                        ));
                        continue;
                    }
                };
                let entries = match &listed {
                    Some((commit, entries)) if *commit == resolved => entries,
                    _ => {
                        let entries = self.git.list_tree(&mirror, &resolved)?;
                        listed = Some((resolved.clone(), entries));
                        listed
                            .as_ref()
                            .map(|(_, entries)| entries)
                            .expect("listed entries were stored")
                    }
                };
                let path_changed = selection.new_skill_path.is_some();
                let skill_path = selection
                    .new_skill_path
                    .clone()
                    .unwrap_or_else(|| record.skill_path.clone());
                let document_path = skill_document_path(&skill_path);
                if !entries
                    .iter()
                    .any(|entry| entry.path.to_string_lossy() == document_path)
                {
                    items.push(update_item_error(
                        selection,
                        record,
                        format!(
                            "the upstream Skill path '{skill_path}' no longer exists at commit {resolved}"
                        ),
                    ));
                    continue;
                }
                if !path_changed && resolved == record.verification_anchor_commit {
                    items.push(update_item_error(
                        selection,
                        record,
                        "the Skill is already up to date".into(),
                    ));
                    continue;
                }
                match self.import.plan_git_reinstall(
                    &record.identity_key,
                    &record.source_url,
                    &resolved,
                    selection.new_skill_path.as_deref(),
                    false,
                ) {
                    Ok(preview) => items.push(UpdatePlanItem {
                        skill_id: selection.skill_id.clone(),
                        directory_name: record.directory_name.clone(),
                        plan_token: preview.plan_token,
                        current_commit: record.verification_anchor_commit.clone(),
                        new_commit: resolved.clone(),
                        modified: self.entity_is_modified(record),
                        path_changed,
                        error: None,
                    }),
                    Err(error) => {
                        items.push(update_item_error(selection, record, error.to_string()))
                    }
                }
            }
        }
        Ok(UpdatePlan { items })
    }

    /// Apply planned Updates. Each Skill is its own operation (its own
    /// journal): a failure rolls back only that Skill and leaves the others
    /// and their Activations untouched. `abandon_changes` is the explicit
    /// Modified confirmation.
    pub fn apply_updates(
        &self,
        requests: &[UpdateApplyRequest],
        abandon_changes: bool,
    ) -> Result<UpdateResult, UpdateError> {
        let records = self.store.load_remote_installs()?;
        let updateable_remote_ids = self.updateable_remote_ids()?;
        let remotes_root = self.active_remotes_root()?;
        let mut results = Vec::with_capacity(requests.len());
        for request in requests {
            let source_updates_allowed = records
                .iter()
                .find(|record| record.skill_id == request.skill_id)
                .is_some_and(|record| {
                    self.source_updates_are_allowed(
                        updateable_remote_ids.as_ref(),
                        &record.remote_id,
                    )
                });
            if !source_updates_allowed {
                results.push(UpdateItemResult {
                    skill_id: request.skill_id.clone(),
                    directory_name: request.directory_name.clone(),
                    updated: false,
                    error: Some(UpdateError::SourceCapabilityClosed.to_string()),
                });
                continue;
            }
            match self
                .import
                .apply_git_reinstall(&request.plan_token, abandon_changes)
            {
                Ok(_) => {
                    // The migrated-legacy parents have no manifest yet;
                    // backfill it from the authoritative Catalog row on the
                    // first successful write (ADR-0013 §4.3: a missing
                    // manifest is derived, never chosen over a live one).
                    if let Ok(records) = self.store.load_remote_installs() {
                        if let Some(record) = records
                            .iter()
                            .find(|record| record.skill_id == request.skill_id)
                        {
                            let aliases = self
                                .store
                                .load_remote_parents()
                                .ok()
                                .and_then(|parents| {
                                    parents
                                        .into_iter()
                                        .find(|parent| parent.remote_id == record.remote_id)
                                        .map(|parent| parent.aliases)
                                })
                                .unwrap_or_default();
                            let _ = ensure_parent_manifest(
                                self.filesystem.as_ref(),
                                &remotes_root,
                                record,
                                self.clock.unix_epoch_nanos(),
                                aliases,
                            );
                        }
                    }
                    results.push(UpdateItemResult {
                        skill_id: request.skill_id.clone(),
                        directory_name: request.directory_name.clone(),
                        updated: true,
                        error: None,
                    });
                }
                Err(error) => results.push(UpdateItemResult {
                    skill_id: request.skill_id.clone(),
                    directory_name: request.directory_name.clone(),
                    updated: false,
                    error: Some(error.to_string()),
                }),
            }
        }
        Ok(UpdateResult { items: results })
    }

    /// Pin one or more remote Installs to their current commit: the recorded
    /// ref becomes the resolved commit, so no further update checks apply.
    pub fn pin_updates(&self, skill_ids: &[SkillId]) -> Result<(), UpdateError> {
        let records = self.store.load_remote_installs()?;
        let updateable_remote_ids = self.updateable_remote_ids()?;
        for skill_id in skill_ids {
            let record = records
                .iter()
                .find(|record| record.skill_id == *skill_id)
                .ok_or_else(|| {
                    UpdateError::Validation("the Skill is not a remote Install".into())
                })?;
            if !self.source_updates_are_allowed(updateable_remote_ids.as_ref(), &record.remote_id) {
                return Err(UpdateError::SourceCapabilityClosed);
            }
            self.store
                .set_remote_requested_ref(skill_id, &record.verification_anchor_commit)?;
        }
        Ok(())
    }

    fn git_mirror_for(&self, repo_url: &str) -> Result<PathBuf, UpdateError> {
        Ok(git_mirror_path(&self.active_git_cache_root()?, repo_url))
    }

    /// Resolve the commit for one tracked Install: "HEAD" uses the repository
    /// default branch; explicit branches resolve via `refs/heads` (already
    /// prefixed refs resolve as-is). `None` when the tracked ref is absent.
    fn resolve_tracked_ref(
        &self,
        mirror: &Path,
        default_resolved: &Option<String>,
        record: &RemoteInstallRecord,
    ) -> Result<Option<String>, UpdateError> {
        if record.requested_ref == DEFAULT_BRANCH_REF {
            return Ok(default_resolved.clone());
        }
        let branch_ref = if record.requested_ref.starts_with("refs/heads/") {
            record.requested_ref.clone()
        } else {
            format!("refs/heads/{}", record.requested_ref)
        };
        Ok(self.git.resolve_commit(mirror, &branch_ref)?)
    }

    /// Authoritative Modified detection at plan time: the entity's current
    /// tree hash differs from the recorded hash (persisted health may be
    /// stale because health checks run at startup).
    fn entity_is_modified(&self, record: &RemoteInstallRecord) -> bool {
        self.filesystem
            .tree_hash(&record.final_entity_path)
            .is_ok_and(|hash| hash != record.recorded_content_hash)
    }

    fn updateable_remote_ids(&self) -> Result<Option<HashSet<String>>, UpdateError> {
        let Some(scan) = &self.source_capability_scan else {
            // Existing focused Update tests exercise pre-vNext mechanics
            // without product composition.  The app always supplies the
            // scan above; its absence must never be used there as a bypass.
            return Ok(None);
        };
        let _capability_report = scan.scan()?;
        Ok(Some(HashSet::new()))
    }

    fn source_updates_are_allowed(
        &self,
        updateable_remote_ids: Option<&HashSet<String>>,
        remote_id: &str,
    ) -> bool {
        updateable_remote_ids.is_none_or(|remote_ids| remote_ids.contains(remote_id))
    }

    /// Parent integrity gate (ADR-0013 §4.3): the manifest must agree with
    /// the Catalog row on remote_id, canonical URL and confirmed aliases.
    /// A missing manifest is the migrated-legacy backfill state and passes
    /// (the migration derived the row from the same legacy data, so the row
    /// is the only authority until the first verified write materializes
    /// the manifest); any present mismatch fails closed for this parent.
    fn parent_integrity_ok(&self, record: &RemoteInstallRecord) -> bool {
        let Ok(remotes_root) = self.active_remotes_root() else {
            return false;
        };
        match self
            .filesystem
            .read_remote_parent_manifest(&remotes_root, &record.remote_id)
        {
            Ok(Some(manifest)) => self.parent_matches_manifest(record, &manifest),
            Ok(None) => true,
            Err(_) => false,
        }
    }

    fn parent_matches_manifest(
        &self,
        record: &RemoteInstallRecord,
        manifest: &RemoteParentManifest,
    ) -> bool {
        if manifest.remote_id != record.remote_id || manifest.canonical_url != record.source_url {
            return false;
        }
        let row_aliases = self
            .store
            .load_remote_parents()
            .ok()
            .and_then(|parents| {
                parents
                    .into_iter()
                    .find(|parent| parent.remote_id == record.remote_id)
                    .map(|parent| parent.aliases)
            })
            .unwrap_or_default();
        let mut manifest_aliases = manifest.aliases.clone();
        let mut row_aliases = row_aliases;
        manifest_aliases.sort();
        row_aliases.sort();
        manifest_aliases == row_aliases
    }
}

/// Backfill the parent manifest from the authoritative Catalog row when a
/// migrated-legacy parent has none (ADR-0013 §4.3).
fn ensure_parent_manifest(
    filesystem: &dyn FileSystem,
    remotes_root: &Path,
    record: &RemoteInstallRecord,
    now_nanos: u128,
    aliases: Vec<String>,
) -> Result<(), UpdateError> {
    // The row is the authority for confirmed aliases: a missing manifest
    // is materialized and a stale alias list is refreshed (the integrity
    // gate already proved the manifest agrees on remote_id and canonical
    // URL before this apply ran).
    let existing_created_at = if let Some(existing) =
        filesystem.read_remote_parent_manifest(remotes_root, &record.remote_id)?
    {
        let mut existing_aliases = existing.aliases;
        let mut aliases = aliases.clone();
        existing_aliases.sort();
        aliases.sort();
        if existing_aliases == aliases {
            return Ok(());
        }
        Some(existing.created_at)
    } else {
        None
    };
    filesystem.write_remote_parent_manifest(
        remotes_root,
        &RemoteParentManifest {
            schema_version: 1,
            remote_id: record.remote_id.clone(),
            canonical_url: record.source_url.clone(),
            provider: None,
            tracking_ref: None,
            current_release_id: None,
            aliases,
            created_at: existing_created_at
                .unwrap_or_else(|| crate::seams::clock::iso_timestamp(now_nanos)),
        },
    )?;
    Ok(())
}

fn update_item_error(
    selection: &UpdateSelection,
    record: &RemoteInstallRecord,
    message: String,
) -> UpdatePlanItem {
    UpdatePlanItem {
        skill_id: selection.skill_id.clone(),
        directory_name: record.directory_name.clone(),
        plan_token: String::new(),
        current_commit: record.verification_anchor_commit.clone(),
        new_commit: String::new(),
        modified: record.health == Health::Modified,
        path_changed: false,
        error: Some(message),
    }
}
