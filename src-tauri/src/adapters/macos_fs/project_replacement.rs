//! Same-parent, exclusive renames keep Agent originals on their own volume.
use super::*;
use crate::seams::filesystem::ProjectLinkJournal;
const OP: &str = "replace project Agent entry";
fn io(path: &Path) -> FileSystemError {
    FileSystemError::Io {
        operation: OP,
        path: path.into(),
        source: std::io::Error::last_os_error(),
    }
}

pub(super) fn hash(path: &Path) -> Result<String, FileSystemError> {
    let parent_path = path.parent().ok_or_else(|| stale_tree_entry(path))?;
    let (parent, _) = open_absolute_directory_chain_nofollow(parent_path, OP)?;
    let name = cstring_path_component(path.file_name(), path, OP)?;
    let mut hash = Sha256::new();
    let mut budget = 128 * 1024 * 1024;
    hash_at(&parent, &name, path, &mut hash, &mut budget, 0)?;
    Ok(format!("{:x}", hash.finalize()))
}
fn hash_at(
    parent: &OwnedFd,
    name: &CString,
    path: &Path,
    hash: &mut Sha256,
    budget: &mut usize,
    depth: usize,
) -> Result<(), FileSystemError> {
    if depth > 128 || *budget == 0 {
        return Err(stale_tree_entry(path));
    }
    *budget -= 1;
    let stat = metadata_at_nofollow(parent, name, path, OP)?;
    hash_field(hash, &stat.st_mode.to_le_bytes());
    hash_field(hash, &stat.st_dev.to_le_bytes());
    hash_field(hash, &stat.st_ino.to_le_bytes());
    match stat.st_mode & libc::S_IFMT {
        libc::S_IFDIR => {
            let (child, opened) = open_directory_at_nofollow(parent, name, path, OP)?;
            if opened.st_ino != stat.st_ino || opened.st_dev != stat.st_dev {
                return Err(stale_tree_entry(path));
            }
            let mut names = directory_entry_names(&child, path, OP)?;
            names.sort();
            for name in names {
                hash_field(hash, name.as_bytes());
                let encoded = cstring_path_component(Some(&name), &path.join(&name), OP)?;
                hash_at(&child, &encoded, &path.join(name), hash, budget, depth + 1)?;
            }
        }
        libc::S_IFLNK => hash_field(
            hash,
            read_link_at(parent, name, path, OP)?.as_os_str().as_bytes(),
        ),
        libc::S_IFREG => {
            let raw = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            if raw < 0 {
                return Err(io(path));
            }
            let mut file = unsafe { fs::File::from_raw_fd(raw) };
            let before = file.metadata().map_err(|_| stale_tree_entry(path))?;
            if before.ino() != stat.st_ino
                || before.dev() != stat.st_dev as u64
                || !before.is_file()
            {
                return Err(stale_tree_entry(path));
            }
            let mut bytes = vec![];
            (&mut file)
                .take((*budget + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| stale_tree_entry(path))?;
            let after = file.metadata().map_err(|_| stale_tree_entry(path))?;
            if bytes.len() > *budget
                || before.mtime() != after.mtime()
                || before.mtime_nsec() != after.mtime_nsec()
                || before.len() != after.len()
            {
                return Err(stale_tree_entry(path));
            }
            *budget -= bytes.len();
            hash_field(hash, &bytes);
        }
        _ => return Err(stale_tree_entry(path)),
    }
    let after = metadata_at_nofollow(parent, name, path, OP)?;
    if after.st_ino != stat.st_ino
        || after.st_dev != stat.st_dev
        || after.st_mtime != stat.st_mtime
        || after.st_mtime_nsec != stat.st_mtime_nsec
    {
        return Err(stale_tree_entry(path));
    }
    Ok(())
}
fn parent(fs: &MacOsFileSystem, link: &ProjectLinkJournal) -> Result<OwnedFd, FileSystemError> {
    if fs.directory_fingerprint(&link.project_root.canonical_path)? != link.project_root
        || fs.resolve_project_target(
            &link.project_root.canonical_path,
            &link.configured_relative_path,
        )? != link.resolution
        || link.entry_path.parent() != Some(link.parent.canonical_path.as_path())
    {
        return Err(stale_tree_entry(&link.entry_path));
    }
    if let Some(replacement) = &link.replacement {
        if replacement.backup_path.parent() != Some(link.parent.canonical_path.as_path())
            || replacement.backup_path == link.entry_path
            || replacement
                .backup_path
                .file_name()
                .and_then(|n| n.to_str())
                .is_none_or(|n| !n.starts_with(".skillman-") || !n.ends_with("-backup"))
        {
            return Err(stale_tree_entry(&replacement.backup_path));
        }
    }
    let (parent, stat) = open_absolute_directory_chain_nofollow(&link.parent.canonical_path, OP)?;
    if stat.st_ino != link.parent.inode || stat.st_dev as u64 != link.parent.device {
        return Err(stale_tree_entry(&link.entry_path));
    }
    Ok(parent)
}
fn rename(parent: &OwnedFd, from: &Path, to: &Path) -> Result<(), FileSystemError> {
    let a = cstring_path_component(from.file_name(), from, OP)?;
    let b = cstring_path_component(to.file_name(), to, OP)?;
    if unsafe {
        libc::renameatx_np(
            parent.as_raw_fd(),
            a.as_ptr(),
            parent.as_raw_fd(),
            b.as_ptr(),
            libc::RENAME_EXCL,
        )
    } != 0
    {
        return Err(io(to));
    }
    sync_descriptor(parent, to.parent().ok_or_else(|| stale_tree_entry(to))?, OP)
}
fn original(
    fs: &MacOsFileSystem,
    link: &ProjectLinkJournal,
    path: &Path,
) -> Result<(), FileSystemError> {
    let r = link
        .replacement
        .as_ref()
        .ok_or_else(|| stale_tree_entry(path))?;
    if fs.occupant_snapshot(path)? != r.original || hash(path)? != r.content_hash {
        return Err(stale_tree_entry(path));
    }
    Ok(())
}
pub(super) fn backup(
    fs: &MacOsFileSystem,
    link: &ProjectLinkJournal,
) -> Result<(), FileSystemError> {
    let parent = parent(fs, link)?;
    let r = link
        .replacement
        .as_ref()
        .ok_or_else(|| stale_tree_entry(&link.entry_path))?;
    original(fs, link, &link.entry_path)?;
    rename(&parent, &link.entry_path, &r.backup_path)?;
    original(fs, link, &r.backup_path)
}
pub(super) fn undo(fs: &MacOsFileSystem, link: &ProjectLinkJournal) -> Result<(), FileSystemError> {
    let parent = parent(fs, link)?;
    let r = link
        .replacement
        .as_ref()
        .ok_or_else(|| stale_tree_entry(&link.entry_path))?;
    if fs.activation_snapshot(&r.backup_path)? == ActivationEntrySnapshot::Missing {
        return original(fs, link, &link.entry_path);
    }
    original(fs, link, &r.backup_path)?;
    // Reuse the identity-bound unlink for the new link; a missing entry is also safe.
    let mut plain = link.clone();
    plain.replacement = None;
    fs.undo_project_link(&plain)?;
    rename(&parent, &r.backup_path, &link.entry_path)?;
    original(fs, link, &link.entry_path)
}
pub(super) fn finalize(
    fs: &MacOsFileSystem,
    link: &ProjectLinkJournal,
    persist: &mut dyn FnMut(&ProjectLinkJournal) -> Result<(), FileSystemError>,
) -> Result<(), FileSystemError> {
    let Some(r) = &link.replacement else {
        return Ok(());
    };
    if link.undone {
        return Ok(());
    }
    if link.phase != ActivationReplacePhase::Committed {
        return Err(stale_tree_entry(&r.backup_path));
    }
    let parent = parent(fs, link)?;
    if fs.activation_snapshot(&r.backup_path)? == ActivationEntrySnapshot::Missing {
        return Ok(());
    }
    if !r.cleanup_authorized {
        if link.occupant.as_ref() != Some(&fs.occupant_snapshot(&link.entry_path)?) {
            return Err(stale_tree_entry(&link.entry_path));
        }
        original(fs, link, &r.backup_path)?;
        let mut updated = link.clone();
        updated.replacement.as_mut().unwrap().cleanup_authorized = true;
        persist(&updated)?;
    }
    // After a persisted cleanup decision, tolerate partial deletion but still pin the root inode.
    if fs.occupant_snapshot(&r.backup_path)? != r.original {
        return Err(stale_tree_entry(&r.backup_path));
    }
    let name = cstring_path_component(r.backup_path.file_name(), &r.backup_path, OP)?;
    if r.original.kind == OccupantKind::RealDirectory {
        remove_child_directory_at(
            &parent,
            r.original.device as libc::dev_t,
            r.original.inode,
            &name,
            &r.backup_path,
            OP,
        )?;
    } else if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(io(&r.backup_path));
    }
    sync_descriptor(&parent, &link.parent.canonical_path, OP)
}
