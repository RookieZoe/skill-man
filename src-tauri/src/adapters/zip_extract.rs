//! Shared ZIP materialization for source adapters.
//!
//! Both the local `.zip` file Import and the Git adapter (via `git archive
//! --format=zip`) produce staged content through this one extraction path,
//! so the traversal and size guards apply identically to every source.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStringExt;
use std::path::{Component, Path, PathBuf};

use zip::ZipArchive;

use crate::seams::source::SourceError;

// These are transport guards, not Skill validation limits. Import Core
// independently validates every discovered candidate against the stricter
// per-Skill policy.
const MAX_STAGED_ENTRY_BYTES: u64 = 256 * 1024 * 1024;
const MAX_SYMLINK_TARGET_BYTES: u64 = 4 * 1024;
pub(crate) const MAX_STAGED_SOURCE_BYTES: u64 = 100 * 256 * 1024 * 1024;

/// Extract `archive_path` into `extraction_root`, stripping `strip_prefix`
/// leading path components (used for Git archives that embed the Skill's
/// repo-relative path). Absolute paths, traversal, oversized entries and
/// unsafe symlinks are rejected.
pub fn extract_zip_archive(
    archive_path: &Path,
    extraction_root: &Path,
    strip_prefix: Option<&Path>,
) -> Result<(), SourceError> {
    fs::create_dir(extraction_root).map_err(|source| SourceError::Io {
        operation: "create ZIP extraction root",
        path: extraction_root.to_path_buf(),
        source,
    })?;
    let archive_file = fs::File::open(archive_path).map_err(|source| SourceError::Io {
        operation: "open ZIP archive",
        path: archive_path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(archive_file)
        .map_err(|error| SourceError::Validation(format!("ZIP archive is invalid: {error}")))?;
    let mut total_bytes = 0_u64;
    let mut has_symlinks = false;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            SourceError::Validation(format!("ZIP archive entry is invalid: {error}"))
        })?;
        let relative_path = entry.enclosed_name().ok_or_else(|| {
            SourceError::Validation(format!(
                "ZIP archive contains an unsafe path: {}",
                entry.name()
            ))
        })?;
        let Some(destination) = selected_entry_destination(
            entry.name(),
            &relative_path,
            extraction_root,
            strip_prefix,
        )?
        else {
            if !entry.is_dir() {
                return Err(SourceError::Validation(format!(
                    "ZIP archive entry is outside the selected tree: {}",
                    entry.name()
                )));
            }
            continue;
        };
        let is_symlink = entry
            .unix_mode()
            .is_some_and(|mode| mode & u32::from(libc::S_IFMT) == u32::from(libc::S_IFLNK));
        if entry.is_dir() {
            fs::create_dir_all(&destination).map_err(|source| SourceError::Io {
                operation: "create ZIP staging directory",
                path: destination,
                source,
            })?;
            continue;
        }
        if entry.size() > MAX_STAGED_ENTRY_BYTES {
            return Err(SourceError::Validation(format!(
                "ZIP archive entry exceeds the transport limit: {}",
                entry.name()
            )));
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|source| SourceError::Io {
                operation: "create ZIP staging parent",
                path: parent.to_path_buf(),
                source,
            })?;
        }
        if is_symlink {
            if entry.size() > MAX_SYMLINK_TARGET_BYTES {
                return Err(SourceError::Validation(format!(
                    "ZIP archive symlink target exceeds the transport limit: {}",
                    entry.name()
                )));
            }
            has_symlinks = true;
        }
        let mut output = if is_symlink {
            None
        } else {
            Some(
                fs::File::create(&destination).map_err(|source| SourceError::Io {
                    operation: "create ZIP staging file",
                    path: destination.clone(),
                    source,
                })?,
            )
        };
        let mut buffer = [0_u8; 64 * 1024];
        let mut entry_bytes = 0_u64;
        loop {
            let count = entry.read(&mut buffer).map_err(|source| SourceError::Io {
                operation: "extract ZIP entry",
                path: destination.clone(),
                source,
            })?;
            if count == 0 {
                break;
            }
            entry_bytes = entry_bytes.saturating_add(count as u64);
            total_bytes = total_bytes.saturating_add(count as u64);
            if entry_bytes > MAX_STAGED_ENTRY_BYTES || total_bytes > MAX_STAGED_SOURCE_BYTES {
                return Err(SourceError::Validation(
                    "ZIP archive exceeds the transport limits".into(),
                ));
            }
            if let Some(output) = &mut output {
                output
                    .write_all(&buffer[..count])
                    .map_err(|source| SourceError::Io {
                        operation: "write ZIP staging file",
                        path: destination.clone(),
                        source,
                    })?;
            }
        }
    }

    // Links are materialized in a second pass so no link is present while
    // regular archive entries are written. Their target bytes are bounded by
    // the native symlink path budget, rather than retained for the whole
    // archive in memory.
    if has_symlinks {
        let archive_file = fs::File::open(archive_path).map_err(|source| SourceError::Io {
            operation: "reopen ZIP archive for symlinks",
            path: archive_path.to_path_buf(),
            source,
        })?;
        let mut archive = ZipArchive::new(archive_file)
            .map_err(|error| SourceError::Validation(format!("ZIP archive is invalid: {error}")))?;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|error| {
                SourceError::Validation(format!("ZIP archive entry is invalid: {error}"))
            })?;
            let is_symlink = entry
                .unix_mode()
                .is_some_and(|mode| mode & u32::from(libc::S_IFMT) == u32::from(libc::S_IFLNK));
            if !is_symlink {
                continue;
            }
            if entry.size() > MAX_SYMLINK_TARGET_BYTES {
                return Err(SourceError::Validation(format!(
                    "ZIP archive symlink target exceeds the transport limit: {}",
                    entry.name()
                )));
            }
            let relative_path = entry.enclosed_name().ok_or_else(|| {
                SourceError::Validation(format!(
                    "ZIP archive contains an unsafe path: {}",
                    entry.name()
                ))
            })?;
            let Some(link_path) = selected_entry_destination(
                entry.name(),
                &relative_path,
                extraction_root,
                strip_prefix,
            )?
            else {
                return Err(SourceError::Validation(format!(
                    "ZIP archive entry is outside the selected tree: {}",
                    entry.name()
                )));
            };
            let mut target = Vec::with_capacity(entry.size() as usize);
            entry
                .read_to_end(&mut target)
                .map_err(|source| SourceError::Io {
                    operation: "read ZIP symlink target",
                    path: link_path.clone(),
                    source,
                })?;
            let target = PathBuf::from(std::ffi::OsString::from_vec(target));
            std::os::unix::fs::symlink(&target, &link_path).map_err(|source| SourceError::Io {
                operation: "create staged ZIP symlink",
                path: link_path,
                source,
            })?;
        }
    }
    Ok(())
}

