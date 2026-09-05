//! Git transport adapter.
//!
//! Speaks to the system `git` binary to fetch mirrors and materialize tree
//! subtrees. The Core owns the mirror layout under `<Library>/cache/git/`;
//! this adapter only performs transport, with hard timeouts and never
//! prompting for credentials. No hooks can run: the mirrors are bare, and
//! `archive`/`ls-tree`/`cat-file` never touch a working tree or config of
//! the remote repository.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::adapters::zip_extract::{MAX_STAGED_SOURCE_BYTES, extract_zip_archive};
use crate::seams::source::{
    GitFetchReport, GitSource, GitTreeEntry, GitTreeEntryKind, SourceError,
};

const CONNECT_TIMEOUT_SECONDS: u64 = 60;
const FETCH_TIMEOUT_SECONDS: u64 = 300;
const LOCAL_OP_TIMEOUT_SECONDS: u64 = 60;
const MAX_GIT_STDERR_BYTES: usize = 64 * 1024;

pub struct SystemGitSource;

impl SystemGitSource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SystemGitSource {
    fn default() -> Self {
        Self::new()
    }
}

struct GitOutput {
    stdout: Vec<u8>,
}

fn run_git(
    args: &[&str],
    total_seconds: u64,
    stdout_cap: Option<usize>,
) -> Result<GitOutput, SourceError> {
    let result = run_git_impl(args, total_seconds, stdout_cap, None)?;
    if result.success {
        return Ok(GitOutput {
            stdout: result.stdout,
        });
    }
    Err(SourceError::Git(format!(
        "`git {}` failed ({}): {}",
        args.iter()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join(" "),
        result
            .exit_code
            .map_or_else(|| "signal".into(), |code| code.to_string()),
        result.stderr.trim()
    )))
}

fn run_git_to_file(
    args: &[&str],
    total_seconds: u64,
    stdout_cap: usize,
    stdout_path: &Path,
) -> Result<GitOutput, SourceError> {
    let result = run_git_impl(args, total_seconds, Some(stdout_cap), Some(stdout_path))?;
    if result.success {
        return Ok(GitOutput {
            stdout: result.stdout,
        });
    }
    Err(SourceError::Git(format!(
        "`git {}` failed ({}): {}",
        args.iter()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join(" "),
        result
            .exit_code
            .map_or_else(|| "signal".into(), |code| code.to_string()),
        result.stderr.trim()
    )))
}

struct GitRunResult {
    success: bool,
    stdout: Vec<u8>,
    stderr: String,
    exit_code: Option<i32>,
}

fn create_git_output_file(path: &Path) -> Result<fs::File, SourceError> {
    let mut options = fs::OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    options.open(path).map_err(|source| SourceError::Io {
        operation: "create Git output file",
        path: path.to_path_buf(),
        source,
    })
}

