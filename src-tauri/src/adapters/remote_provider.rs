//! System RemoteProvider adapter: speaks to the existing Git seam for
//! transport/materialization and to the system `git` binary for local mirror
//! facts (ref disposition, subtree tree SHA, bounded ancestry walk). Fetch
//! failures are `Deferred`; post-fetch contradictions are `Conflict`.
//! Nothing here writes Home or the lock; mirrors and materialized trees live
//! in the caller workspace.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::seams::installer_lock_store::LockEntry;
use crate::seams::remote_provider::{
    AnchorResolution, RefDisposition, RemoteKind, RemoteProvider, RemoteProviderError,
    RemoteRequest, RemoteTreeFacts,
};
use crate::seams::source::{GitFetchReport, GitSource};

/// Bounded ancestry walk for moving-ref anchors: the newest commit whose
/// skill subtree matches the lock hash, up to this many path-touching
/// commits. Exhaustion is a fail-closed Provenance Conflict, never a guess.
const MAX_ANCHOR_WALK_COMMITS: usize = 2000;
const LOCAL_GIT_TIMEOUT_SECONDS: u64 = 60;

pub struct SystemRemoteProvider {
    git: Arc<dyn GitSource>,
    fetch_cache: Mutex<HashMap<(PathBuf, String), FetchOutcome>>,
}

#[derive(Clone)]
enum FetchOutcome {
    Fetched(GitFetchReport),
    Deferred(String),
}

impl SystemRemoteProvider {
    pub fn new(git: Arc<dyn GitSource>) -> Self {
        Self {
            git,
            fetch_cache: Mutex::new(HashMap::new()),
        }
    }

    fn is_ref_kind(&self, mirror_dir: &Path, namespace: &str, reference: &str) -> bool {
        run_git_local(&[
            "-C",
            mirror_dir.to_str().unwrap_or("."),
            "rev-parse",
            "--verify",
            &format!("{namespace}/{reference}"),
        ])
        .is_ok()
    }

