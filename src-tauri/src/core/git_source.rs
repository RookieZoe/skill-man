//! Pure Git source parsing and discovery rules.
//!
//! This module owns the *policy* of remote Install sources (ADR-0004):
//! which inputs are accepted, how a URL is split into repo + ref + path,
//! the two-phase discovery semantics, and which refs are trackable for
//! update checks. It performs no I/O; the `GitSource` seam carries the
//! transport.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use thiserror::Error;

use crate::seams::source::{GitFetchReport, GitSource, SourceError};

pub const MAX_SOURCE_SKILLS: usize = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitSourceSpec {
    /// Normalized clone URL (https, or file:// for local fixtures).
    pub url: String,
    /// Requested ref; `None` tracks the remote default branch ("HEAD").
    pub requested_ref: Option<String>,
    /// Repo-relative directory the discovery is scoped to (from `tree/<ref>/<path>` URLs).
    pub path_prefix: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredSkill {
    /// Product identity (directory name of the Skill).
    pub directory_name: String,
    /// Repo-relative path of the Skill directory; empty means the repo root.
    pub skill_path: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiscoveryMode {
    /// Scan standard positions; recurse only when nothing is found.
    StandardWithRecursiveFallback,
    /// Always recurse (the "force full depth" advanced option).
    ForceFullDepth,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum GitSourceParseError {
    #[error("a Git source is required")]
    Empty,
    #[error("SSH Git sources are not supported; use a public HTTPS URL or clone first and Link it")]
    SshNotSupported,
    #[error("Git sources must use HTTPS (or a file:// URL); got scheme '{0}'")]
    UnsupportedScheme(String),
    #[error("Git sources must not embed credentials; remove the user information from '{0}'")]
    CredentialsNotSupported(String),
    #[error("'{0}' is not a valid GitHub shorthand; expected owner/repo")]
    InvalidShorthand(String),
    #[error("'{0}' is not a valid Git source reference")]
    InvalidRef(String),
    #[error("blob URLs point at a single file; use a tree URL or the repository root instead")]
    BlobUrlNotSupported,
    #[error("'{0}' contains an unsafe path component")]
    UnsafePath(String),
}

pub fn is_commit_sha(value: &str) -> bool {
    value.len() == 40 && value.chars().all(|character| character.is_ascii_hexdigit())
}

/// A ref is fixed when it can never move: an exact commit or an explicit tag.
pub fn is_pinned_ref(requested_ref: &str) -> bool {
    is_commit_sha(requested_ref) || requested_ref.starts_with("refs/tags/")
}

/// A ref is trackable when the remote may advance it (default branch or a branch).
pub fn is_trackable_ref(requested_ref: &str) -> bool {
    !is_pinned_ref(requested_ref)
}

/// The recordable form of "no ref requested": track the remote default branch.
pub const DEFAULT_BRANCH_REF: &str = "HEAD";

/// Extract a display/clone-URL repo name from a URL, for suggested root names.
pub fn repo_name_from_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    trimmed
        .rsplit('/')
        .next()
        .unwrap_or(trimmed)
        .strip_suffix(".git")
        .unwrap_or_else(|| trimmed.rsplit('/').next().unwrap_or(trimmed))
        .to_owned()
}

pub fn parse_git_source_input(input: &str) -> Result<GitSourceSpec, GitSourceParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(GitSourceParseError::Empty);
    }
    if let Some(_shorthand) = input.strip_prefix("git@") {
        return Err(GitSourceParseError::SshNotSupported);
    }
    if input.starts_with("ssh://") {
        return Err(GitSourceParseError::SshNotSupported);
    }
    if let Some(rest) = input.strip_prefix("file://") {
        if rest.is_empty() || rest.contains("://") {
            return Err(GitSourceParseError::InvalidShorthand(input.into()));
        }
        return Ok(GitSourceSpec {
            url: input.to_owned(),
            requested_ref: None,
            path_prefix: None,
        });
    }
    let scheme_end = input.find("://");
    match scheme_end {
        Some(index) => {
            let scheme = &input[..index];
            if scheme != "http" && scheme != "https" {
                return Err(GitSourceParseError::UnsupportedScheme(scheme.into()));
            }
            parse_url_spec(input)
        }
        None => parse_shorthand_spec(input),
    }
}

