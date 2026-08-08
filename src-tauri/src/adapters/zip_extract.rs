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
const MAX_STAGED_SOURCE_BYTES: u64 = 100 * 256 * 1024 * 1024;

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
    let strip_components = strip_prefix
        .map(|prefix| prefix.components().count())
        .unwrap_or(0);
    let mut total_bytes = 0_u64;
    let mut symlinks = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            SourceError::Validation(format!("ZIP archive entry is invalid: {error}"))
        })?;
        let raw_path = Path::new(entry.name());
        if raw_path.is_absolute()
            || raw_path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(SourceError::Validation(format!(
                "ZIP archive contains an unsafe path: {}",
                entry.name()
            )));
        }
        let relative_path = entry.enclosed_name().ok_or_else(|| {
            SourceError::Validation(format!(
                "ZIP archive contains an unsafe path: {}",
                entry.name()
            ))
        })?;
        let stripped =
            strip_relative_prefix(&relative_path, strip_components).ok_or_else(|| {
                SourceError::Validation(format!(
                    "ZIP archive entry is outside the selected tree: {}",
                    entry.name()
                ))
            })?;
        let is_symlink = entry
            .unix_mode()
            .is_some_and(|mode| mode & u32::from(libc::S_IFMT) == u32::from(libc::S_IFLNK));
        let destination = extraction_root.join(&stripped);
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
        let mut symlink_target = Vec::new();
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
            } else {
                symlink_target.extend_from_slice(&buffer[..count]);
            }
        }
        if is_symlink {
            symlinks.push((
                destination,
                PathBuf::from(std::ffi::OsString::from_vec(symlink_target)),
            ));
        }
    }

    // Links are materialized but never followed here. Import Core validates
    // their relative targets and containment before the staged tree is
    // eligible to apply.
    for (link_path, target) in symlinks {
        std::os::unix::fs::symlink(&target, &link_path).map_err(|source| SourceError::Io {
            operation: "create staged ZIP symlink",
            path: link_path,
            source,
        })?;
    }
    Ok(())
}

fn strip_relative_prefix(path: &Path, components: usize) -> Option<PathBuf> {
    let mut remaining = components;
    let mut stripped = PathBuf::new();
    for component in path.components() {
        if remaining > 0 {
            remaining -= 1;
            continue;
        }
        match component {
            Component::Normal(value) => stripped.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(stripped)
}