    /// Git tree SHA of the skill subtree at `commit`; for the repo root the
    /// commit's tree SHA. `None` when the path does not exist at the commit.
    fn subtree_sha(
        &self,
        mirror_dir: &Path,
        commit: &str,
        skill_path: &str,
    ) -> Result<Option<String>, RemoteProviderError> {
        let rev = if skill_path.is_empty() {
            format!("{commit}^{{tree}}")
        } else {
            format!("{commit}:{skill_path}")
        };
        match run_git_local(&[
            "-C",
            mirror_dir.to_str().unwrap_or("."),
            "rev-parse",
            "--verify",
            &rev,
        ]) {
            Ok(output) => Ok(Some(output.trim().to_owned())),
            Err(RemoteProviderError::Conflict(message))
                if message.contains("does not exist")
                    || message.contains("exists on disk, but not in")
                    || message.contains("Not a valid object name") =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Newest commit in the requested-ref ancestry whose skill subtree tree
    /// SHA equals the lock's provider hash (ADR-0013 §2.2 moving-ref rule).
    fn find_ancestry_match(
        &self,
        mirror_dir: &Path,
        tip: &str,
        skill_path: &str,
        provider_hash: &str,
    ) -> Result<Option<String>, RemoteProviderError> {
        let walk_limit = format!("-n{MAX_ANCHOR_WALK_COMMITS}");
        let mut args = vec![
            "-C",
            mirror_dir.to_str().unwrap_or("."),
            "log",
            &walk_limit,
            "--format=%H",
            tip,
        ];
        if !skill_path.is_empty() {
            args.push("--");
            args.push(skill_path);
        }
        let output = run_git_local(&args)?;
        for commit in output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            if let Some(sha) = self.subtree_sha(mirror_dir, commit, skill_path)? {
                if sha == provider_hash {
                    return Ok(Some(commit.to_owned()));
                }
            }
        }
        Ok(None)
    }

    fn resolve_or_conflict(
        &self,
        mirror_dir: &Path,
        reference: &str,
    ) -> Result<String, RemoteProviderError> {
        self.git
            .resolve_commit(mirror_dir, reference)
            .map_err(|error| {
                RemoteProviderError::Conflict(format!(
                    "ref '{reference}' cannot be resolved: {error}"
                ))
            })?
            .ok_or_else(|| {
                RemoteProviderError::Conflict(format!(
                    "ref '{reference}' cannot be resolved in the fetched remote"
                ))
            })
    }

    /// An evidence scan creates one workspace for every normalized remote.
    /// Reuse the transport result inside that workspace while keeping each
    /// Skill's ref, anchor, and materialized tree verification independent.
    fn fetch_mirror_once(
        &self,
        request: &RemoteRequest,
        workspace: &Path,
        mirror_dir: &Path,
    ) -> Result<GitFetchReport, RemoteProviderError> {
        let mut cache = self.fetch_cache.lock().map_err(|_| {
            RemoteProviderError::Deferred("remote verification cache is unavailable".into())
        })?;
        cache.retain(|(cached_workspace, _), _| cached_workspace.is_dir());
        let key = (workspace.to_path_buf(), request.canonical_url.clone());
        if let Some(outcome) = cache.get(&key) {
            return match outcome {
                FetchOutcome::Fetched(report) => Ok(report.clone()),
                FetchOutcome::Deferred(detail) => {
                    Err(RemoteProviderError::Deferred(detail.clone()))
                }
            };
        }

        let outcome = match self.git.fetch_mirror(&request.canonical_url, mirror_dir) {
            Ok(report) => FetchOutcome::Fetched(report),
            Err(error) => {
                FetchOutcome::Deferred(format!("cannot reach '{}': {error}", request.canonical_url))
            }
        };
        let result = match &outcome {
            FetchOutcome::Fetched(report) => Ok(report.clone()),
            FetchOutcome::Deferred(detail) => Err(RemoteProviderError::Deferred(detail.clone())),
        };
        cache.insert(key, outcome);
        result
    }
}

impl RemoteProvider for SystemRemoteProvider {
    fn parse_request(&self, entry: &LockEntry) -> Result<RemoteRequest, RemoteProviderError> {
        let kind = match entry.source_type.as_str() {
            "github" => RemoteKind::Github,
            "gitlab" => RemoteKind::Gitlab,
            "git" | "https" => RemoteKind::GenericGit,
            other => {
                return Err(RemoteProviderError::Conflict(format!(
                    "unsupported sourceType '{other}'; it cannot be verified as Git"
                )));
            }
        };
        if entry.source_url.trim().is_empty() {
            return Err(RemoteProviderError::Conflict("sourceUrl is empty".into()));
        }
        let canonical_url = normalize_url(&entry.source_url, kind)?;
        let source = entry.source.trim();
        if !source.is_empty() {
            if source.contains("://") {
                let normalized = normalize_url(source, kind)?;
                if normalized != canonical_url {
                    return Err(RemoteProviderError::Conflict(format!(
                        "source '{source}' contradicts sourceUrl '{}'",
                        entry.source_url
                    )));
                }
            } else if matches!(kind, RemoteKind::Github | RemoteKind::Gitlab) {
                let url_path = canonical_path_components(&canonical_url);
                let source_parts = source
                    .split('/')
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>();
                if source_parts.len() != url_path.len()
                    || !source_parts
                        .iter()
                        .zip(url_path.iter())
                        .all(|(left, right)| left.eq_ignore_ascii_case(right))
                {
                    return Err(RemoteProviderError::Conflict(format!(
                        "source '{source}' contradicts sourceUrl '{}'",
                        entry.source_url
                    )));
                }
            }
        }
        let requested_ref = match entry.requested_ref.as_deref() {
            None | Some("") => "HEAD".to_owned(),
            Some(reference) => {
                validate_ref(reference)?;
                reference.to_owned()
            }
        };
        let skill_path = normalize_skill_path(&entry.skill_path)?;
        let provider_hash = entry.skill_folder_hash.trim().to_lowercase();
        if (provider_hash.len() != 40 && provider_hash.len() != 64)
            || !provider_hash
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            return Err(RemoteProviderError::Conflict(
                "skillFolderHash is not a known hash format".into(),
            ));
        }
        Ok(RemoteRequest {
            kind,
            canonical_url,
            requested_ref,
            skill_path,
            provider_hash,
        })
    }