fn parse_shorthand_spec(input: &str) -> Result<GitSourceSpec, GitSourceParseError> {
    if input.contains(':') || input.starts_with('-') || input.contains('\\') {
        return Err(GitSourceParseError::InvalidShorthand(input.into()));
    }
    let Some((owner, repo)) = input.split_once('/') else {
        return Err(GitSourceParseError::InvalidShorthand(input.into()));
    };
    if owner.is_empty()
        || repo.is_empty()
        || repo.contains('/')
        || !valid_shorthand_component(owner)
        || !valid_shorthand_component(repo)
    {
        return Err(GitSourceParseError::InvalidShorthand(input.into()));
    }
    Ok(GitSourceSpec {
        url: format!("https://github.com/{owner}/{repo}"),
        requested_ref: None,
        path_prefix: None,
    })
}

fn valid_shorthand_component(component: &str) -> bool {
    !component.starts_with('-')
        && component.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        })
}

fn parse_url_spec(url: &str) -> Result<GitSourceSpec, GitSourceParseError> {
    let rest = &url["https://".len().min(url.len())..];
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.contains('@') {
        return Err(GitSourceParseError::CredentialsNotSupported(url.into()));
    }
    let host = authority.split(':').next().unwrap_or(authority);
    let path = &rest[authority_end..];
    match host {
        "github.com" | "www.github.com" => parse_github_path(url, path),
        "gitlab.com" | "www.gitlab.com" => parse_gitlab_path(url, path),
        _ => parse_generic_path(url, path),
    }
}

fn split_clean_path(path: &str) -> Result<Vec<String>, GitSourceParseError> {
    let mut segments = Vec::new();
    for raw in path.split('/') {
        if raw.is_empty() {
            continue;
        }
        if raw == "." || raw == ".." {
            return Err(GitSourceParseError::UnsafePath(path.into()));
        }
        if raw
            .chars()
            .any(|character| character.is_control() || character == '\0')
        {
            return Err(GitSourceParseError::UnsafePath(path.into()));
        }
        segments.push(raw.to_owned());
    }
    Ok(segments)
}

fn parse_github_path(url: &str, path: &str) -> Result<GitSourceSpec, GitSourceParseError> {
    let segments = split_clean_path(path)?;
    if segments.len() < 2 {
        return Err(GitSourceParseError::InvalidShorthand(url.into()));
    }
    let mut base = format!("https://github.com/{}/{}", segments[0], segments[1]);
    let mut requested_ref = None;
    let mut path_prefix = None;
    if segments.len() > 2 {
        match segments[2].as_str() {
            "tree" => {
                if segments.len() < 4 {
                    return Err(GitSourceParseError::InvalidRef(url.into()));
                }
                let reference = segments[3].as_str();
                validate_ref_token(reference)?;
                requested_ref = Some(reference.to_owned());
                if segments.len() > 4 {
                    path_prefix = Some(segments[4..].join("/"));
                }
            }
            "blob" => return Err(GitSourceParseError::BlobUrlNotSupported),
            _ => return Err(GitSourceParseError::InvalidShorthand(url.into())),
        }
    }
    if base.ends_with(".git") {
        base.truncate(base.len() - 4);
    }
    Ok(GitSourceSpec {
        url: base,
        requested_ref,
        path_prefix,
    })
}

fn parse_gitlab_path(url: &str, path: &str) -> Result<GitSourceSpec, GitSourceParseError> {
    let segments = split_clean_path(path)?;
    if segments.len() < 2 {
        return Err(GitSourceParseError::InvalidShorthand(url.into()));
    }
    let tree_marker = segments
        .windows(2)
        .position(|window| window[0] == "-" && window[1] == "tree");
    let (base_segments, requested_ref, path_prefix) = match tree_marker {
        Some(index) => {
            let reference = segments
                .get(index + 2)
                .ok_or_else(|| GitSourceParseError::InvalidRef(url.into()))?;
            validate_ref_token(reference)?;
            (
                &segments[..index],
                Some(reference.clone()),
                (index + 3 < segments.len()).then(|| segments[index + 3..].join("/")),
            )
        }
        None => (segments.as_slice(), None, None),
    };
    Ok(GitSourceSpec {
        url: format!("https://gitlab.com/{}", base_segments.join("/")),
        requested_ref,
        path_prefix,
    })
}

fn parse_generic_path(url: &str, path: &str) -> Result<GitSourceSpec, GitSourceParseError> {
    split_clean_path(path)?;
    Ok(GitSourceSpec {
        url: url.to_owned(),
        requested_ref: None,
        path_prefix: None,
    })
}