fn run_git_impl(
    args: &[&str],
    total_seconds: u64,
    stdout_cap: Option<usize>,
    stdout_path: Option<&Path>,
) -> Result<GitRunResult, SourceError> {
    let started = Instant::now();
    let deadline = started + Duration::from_secs(total_seconds);
    let mut command = Command::new("git");
    command
        .args(["-c", "http.lowSpeedLimit=1", "-c", "http.lowSpeedTime=60"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let output_file_path = stdout_path.map(Path::to_path_buf);
    let output_file = output_file_path
        .as_deref()
        .map(create_git_output_file)
        .transpose()?;
    let mut child = command.spawn().map_err(|source| SourceError::Io {
        operation: "spawn git",
        path: Path::new("git").to_path_buf(),
        source,
    })?;
    let mut stdout = child.stdout.take().expect("git stdout was piped");
    let mut stderr = child.stderr.take().expect("git stderr was piped");
    let cap = stdout_cap;
    let stdout_reader = thread::spawn(move || -> Result<_, std::io::Error> {
        let mut output = Vec::new();
        let keep_output = output_file.is_none();
        let mut file = output_file;
        let mut stream_error = None;
        let mut buffer = [0_u8; 64 * 1024];
        let mut bytes_read = 0_usize;
        let mut over_cap = false;
        loop {
            match stdout.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let next_bytes_read = bytes_read.saturating_add(count);
                    let exceeds_cap = cap.is_some_and(|cap| next_bytes_read > cap);
                    bytes_read = next_bytes_read;
                    if exceeds_cap {
                        over_cap = true;
                        output.clear();
                    } else if !over_cap {
                        if keep_output {
                            output.extend_from_slice(&buffer[..count]);
                        }
                        if let Some(file) = &mut file {
                            if let Err(source) = file.write_all(&buffer[..count])
                                && stream_error.is_none()
                            {
                                stream_error = Some(source);
                            }
                        }
                    }
                }
                Err(source) => {
                    if stream_error.is_none() {
                        stream_error = Some(source);
                    }
                    break;
                }
            }
        }
        if let Some(source) = stream_error {
            Err(source)
        } else {
            Ok((output, over_cap))
        }
    });
    let stderr_reader = thread::spawn(move || {
        let mut output = Vec::with_capacity(MAX_GIT_STDERR_BYTES);
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            match stderr.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    let remaining = MAX_GIT_STDERR_BYTES.saturating_sub(output.len());
                    if remaining > 0 {
                        output.extend_from_slice(&buffer[..count.min(remaining)]);
                    }
                }
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&output).into_owned()
    });
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
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let (stdout, over_cap) = stdout_reader
                    .join()
                    .map_err(|_| SourceError::Git("git stdout reader panicked".into()))?
                    .map_err(|source| SourceError::Io {
                        operation: "write Git output file",
                        path: output_file_path
                            .clone()
                            .unwrap_or_else(|| Path::new("git").to_path_buf()),
                        source,
                    })?;
                let stderr = stderr_reader
                    .join()
                    .map_err(|_| SourceError::Git("git stderr reader panicked".into()))?;
                if over_cap {
                    return Err(SourceError::Validation(format!(
                        "git output exceeded the size limit: {summary}"
                    )));
                }
                return Ok(GitRunResult {
                    success: status.success(),
                    stdout,
                    stderr,
                    exit_code: status.code(),
                });
            }
            Ok(None) => {}
            Err(source) => {
                return Err(SourceError::Io {
                    operation: "wait for git",
                    path: Path::new("git").to_path_buf(),
                    source,
                });
            }
        }
        if Instant::now() >= deadline {
            // Kill the whole process group (git may have spawned helpers).
            let _ = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(SourceError::Git(format!(
                "`git {summary}` timed out after {total_seconds}s"
            )));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn is_mirror_present(mirror_dir: &Path) -> bool {
    mirror_dir.join("HEAD").is_file()
}

fn validate_cache_directory(path: &std::path::Path) -> Result<(), SourceError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(SourceError::Validation(
            "Git cache must use real directories inside the configured Home".into(),
        )),
    }
}