    fn verify(
        &self,
        request: &RemoteRequest,
        workspace: &Path,
    ) -> Result<RemoteTreeFacts, RemoteProviderError> {
        let url_hash = format!("{:x}", Sha256::digest(request.canonical_url.as_bytes()));
        let url_hash = &url_hash[..16];
        let mirror_dir = workspace.join(format!("mirror-{url_hash}.git"));
        let fetch_report = self.fetch_mirror_once(request, workspace, &mirror_dir)?;
        let default_branch = fetch_report.default_branch.clone();

        let (disposition, resolved_commit) = if request.requested_ref == "HEAD" {
            let branch = fetch_report
                .default_branch
                .as_deref()
                .unwrap_or("HEAD")
                .to_owned();
            (
                RefDisposition::Head,
                self.resolve_or_conflict(&mirror_dir, &branch)?,
            )
        } else if self.is_ref_kind(&mirror_dir, "refs/heads", &request.requested_ref) {
            (
                RefDisposition::Branch,
                self.resolve_or_conflict(&mirror_dir, &request.requested_ref)?,
            )
        } else if self.is_ref_kind(&mirror_dir, "refs/tags", &request.requested_ref) {
            (
                RefDisposition::Tag,
                self.resolve_or_conflict(&mirror_dir, &request.requested_ref)?,
            )
        } else {
            (
                RefDisposition::Commit,
                self.resolve_or_conflict(&mirror_dir, &request.requested_ref)?,
            )
        };

        // Verification Anchor per moving/pinned rules (ADR-0013 §2.2, spec
        // §8.2): branches and HEAD search the ancestry for the newest
        // subtree match; tags and commits must match exactly.
        let mut anchor_commit = resolved_commit.clone();
        let mut original_install_commit_known = false;
        match disposition {
            RefDisposition::Head | RefDisposition::Branch => {
                if request.kind == RemoteKind::Github {
                    let tip_sha = self
                        .subtree_sha(&mirror_dir, &resolved_commit, &request.skill_path)?
                        .ok_or_else(|| {
                            RemoteProviderError::Conflict(format!(
                                "skillPath '{}' does not exist at the ref tip",
                                request.skill_path
                            ))
                        })?;
                    if tip_sha != request.provider_hash {
                        anchor_commit = self
                            .find_ancestry_match(
                                &mirror_dir,
                                &resolved_commit,
                                &request.skill_path,
                                &request.provider_hash,
                            )?
                            .ok_or_else(|| {
                                RemoteProviderError::Conflict(format!(
                                    "no commit in the '{requested}' ancestry matches the lock hash \
                                     (the lock is stale or the skill path disappeared)",
                                    requested = request.requested_ref
                                ))
                            })?;
                    }
                }
            }
            RefDisposition::Tag | RefDisposition::Commit => {
                original_install_commit_known = true;
                if request.kind == RemoteKind::Github {
                    let sha = self
                        .subtree_sha(&mirror_dir, &resolved_commit, &request.skill_path)?
                        .ok_or_else(|| {
                            RemoteProviderError::Conflict(format!(
                                "skillPath '{}' does not exist at the pinned ref",
                                request.skill_path
                            ))
                        })?;
                    if sha != request.provider_hash {
                        return Err(RemoteProviderError::Conflict(
                            "the pinned ref tree does not match the lock hash".into(),
                        ));
                    }
                }
            }
        }

        // The workspace is shared by every candidate of one normalized
        // remote (ADR-0013 §2.3: 同 remote 共享一次 fetch); the materialized
        // tree must be per candidate or concurrent verifications collide.
        let tree_id = format!(
            "{:x}",
            Sha256::digest(
                format!(
                    "{}-{}-{}",
                    request.skill_path, anchor_commit, request.provider_hash
                )
                .as_bytes()
            )
        );
        let tree_id = &tree_id[..12];
        let materialized_root = workspace.join(format!("tree-{tree_id}"));
        self.git
            .stage_skill(
                &mirror_dir,
                &anchor_commit,
                &request.skill_path,
                &materialized_root,
            )
            .map_err(|error| {
                RemoteProviderError::Conflict(format!(
                    "skillPath '{}' could not be materialized at the anchor: {error}",
                    request.skill_path
                ))
            })?;
        if !materialized_root.join("SKILL.md").is_file() {
            return Err(RemoteProviderError::Conflict(format!(
                "skillPath '{}' does not contain a readable SKILL.md at the anchor",
                request.skill_path
            )));
        }

        // Provider-specific hash check at the anchor (ADR-0013 §2.2.8):
        // GitHub compares the subtree tree SHA; GitLab/generic compare the
        // installer CLI's local SHA-256 over the materialized tree.
        let provider_hash_matched = match request.kind {
            RemoteKind::Github => true,
            RemoteKind::Gitlab | RemoteKind::GenericGit => {
                let cli_hash = cli_skill_folder_hash(&materialized_root)?;
                cli_hash.eq_ignore_ascii_case(&request.provider_hash)
            }
        };
        if !provider_hash_matched {
            return Err(RemoteProviderError::Conflict(
                "the provider hash does not match at the Verification Anchor".into(),
            ));
        }
        let subtree_tree_sha = self
            .subtree_sha(&mirror_dir, &anchor_commit, &request.skill_path)?
            .ok_or_else(|| {
                RemoteProviderError::Conflict("the anchor subtree vanished while verifying".into())
            })?;
        Ok(RemoteTreeFacts {
            disposition,
            anchor: AnchorResolution {
                anchor_commit,
                original_install_commit_known,
            },
            subtree_tree_sha,
            provider_hash_matched,
            materialized_root,
            default_branch,
        })
    }
}

fn validate_ref(reference: &str) -> Result<(), RemoteProviderError> {
    if reference.is_empty()
        || reference.contains(' ')
        || reference.contains('\t')
        || reference.contains('\n')
        || reference.contains("..")
        || reference.contains('@')
        || reference.contains('{')
        || reference.contains('\\')
        || reference.starts_with('-')
    {
        return Err(RemoteProviderError::Conflict(format!(
            "ref '{reference}' is not a safe ref"
        )));
    }
    Ok(())
}

