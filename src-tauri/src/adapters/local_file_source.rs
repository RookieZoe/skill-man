use std::fs;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use zip::ZipArchive;

use crate::adapters::zip_extract::extract_zip_archive;
use crate::seams::source::{FileSource, SourceError, StagedFileSource};

pub struct LocalFileSource;

impl LocalFileSource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LocalFileSource {
    fn default() -> Self {
        Self::new()
    }
}

impl FileSource for LocalFileSource {
    fn estimated_size(&self, source_path: &Path) -> Result<u64, SourceError> {
        let original_path = source_path
            .canonicalize()
            .map_err(|source| SourceError::Io {
                operation: "canonicalize file Import source for size preflight",
                path: source_path.to_path_buf(),
                source,
            })?;
        if original_path.is_dir() {
            estimate_folder_size(&original_path)
        } else if original_path.is_file()
            && original_path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
        {
            estimate_zip_size(&original_path)
        } else {
            Err(SourceError::Validation(format!(
                "file Import source '{}' must be a Skill folder or ZIP archive",
                original_path.display()
            )))
        }
    }

    fn stage(
        &self,
        source_path: &Path,
        staging_root: &Path,
    ) -> Result<StagedFileSource, SourceError> {
        let original_path = source_path
            .canonicalize()
            .map_err(|source| SourceError::Io {
                operation: "canonicalize file Import source",
                path: source_path.to_path_buf(),
                source,
            })?;
        let original_filename = original_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                SourceError::Validation("file Import source name must be valid UTF-8".into())
            })?
            .to_owned();
        fs::create_dir_all(staging_root).map_err(|source| SourceError::Io {
            operation: "create file Import staging directory",
            path: staging_root.to_path_buf(),
            source,
        })?;
        let staged = if original_path.is_dir() {
            let staged_content_root = staging_root.join(&original_filename);
            copy_source_tree(&original_path, &original_path, &staged_content_root).map(|()| {
                StagedFileSource {
                    original_path: original_path.clone(),
                    original_filename: original_filename.clone(),
                    suggested_root_name: original_filename.clone(),
                    staged_content_root,
                }
            })
        } else if original_path.is_file()
            && original_path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
        {
            let suggested_root_name = original_path
                .file_stem()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    SourceError::Validation("ZIP archive stem must be valid UTF-8".into())
                })?
                .to_owned();
            let staged_content_root = staging_root.join("archive");
            stage_zip(&original_path, &staged_content_root).map(|()| StagedFileSource {
                original_path: original_path.clone(),
                original_filename: original_filename.clone(),
                suggested_root_name,
                staged_content_root,
            })
        } else {
            Err(SourceError::Validation(format!(
                "file Import source '{}' must be a Skill folder or ZIP archive",
                original_path.display()
            )))
        };

        match staged {
            Ok(staged) => Ok(staged),
            Err(error) => match fs::remove_dir_all(staging_root) {
                Ok(()) => Err(error),
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => Err(error),
                Err(source) => Err(SourceError::Io {
                    operation: "clean failed file Import staging",
                    path: staging_root.to_path_buf(),
                    source,
                }),
            },
        }
    }
}

fn estimate_folder_size(directory: &Path) -> Result<u64, SourceError> {
    let mut total = 0_u64;
    for entry in fs::read_dir(directory).map_err(|source| SourceError::Io {
        operation: "enumerate file Import source size",
        path: directory.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| SourceError::Io {
            operation: "enumerate file Import source size",
            path: directory.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| SourceError::Io {
            operation: "inspect file Import source size",
            path: path.clone(),
            source,
        })?;
        let bytes = if metadata.file_type().is_symlink() {
            fs::read_link(&path)
                .map_err(|source| SourceError::Io {
                    operation: "read file Import source link size",
                    path: path.clone(),
                    source,
                })?
                .as_os_str()
                .len() as u64
        } else if metadata.is_dir() {
            estimate_folder_size(&path)?
        } else if metadata.is_file() {
            metadata.len()
        } else {
            return Err(SourceError::Validation(format!(
                "file Import source contains an unsupported entry: {}",
                path.display()
            )));
        };
        total = total.saturating_add(bytes);
    }
    Ok(total)
}

fn estimate_zip_size(path: &Path) -> Result<u64, SourceError> {
    let archive_file = fs::File::open(path).map_err(|source| SourceError::Io {
        operation: "open ZIP file Import source for size preflight",
        path: path.to_path_buf(),
        source,
    })?;
    let mut archive = ZipArchive::new(archive_file)
        .map_err(|error| SourceError::Validation(format!("file Import ZIP is invalid: {error}")))?;
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|error| {
            SourceError::Validation(format!("file Import ZIP entry is invalid: {error}"))
        })?;
        total = total.saturating_add(entry.size());
    }
    Ok(total)
}