fn validate_ref_token(reference: &str) -> Result<(), GitSourceParseError> {
    if reference.is_empty()
        || reference.starts_with('-')
        || reference.contains("..")
        || reference
            .chars()
            .any(|character| character.is_control() || character == '\0')
    {
        return Err(GitSourceParseError::InvalidRef(reference.into()));
    }
    Ok(())
}

/// Two-phase Skill discovery over a repository tree, scoped to `scope`.
///
/// `tree_files` are the repo-relative paths of every tree entry at the
/// resolved commit. Standard positions are scanned first (the scan root
/// itself, its immediate children, `<root>/skills/` and `<plugin>/skills/`);
/// when nothing is found the scan recurses, unless the caller forced full
/// depth. Candidates are sorted by directory name then path, deduplicated
/// by path, and truncated at `MAX_SOURCE_SKILLS`.
pub fn discover_skills_from_paths(
    repo_name: &str,
    tree_files: &[PathBuf],
    scope: Option<&str>,
    mode: DiscoveryMode,
) -> Vec<DiscoveredSkill> {
    let mut skill_directories = HashSet::new();
    for file in tree_files {
        if file.file_name().is_some_and(|name| name == "SKILL.md") {
            skill_directories.insert(file.parent().unwrap_or_else(|| Path::new("")).to_path_buf());
        }
    }
    if skill_directories.is_empty() {
        return Vec::new();
    }
    let scope_root = scope.map(PathBuf::from).unwrap_or_default();
    let in_scope = |path: &Path| path == scope_root || path.starts_with(&scope_root);

    let mut candidates = Vec::new();
    let push_skill = |candidates: &mut Vec<DiscoveredSkill>, path: &Path| {
        if !in_scope(path) || !skill_directories.contains(path) {
            return;
        }
        let skill_path = if path.as_os_str().is_empty() {
            String::new()
        } else {
            path.to_string_lossy().into_owned()
        };
        let directory_name = if skill_path.is_empty() {
            if scope_root.as_os_str().is_empty() {
                repo_name.to_owned()
            } else {
                scope_root
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| repo_name.to_owned())
            }
        } else {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        candidates.push(DiscoveredSkill {
            directory_name,
            skill_path,
        });
    };

    let scan_standard = |candidates: &mut Vec<DiscoveredSkill>| {
        push_skill(candidates, &scope_root);
        // Immediate children of the scan root: a/<SKILL.md>, b/<SKILL.md>, ...
        for path in &skill_directories {
            if path.parent().is_some_and(|parent| parent == scope_root) {
                push_skill(candidates, path);
            }
        }
        // <root>/skills/<name>/SKILL.md
        let skills_dir = scope_root.join("skills");
        for path in &skill_directories {
            if path.parent().is_some_and(|parent| parent == skills_dir) {
                push_skill(candidates, path);
            }
        }
        // <root>/<plugin>/skills/<name>/SKILL.md
        for path in &skill_directories {
            let Some(parent) = path.parent() else {
                continue;
            };
            if parent.file_name() != Some(std::ffi::OsStr::new("skills")) {
                continue;
            }
            let Some(plugin) = parent.parent() else {
                continue;
            };
            if plugin == scope_root || plugin.parent().is_some_and(|great| great == scope_root) {
                push_skill(candidates, path);
            }
        }
    };

    match mode {
        DiscoveryMode::ForceFullDepth => {
            for path in &skill_directories {
                push_skill(&mut candidates, path);
            }
        }
        DiscoveryMode::StandardWithRecursiveFallback => {
            scan_standard(&mut candidates);
            if candidates.is_empty() {
                for path in &skill_directories {
                    push_skill(&mut candidates, path);
                }
            }
        }
    }

    candidates.sort_by(|left, right| {
        left.directory_name
            .cmp(&right.directory_name)
            .then(left.skill_path.cmp(&right.skill_path))
    });
    candidates.dedup_by(|left, right| left.skill_path == right.skill_path);
    candidates
}

/// The blob path of a Skill's SKILL.md inside the repo.
pub fn skill_document_path(skill_path: &str) -> String {
    if skill_path.is_empty() {
        "SKILL.md".into()
    } else {
        format!("{skill_path}/SKILL.md")
    }
}

/// The resolved, recordable form of a requested ref.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedGitRef {
    pub commit: String,
    /// The ref to persist: "HEAD" tracks the default branch, otherwise the
    /// requested ref (branches stay trackable; tags and commits stay pinned).
    pub recorded_ref: String,
    pub trackable: bool,
}