fn normalize_skill_path(raw: &str) -> Result<String, RemoteProviderError> {
    let trimmed = raw.trim().trim_matches('/');
    if trimmed.is_empty() || trimmed == "." {
        return Ok(String::new());
    }
    let mut parts = Vec::new();
    for part in trimmed.split('/') {
        if part.is_empty() || part == "." {
            return Err(RemoteProviderError::Conflict(format!(
                "skillPath '{raw}' contains an empty or '.' component"
            )));
        }
        if part == ".." {
            return Err(RemoteProviderError::Conflict(format!(
                "skillPath '{raw}' escapes the repository"
            )));
        }
        if part
            .chars()
            .any(|character| character.is_control() || character == '\\' || character == ':')
        {
            return Err(RemoteProviderError::Conflict(format!(
                "skillPath '{raw}' contains an unsafe component"
            )));
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}

/// The path components of a canonical URL, normalized and validated.
fn canonical_path_components(canonical_url: &str) -> Vec<String> {
    let (_, path) = split_url_parts(canonical_url);
    path
}

fn split_url_parts(url: &str) -> (&str, Vec<String>) {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("file://"))
        .unwrap_or(url);
    let (_, path) = rest.split_once('/').unwrap_or((rest, ""));
    let path = path.split(['?', '#']).next().unwrap_or("");
    let components = path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(|part| part.to_owned())
        .collect::<Vec<_>>();
    (url, components)
}

/// Normalize a remote URL for repository identity (ADR-0013 §4.2): https
/// (or file for the generic provider in local verification), scheme/host
/// lowercase, default port and trailing `/`/`.git` removed, query/fragment
/// dropped, path case preserved. Embedded credentials are rejected.
pub fn normalize_url(raw: &str, kind: RemoteKind) -> Result<String, RemoteProviderError> {
    let (scheme, rest) = if let Some(rest) = raw.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = raw.strip_prefix("file://") {
        if !matches!(kind, RemoteKind::GenericGit) {
            return Err(RemoteProviderError::Conflict(format!(
                "file URLs are only accepted for the generic Git provider: '{raw}'"
            )));
        }
        ("file", rest)
    } else {
        return Err(RemoteProviderError::Conflict(format!(
            "unsupported URL scheme in '{raw}'"
        )));
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.contains('@') {
        return Err(RemoteProviderError::Conflict(
            "URLs with embedded credentials are rejected".into(),
        ));
    }
    let host = authority.to_lowercase();
    let host = if let Some((host_without_port, port)) = host.rsplit_once(':') {
        if port == "443" {
            host_without_port.to_owned()
        } else {
            return Err(RemoteProviderError::Conflict(format!(
                "non-default ports are not supported for remote verification: '{raw}'"
            )));
        }
    } else {
        host
    };
    let host = host.trim_end_matches('/');
    match kind {
        RemoteKind::Github if host != "github.com" => {
            return Err(RemoteProviderError::Conflict(format!(
                "sourceType github requires github.com, got '{host}'"
            )));
        }
        RemoteKind::Gitlab if host != "gitlab.com" => {
            return Err(RemoteProviderError::Conflict(format!(
                "sourceType gitlab requires gitlab.com, got '{host}'"
            )));
        }
        _ => {}
    }
    let path = path.split(['?', '#']).next().unwrap_or("");
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut components = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return Err(RemoteProviderError::Conflict(format!(
                "URL path escapes the repository root: '{raw}'"
            )));
        }
        components.push(part);
    }
    if matches!(kind, RemoteKind::Github | RemoteKind::Gitlab) && components.len() < 2 {
        return Err(RemoteProviderError::Conflict(format!(
            "URL '{raw}' does not identify an owner and repository"
        )));
    }
    let mut normalized = format!("{scheme}://{host}");
    for component in &components {
        normalized.push('/');
        normalized.push_str(component);
    }
    Ok(normalized)
}

/// Normalize a persisted remote URL for repository identity without a
/// provider kind (spec §3.4 v6 migration, parent lookups and Update
/// grouping): the generic Git rules accept https and file URLs and do not
/// enforce a provider host; embedded credentials are still rejected.
pub fn normalize_catalog_url(raw: &str) -> Result<String, RemoteProviderError> {
    normalize_url(raw, RemoteKind::GenericGit)
}

