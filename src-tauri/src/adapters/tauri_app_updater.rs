//! Production application updater backed by Tauri's signed updater plugin.

use std::sync::Mutex;

use futures_util::future::{AbortHandle, Abortable};
use sha2::{Digest, Sha256};
use tauri::AppHandle;
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::core::app_update::AppUpdateOffer;
use crate::seams::app_updater::{AppUpdater, AppUpdaterError, AppUpdaterFuture};

struct PendingUpdate {
    update_id: String,
    update: Update,
    expected_download_size_bytes: u64,
    verified_bytes: Option<Vec<u8>>,
    download_abort: Option<AbortHandle>,
}

pub struct TauriAppUpdater {
    app: AppHandle,
    pending: Mutex<Option<PendingUpdate>>,
}

impl TauriAppUpdater {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            pending: Mutex::new(None),
        }
    }

    fn pending(&self) -> Result<std::sync::MutexGuard<'_, Option<PendingUpdate>>, AppUpdaterError> {
        self.pending.lock().map_err(|_| {
            AppUpdaterError::SourceUnavailable("App Update state lock poisoned".into())
        })
    }
}

impl AppUpdater for TauriAppUpdater {
    fn check(&self) -> AppUpdaterFuture<'_, Option<AppUpdateOffer>> {
        Box::pin(async move {
            let update = self
                .app
                .updater()
                .map_err(source_unavailable)?
                .check()
                .await
                .map_err(source_unavailable)?;
            let Some(update) = update else {
                *self.pending()? = None;
                return Ok(None);
            };

            let download_size_bytes = update
                .raw_json
                .get("download_size")
                .and_then(serde_json::Value::as_u64)
                .filter(|size| *size > 0)
                .ok_or_else(|| {
                    AppUpdaterError::SourceUnavailable(
                        "updater metadata is missing a positive download_size".into(),
                    )
                })?;
            let update_id = update_id(&update);
            let offer = AppUpdateOffer {
                update_id: update_id.clone(),
                current_version: update.current_version.clone(),
                version: update.version.clone(),
                release_notes: update.body.clone().unwrap_or_default(),
                download_size_bytes,
            };
            *self.pending()? = Some(PendingUpdate {
                update_id,
                update,
                expected_download_size_bytes: download_size_bytes,
                verified_bytes: None,
                download_abort: None,
            });
            Ok(Some(offer))
        })
    }

    fn download<'a>(&'a self, update_id: &'a str) -> AppUpdaterFuture<'a, ()> {
        let setup = (|| {
            let mut pending = self.pending()?;
            match pending.as_mut() {
                Some(pending) if pending.update_id == update_id => {
                    let (abort, registration) = AbortHandle::new_pair();
                    pending.download_abort = Some(abort);
                    Ok((
                        pending.update.clone(),
                        pending.expected_download_size_bytes,
                        registration,
                    ))
                }
                _ => Err(AppUpdaterError::NoPendingUpdate),
            }
        })();

        Box::pin(async move {
            let (update, expected_download_size_bytes, registration) = setup?;

            // Tauri verifies the complete package against the embedded updater
            // public key before returning these bytes.
            let verified_bytes =
                match Abortable::new(update.download(|_, _| {}, || {}), registration).await {
                    Ok(Ok(bytes)) => bytes,
                    Ok(Err(error)) => {
                        if !self.finish_download_attempt(update_id)? {
                            return Err(AppUpdaterError::Cancelled);
                        }
                        return Err(AppUpdaterError::DownloadFailed(error.to_string()));
                    }
                    Err(_) => return Err(AppUpdaterError::Cancelled),
                };
            let actual_download_size_bytes =
                u64::try_from(verified_bytes.len()).unwrap_or(u64::MAX);
            if actual_download_size_bytes != expected_download_size_bytes {
                if !self.finish_download_attempt(update_id)? {
                    return Err(AppUpdaterError::Cancelled);
                }
                return Err(AppUpdaterError::DownloadFailed(format!(
                    "verified archive size {actual_download_size_bytes} did not match manifest size {expected_download_size_bytes}"
                )));
            }

            let mut pending = self.pending()?;
            match pending.as_mut() {
                Some(pending) if pending.update_id == update_id => {
                    pending.download_abort = None;
                    pending.verified_bytes = Some(verified_bytes);
                    Ok(())
                }
                None => Err(AppUpdaterError::Cancelled),
                _ => Err(AppUpdaterError::NoPendingUpdate),
            }
        })
    }

    fn cancel(&self, update_id: &str) -> Result<(), AppUpdaterError> {
        let mut slot = self.pending()?;
        match slot.as_ref() {
            Some(pending) if pending.update_id == update_id => {
                let mut pending = slot.take().ok_or(AppUpdaterError::NoPendingUpdate)?;
                if let Some(abort) = pending.download_abort.take() {
                    abort.abort();
                }
                Ok(())
            }
            None => Ok(()),
            _ => Err(AppUpdaterError::NoPendingUpdate),
        }
    }

    fn install_and_restart(&self, update_id: &str) -> Result<(), AppUpdaterError> {
        let pending = {
            let mut slot = self.pending()?;
            let pending = slot.take().ok_or(AppUpdaterError::NoPendingUpdate)?;
            if pending.update_id != update_id || pending.verified_bytes.is_none() {
                *slot = Some(pending);
                return Err(AppUpdaterError::NoPendingUpdate);
            }
            pending
        };

        let verified_bytes = pending
            .verified_bytes
            .as_ref()
            .ok_or(AppUpdaterError::NoPendingUpdate)?;
        if let Err(error) = pending.update.install(verified_bytes) {
            *self.pending()? = Some(pending);
            return Err(AppUpdaterError::InstallFailed(error.to_string()));
        }

        self.app.restart()
    }
}

impl TauriAppUpdater {
    fn finish_download_attempt(&self, update_id: &str) -> Result<bool, AppUpdaterError> {
        let mut pending = self.pending()?;
        match pending.as_mut() {
            Some(pending) if pending.update_id == update_id => {
                pending.download_abort = None;
                Ok(true)
            }
            None => Ok(false),
            _ => Err(AppUpdaterError::NoPendingUpdate),
        }
    }
}

fn update_id(update: &Update) -> String {
    let mut digest = Sha256::new();
    for part in [
        update.current_version.as_str(),
        update.version.as_str(),
        update.download_url.as_str(),
        update.signature.as_str(),
    ] {
        digest.update(part.as_bytes());
        digest.update([0]);
    }
    format!("{:x}", digest.finalize())
}

fn source_unavailable(error: impl std::fmt::Display) -> AppUpdaterError {
    AppUpdaterError::SourceUnavailable(error.to_string())
}
