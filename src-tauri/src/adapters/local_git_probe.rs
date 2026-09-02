//! System local Git probe (zero network): reads `.git` directories or
//! `gitdir:` files and parses remotes/`HEAD` directly from local metadata.
//! A missing/unsafe gitdir target or unreadable metadata is typed
//! `uninterpretable` — the probe never guesses repository identity and never
//! spawns a Git process.

use std::fs;
use std::path::{Path, PathBuf};

use crate::seams::local_git_probe::{
    GitdirKind, LocalGitProbe, LocalGitProbeError, RemoteUrlEvidence, WorktreeHint,
};

pub struct SystemLocalGitProbe;

impl SystemLocalGitProbe {
    fn gitdir_at(&self, path: &Path) -> Result<Option<GitdirKind>, LocalGitProbeError> {
        let gitdir_path = path.join(".git");
        let metadata = match fs::symlink_metadata(&gitdir_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(LocalGitProbeError::Io(error.to_string())),
        };
        if metadata.is_dir() {
            return Ok(Some(GitdirKind::Directory));
        }
        if metadata.file_type().is_symlink() {
            // A symlink `.git` can point anywhere; it is a metadata shape we
            // do not interpret as a repository root here.
            return Ok(Some(GitdirKind::Invalid));
        }
        // A text file expected to hold `gitdir: <target>` (worktree).
        let content = fs::read_to_string(&gitdir_path)
            .map_err(|error| LocalGitProbeError::Io(error.to_string()))?;
        let Some(target) = content.trim().strip_prefix("gitdir:") else {
            return Ok(Some(GitdirKind::Invalid));
        };
        let target = target.trim();
        let mut resolved = PathBuf::from(target);
        if resolved.is_relative() {
            resolved = path.join(&resolved);
        }
        let Some(name) = resolved.file_name().and_then(|name| name.to_str()) else {
            return Ok(Some(GitdirKind::Invalid));
        };
        if name.is_empty() {
            return Ok(Some(GitdirKind::Invalid));
        }
        if fs::symlink_metadata(&resolved).is_err() {
            // Missing gitdir target: worktree metadata exists but cannot be
            // interpreted as a repository root.
            return Ok(Some(GitdirKind::Invalid));
        }
        Ok(Some(GitdirKind::Gitfile { target: resolved }))
    }

    fn read_remotes(&self, repository_root: &Path) -> Vec<RemoteUrlEvidence> {
        let config_path = repository_root.join(".git").join("config");
        let config = match fs::read_to_string(&config_path) {
            Ok(config) => config,
            Err(_) => return Vec::new(),
        };
        let mut remotes = Vec::new();
        let mut current: Option<String> = None;
        let mut current_url: Option<String> = None;
        let take_current = |current: &mut Option<String>,
                            url: &mut Option<String>,
                            out: &mut Vec<RemoteUrlEvidence>| {
            if let (Some(name), Some(url)) = (current.take(), url.take()) {
                out.push(RemoteUrlEvidence { name, url });
            }
        };
        for line in config.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("[remote \"") {
                if let Some(name) = rest.strip_suffix("\"]") {
                    take_current(&mut current, &mut current_url, &mut remotes);
                    current = Some(name.to_string());
                }
            } else if let Some(rest) = line.strip_prefix("url = ") {
                if current.is_some() {
                    current_url = Some(rest.trim().to_string());
                }
            }
        }
        take_current(&mut current, &mut current_url, &mut remotes);
        remotes
    }

    fn read_head(&self, repository_root: &Path) -> Option<String> {
        let head_path = repository_root.join(".git").join("HEAD");
        let content = fs::read_to_string(head_path).ok()?;
        let head = content.trim();
        // Detached HEAD: the commit sha is not an interview hint of a
        // branch; record nothing rather than guessing.
        head.strip_prefix("ref: ")
            .map(|rest| rest.trim().to_string())
    }
}