/// The installer CLI's local skill hash (skills CLI `local-lock.ts`): SHA-256
/// over the concatenation of (relative path + file content) of every regular
/// file, sorted by relative path, skipping `.git`/`node_modules` directories.
/// Symlinks are skipped exactly like the CLI; the sort uses byte order,
/// which matches the CLI's `localeCompare` for ASCII paths.
pub fn cli_skill_folder_hash(root: &Path) -> Result<String, RemoteProviderError> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    collect_cli_files(root, root, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    for (relative_path, content) in files {
        hasher.update(relative_path.as_bytes());
        hasher.update(&content);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn collect_cli_files(
    directory: &Path,
    base: &Path,
    files: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), RemoteProviderError> {
    let entries = fs::read_dir(directory).map_err(|source| {
        RemoteProviderError::Conflict(format!(
            "cannot enumerate the materialized tree '{}': {source}",
            directory.display()
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| {
            RemoteProviderError::Conflict(format!(
                "cannot enumerate the materialized tree '{}': {source}",
                directory.display()
            ))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| {
            RemoteProviderError::Conflict(format!("cannot inspect '{}': {source}", path.display()))
        })?;
        let name = entry.file_name();
        if file_type.is_dir() {
            if name == ".git" || name == "node_modules" {
                continue;
            }
            collect_cli_files(&path, base, files)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let content = fs::read(&path).map_err(|source| {
                RemoteProviderError::Conflict(format!("cannot read '{}': {source}", path.display()))
            })?;
            files.push((relative, content));
        }
    }
    Ok(())
}

/// Run one local git command against a mirror with a hard timeout; `Ok` is
/// stdout as UTF-8, `Err(Conflict)` carries git's stderr.
fn run_git_local(args: &[&str]) -> Result<String, RemoteProviderError> {
    let mut child = std::process::Command::new("git")
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|source| RemoteProviderError::Conflict(format!("cannot run git: {source}")))?;
    let deadline = Instant::now() + Duration::from_secs(LOCAL_GIT_TIMEOUT_SECONDS);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(source) => {
                return Err(RemoteProviderError::Conflict(format!(
                    "cannot wait for git: {source}"
                )));
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RemoteProviderError::Conflict(
                "git mirror operation timed out".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let stdout_reader = std::thread::spawn({
        let mut stdout = child.stdout.take().expect("git stdout pipe");
        move || {
            let mut buffer = Vec::new();
            let _ = std::io::Read::read_to_end(&mut stdout, &mut buffer);
            buffer
        }
    });
    let stderr_reader = std::thread::spawn({
        let mut stderr = child.stderr.take().expect("git stderr pipe");
        move || {
            let mut buffer = Vec::new();
            let _ = std::io::Read::read_to_end(&mut stderr, &mut buffer);
            String::from_utf8_lossy(&buffer).into_owned()
        }
    });
    let stdout = stdout_reader
        .join()
        .map_err(|_| RemoteProviderError::Conflict("git stdout reader panicked".into()))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| RemoteProviderError::Conflict("git stderr reader panicked".into()))?;
    if status.success() {
        Ok(String::from_utf8_lossy(&stdout).into_owned())
    } else {
        let summary = args
            .iter()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        Err(RemoteProviderError::Conflict(format!(
            "`git {summary}` failed ({}): {}",
            status
                .code()
                .map_or_else(|| "signal".into(), |code| code.to_string()),
            stderr.trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::git_source::SystemGitSource;
    use crate::seams::remote_provider::RemoteProvider;
    use crate::seams::source::{GitFetchReport, GitSource, GitTreeEntry, SourceError};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingGitSource {
        inner: SystemGitSource,
        fetches: AtomicUsize,
        fetch_error: Option<String>,
    }

    impl CountingGitSource {
        fn new() -> Self {
            Self {
                inner: SystemGitSource::new(),
                fetches: AtomicUsize::new(0),
                fetch_error: None,
            }
        }

        fn unavailable() -> Self {
            Self {
                inner: SystemGitSource::new(),
                fetches: AtomicUsize::new(0),
                fetch_error: Some("offline".into()),
            }
        }

        fn fetch_count(&self) -> usize {
            self.fetches.load(Ordering::Relaxed)
        }
    }

    impl GitSource for CountingGitSource {
        fn fetch_mirror(
            &self,
            url: &str,
            mirror_dir: &Path,
        ) -> Result<GitFetchReport, SourceError> {
            self.fetches.fetch_add(1, Ordering::Relaxed);
            if let Some(error) = &self.fetch_error {
                return Err(SourceError::Git(error.clone()));
            }
            self.inner.fetch_mirror(url, mirror_dir)
        }

        fn resolve_commit(
            &self,
            mirror_dir: &Path,
            rev: &str,
        ) -> Result<Option<String>, SourceError> {
            self.inner.resolve_commit(mirror_dir, rev)
        }

        fn list_tree(
            &self,
            mirror_dir: &Path,
            commit: &str,
        ) -> Result<Vec<GitTreeEntry>, SourceError> {
            self.inner.list_tree(mirror_dir, commit)
        }

        fn read_blob(
            &self,
            mirror_dir: &Path,
            commit: &str,
            path: &str,
            max_bytes: usize,
        ) -> Result<Option<Vec<u8>>, SourceError> {
            self.inner.read_blob(mirror_dir, commit, path, max_bytes)
        }

        fn stage_skill(
            &self,
            mirror_dir: &Path,
            commit: &str,
            skill_path: &str,
            destination: &Path,
        ) -> Result<(), SourceError> {
            self.inner
                .stage_skill(mirror_dir, commit, skill_path, destination)
        }
    }

    fn entry(
        source_type: &str,
        source: &str,
        source_url: &str,
        reference: Option<&str>,
        skill_path: &str,
        hash: &str,
    ) -> LockEntry {
        LockEntry {
            name: "networking".into(),
            source_type: source_type.into(),
            source: source.into(),
            source_url: source_url.into(),
            requested_ref: reference.map(str::to_owned),
            skill_path: skill_path.into(),
            skill_folder_hash: hash.into(),
            installed_at: Some("2026-08-01T00:00:00Z".into()),
            updated_at: Some("2026-08-01T00:00:00Z".into()),
            plugin_name: None,
        }
    }

    fn fixture_repo(root: &Path, files: &[(&str, &str)], branch: &str) -> PathBuf {
        let repo = root.join("repo");
        fs::create_dir_all(&repo).expect("create fixture repo");
        for (path, contents) in files {
            let file = repo.join(path);
            fs::create_dir_all(file.parent().expect("fixture parent")).expect("create parent");
            fs::write(&file, contents).expect("write fixture file");
        }
        let output = std::process::Command::new("git")
            .args(["init", "-q", "-b", branch])
            .current_dir(&repo)
            .output()
            .expect("git init");
        assert!(output.status.success());
        let output = std::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(&repo)
            .output()
            .expect("git add");
        assert!(output.status.success());
        let output = std::process::Command::new("git")
            .args(["commit", "-q", "-m", "fixture commit"])
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .current_dir(&repo)
            .output()
            .expect("git commit");
        assert!(output.status.success());
        repo
    }

    fn commit_file(repo: &Path, path: &str, contents: &str) -> String {
        let file = repo.join(path);
        fs::create_dir_all(file.parent().expect("parent")).expect("create parent");
        fs::write(&file, contents).expect("write file");
        let output = std::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(repo)
            .output()
            .expect("git add");
        assert!(output.status.success());
        let output = std::process::Command::new("git")
            .args(["commit", "-q", "-m", "fixture update"])
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .current_dir(repo)
            .output()
            .expect("git commit");
        assert!(output.status.success());
        let output = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .output()
            .expect("git rev-parse");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn tag(repo: &Path, name: &str) {
        let output = std::process::Command::new("git")
            .args(["tag", name])
            .current_dir(repo)
            .output()
            .expect("git tag");
        assert!(output.status.success());
    }

    fn provider() -> SystemRemoteProvider {
        SystemRemoteProvider::new(Arc::new(SystemGitSource::new()))
    }

    #[test]
    fn parse_request_accepts_github_and_normalizes_the_url() {
        let provider = provider();
        let lock = entry(
            "github",
            "acme/networking",
            "https://github.com/acme/networking.git",
            None,
            "skills/networking",
            &"a".repeat(40),
        );
        let request = provider.parse_request(&lock).expect("parse");
        assert_eq!(request.kind, RemoteKind::Github);
        assert_eq!(request.canonical_url, "https://github.com/acme/networking");
        assert_eq!(request.requested_ref, "HEAD");
        assert_eq!(request.skill_path, "skills/networking");
    }

    #[test]
    fn parse_request_rejects_credentials_and_unknown_types() {
        let provider = provider();
        let credential = entry(
            "git",
            "acme/networking",
            "https://user:pass@example.com/acme/networking.git",
            None,
            "skills/networking",
            &"a".repeat(64),
        );
        assert!(provider.parse_request(&credential).is_err());

        let unknown = entry(
            "download",
            "acme/networking",
            "https://example.com/acme/networking.git",
            None,
            "skills/networking",
            &"a".repeat(64),
        );
        let error = provider.parse_request(&unknown).expect_err("parse");
        assert!(matches!(error, RemoteProviderError::Conflict(_)));
    }

    #[test]
    fn parse_request_rejects_contradictory_sources_and_unsafe_fields() {
        let provider = provider();
        let contradiction = entry(
            "github",
            "other/repo",
            "https://github.com/acme/networking.git",
            None,
            "skills/networking",
            &"a".repeat(40),
        );
        assert!(matches!(
            provider.parse_request(&contradiction),
            Err(RemoteProviderError::Conflict(_))
        ));

        let bad_path = entry(
            "github",
            "acme/networking",
            "https://github.com/acme/networking.git",
            None,
            "../escape",
            &"a".repeat(40),
        );
        assert!(matches!(
            provider.parse_request(&bad_path),
            Err(RemoteProviderError::Conflict(_))
        ));

        let bad_ref = entry(
            "github",
            "acme/networking",
            "https://github.com/acme/networking.git",
            Some("--upload-pack=evil"),
            "skills/networking",
            &"a".repeat(40),
        );
        assert!(matches!(
            provider.parse_request(&bad_ref),
            Err(RemoteProviderError::Conflict(_))
        ));

        let bad_hash = entry(
            "github",
            "acme/networking",
            "https://github.com/acme/networking.git",
            None,
            "skills/networking",
            "not-a-hash",
        );
        assert!(matches!(
            provider.parse_request(&bad_hash),
            Err(RemoteProviderError::Conflict(_))
        ));
    }

    #[test]
    fn verify_generic_moving_ref_matches_the_tip() {
        let root = tempfile::tempdir().expect("temporary verify root");
        let repo = fixture_repo(
            root.path(),
            &[("skills/networking/SKILL.md", "# Networking\n")],
            "main",
        );
        let url = format!("file://{}", repo.display());
        let hash =
            cli_skill_folder_hash(&repo.join("skills/networking")).expect("fixture cli hash");
        let lock = entry("git", &url, &url, None, "skills/networking", &hash);
        let request = provider().parse_request(&lock).expect("parse");
        let workspace = root.path().join("workspace");
        fs::create_dir_all(&workspace).expect("create workspace");
        let facts = provider()
            .verify(&request, &workspace)
            .expect("verify with a matching tip");
        assert_eq!(facts.disposition, RefDisposition::Head);
        assert!(!facts.anchor.original_install_commit_known);
        assert!(facts.provider_hash_matched);
        assert!(facts.materialized_root.join("SKILL.md").is_file());
    }

    #[test]
    fn verify_fetches_a_remote_once_per_shared_workspace() {
        let root = tempfile::tempdir().expect("temporary verify root");
        let repo = fixture_repo(
            root.path(),
            &[
                ("skills/networking/SKILL.md", "# Networking\n"),
                ("skills/debugging/SKILL.md", "# Debugging\n"),
            ],
            "main",
        );
        let url = format!("file://{}", repo.display());
        let networking_hash =
            cli_skill_folder_hash(&repo.join("skills/networking")).expect("networking hash");
        let debugging_hash =
            cli_skill_folder_hash(&repo.join("skills/debugging")).expect("debugging hash");
        let git = Arc::new(CountingGitSource::new());
        let provider = SystemRemoteProvider::new(git.clone());
        let networking = provider
            .parse_request(&entry(
                "git",
                &url,
                &url,
                None,
                "skills/networking",
                &networking_hash,
            ))
            .expect("parse networking");
        let debugging = provider
            .parse_request(&entry(
                "git",
                &url,
                &url,
                None,
                "skills/debugging",
                &debugging_hash,
            ))
            .expect("parse debugging");
        let workspace = root.path().join("workspace");
        fs::create_dir_all(&workspace).expect("create workspace");

        provider
            .verify(&networking, &workspace)
            .expect("verify networking");
        provider
            .verify(&debugging, &workspace)
            .expect("verify debugging");

        assert_eq!(
            git.fetch_count(),
            1,
            "one Adopt scan must fetch a normalized remote only once"
        );

        let next_workspace = root.path().join("next-workspace");
        fs::create_dir_all(&next_workspace).expect("create next workspace");
        provider
            .verify(&networking, &next_workspace)
            .expect("verify networking in next scan");
        assert_eq!(
            git.fetch_count(),
            2,
            "a later Adopt scan must refresh the remote"
        );
    }

    #[test]
    fn verify_reuses_a_deferred_fetch_in_the_shared_workspace() {
        let root = tempfile::tempdir().expect("temporary verify root");
        let url = "https://example.invalid/acme/skills";
        let git = Arc::new(CountingGitSource::unavailable());
        let provider = SystemRemoteProvider::new(git.clone());
        let alpha = provider
            .parse_request(&entry(
                "git",
                url,
                url,
                None,
                "skills/alpha",
                &"a".repeat(64),
            ))
            .expect("parse alpha");
        let beta = provider
            .parse_request(&entry(
                "git",
                url,
                url,
                None,
                "skills/beta",
                &"b".repeat(64),
            ))
            .expect("parse beta");
        let workspace = root.path().join("workspace");
        fs::create_dir_all(&workspace).expect("create workspace");

        assert!(matches!(
            provider.verify(&alpha, &workspace),
            Err(RemoteProviderError::Deferred(_))
        ));
        assert!(matches!(
            provider.verify(&beta, &workspace),
            Err(RemoteProviderError::Deferred(_))
        ));
        assert_eq!(
            git.fetch_count(),
            1,
            "one unavailable remote must defer the whole scan group without retrying"
        );
    }

    #[test]
    fn verify_generic_provider_hash_mismatch_is_a_conflict() {
        let root = tempfile::tempdir().expect("temporary verify root");
        let repo = fixture_repo(
            root.path(),
            &[("skills/networking/SKILL.md", "# Networking\n")],
            "main",
        );
        let url = format!("file://{}", repo.display());
        let lock = entry(
            "git",
            &url,
            &url,
            None,
            "skills/networking",
            &"b".repeat(64),
        );
        let request = provider().parse_request(&lock).expect("parse");
        let workspace = root.path().join("workspace");
        fs::create_dir_all(&workspace).expect("create workspace");
        assert!(matches!(
            provider().verify(&request, &workspace),
            Err(RemoteProviderError::Conflict(_))
        ));
    }

    #[test]
    fn verify_pinned_tag_requires_exact_match() {
        let root = tempfile::tempdir().expect("temporary verify root");
        let repo = fixture_repo(
            root.path(),
            &[("skills/networking/SKILL.md", "# Networking\n")],
            "main",
        );
        tag(&repo, "v1.0.0");
        let url = format!("file://{}", repo.display());
        let hash =
            cli_skill_folder_hash(&repo.join("skills/networking")).expect("fixture cli hash");
        let lock = entry(
            "git",
            &url,
            &url,
            Some("v1.0.0"),
            "skills/networking",
            &hash,
        );
        let request = provider().parse_request(&lock).expect("parse");
        let workspace = root.path().join("workspace");
        fs::create_dir_all(&workspace).expect("create workspace");
        let facts = provider()
            .verify(&request, &workspace)
            .expect("pinned tag verifies");
        assert_eq!(facts.disposition, RefDisposition::Tag);
        assert!(facts.anchor.original_install_commit_known);
    }

    #[test]
    fn verify_pinned_commit_mismatch_is_a_conflict() {
        let root = tempfile::tempdir().expect("temporary verify root");
        let repo = fixture_repo(
            root.path(),
            &[("skills/networking/SKILL.md", "# Networking v1\n")],
            "main",
        );
        let first = commit_file(&repo, "skills/networking/SKILL.md", "# Networking v2\n");
        let url = format!("file://{}", repo.display());
        // The lock pins the FIRST commit with the SECOND commit's CLI hash:
        // pinned refs must match exactly and never search substitutes.
        let lock = entry(
            "git",
            &url,
            &url,
            Some(&first),
            "skills/networking",
            &"a".repeat(64),
        );
        let request = provider().parse_request(&lock).expect("parse");
        let workspace = root.path().join("workspace");
        fs::create_dir_all(&workspace).expect("create workspace");
        assert!(matches!(
            provider().verify(&request, &workspace),
            Err(RemoteProviderError::Conflict(_))
        ));
    }

    #[test]
    fn verify_github_ancestry_finds_the_newest_subtree_match() {
        let root = tempfile::tempdir().expect("temporary verify root");
        let repo = fixture_repo(
            root.path(),
            &[
                ("skills/networking/SKILL.md", "# Networking v1\n"),
                ("README.md", "readme\n"),
            ],
            "main",
        );
        // The skill subtree is unchanged by this commit (v1 tree persists).
        commit_file(&repo, "README.md", "readme updated\n");
        // Now the skill changes: the tip no longer matches the v1 lock hash.
        commit_file(&repo, "skills/networking/SKILL.md", "# Networking v2\n");
        let url = format!("file://{}", repo.display());
        // Build a GitHub-kind request directly: the provider hash is the
        // subtree Git tree SHA of the skill path at the FIRST commit.
        let first = std::process::Command::new("git")
            .args(["rev-list", "--max-parents=0", "HEAD"])
            .current_dir(&repo)
            .output()
            .expect("git rev-list");
        let first = String::from_utf8_lossy(&first.stdout).trim().to_owned();
        let tree_sha = std::process::Command::new("git")
            .args(["rev-parse", &format!("{first}:skills/networking")])
            .current_dir(&repo)
            .output()
            .expect("git rev-parse subtree");
        let tree_sha = String::from_utf8_lossy(&tree_sha.stdout).trim().to_owned();
        assert_eq!(tree_sha.len(), 40);
        let request = RemoteRequest {
            kind: RemoteKind::Github,
            canonical_url: url,
            requested_ref: "main".into(),
            skill_path: "skills/networking".into(),
            provider_hash: tree_sha,
        };
        let workspace = root.path().join("workspace");
        fs::create_dir_all(&workspace).expect("create workspace");
        let facts = provider().verify(&request, &workspace).expect("verify");
        assert_eq!(facts.disposition, RefDisposition::Branch);
        assert!(!facts.anchor.original_install_commit_known);
        assert_eq!(facts.anchor.anchor_commit, first);
        assert!(facts.provider_hash_matched);
    }

    #[test]
    fn verify_github_moving_ref_with_no_ancestry_match_is_a_conflict() {
        let root = tempfile::tempdir().expect("temporary verify root");
        let repo = fixture_repo(
            root.path(),
            &[("skills/networking/SKILL.md", "# Networking v1\n")],
            "main",
        );
        commit_file(&repo, "skills/networking/SKILL.md", "# Networking v2\n");
        let url = format!("file://{}", repo.display());
        let request = RemoteRequest {
            kind: RemoteKind::Github,
            canonical_url: url,
            requested_ref: "main".into(),
            skill_path: "skills/networking".into(),
            provider_hash: "f".repeat(40),
        };
        let workspace = root.path().join("workspace");
        fs::create_dir_all(&workspace).expect("create workspace");
        assert!(matches!(
            provider().verify(&request, &workspace),
            Err(RemoteProviderError::Conflict(_))
        ));
    }

    #[test]
    fn cli_hash_matches_the_installer_algorithm_shape() {
        let root = tempfile::tempdir().expect("temporary cli hash root");
        let tree = root.path().join("tree");
        fs::create_dir_all(tree.join("nested")).expect("create nested");
        fs::create_dir_all(tree.join(".git")).expect("create .git");
        fs::create_dir_all(tree.join("node_modules")).expect("create node_modules");
        fs::write(tree.join("SKILL.md"), "# Skill\n").expect("write Skill");
        fs::write(tree.join("nested/helper.txt"), "helper\n").expect("write helper");
        fs::write(tree.join(".git/config"), "secret\n").expect("write git file");
        fs::write(tree.join("node_modules/dep.js"), "dep\n").expect("write dep");
        std::os::unix::fs::symlink("SKILL.md", tree.join("link.md")).expect("create symlink");
        let hash = cli_skill_folder_hash(&tree).expect("cli hash");
        assert_eq!(hash.len(), 64);
        // Stable: the same tree hashes identically with or without the
        // skipped directories and the symlink.
        fs::remove_dir_all(tree.join(".git")).expect("remove .git");
        fs::remove_dir_all(tree.join("node_modules")).expect("remove node_modules");
        fs::remove_file(tree.join("link.md")).expect("remove symlink");
        let again = cli_skill_folder_hash(&tree).expect("cli hash again");
        assert_eq!(hash, again);
    }
}
