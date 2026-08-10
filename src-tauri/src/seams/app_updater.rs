//! External application updater seam (ADR-0006).
//!
//! A successful `download` means the adapter retained a complete package and
//! verified its updater signature. Core never receives package bytes or a
//! download URL, and the frontend never receives updater/plugin capabilities.

use std::future::Future;
use std::pin::Pin;

use thiserror::Error;

use crate::core::app_update::AppUpdateOffer;

pub type AppUpdaterFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, AppUpdaterError>> + Send + 'a>>;

#[derive(Debug, Error)]
pub enum AppUpdaterError {
    #[error("the App Update source is unavailable: {0}")]
    SourceUnavailable(String),
    #[error("the pending App Update is no longer available")]
    NoPendingUpdate,
    #[error("the App Update was cancelled")]
    Cancelled,
    #[error("the App Update download failed: {0}")]
    DownloadFailed(String),
    #[error("the App Update could not be installed: {0}")]
    InstallFailed(String),
}

pub trait AppUpdater: Send + Sync {
    fn check(&self) -> AppUpdaterFuture<'_, Option<AppUpdateOffer>>;

    /// Success guarantees that the complete package is retained by the
    /// adapter and its updater signature has been verified.
    fn download<'a>(&'a self, update_id: &'a str) -> AppUpdaterFuture<'a, ()>;

    /// Abort an in-flight download or discard a retained update package.
    /// Implementations must release partial and verified package bytes.
    fn cancel(&self, update_id: &str) -> Result<(), AppUpdaterError>;

    /// Install the retained, verified package and request an application
    /// restart. Production does not normally return after requesting restart.
    fn install_and_restart(&self, update_id: &str) -> Result<(), AppUpdaterError>;
}