impl LocalGitProbe for SystemLocalGitProbe {
    fn probe_worktree(
        &self,
        entry: &Path,
        originating_root: &Path,
    ) -> Result<Option<WorktreeHint>, LocalGitProbeError> {
        if !entry.starts_with(originating_root) {
            return Err(LocalGitProbeError::OutsideRoot);
        }
        let mut level = entry.to_path_buf();
        loop {
            let gitdir = self.gitdir_at(&level)?;
            if let Some(gitdir) = gitdir {
                // The `.git` marker was found inside the containment; the
                // repository root is this level. The marker itself may be
                // uninterpretable — the typed result is never a guess.
                let uninterpretable = match gitdir {
                    GitdirKind::Invalid => Some("uninterpretable gitdir metadata".to_owned()),
                    _ => None,
                };
                return Ok(Some(WorktreeHint {
                    repository_root: level.clone(),
                    gitdir_kind: gitdir,
                    remote_urls: if uninterpretable.is_some() {
                        Vec::new()
                    } else {
                        self.read_remotes(&level)
                    },
                    head_ref: if uninterpretable.is_some() {
                        None
                    } else {
                        self.read_head(&level)
                    },
                    uninterpretable,
                }));
            }
            if level == originating_root {
                return Ok(None);
            }
            let Some(parent) = level.parent() else {
                return Ok(None);
            };
            if parent == level {
                return Ok(None);
            }
            level = parent.to_path_buf();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_repo(dir: &Path, name: &str) -> PathBuf {
        let repo = dir.join(name);
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::write(
            repo.join(".git/config"),
            "[remote \"origin\"]\n\turl = https://github.com/example/repo.git\n[remote \"upstream\"]\n\turl = https://gitlab.com/example/repo.git\n",
        )
        .unwrap();
        fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        repo
    }

    #[test]
    fn probe_finds_nearest_repo_within_root_and_stops_at_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = setup_repo(temp.path(), "root-skills");
        let entry = root.join("alpha-skills");
        fs::create_dir_all(&entry).unwrap();
        let probe = SystemLocalGitProbe;
        let hint = probe
            .probe_worktree(&entry, &root)
            .expect("probe")
            .expect("hint");
        assert_eq!(hint.repository_root, root);
        assert_eq!(hint.gitdir_kind, GitdirKind::Directory);
        assert_eq!(hint.head_ref.as_deref(), Some("refs/heads/main"));
        assert_eq!(hint.remote_urls.len(), 2);
        // The walk stops at the originating Root: a repo above it is ambient.
        let ambient = setup_repo(temp.path(), "projects");
        let inside = ambient.join("skills").join("entry");
        fs::create_dir_all(&inside).unwrap();
        let probe = SystemLocalGitProbe;
        let None = probe
            .probe_worktree(&inside, &ambient.join("skills"))
            .expect("probe")
        else {
            panic!("a repository above the Root must not be reported");
        };
        let _ = ambient;
    }

    #[test]
    fn gitfile_worktree_shape_is_interpreted_and_broken_shapes_are_typed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("worktree-skills");
        fs::create_dir_all(&root).unwrap();
        let gitdir = temp.path().join("shared-gitdir");
        fs::create_dir_all(&gitdir).unwrap();
        fs::write(root.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        let probe = SystemLocalGitProbe;
        let hint = probe
            .probe_worktree(&root, &root)
            .expect("probe")
            .expect("hint");
        assert!(matches!(hint.gitdir_kind, GitdirKind::Gitfile { .. }));
        assert!(hint.is_valid());

        // Broken gitdir file → typed uninterpretable, never guessed.
        fs::write(root.join(".git"), "gitdir: /missing/gitdir\n").unwrap();
        let hint = probe
            .probe_worktree(&root, &root)
            .expect("probe")
            .expect("hint");
        assert_eq!(hint.gitdir_kind, GitdirKind::Invalid);
        assert!(hint.uninterpretable.is_some());
    }

    #[test]
    fn outside_root_is_rejected() {
        let probe = SystemLocalGitProbe;
        assert!(matches!(
            probe.probe_worktree(Path::new("/tmp/entry"), Path::new("/tmp/root")),
            Err(LocalGitProbeError::OutsideRoot)
        ));
    }
}