#[derive(Debug, Error)]
pub enum GitResolveError {
    #[error("ref '{0}' was not found in the repository")]
    RefNotFound(String),
    #[error(transparent)]
    Source(#[from] SourceError),
}

/// Resolve a requested ref to a commit in the mirror, following ADR-0004:
/// no ref means the remote default branch ("HEAD"); branches resolve head
/// first, then tags; explicit tags and commit SHAs are pinned.
pub fn resolve_git_ref(
    git: &dyn GitSource,
    mirror: &Path,
    spec: &GitSourceSpec,
    report: &GitFetchReport,
) -> Result<ResolvedGitRef, GitResolveError> {
    match &spec.requested_ref {
        None => {
            let branch = report.default_branch.as_deref();
            if let Some(branch) = branch {
                let commit = git
                    .resolve_commit(mirror, &format!("refs/heads/{branch}"))?
                    .ok_or_else(|| GitResolveError::RefNotFound(branch.into()))?;
                return Ok(ResolvedGitRef {
                    commit,
                    recorded_ref: DEFAULT_BRANCH_REF.into(),
                    trackable: true,
                });
            }
            let commit = git
                .resolve_commit(mirror, "HEAD")?
                .ok_or_else(|| GitResolveError::RefNotFound(DEFAULT_BRANCH_REF.into()))?;
            Ok(ResolvedGitRef {
                commit,
                recorded_ref: DEFAULT_BRANCH_REF.into(),
                trackable: true,
            })
        }
        Some(reference) => {
            if is_commit_sha(reference) {
                let commit = git
                    .resolve_commit(mirror, reference)?
                    .ok_or_else(|| GitResolveError::RefNotFound(reference.clone()))?;
                return Ok(ResolvedGitRef {
                    commit,
                    recorded_ref: reference.clone(),
                    trackable: false,
                });
            }
            if reference.starts_with("refs/") {
                let commit = git
                    .resolve_commit(mirror, reference)?
                    .ok_or_else(|| GitResolveError::RefNotFound(reference.clone()))?;
                return Ok(ResolvedGitRef {
                    commit,
                    recorded_ref: reference.clone(),
                    trackable: reference.starts_with("refs/heads/"),
                });
            }
            let branch_ref = format!("refs/heads/{reference}");
            if let Some(commit) = git.resolve_commit(mirror, &branch_ref)? {
                return Ok(ResolvedGitRef {
                    commit,
                    recorded_ref: reference.clone(),
                    trackable: true,
                });
            }
            let tag_ref = format!("refs/tags/{reference}");
            if let Some(commit) = git.resolve_commit(mirror, &tag_ref)? {
                return Ok(ResolvedGitRef {
                    commit,
                    recorded_ref: reference.clone(),
                    trackable: false,
                });
            }
            if let Some(commit) = git.resolve_commit(mirror, reference)? {
                return Ok(ResolvedGitRef {
                    commit,
                    recorded_ref: reference.clone(),
                    trackable: false,
                });
            }
            Err(GitResolveError::RefNotFound(reference.clone()))
        }
    }
}

/// Stable cache directory id for a source URL.
pub fn git_cache_repo_id(url: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(url.as_bytes());
    let mut hex = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// The mirror path for a source URL under a Library cache root.
pub fn git_mirror_path(cache_root: &Path, repo_url: &str) -> PathBuf {
    cache_root
        .join("git")
        .join(format!("{}.git", git_cache_repo_id(repo_url)))
}

/// Validate that a skill path is safe to pass to git (no traversal, no option injection).
pub fn validate_skill_path(skill_path: &str) -> Result<(), GitSourceParseError> {
    let path = Path::new(skill_path);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        return Err(GitSourceParseError::UnsafePath(skill_path.into()));
    }
    for component in path.components() {
        let Component::Normal(value) = component else {
            continue;
        };
        let value = value.to_string_lossy();
        if value.starts_with('-')
            || value
                .chars()
                .any(|character| character.is_control() || character == '\0')
        {
            return Err(GitSourceParseError::UnsafePath(skill_path.into()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        DEFAULT_BRANCH_REF, DiscoveryMode, GitSourceParseError, discover_skills_from_paths,
        is_commit_sha, is_pinned_ref, is_trackable_ref, parse_git_source_input, repo_name_from_url,
        skill_document_path, validate_skill_path,
    };

    fn paths(entries: &[&str]) -> Vec<PathBuf> {
        entries.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn github_shorthand_parses_to_https() {
        let spec = parse_git_source_input("vercel-labs/skills").expect("shorthand");
        assert_eq!(spec.url, "https://github.com/vercel-labs/skills");
        assert_eq!(spec.requested_ref, None);
        assert_eq!(spec.path_prefix, None);
    }

    #[test]
    fn github_tree_url_parses_ref_and_subdir() {
        let spec =
            parse_git_source_input("https://github.com/owner/repo/tree/main/packages/skills")
                .expect("tree URL");
        assert_eq!(spec.url, "https://github.com/owner/repo");
        assert_eq!(spec.requested_ref.as_deref(), Some("main"));
        assert_eq!(spec.path_prefix.as_deref(), Some("packages/skills"));
    }

    #[test]
    fn github_tree_url_with_tag_keeps_tag() {
        let spec = parse_git_source_input("https://github.com/owner/repo/tree/v1.2.3")
            .expect("tag tree URL");
        assert_eq!(spec.url, "https://github.com/owner/repo");
        assert_eq!(spec.requested_ref.as_deref(), Some("v1.2.3"));
    }

    #[test]
    fn github_url_with_git_suffix_normalizes() {
        let spec = parse_git_source_input("https://github.com/owner/repo.git/tree/dev/skills")
            .expect("URL with .git");
        assert_eq!(spec.url, "https://github.com/owner/repo");
        assert_eq!(spec.requested_ref.as_deref(), Some("dev"));
    }

    #[test]
    fn gitlab_tree_url_parses() {
        let spec =
            parse_git_source_input("https://gitlab.com/group/subgroup/repo/-/tree/dev/skills")
                .expect("GitLab tree URL");
        assert_eq!(spec.url, "https://gitlab.com/group/subgroup/repo");
        assert_eq!(spec.requested_ref.as_deref(), Some("dev"));
        assert_eq!(spec.path_prefix.as_deref(), Some("skills"));
    }

    #[test]
    fn generic_https_url_passes_through() {
        let spec = parse_git_source_input("https://example.com/team/repo.git").expect("generic");
        assert_eq!(spec.url, "https://example.com/team/repo.git");
        assert_eq!(spec.requested_ref, None);
    }

    #[test]
    fn file_url_passes_through_for_local_fixtures() {
        let spec = parse_git_source_input("file:///tmp/fixture-repo").expect("file URL");
        assert_eq!(spec.url, "file:///tmp/fixture-repo");
    }

    #[test]
    fn ssh_and_credentials_are_rejected() {
        assert!(matches!(
            parse_git_source_input("git@github.com:owner/repo.git"),
            Err(GitSourceParseError::SshNotSupported)
        ));
        assert!(matches!(
            parse_git_source_input("ssh://git@github.com/owner/repo.git"),
            Err(GitSourceParseError::SshNotSupported)
        ));
        assert!(matches!(
            parse_git_source_input("https://token@github.com/owner/repo"),
            Err(GitSourceParseError::CredentialsNotSupported(_))
        ));
        assert!(matches!(
            parse_git_source_input("ftp://example.com/repo"),
            Err(GitSourceParseError::UnsupportedScheme(_))
        ));
    }

    #[test]
    fn blob_urls_and_bad_shorthands_are_rejected() {
        assert!(matches!(
            parse_git_source_input("https://github.com/owner/repo/blob/main/SKILL.md"),
            Err(GitSourceParseError::BlobUrlNotSupported)
        ));
        assert!(matches!(
            parse_git_source_input("owner"),
            Err(GitSourceParseError::InvalidShorthand(_))
        ));
        assert!(matches!(
            parse_git_source_input("owner/repo/extra"),
            Err(GitSourceParseError::InvalidShorthand(_))
        ));
        assert!(matches!(
            parse_git_source_input(""),
            Err(GitSourceParseError::Empty)
        ));
    }

    #[test]
    fn unsafe_refs_and_paths_are_rejected() {
        assert!(matches!(
            parse_git_source_input("https://github.com/o/r/tree/-oops/skills"),
            Err(GitSourceParseError::InvalidRef(_))
        ));
        assert!(matches!(
            parse_git_source_input("https://github.com/o/r/tree/main/../skills"),
            Err(GitSourceParseError::UnsafePath(_))
        ));
        assert!(matches!(
            validate_skill_path("../escape"),
            Err(GitSourceParseError::UnsafePath(_))
        ));
        assert!(validate_skill_path("packages/skill-name").is_ok());
    }

    #[test]
    fn ref_classification_matches_adr() {
        let commit = "0123456789abcdef0123456789abcdef01234567";
        assert!(is_commit_sha(commit));
        assert!(!is_commit_sha("main"));
        assert!(is_pinned_ref(commit));
        assert!(is_pinned_ref("refs/tags/v1.0"));
        assert!(!is_pinned_ref("main"));
        assert!(!is_pinned_ref(DEFAULT_BRANCH_REF));
        assert!(is_trackable_ref("main"));
        assert!(is_trackable_ref(DEFAULT_BRANCH_REF));
        assert!(!is_trackable_ref(commit));
    }

    #[test]
    fn repo_name_strips_git_suffix_and_trailing_slash() {
        assert_eq!(repo_name_from_url("https://github.com/o/r.git"), "r");
        assert_eq!(repo_name_from_url("https://github.com/o/r/"), "r");
        assert_eq!(repo_name_from_url("https://example.com/a/b"), "b");
    }

    #[test]
    fn discovery_finds_root_skill_and_standard_positions() {
        let files = paths(&[
            "SKILL.md",
            "README.md",
            "skills/a/SKILL.md",
            "skills/b/SKILL.md",
        ]);
        let found = discover_skills_from_paths(
            "repo",
            &files,
            None,
            DiscoveryMode::StandardWithRecursiveFallback,
        );
        let names = found
            .iter()
            .map(|skill| (skill.directory_name.as_str(), skill.skill_path.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![("a", "skills/a"), ("b", "skills/b"), ("repo", ""),]
        );
    }

    #[test]
    fn discovery_recurses_when_standard_positions_are_empty() {
        let files = paths(&["packages/one/SKILL.md", "packages/two/deep/SKILL.md"]);
        let found = discover_skills_from_paths(
            "repo",
            &files,
            None,
            DiscoveryMode::StandardWithRecursiveFallback,
        );
        let mut names = found
            .iter()
            .map(|skill| (skill.directory_name.as_str(), skill.skill_path.as_str()))
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            vec![("deep", "packages/two/deep"), ("one", "packages/one")]
        );
    }

    #[test]
    fn force_full_depth_skips_the_standard_positions_gate() {
        let files = paths(&[
            "SKILL.md",
            "skills/standard/SKILL.md",
            "nested/deep/SKILL.md",
        ]);
        let standard = discover_skills_from_paths(
            "repo",
            &files,
            None,
            DiscoveryMode::StandardWithRecursiveFallback,
        );
        assert!(
            !standard
                .iter()
                .any(|skill| skill.skill_path == "nested/deep"),
            "standard scan must not recurse when it found candidates"
        );
        let full = discover_skills_from_paths("repo", &files, None, DiscoveryMode::ForceFullDepth);
        assert!(full.iter().any(|skill| skill.skill_path == "nested/deep"));
        assert!(
            full.iter()
                .any(|skill| skill.skill_path == "skills/standard")
        );
    }

    #[test]
    fn discovery_scopes_to_the_url_path_prefix() {
        let files = paths(&[
            "SKILL.md",
            "packages/skills/one/SKILL.md",
            "packages/other/two/SKILL.md",
        ]);
        let found = discover_skills_from_paths(
            "repo",
            &files,
            Some("packages/skills"),
            DiscoveryMode::ForceFullDepth,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].skill_path, "packages/skills/one");
        assert_eq!(found[0].directory_name, "one");
    }

    #[test]
    fn discovery_deduplicates_and_sorts_by_directory_name() {
        let files = paths(&["b/SKILL.md", "a/SKILL.md", "a/SKILL.md"]);
        let found = discover_skills_from_paths("repo", &files, None, DiscoveryMode::ForceFullDepth);
        let names = found
            .iter()
            .map(|skill| skill.directory_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn discovery_with_no_skills_returns_empty() {
        let found = discover_skills_from_paths(
            "repo",
            &paths(&["README.md", "src/lib.rs"]),
            None,
            DiscoveryMode::ForceFullDepth,
        );
        assert!(found.is_empty());
    }

    #[test]
    fn skill_document_paths_join_repo_relative_paths() {
        assert_eq!(skill_document_path(""), "SKILL.md");
        assert_eq!(skill_document_path("packages/foo"), "packages/foo/SKILL.md");
    }
}
