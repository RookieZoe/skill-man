//! Descriptor-bound portable payload delivery. Shared filesystem adapter helpers
//! keep project writes below the observed root and publish with RENAME_EXCL.
use super::*;
use crate::seams::filesystem::{
    ProjectCopyEntry, ProjectCopyEntryKind, ProjectCopyJournal, ProjectCopyPayload,
};

const OP: &str = "deliver project Skill copy";
fn invalid(path: &Path, message: &str) -> FileSystemError {
    FileSystemError::Io {
        operation: OP,
        path: path.into(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, message),
    }
}
fn io(path: &Path, error: std::io::Error) -> FileSystemError {
    FileSystemError::Io {
        operation: OP,
        path: path.into(),
        source: error,
    }
}
fn fingerprint(path: &Path, metadata: &libc::stat) -> DirectoryFingerprint {
    DirectoryFingerprint {
        canonical_path: path.into(),
        device: metadata.st_dev as u64,
        inode: metadata.st_ino,
    }
}
fn relative_link(from: &Path, to: &Path) -> PathBuf {
    let a: Vec<_> = from.components().collect();
    let b: Vec<_> = to.components().collect();
    let shared = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut result = PathBuf::new();
    for _ in shared..a.len() {
        result.push("..");
    }
    for item in &b[shared..] {
        result.push(item.as_os_str());
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    result
}

pub(super) fn payload(path: &Path, reuse: bool) -> Result<ProjectCopyPayload, FileSystemError> {
    let (root, stat) = open_absolute_directory_chain_nofollow(path, OP)?;
    let mut entries = Vec::new();
    let mut size = 0;
    snapshot(&root, path, Path::new(""), reuse, &mut entries, &mut size)?;
    let document = resolve_payload_link(path, &path.join("SKILL.md"), reuse)?;
    let document = document
        .strip_prefix(path)
        .map_err(|_| invalid(path, "invalid Skill document"))?;
    if !entries.iter().any(|e| e.path == document && matches!(&e.kind, ProjectCopyEntryKind::File(bytes) if !bytes.is_empty() && bytes.len() <= MAX_SKILL_DOCUMENT_BYTES as usize && std::str::from_utf8(bytes).is_ok())) {
        return Err(invalid(path, "a readable UTF-8 SKILL.md is required"));
    }
    reject_directory_cycles(path, &entries)?;
    Ok(ProjectCopyPayload {
        root_mode: u32::from(stat.st_mode & 0o777),
        root: fingerprint(path, &stat),
        entries,
    })
}
fn snapshot(
    fd: &OwnedFd,
    root: &Path,
    relative: &Path,
    reuse: bool,
    entries: &mut Vec<ProjectCopyEntry>,
    size: &mut usize,
) -> Result<(), FileSystemError> {
    let display = root.join(relative);
    if relative.components().count() > 128 || entries.len() > 100_000 {
        return Err(invalid(&display, "payload nesting or entry limit exceeded"));
    }
    let mut names = directory_entry_names(fd, &display, OP)?;
    names.sort();
    for name in names {
        if !reuse && name == ".git" {
            continue;
        }
        let rel = relative.join(&name);
        let path = root.join(&rel);
        let encoded = cstring_path_component(Some(&name), &path, OP)?;
        let stat = metadata_at_nofollow(fd, &encoded, &path, OP)?;
        let mode = u32::from(stat.st_mode & 0o777);
        let kind = match stat.st_mode & libc::S_IFMT {
            libc::S_IFDIR => {
                let (child, opened) = open_directory_at_nofollow(fd, &encoded, &path, OP)?;
                if stat.st_dev != opened.st_dev || stat.st_ino != opened.st_ino {
                    return Err(stale_tree_entry(&path));
                }
                entries.push(ProjectCopyEntry {
                    device: stat.st_dev as u64,
                    inode: stat.st_ino,
                    path: rel.clone(),
                    mode,
                    kind: ProjectCopyEntryKind::Directory,
                });
                snapshot(&child, root, &rel, reuse, entries, size)?;
                continue;
            }
            libc::S_IFREG => {
                if stat.st_size < 0 || stat.st_size as usize > 128 * 1024 * 1024 - *size {
                    return Err(invalid(&path, "payload exceeds 128 MiB preview limit"));
                }
                let raw = unsafe {
                    libc::openat(
                        fd.as_raw_fd(),
                        encoded.as_ptr(),
                        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                    )
                };
                if raw < 0 {
                    return Err(io(&path, std::io::Error::last_os_error()));
                }
                let mut file = unsafe { fs::File::from_raw_fd(raw) };
                let before = file.metadata().map_err(|e| io(&path, e))?;
                if before.dev() != stat.st_dev as u64
                    || before.ino() != stat.st_ino
                    || !before.is_file()
                {
                    return Err(stale_tree_entry(&path));
                }
                let mut bytes = vec![];
                (&mut file)
                    .take((128 * 1024 * 1024 - *size + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|e| io(&path, e))?;
                let after = file.metadata().map_err(|e| io(&path, e))?;
                if bytes.len() > 128 * 1024 * 1024 - *size
                    || before.len() != after.len()
                    || before.mtime() != after.mtime()
                    || before.mtime_nsec() != after.mtime_nsec()
                {
                    return Err(stale_tree_entry(&path));
                }
                *size += bytes.len();
                ProjectCopyEntryKind::File(bytes)
            }
            libc::S_IFLNK => {
                let raw = read_link_at(fd, &encoded, &path, OP)?;
                let target = resolve_payload_link(root, &path, reuse)?;
                let included = target
                    .strip_prefix(root)
                    .map_err(|_| invalid(&path, "payload link leaves the Skill"))?;
                if (!reuse && included.components().any(|c| c.as_os_str() == ".git"))
                    || target == root
                    || path.starts_with(&target)
                {
                    return Err(invalid(
                        &path,
                        "payload link targets excluded content or creates a directory cycle",
                    ));
                }
                if reuse && raw.is_absolute() {
                    return Err(invalid(
                        &path,
                        "existing payload contains a nonportable absolute link",
                    ));
                }
                ProjectCopyEntryKind::Link(if reuse {
                    raw
                } else {
                    relative_link(relative, included)
                })
            }
            _ => return Err(invalid(&path, "unsupported special payload object")),
        };
        if metadata_at_nofollow(fd, &encoded, &path, OP)?.st_ino != stat.st_ino {
            return Err(stale_tree_entry(&path));
        }
        entries.push(ProjectCopyEntry {
            device: stat.st_dev as u64,
            inode: stat.st_ino,
            path: rel,
            mode,
            kind,
        });
    }
    Ok(())
}

fn pinned_root(expected: &DirectoryFingerprint) -> Result<OwnedFd, FileSystemError> {
    let (fd, stat) = open_absolute_directory_chain_nofollow(&expected.canonical_path, OP)?;
    if fingerprint(&expected.canonical_path, &stat) != *expected {
        return Err(stale_tree_entry(&expected.canonical_path));
    }
    Ok(fd)
}
fn parent(journal: &ProjectCopyJournal) -> Result<OwnedFd, FileSystemError> {
    let root = pinned_root(&journal.project_root)?;
    if resolve_project_skills_dir(
        &journal.project_root.canonical_path,
        Path::new(".agents/skills"),
    ) != journal.target_resolution
    {
        return Err(stale_tree_entry(&journal.entry_path));
    }
    let (fd, stat) = open_project_relative_directory_nofollow(
        &root,
        &journal.project_root.canonical_path,
        &journal.parent.canonical_path,
        OP,
    )?;
    if fingerprint(&journal.parent.canonical_path, &stat) != journal.parent
        || journal.entry_path.parent() != Some(journal.parent.canonical_path.as_path())
        || journal.staging_path.parent() != Some(journal.parent.canonical_path.as_path())
    {
        return Err(stale_tree_entry(&journal.entry_path));
    }
    Ok(fd)
}
pub(super) fn prepare(
    root: &DirectoryFingerprint,
    resolution: &ProjectTargetResolution,
) -> Result<DirectoryFingerprint, FileSystemError> {
    let fd = pinned_root(root)?;
    if resolve_project_skills_dir(&root.canonical_path, Path::new(".agents/skills")) != *resolution
    {
        return Err(stale_tree_entry(&resolution.resolved_container));
    }
    if resolution.fault.is_some() {
        return Err(stale_tree_entry(&resolution.resolved_container));
    }
    let relative = resolution
        .resolved_container
        .strip_prefix(&root.canonical_path)
        .map_err(|_| stale_tree_entry(&resolution.resolved_container))?;
    let mut current = fd;
    let mut path = root.canonical_path.clone();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(stale_tree_entry(&path));
        };
        path.push(name);
        let encoded = cstring_path_component(Some(name), &path, OP)?;
        if resolution.create_steps.contains(&path) {
            if unsafe { libc::mkdirat(current.as_raw_fd(), encoded.as_ptr(), 0o755) } != 0 {
                return Err(io(&path, std::io::Error::last_os_error()));
            }
            sync_descriptor(&current, path.parent().unwrap(), OP)?;
        }
        let (next, stat) = open_directory_at_nofollow(&current, &encoded, &path, OP)?;
        if let Some(hop) = resolution
            .hops
            .iter()
            .find(|hop| hop.path == path && matches!(hop.kind, EvidenceChainHopKind::Directory))
        {
            if stat.st_dev as u64 != hop.device || stat.st_ino != hop.inode {
                return Err(stale_tree_entry(&path));
            }
        }
        current = next;
    }
    Ok(fingerprint(
        &path,
        &directory_descriptor_metadata(&current, &path)?,
    ))
}
pub(super) fn stage(
    journal: &ProjectCopyJournal,
    payload: &ProjectCopyPayload,
) -> Result<DirectoryFingerprint, FileSystemError> {
    // Source is checked again at the write boundary; only frozen bytes are emitted.
    if self::payload(&payload.root.canonical_path, false)? != *payload {
        return Err(stale_tree_entry(&payload.root.canonical_path));
    }
    let parent = parent(journal)?;
    let path = &journal.staging_path;
    let name = cstring_path_component(path.file_name(), path, OP)?;
    let (root, stat) = open_directory_at_nofollow(&parent, &name, path, OP)?;
    if journal.staged_identity.as_ref() != Some(&fingerprint(path, &stat)) {
        return Err(stale_tree_entry(path));
    }
    for entry in &payload.entries {
        let destination = path.join(&entry.path);
        let (container, _) = open_project_relative_directory_nofollow(
            &root,
            path,
            destination.parent().unwrap(),
            OP,
        )?;
        let name = cstring_path_component(destination.file_name(), &destination, OP)?;
        let status = match &entry.kind {
            ProjectCopyEntryKind::Directory => unsafe {
                libc::mkdirat(container.as_raw_fd(), name.as_ptr(), 0o700)
            },
            ProjectCopyEntryKind::Link(target) => {
                let target = CString::new(target.as_os_str().as_bytes())
                    .map_err(|_| invalid(&destination, "invalid link text"))?;
                unsafe { libc::symlinkat(target.as_ptr(), container.as_raw_fd(), name.as_ptr()) }
            }
            ProjectCopyEntryKind::File(bytes) => {
                let fd = unsafe {
                    libc::openat(
                        container.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_WRONLY
                            | libc::O_CREAT
                            | libc::O_EXCL
                            | libc::O_NOFOLLOW
                            | libc::O_CLOEXEC,
                        0o600,
                    )
                };
                if fd < 0 {
                    return Err(io(&destination, std::io::Error::last_os_error()));
                }
                let mut file = unsafe { fs::File::from_raw_fd(fd) };
                file.write_all(bytes).map_err(|e| io(&destination, e))?;
                if unsafe { libc::fchmod(file.as_raw_fd(), entry.mode as libc::mode_t) } != 0 {
                    return Err(io(&destination, std::io::Error::last_os_error()));
                }
                file.sync_all().map_err(|e| io(&destination, e))?;
                0
            }
        };
        if status != 0 {
            return Err(io(&destination, std::io::Error::last_os_error()));
        }
        sync_descriptor(&container, destination.parent().unwrap(), OP)?;
    }
    for entry in payload
        .entries
        .iter()
        .rev()
        .filter(|e| matches!(e.kind, ProjectCopyEntryKind::Directory))
    {
        let dest = path.join(&entry.path);
        let (fd, _) = open_project_relative_directory_nofollow(&root, path, &dest, OP)?;
        if unsafe { libc::fchmod(fd.as_raw_fd(), entry.mode as libc::mode_t) } != 0 {
            return Err(io(&dest, std::io::Error::last_os_error()));
        }
        sync_descriptor(&fd, &dest, OP)?;
    }
    if unsafe { libc::fchmod(root.as_raw_fd(), payload.root_mode as libc::mode_t) } != 0 {
        return Err(io(path, std::io::Error::last_os_error()));
    }
    sync_descriptor(&root, path, OP)?;
    sync_descriptor(&parent, &journal.parent.canonical_path, OP)?;
    let staged = self::payload(path, true)?;
    if staged.entries.len() != payload.entries.len()
        || !staged
            .entries
            .iter()
            .zip(&payload.entries)
            .all(|(a, b)| a.path == b.path && a.mode == b.mode && a.kind == b.kind)
    {
        return Err(stale_tree_entry(path));
    }
    Ok(fingerprint(path, &stat))
}
pub(super) fn publish(journal: &ProjectCopyJournal) -> Result<(), FileSystemError> {
    let parent = parent(journal)?;
    verify_artifact(journal, &journal.staging_path)?;
    let from = cstring_path_component(journal.staging_path.file_name(), &journal.staging_path, OP)?;
    let to = cstring_path_component(journal.entry_path.file_name(), &journal.entry_path, OP)?;
    if unsafe {
        libc::renameatx_np(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_EXCL,
        )
    } != 0
    {
        return Err(io(&journal.entry_path, std::io::Error::last_os_error()));
    }
    sync_descriptor(&parent, &journal.parent.canonical_path, OP)
}
fn verify_artifact(
    journal: &ProjectCopyJournal,
    path: &Path,
) -> Result<DirectoryFingerprint, FileSystemError> {
    let _parent = parent(journal)?;
    let (_, stat) = open_absolute_directory_chain_nofollow(path, OP)?;
    let expected = journal
        .staged_identity
        .as_ref()
        .ok_or_else(|| invalid(path, "artifact identity is unknown; recovery required"))?;
    let actual = fingerprint(path, &stat);
    if actual.device != expected.device
        || actual.inode != expected.inode
        || journal.content_hash.as_ref() != Some(&artifact_hash(path)?)
    {
        return Err(stale_tree_entry(path));
    }
    Ok(actual)
}
pub(super) fn recover(
    journal: &ProjectCopyJournal,
    persist_cleanup: &mut dyn FnMut(&ProjectCopyJournal) -> Result<(), FileSystemError>,
) -> Result<(), FileSystemError> {
    // Completed copies belong to the project. Restart closes Undo, independent
    // of subsequent project edits, moves, removal, or source availability.
    if journal.phase == ActivationReplacePhase::Committed {
        return Ok(());
    }
    let parent = parent(journal)?;
    // Quarantine a still-published copy before recursive deletion. A crash
    // can leave only a private stage, never a half-deleted visible Skill.
    let entry_name =
        cstring_path_component(journal.entry_path.file_name(), &journal.entry_path, OP)?;
    match metadata_at_nofollow(&parent, &entry_name, &journal.entry_path, OP) {
        Ok(stat) => {
            let ours = journal
                .staged_identity
                .as_ref()
                .is_some_and(|s| s.device == stat.st_dev as u64 && s.inode == stat.st_ino);
            if ours {
                verify_artifact(journal, &journal.entry_path)?;
                let stage_name = cstring_path_component(
                    journal.staging_path.file_name(),
                    &journal.staging_path,
                    OP,
                )?;
                if unsafe {
                    libc::renameatx_np(
                        parent.as_raw_fd(),
                        entry_name.as_ptr(),
                        parent.as_raw_fd(),
                        stage_name.as_ptr(),
                        libc::RENAME_EXCL,
                    )
                } != 0
                {
                    return Err(io(&journal.entry_path, std::io::Error::last_os_error()));
                }
                sync_descriptor(&parent, &journal.parent.canonical_path, OP)?;
            } else if journal.phase != ActivationReplacePhase::Applying {
                return Err(stale_tree_entry(&journal.entry_path));
            }
        }
        Err(FileSystemError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let path = &journal.staging_path;
    let name = cstring_path_component(path.file_name(), path, OP)?;
    let stat = match metadata_at_nofollow(&parent, &name, path, OP) {
        Ok(stat) => stat,
        Err(FileSystemError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    if journal.staged_identity.as_ref().is_none_or(|s| {
        s.device != stat.st_dev as u64
            || s.inode != stat.st_ino
            || stat.st_mode & libc::S_IFMT != libc::S_IFDIR
    }) {
        return Err(invalid(
            path,
            "artifact identity is unknown or changed; recovery required",
        ));
    }
    if !journal.cleanup_authorized {
        if journal.content_hash.is_some() {
            verify_artifact(journal, path)?;
        }
        let mut cleanup = journal.clone();
        cleanup.cleanup_authorized = true;
        persist_cleanup(&cleanup)?;
    }
    let (stage, opened) = open_directory_at_nofollow(&parent, &name, path, OP)?;
    if opened.st_dev != stat.st_dev || opened.st_ino != stat.st_ino {
        return Err(stale_tree_entry(path));
    }
    make_stage_removable(&stage, path, stat.st_dev)?;
    remove_child_directory_at(&parent, stat.st_dev, stat.st_ino, &name, path, OP)?;
    sync_descriptor(&parent, &journal.parent.canonical_path, OP)?;
    Ok(())
}

fn resolve_payload_link(root: &Path, path: &Path, reuse: bool) -> Result<PathBuf, FileSystemError> {
    let mut pending: VecDeque<OsString> = path
        .strip_prefix(root)
        .map_err(|_| invalid(path, "link outside Skill"))?
        .components()
        .map(|c| c.as_os_str().to_os_string())
        .collect();
    let mut current = root.to_path_buf();
    let mut visited = std::collections::HashSet::new();
    while let Some(component) = pending.pop_front() {
        if component == "." {
            continue;
        }
        if component == ".." {
            if current == root {
                return Err(invalid(path, "payload link leaves the Skill"));
            }
            current.pop();
            continue;
        }
        if !reuse && component == ".git" {
            return Err(invalid(
                path,
                "payload link traverses excluded Git metadata",
            ));
        }
        current.push(&component);
        let meta = fs::symlink_metadata(&current).map_err(|e| io(path, e))?;
        if meta.file_type().is_symlink() {
            if !visited.insert(current.clone()) || visited.len() > 64 {
                return Err(invalid(path, "cyclic payload link"));
            }
            let mut target = fs::read_link(&current).map_err(|e| io(path, e))?;
            if target.is_absolute() {
                // macOS canonical /private/var spelling is the same filesystem location.
                if let Ok(suffix) = target.strip_prefix("/var") {
                    target = Path::new("/private/var").join(suffix);
                }
                let relative = target
                    .strip_prefix(root)
                    .map_err(|_| invalid(path, "payload link leaves the Skill"))?;
                current = root.to_path_buf();
                target = relative.to_path_buf();
            } else {
                current.pop();
            }
            let mut next: VecDeque<_> = target
                .components()
                .map(|c| c.as_os_str().to_os_string())
                .collect();
            next.append(&mut pending);
            pending = next;
        } else if !pending.is_empty() && !meta.is_dir() {
            return Err(invalid(path, "link traverses a non-directory"));
        }
    }
    Ok(current)
}
fn reject_directory_cycles(
    root: &Path,
    entries: &[ProjectCopyEntry],
) -> Result<(), FileSystemError> {
    let mut graph: std::collections::HashMap<PathBuf, Vec<PathBuf>> =
        std::collections::HashMap::new();
    for entry in entries {
        let parent = entry.path.parent().unwrap_or(Path::new(""));
        match &entry.kind {
            ProjectCopyEntryKind::Directory => graph
                .entry(parent.into())
                .or_default()
                .push(entry.path.clone()),
            ProjectCopyEntryKind::Link(_) => {
                let destination = resolve_payload_link(root, &root.join(&entry.path), true)?;
                let resolved = destination
                    .strip_prefix(root)
                    .map_err(|_| invalid(root, "payload leaves root"))?
                    .to_path_buf();
                if entries.iter().any(|e| {
                    e.path == resolved && matches!(e.kind, ProjectCopyEntryKind::Directory)
                }) {
                    graph.entry(parent.into()).or_default().push(resolved);
                }
            }
            _ => {}
        }
    }
    fn visit(
        node: &Path,
        graph: &std::collections::HashMap<PathBuf, Vec<PathBuf>>,
        active: &mut std::collections::HashSet<PathBuf>,
        done: &mut std::collections::HashSet<PathBuf>,
    ) -> bool {
        if done.contains(node) {
            return true;
        }
        if !active.insert(node.into()) {
            return false;
        }
        if let Some(children) = graph.get(node) {
            for child in children {
                if !visit(child, graph, active, done) {
                    return false;
                }
            }
        }
        active.remove(node);
        done.insert(node.into());
        true
    }
    if !visit(
        Path::new(""),
        &graph,
        &mut Default::default(),
        &mut Default::default(),
    ) {
        return Err(invalid(root, "directory links create a payload cycle"));
    }
    Ok(())
}

pub(super) fn reserve(
    journal: &ProjectCopyJournal,
) -> Result<DirectoryFingerprint, FileSystemError> {
    let parent = parent(journal)?;
    let path = &journal.staging_path;
    let name = cstring_path_component(path.file_name(), path, OP)?;
    if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
        return Err(io(path, std::io::Error::last_os_error()));
    }
    let (root, stat) = open_directory_at_nofollow(&parent, &name, path, OP)?;
    sync_descriptor(&parent, &journal.parent.canonical_path, OP)?;
    sync_descriptor(&root, path, OP)?;
    Ok(fingerprint(path, &stat))
}

pub(super) fn artifact_hash(path: &Path) -> Result<String, FileSystemError> {
    let observed = payload(path, true)?;
    let mut hash = Sha256::new();
    hash_field(&mut hash, &observed.root_mode.to_le_bytes());
    for entry in &observed.entries {
        hash_field(&mut hash, entry.path.as_os_str().as_bytes());
        hash_field(&mut hash, &entry.mode.to_le_bytes());
        hash_field(&mut hash, &entry.device.to_le_bytes());
        hash_field(&mut hash, &entry.inode.to_le_bytes());
        match &entry.kind {
            ProjectCopyEntryKind::Directory => hash_field(&mut hash, b"directory"),
            ProjectCopyEntryKind::File(bytes) => {
                hash_field(&mut hash, b"file");
                hash_field(&mut hash, bytes);
            }
            ProjectCopyEntryKind::Link(target) => {
                hash_field(&mut hash, b"link");
                hash_field(&mut hash, target.as_os_str().as_bytes());
            }
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(super) fn unchanged(journal: &ProjectCopyJournal) -> bool {
    verify_artifact(journal, &journal.entry_path).is_ok()
}
fn make_stage_removable(
    fd: &OwnedFd,
    path: &Path,
    device: libc::dev_t,
) -> Result<(), FileSystemError> {
    // The hash/identity check and exclusive quarantine precede any mode change.
    // Only operation-owned staging is made writable for resumable cleanup.
    if unsafe { libc::fchmod(fd.as_raw_fd(), 0o700) } != 0 {
        return Err(io(path, std::io::Error::last_os_error()));
    }
    for name in directory_entry_names(fd, path, OP)? {
        let child = path.join(&name);
        let encoded = cstring_path_component(Some(&name), &child, OP)?;
        let stat = metadata_at_nofollow(fd, &encoded, &child, OP)?;
        if stat.st_mode & libc::S_IFMT == libc::S_IFDIR {
            if stat.st_dev != device {
                return Err(stale_tree_entry(&child));
            }
            let (opened, actual) = open_directory_at_nofollow(fd, &encoded, &child, OP)?;
            if actual.st_ino != stat.st_ino || actual.st_dev != stat.st_dev {
                return Err(stale_tree_entry(&child));
            }
            make_stage_removable(&opened, &child, device)?;
        }
    }
    Ok(())
}
