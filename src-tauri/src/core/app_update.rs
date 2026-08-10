//! Skill Man application Update policy and state machine (ADR-0006).
//!
//! This is deliberately separate from `core::update`, which updates Skill
//! content. The public interface is the test surface; transport details and
//! updater package bytes stay behind the `AppUpdater` seam.

use std::sync::{Arc, Mutex};

use thiserror::Error;

use crate::seams::app_updater::{AppUpdater, AppUpdaterError};
use crate::seams::clock::Clock;
use crate::seams::preferences_store::{PreferencesStore, PreferencesStoreError};

const APP_UPDATE_COOLDOWN_SECONDS: i64 = 24 * 60 * 60;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppUpdateOffer {
    pub update_id: String,
    pub current_version: String,
    pub version: String,
    pub release_notes: String,
    pub download_size_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DownloadedAppUpdate {
    pub update_id: String,
    pub version: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancelledAppUpdate {
    pub update_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppUpdateCheck {
    SkippedCooldown,
    UpToDate,
    Available(AppUpdateOffer),
}

#[derive(Debug, Error)]
pub enum AppUpdateError {
    #[error(transparent)]
    Preferences(#[from] PreferencesStoreError),
    #[error(transparent)]
    Updater(#[from] AppUpdaterError),
    #[error("invalid App Update state: {0}")]
    InvalidState(String),
    #[error("internal App Update error: {0}")]
    Internal(String),
}

#[derive(Clone, Debug)]
enum AppUpdatePhase {
    Idle,
    Checking,
    Available(AppUpdateOffer),
    Downloading(AppUpdateOffer),
    Downloaded(AppUpdateOffer),
    Installing,
}

pub struct AppUpdateService {
    updater: Arc<dyn AppUpdater>,
    preferences: Arc<dyn PreferencesStore>,
    clock: Arc<dyn Clock>,
    phase: Mutex<AppUpdatePhase>,
}

impl AppUpdateService {
    pub fn new(
        updater: Arc<dyn AppUpdater>,
        preferences: Arc<dyn PreferencesStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            updater,
            preferences,
            clock,
            phase: Mutex::new(AppUpdatePhase::Idle),
        }
    }

    pub async fn check(&self, force: bool) -> Result<AppUpdateCheck, AppUpdateError> {
        let previous_phase = {
            let mut phase = self.phase()?;
            match &*phase {
                AppUpdatePhase::Idle | AppUpdatePhase::Available(_) => {
                    let previous = phase.clone();
                    *phase = AppUpdatePhase::Checking;
                    previous
                }
                current => return Err(invalid_phase("an idle or available updater", current)),
            }
        };
        let now = i64::try_from(self.clock.unix_epoch_nanos() / 1_000_000_000).unwrap_or(i64::MAX);
        let last_checked_at = match self.preferences.last_app_update_check_at() {
            Ok(last_checked_at) => last_checked_at,
            Err(error) => {
                *self.phase()? = previous_phase;
                return Err(error.into());
            }
        };
        if !force
            && last_checked_at.is_some_and(|checked_at| {
                now.saturating_sub(checked_at) < APP_UPDATE_COOLDOWN_SECONDS
            })
        {
            *self.phase()? = previous_phase;
            return Ok(AppUpdateCheck::SkippedCooldown);
        }

        let offer = match self.updater.check().await {
            Ok(offer) => offer,
            Err(error) => {
                *self.phase()? = previous_phase;
                return Err(error.into());
            }
        };
        let mut phase = self.phase()?;
        let check = match offer {
            Some(offer) => {
                *phase = AppUpdatePhase::Available(offer.clone());
                AppUpdateCheck::Available(offer)
            }
            None => {
                *phase = AppUpdatePhase::Idle;
                AppUpdateCheck::UpToDate
            }
        };
        drop(phase);
        self.preferences.record_app_update_check_at(now)?;
        Ok(check)
    }

    pub async fn download(&self, update_id: &str) -> Result<DownloadedAppUpdate, AppUpdateError> {
        let offer = {
            let mut phase = self.phase()?;
            match &*phase {
                AppUpdatePhase::Available(offer) if offer.update_id == update_id => {
                    let offer = offer.clone();
                    *phase = AppUpdatePhase::Downloading(offer.clone());
                    offer
                }
                current => return Err(invalid_phase("an available offer", current)),
            }
        };

        match self.updater.download(update_id).await {
            Ok(()) => {
                let downloaded = DownloadedAppUpdate {
                    update_id: offer.update_id.clone(),
                    version: offer.version.clone(),
                };
                let mut phase = self.phase()?;
                if matches!(
                    &*phase,
                    AppUpdatePhase::Downloading(current)
                        if current.update_id == offer.update_id
                ) {
                    *phase = AppUpdatePhase::Downloaded(offer);
                    Ok(downloaded)
                } else {
                    drop(phase);
                    let _ = self.updater.cancel(update_id);
                    Err(AppUpdaterError::Cancelled.into())
                }
            }
            Err(error) => {
                let mut phase = self.phase()?;
                if matches!(
                    &*phase,
                    AppUpdatePhase::Downloading(current)
                        if current.update_id == offer.update_id
                ) {
                    *phase = if matches!(&error, AppUpdaterError::DownloadFailed(_)) {
                        AppUpdatePhase::Available(offer)
                    } else {
                        AppUpdatePhase::Idle
                    };
                }
                Err(error.into())
            }
        }
    }

    pub fn cancel(&self, update_id: &str) -> Result<CancelledAppUpdate, AppUpdateError> {
        let mut phase = self.phase()?;
        let matches_update = match &*phase {
            AppUpdatePhase::Available(offer)
            | AppUpdatePhase::Downloading(offer)
            | AppUpdatePhase::Downloaded(offer) => offer.update_id == update_id,
            _ => false,
        };
        if !matches_update {
            return Err(invalid_phase("a cancellable update", &phase));
        }

        let previous = phase.clone();
        *phase = AppUpdatePhase::Idle;
        if let Err(error) = self.updater.cancel(update_id) {
            *phase = previous;
            return Err(error.into());
        }
        Ok(CancelledAppUpdate {
            update_id: update_id.into(),
        })
    }

    pub fn install_and_restart(&self, update_id: &str) -> Result<(), AppUpdateError> {
        let offer = {
            let mut phase = self.phase()?;
            match &*phase {
                AppUpdatePhase::Downloaded(offer) if offer.update_id == update_id => {
                    let offer = offer.clone();
                    *phase = AppUpdatePhase::Installing;
                    offer
                }
                current => return Err(invalid_phase("a downloaded update", current)),
            }
        };

        match self.updater.install_and_restart(update_id) {
            Ok(()) => {
                *self.phase()? = AppUpdatePhase::Idle;
                Ok(())
            }
            Err(error) => {
                *self.phase()? = AppUpdatePhase::Downloaded(offer);
                Err(error.into())
            }
        }
    }

    fn phase(&self) -> Result<std::sync::MutexGuard<'_, AppUpdatePhase>, AppUpdateError> {
        self.phase
            .lock()
            .map_err(|_| AppUpdateError::Internal("App Update state lock poisoned".into()))
    }
}

fn invalid_phase(expected: &str, actual: &AppUpdatePhase) -> AppUpdateError {
    let actual = match actual {
        AppUpdatePhase::Idle => "idle",
        AppUpdatePhase::Checking => "checking",
        AppUpdatePhase::Available(_) => "available",
        AppUpdatePhase::Downloading(_) => "downloading",
        AppUpdatePhase::Downloaded(_) => "downloaded",
        AppUpdatePhase::Installing => "installing",
    };
    AppUpdateError::InvalidState(format!("expected {expected}, found {actual}"))
}