fn stage_zip(original_path: &Path, extraction_root: &Path) -> Result<(), SourceError> {
    extract_zip_archive(original_path, extraction_root, None)
}

fn copy_source_tree(
    source_root: &Path,
    source: &Path,
    destination: &Path,
) -> Result<(), SourceError> {
    let canonical_source = source
        .canonicalize()
        .map_err(|source_error| SourceError::Io {
            operation: "canonicalize staged source directory",
            path: source.to_path_buf(),
            source: source_error,
        })?;
    if !canonical_source.starts_with(source_root) {
        return Err(SourceError::Validation(format!(
            "file Import source directory escaped while it was staged: {}",
            source.display()
        )));
    }
    let source_metadata = fs::symlink_metadata(source).map_err(|source_error| SourceError::Io {
        operation: "reinspect staged source directory",
        path: source.to_path_buf(),
        source: source_error,
    })?;
    if !source_metadata.is_dir() || source_metadata.file_type().is_symlink() {
        return Err(SourceError::Validation(format!(
            "file Import source directory changed while it was staged: {}",
            source.display()
        )));
    }
    fs::create_dir(destination).map_err(|source_error| SourceError::Io {
        operation: "create staged source directory",
        path: destination.to_path_buf(),
        source: source_error,
    })?;
    for entry in fs::read_dir(&canonical_source).map_err(|source_error| SourceError::Io {
        operation: "enumerate file Import source",
        path: source.to_path_buf(),
        source: source_error,
    })? {
        let entry = entry.map_err(|source_error| SourceError::Io {
            operation: "enumerate file Import source",
            path: source.to_path_buf(),
            source: source_error,
        })?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata =
            fs::symlink_metadata(&source_path).map_err(|source_error| SourceError::Io {
                operation: "inspect file Import source entry",
                path: source_path.clone(),
                source: source_error,
            })?;
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(&source_path).map_err(|source_error| SourceError::Io {
                operation: "read file Import source symlink",
                path: source_path.clone(),
                source: source_error,
            })?;
            std::os::unix::fs::symlink(&target, &destination_path).map_err(|source_error| {
                SourceError::Io {
                    operation: "copy file Import source symlink",
                    path: source_path,
                    source: source_error,
                }
            })?;
        } else if metadata.is_dir() {
            copy_source_tree(source_root, &source_path, &destination_path)?;
        } else if metadata.is_file() {
            let mut input = fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
                .open(&source_path)
                .map_err(|source_error| SourceError::Io {
                    operation: "open file Import source entry without following links",
                    path: source_path.clone(),
                    source: source_error,
                })?;
            let opened_metadata = input.metadata().map_err(|source_error| SourceError::Io {
                operation: "inspect opened file Import source entry",
                path: source_path.clone(),
                source: source_error,
            })?;
            if opened_metadata.dev() != metadata.dev()
                || opened_metadata.ino() != metadata.ino()
                || opened_metadata.len() != metadata.len()
            {
                return Err(SourceError::Validation(format!(
                    "file Import source entry changed while it was staged: {}",
                    source_path.display()
                )));
            }
            let mut output =
                fs::File::create(&destination_path).map_err(|source_error| SourceError::Io {
                    operation: "create staged file Import entry",
                    path: destination_path.clone(),
                    source: source_error,
                })?;
            std::io::copy(&mut input, &mut output).map_err(|source_error| SourceError::Io {
                operation: "copy file Import source entry",
                path: source_path.clone(),
                source: source_error,
            })?;
            let finished_metadata = input.metadata().map_err(|source_error| SourceError::Io {
                operation: "reinspect copied file Import source entry",
                path: source_path.clone(),
                source: source_error,
            })?;
            if finished_metadata.dev() != metadata.dev()
                || finished_metadata.ino() != metadata.ino()
                || finished_metadata.len() != metadata.len()
                || finished_metadata.mtime() != metadata.mtime()
                || finished_metadata.mtime_nsec() != metadata.mtime_nsec()
            {
                return Err(SourceError::Validation(format!(
                    "file Import source entry changed while it was copied: {}",
                    source_path.display()
                )));
            }
        } else {
            return Err(SourceError::Validation(format!(
                "file Import source contains an unsupported entry: {}",
                source_path.display()
            )));
        }
    }
    Ok(())
}