fn validate_cache_tree(path: &std::path::Path) -> Result<(), SourceError> {
    validate_cache_directory(path)?;
    if !path.exists() {
        return Ok(());
    }
    let mut pending = vec![path.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = std::fs::read_dir(&directory).map_err(|source| SourceError::Io {
            operation: "inspect Git cache",
            path: directory.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| SourceError::Io {
                operation: "inspect Git cache entry",
                path: directory.clone(),
                source,
            })?;
            let kind = entry.file_type().map_err(|source| SourceError::Io {
                operation: "inspect Git cache type",
                path: entry.path(),
                source,
            })?;
            if kind.is_symlink()
                || (!kind.is_dir() && !kind.is_file())
                || entry.file_name() == "alternates"
            {
                return Err(SourceError::Validation("Git cache contains an unsafe entry; remove this disposable cache before retrying".into()));
            }
            if kind.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    Ok(())
}

impl GitSource for SystemGitSource {
    fn validate_home_cache(&self, home: &Path, mirror: &Path) -> Result<(), SourceError> {
        if !fs::symlink_metadata(home)
            .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        {
            return Err(SourceError::Validation(
                "The configured Home is unavailable".into(),
            ));
        }
        let cache = home.join("cache");
        if mirror.parent() != Some(cache.join("git").as_path()) {
            return Err(SourceError::Validation(
                "Git cache is outside the configured Home".into(),
            ));
        }
        validate_cache_directory(&cache)?;
        validate_cache_directory(&cache.join("git"))?;
        validate_cache_tree(mirror)
    }
    fn fetch_mirror(&self, url: &str, mirror_dir: &Path) -> Result<GitFetchReport, SourceError> {
        if !is_mirror_present(mirror_dir) {
            if mirror_dir.exists() {
                fs::remove_dir_all(mirror_dir).map_err(|source| SourceError::Io {
                    operation: "clear partial Git mirror",
                    path: mirror_dir.to_path_buf(),
                    source,
                })?;
            }
            if let Some(parent) = mirror_dir.parent() {
                fs::create_dir_all(parent).map_err(|source| SourceError::Io {
                    operation: "create Git cache directory",
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            if let Err(error) = run_git(
                &["clone", "--mirror", url, mirror_dir.to_str().unwrap_or(".")],
                FETCH_TIMEOUT_SECONDS,
                None,
            )
            .map(|_| ())
            {
                let _ = fs::remove_dir_all(mirror_dir);
                return Err(error);
            }
        } else {
            run_git(
                &[
                    "-C",
                    mirror_dir.to_str().unwrap_or("."),
                    "fetch",
                    "--prune",
                    url,
                    "+refs/*:refs/*",
                ],
                FETCH_TIMEOUT_SECONDS,
                None,
            )
            .map_err(|error| {
                // A failed refresh leaves the previous mirror state intact, so a
                // later retry can still succeed; only the check is reported.
                SourceError::Git(format!(
                    "could not refresh the Git mirror for {url}: {error}"
                ))
            })?;
        }
        let default_branch = run_git(
            &["ls-remote", "--symref", url, "HEAD"],
            CONNECT_TIMEOUT_SECONDS,
            Some(4096),
        )
        .ok()
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .find_map(|line| {
                    let line = line.trim();
                    line.strip_prefix("ref: refs/heads/")
                        .and_then(|rest| rest.split_once('\t'))
                        .map(|(branch, _)| branch.to_owned())
                })
        });
        Ok(GitFetchReport { default_branch })
    }

    fn resolve_commit(&self, mirror_dir: &Path, rev: &str) -> Result<Option<String>, SourceError> {
        let output = run_git(
            &[
                "-C",
                mirror_dir.to_str().unwrap_or("."),
                "rev-parse",
                "--verify",
                &format!("{rev}^{{commit}}"),
            ],
            LOCAL_OP_TIMEOUT_SECONDS,
            Some(128),
        );
        match output {
            Ok(output) => {
                let commit = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                if commit.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(commit))
                }
            }
            Err(SourceError::Git(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn list_tree(&self, mirror_dir: &Path, commit: &str) -> Result<Vec<GitTreeEntry>, SourceError> {
        let output = run_git(
            &[
                "-C",
                mirror_dir.to_str().unwrap_or("."),
                "ls-tree",
                "-r",
                "-z",
                commit,
            ],
            LOCAL_OP_TIMEOUT_SECONDS,
            None,
        )?;
        let mut entries = Vec::new();
        for record in output.stdout.split(|byte| *byte == 0) {
            if record.is_empty() {
                continue;
            }
            let Some(separator) = record.iter().position(|byte| *byte == b'\t') else {
                continue;
            };
            let (metadata, path) = record.split_at(separator);
            let path = std::path::PathBuf::from(String::from_utf8_lossy(&path[1..]).into_owned());
            let mut fields = metadata.splitn(3, |byte| *byte == b' ');
            let mode = fields.next().unwrap_or_default();
            let kind = if mode == b"40000" {
                GitTreeEntryKind::Tree
            } else if mode == b"120000" {
                GitTreeEntryKind::Symlink
            } else if mode == b"160000" {
                GitTreeEntryKind::Submodule
            } else {
                GitTreeEntryKind::Blob
            };
            entries.push(GitTreeEntry { path, kind });
        }
        Ok(entries)
    }

    fn read_blob(
        &self,
        mirror_dir: &Path,
        commit: &str,
        path: &str,
        max_bytes: usize,
    ) -> Result<Option<Vec<u8>>, SourceError> {
        let output = run_git(
            &[
                "-C",
                mirror_dir.to_str().unwrap_or("."),
                "cat-file",
                "blob",
                &format!("{commit}:{path}"),
            ],
            LOCAL_OP_TIMEOUT_SECONDS,
            Some(max_bytes.saturating_add(1)),
        );
        match output {
            Ok(output) => Ok(Some(output.stdout)),
            Err(SourceError::Git(message))
                if message.contains("does not exist")
                    || message.contains("exists on disk, but not in")
                    || message.contains("Not a valid object name") =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn tree_summary(
        &self,
        mirror_dir: &Path,
        commit: &str,
        skill_path: &str,
    ) -> Result<String, SourceError> {
        let revision = if skill_path.is_empty() {
            format!("{commit}^{{tree}}")
        } else {
            format!("{commit}:{skill_path}")
        };
        let output = run_git(
            &[
                "-C",
                mirror_dir.to_str().unwrap_or("."),
                "rev-parse",
                "--verify",
                &revision,
            ],
            LOCAL_OP_TIMEOUT_SECONDS,
            Some(128),
        )?;
        let tree = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if tree.len() != 40 || !tree.chars().all(|character| character.is_ascii_hexdigit()) {
            return Err(SourceError::Git(format!(
                "Git did not return a tree object ID for '{revision}'"
            )));
        }
        Ok(tree)
    }

    fn stage_skill(
        &self,
        mirror_dir: &Path,
        commit: &str,
        skill_path: &str,
        destination: &Path,
    ) -> Result<(), SourceError> {
        let parent = destination.parent().ok_or_else(|| {
            SourceError::Validation("Git staging destination has no parent".into())
        })?;
        fs::create_dir_all(parent).map_err(|source| SourceError::Io {
            operation: "create Git staging directory",
            path: parent.to_path_buf(),
            source,
        })?;
        let archive_name = destination
            .file_name()
            .map(|name| format!(".{}.skill.zip", name.to_string_lossy()))
            .unwrap_or_else(|| ".skill.zip".into());
        let archive_path = parent.join(archive_name);
        let mut args = vec![
            "-C",
            mirror_dir.to_str().unwrap_or("."),
            "archive",
            "--format=zip",
            commit,
            "--",
        ];
        if !skill_path.is_empty() {
            args.push(skill_path);
        }
        let archive_cap = usize::try_from(MAX_STAGED_SOURCE_BYTES)
            .expect("the staged source transport limit must fit in usize");
        let result = run_git_to_file(&args, LOCAL_OP_TIMEOUT_SECONDS, archive_cap, &archive_path)
            .map(|_| ())
            .and_then(|()| {
                extract_zip_archive(
                    &archive_path,
                    destination,
                    (!skill_path.is_empty()).then(|| Path::new(skill_path)),
                )
            });
        let _ = fs::remove_file(&archive_path);
        result
    }

    fn list_tags(
        &self,
        mirror_dir: &Path,
    ) -> Result<Vec<crate::seams::source::GitTagFact>, SourceError> {
        let output = run_git(
            &[
                "-C",
                mirror_dir.to_str().unwrap_or("."),
                "for-each-ref",
                "--format=%(refname:strip=2)%00%(objectname)%00%(*objectname)%00%(creatordate:unix)%0a",
                "refs/tags",
            ],
            LOCAL_OP_TIMEOUT_SECONDS,
            Some(16 * 1024 * 1024),
        )?;
        let mut tags = Vec::new();
        for record in output.stdout.split(|byte| *byte == b'\n') {
            if record.is_empty() {
                continue;
            }
            let mut fields = record.split(|byte| *byte == 0);
            let name = fields.next().unwrap_or_default();
            let object = fields.next().unwrap_or_default();
            let peeled = fields.next().unwrap_or_default();
            let created = fields.next().unwrap_or_default();
            if name.is_empty() || object.is_empty() {
                continue;
            }
            let commit = if peeled.is_empty() { object } else { peeled };
            let created_epoch_secs = std::str::from_utf8(created)
                .ok()
                .and_then(|value| value.trim().parse::<i64>().ok());
            tags.push(crate::seams::source::GitTagFact {
                name: String::from_utf8_lossy(name).into_owned(),
                commit: String::from_utf8_lossy(commit).into_owned(),
                created_epoch_secs,
            });
        }
        Ok(tags)
    }

    fn is_ancestor(
        &self,
        mirror_dir: &Path,
        ancestor: &str,
        descendant: &str,
    ) -> Result<bool, SourceError> {
        let ancestor = self
            .resolve_commit(mirror_dir, ancestor)?
            .ok_or_else(|| SourceError::Git(format!("the ancestor '{ancestor}' is not present")))?;
        let descendant = self
            .resolve_commit(mirror_dir, descendant)?
            .ok_or_else(|| {
                SourceError::Git(format!("the descendant '{descendant}' is not present"))
            })?;
        let result = run_git_impl(
            &[
                "-C",
                mirror_dir.to_str().unwrap_or("."),
                "merge-base",
                "--is-ancestor",
                &ancestor,
                &descendant,
            ],
            LOCAL_OP_TIMEOUT_SECONDS,
            Some(4096),
            None,
        )?;
        if result.success {
            return Ok(true);
        }
        if result.exit_code == Some(1) {
            return Ok(false);
        }
        Err(SourceError::Git(format!(
            "`git merge-base --is-ancestor` failed: {}",
            result.stderr.trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::{LOCAL_OP_TIMEOUT_SECONDS, SystemGitSource, is_mirror_present, run_git_to_file};
    use crate::seams::source::{GitSource, SourceError};

    /// Create a local fixture repository with a couple of commits.
    fn fixture_repo(root: &Path, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let repo = root.join(name);
        std::fs::create_dir_all(&repo).expect("create fixture repo");
        for (path, contents) in files {
            let file = repo.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).expect("create fixture parent");
            std::fs::File::create(&file)
                .expect("create fixture file")
                .write_all(contents.as_bytes())
                .expect("write fixture file");
        }
        let output = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&repo)
            .output()
            .expect("git init");
        assert!(
            output.status.success(),
            "git init: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = Command::new("git")
            .args(["add", "-A"])
            .current_dir(&repo)
            .output()
            .expect("git add");
        assert!(output.status.success());
        let output = Command::new("git")
            .args(["config", "user.email", "fixture@example.com"])
            .current_dir(&repo)
            .output()
            .expect("git config email");
        assert!(output.status.success());
        let output = Command::new("git")
            .args(["config", "user.name", "Fixture"])
            .current_dir(&repo)
            .output()
            .expect("git config name");
        assert!(output.status.success());
        let output = Command::new("git")
            .args(["commit", "-q", "-m", "fixture commit"])
            .current_dir(&repo)
            .output()
            .expect("git commit");
        assert!(
            output.status.success(),
            "git commit: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        repo
    }

    fn file_url(path: &std::path::Path) -> String {
        format!("file://{}", path.display())
    }

    #[test]
    fn fetches_mirrors_and_resolves_commits_from_local_fixtures() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = fixture_repo(
            temp.path(),
            "fixture",
            &[
                ("SKILL.md", "---\nname: fixture\n---\n"),
                ("README.md", "readme"),
            ],
        );
        let mirror = temp.path().join("cache/git/fixture");
        let source = SystemGitSource::new();
        let report = source
            .fetch_mirror(&file_url(&repo), &mirror)
            .expect("fetch mirror");
        assert_eq!(report.default_branch.as_deref(), Some("main"));
        assert!(is_mirror_present(&mirror));
        let commit = source
            .resolve_commit(&mirror, "main")
            .expect("resolve branch")
            .expect("branch present");
        assert_eq!(commit.len(), 40);
        assert_eq!(
            source
                .resolve_commit(&mirror, "refs/heads/main")
                .expect("resolve ref"),
            Some(commit.clone())
        );
        assert_eq!(
            source.resolve_commit(&mirror, "nope").expect("missing ref"),
            None
        );
    }

    #[test]
    fn fetches_refresh_an_existing_mirror() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = fixture_repo(
            temp.path(),
            "fixture",
            &[("SKILL.md", "---\nname: fixture\n---\n")],
        );
        let mirror = temp.path().join("cache/git/fixture");
        let source = SystemGitSource::new();
        source
            .fetch_mirror(&file_url(&repo), &mirror)
            .expect("first fetch");
        let before = source
            .resolve_commit(&mirror, "main")
            .expect("resolve")
            .expect("present");
        std::fs::write(repo.join("SKILL.md"), "---\nname: fixture\n---\n# v2\n")
            .expect("update file");
        let output = Command::new("git")
            .args(["add", "-A"])
            .current_dir(&repo)
            .output()
            .expect("git add");
        assert!(output.status.success());
        let output = Command::new("git")
            .args(["commit", "-q", "-m", "second commit"])
            .current_dir(&repo)
            .output()
            .expect("commit");
        assert!(output.status.success());
        source
            .fetch_mirror(&file_url(&repo), &mirror)
            .expect("second fetch");
        let after = source
            .resolve_commit(&mirror, "main")
            .expect("resolve")
            .expect("present");
        assert_ne!(before, after);
    }

    #[test]
    fn lists_trees_and_reads_blobs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = fixture_repo(
            temp.path(),
            "fixture",
            &[
                ("SKILL.md", "---\nname: fixture\n---\n"),
                ("skills/a/SKILL.md", "# A\n"),
            ],
        );
        let mirror = temp.path().join("cache/git/fixture");
        let source = SystemGitSource::new();
        source
            .fetch_mirror(&file_url(&repo), &mirror)
            .expect("fetch mirror");
        let commit = source
            .resolve_commit(&mirror, "main")
            .expect("resolve")
            .expect("present");
        let entries = source.list_tree(&mirror, &commit).expect("list tree");
        let paths = entries
            .iter()
            .map(|entry| entry.path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(paths.contains(&"SKILL.md".into()));
        assert!(paths.contains(&"skills/a/SKILL.md".into()));
        let blob = source
            .read_blob(&mirror, &commit, "skills/a/SKILL.md", 4096)
            .expect("read blob")
            .expect("blob present");
        assert_eq!(String::from_utf8_lossy(&blob), "# A\n");
        assert_eq!(
            source
                .read_blob(&mirror, &commit, "missing.md", 4096)
                .expect("read missing"),
            None
        );
    }

    #[test]
    fn stages_skill_directories_from_commits() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = fixture_repo(
            temp.path(),
            "fixture",
            &[
                ("packages/skills/one/SKILL.md", "# One\n"),
                ("packages/skills/one/lib.rs", "fn one() {}\n"),
            ],
        );
        let mirror = temp.path().join("cache/git/fixture");
        let source = SystemGitSource::new();
        source
            .fetch_mirror(&file_url(&repo), &mirror)
            .expect("fetch mirror");
        let commit = source
            .resolve_commit(&mirror, "main")
            .expect("resolve")
            .expect("present");
        let destination = temp.path().join("staging/one");
        source
            .stage_skill(&mirror, &commit, "packages/skills/one", &destination)
            .expect("stage skill");
        assert_eq!(
            std::fs::read_to_string(destination.join("SKILL.md")).expect("read SKILL.md"),
            "# One\n"
        );
        assert_eq!(
            std::fs::read_to_string(destination.join("lib.rs")).expect("read lib.rs"),
            "fn one() {}\n"
        );
        assert!(
            !destination.join("packages").exists(),
            "the repo-relative prefix must be stripped"
        );
        assert!(
            !temp.path().join("staging").join(".one.skill.zip").exists(),
            "the transient archive must be removed"
        );
    }

    #[test]
    fn git_archive_output_is_capped_before_zip_extraction() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = fixture_repo(
            temp.path(),
            "fixture",
            &[("SKILL.md", "---\nname: fixture\n---\n")],
        );
        let mirror = temp.path().join("cache/git/fixture");
        let source = SystemGitSource::new();
        source
            .fetch_mirror(&file_url(&repo), &mirror)
            .expect("fetch mirror");
        let commit = source
            .resolve_commit(&mirror, "main")
            .expect("resolve")
            .expect("present");
        let archive_path = temp.path().join("staging/archive.zip");
        std::fs::create_dir_all(archive_path.parent().expect("archive parent"))
            .expect("create archive parent");
        let args = [
            "-C",
            mirror.to_str().expect("mirror path"),
            "archive",
            "--format=zip",
            commit.as_str(),
        ];

        let error = match run_git_to_file(&args, LOCAL_OP_TIMEOUT_SECONDS, 1, &archive_path) {
            Err(error) => error,
            Ok(_) => panic!("archive output must exceed the one-byte cap"),
        };
        assert!(
            matches!(error, SourceError::Validation(message) if message.contains("size limit"))
        );
        assert!(
            std::fs::metadata(&archive_path)
                .expect("partial archive")
                .len()
                <= 1,
            "the capped transport must not write beyond its byte limit"
        );
        std::fs::remove_file(&archive_path).expect("remove capped archive");

        let output = run_git_to_file(&args, LOCAL_OP_TIMEOUT_SECONDS, 64 * 1024, &archive_path)
            .expect("archive within the cap");
        assert!(
            output.stdout.is_empty(),
            "file-backed Git output must not be retained in memory"
        );
    }

    #[test]
    fn file_backed_git_output_rejects_a_symlink_destination() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = fixture_repo(
            temp.path(),
            "fixture",
            &[("SKILL.md", "---\nname: fixture\n---\n")],
        );
        let mirror = temp.path().join("cache/git/fixture");
        let source = SystemGitSource::new();
        source
            .fetch_mirror(&file_url(&repo), &mirror)
            .expect("fetch mirror");
        let commit = source
            .resolve_commit(&mirror, "main")
            .expect("resolve")
            .expect("present");
        let archive_path = temp.path().join("staging/archive.zip");
        let outside = temp.path().join("outside.bin");
        std::fs::create_dir_all(archive_path.parent().expect("archive parent"))
            .expect("create archive parent");
        std::fs::write(&outside, b"preserve me").expect("write outside file");
        std::os::unix::fs::symlink(&outside, &archive_path).expect("create archive symlink");
        let args = [
            "-C",
            mirror.to_str().expect("mirror path"),
            "archive",
            "--format=zip",
            commit.as_str(),
        ];

        let error = match run_git_to_file(&args, LOCAL_OP_TIMEOUT_SECONDS, 64 * 1024, &archive_path)
        {
            Err(error) => error,
            Ok(_) => panic!("a symlink archive destination must be rejected"),
        };
        assert!(matches!(error, SourceError::Io { .. }));
        assert_eq!(
            std::fs::read(&outside).expect("read outside file"),
            b"preserve me"
        );
    }

    #[test]
    fn lists_tags_with_peeled_commits_and_creation_time() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = fixture_repo(
            temp.path(),
            "fixture",
            &[("SKILL.md", "---\nname: fixture\n---\n")],
        );
        let mirror = temp.path().join("cache/git/fixture");
        let source = SystemGitSource::new();
        source
            .fetch_mirror(&file_url(&repo), &mirror)
            .expect("fetch mirror");
        let output = Command::new("git")
            .args(["tag", "-a", "v1.0.0", "-m", "release one"])
            .current_dir(&repo)
            .output()
            .expect("annotated tag");
        assert!(output.status.success());
        let output = Command::new("git")
            .args(["tag", "lightweight"])
            .current_dir(&repo)
            .output()
            .expect("lightweight tag");
        assert!(output.status.success());
        source
            .fetch_mirror(&file_url(&repo), &mirror)
            .expect("refetch mirror");
        let tags = source.list_tags(&mirror).expect("list tags");
        let names = tags.iter().map(|tag| tag.name.as_str()).collect::<Vec<_>>();
        assert!(names.contains(&"v1.0.0"));
        assert!(names.contains(&"lightweight"));
        for tag in &tags {
            assert_eq!(tag.commit.len(), 40);
            assert!(
                tag.created_epoch_secs.is_some(),
                "tags carry a creation time"
            );
        }
        let main = source
            .resolve_commit(&mirror, "main")
            .expect("resolve")
            .expect("present");
        let v1 = tags
            .iter()
            .find(|tag| tag.name == "v1.0.0")
            .expect("v1 tag");
        assert_eq!(v1.commit, main);
        assert!(
            source
                .is_ancestor(&mirror, "v1.0.0", "main")
                .expect("ancestor")
        );
    }

    #[test]
    fn ancestry_reports_unrelated_or_future_commits_as_false() {
        let temp = tempfile::tempdir().expect("tempdir");
        let repo = fixture_repo(
            temp.path(),
            "fixture",
            &[("SKILL.md", "---\nname: fixture\n---\n")],
        );
        let mirror = temp.path().join("cache/git/fixture");
        let source = SystemGitSource::new();
        source
            .fetch_mirror(&file_url(&repo), &mirror)
            .expect("fetch mirror");
        let main = source
            .resolve_commit(&mirror, "main")
            .expect("resolve")
            .expect("present");
        let tag_commit = "0123456789abcdef0123456789abcdef01234567";
        // A missing ref is a closed error, not a silent "not an ancestor".
        assert!(
            source.is_ancestor(&mirror, tag_commit, &main).is_err(),
            "unknown descent refs fail closed"
        );
        assert!(
            source.is_ancestor(&mirror, "v999.0.0", "main").is_err(),
            "unknown ancestor refs fail closed"
        );
    }
}