fn selected_entry_destination(
    entry_name: &str,
    relative_path: &Path,
    extraction_root: &Path,
    strip_prefix: Option<&Path>,
) -> Result<Option<PathBuf>, SourceError> {
    let raw_path = Path::new(entry_name);
    if raw_path.is_absolute()
        || raw_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(SourceError::Validation(format!(
            "ZIP archive contains an unsafe path: {entry_name}"
        )));
    }
    let Some(stripped) = strip_relative_prefix(relative_path, strip_prefix) else {
        if strip_prefix.is_some_and(|prefix| prefix.starts_with(relative_path)) {
            // `git archive -- <subtree>` includes the selected subtree's
            // parent directory records. They are metadata, not content
            // outside the requested tree.
            return Ok(None);
        }
        return Err(SourceError::Validation(format!(
            "ZIP archive entry is outside the selected tree: {entry_name}"
        )));
    };
    Ok(Some(extraction_root.join(stripped)))
}

fn strip_relative_prefix(path: &Path, prefix: Option<&Path>) -> Option<PathBuf> {
    let stripped = match prefix {
        Some(prefix) => path.strip_prefix(prefix).ok()?.to_path_buf(),
        None => path.to_path_buf(),
    };
    stripped
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
        .then_some(stripped)
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::path::Path;

    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::extract_zip_archive;

    #[test]
    fn selected_tree_extraction_rejects_an_unrelated_archive_entry() {
        let temp = tempfile::tempdir().expect("tempdir");
        let archive_path = temp.path().join("source.zip");
        let archive_file = File::create(&archive_path).expect("create archive");
        let mut archive = ZipWriter::new(archive_file);
        archive
            .start_file("other/SKILL.md", SimpleFileOptions::default())
            .expect("start archive entry");
        std::io::Write::write_all(&mut archive, b"not selected").expect("write archive entry");
        archive.finish().expect("finish archive");

        let extraction_root = temp.path().join("extracted");
        let error =
            extract_zip_archive(&archive_path, &extraction_root, Some(Path::new("selected")))
                .expect_err("an entry outside the selected tree must be rejected");
        assert!(error.to_string().contains("outside the selected tree"));
        assert!(
            !extraction_root.join("SKILL.md").exists(),
            "unrelated archive content must not be materialized"
        );
    }
}
