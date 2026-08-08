use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedFileSource {
    pub original_path: PathBuf,
    pub original_filename: String,
    pub suggested_root_name: String,
    pub staged_content_root: PathBuf,
}

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("{0}")]
    Validation(String),
    #[error("{operation} failed for '{}': {source}", path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub trait FileSource: Send + Sync {
    fn estimated_size(&self, source_path: &Path) -> Result<u64, SourceError>;

    fn stage(
        &self,
        source_path: &Path,
        staging_root: &Path,
    ) -> Result<StagedFileSource, SourceError>;
}
